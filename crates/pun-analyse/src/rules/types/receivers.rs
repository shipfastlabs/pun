use std::collections::HashSet;

use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::atomic::object::{TObject, named::TNamedObject};
use mago_codex::ttype::atomic::scalar::{TScalar, class_like_string::TClassLikeString};
use mago_codex::ttype::union::TUnion;
use mago_span::{HasSpan, Span};
use mago_syntax::cst::*;
use pun_phpstan::types::{Accessory, Level, Tri, Type, intersect, is_super_type_of};

use super::super::class_reflection::{MethodAnswer, MethodPhase, Reflected};
use super::super::overriding::Class;
use super::super::reflection::Lookup;
use super::super::{Checker, calls, declared, members, method_reflection, params};
use super::flow::is_this;

pub(super) const IDENTIFIERS: &[&str] = &["method.nonObject", "staticMethod.nonObject"];

/// A method lookup that cannot tell which method it finds. `Some` is the declaring class of the static method an
/// extension says exists there.
pub(super) struct Unknown(Option<String>);

impl<'a> Checker<'a, '_> {
    pub(in super::super) fn refine_method_guards(&self, e: &Expression, ty: Type) -> Type {
        self.refine_method_guards_in(e, ty, &self.types)
    }

    pub(super) fn refine_method_guards_in(
        &self,
        e: &Expression,
        mut ty: Type,
        types: &std::collections::HashMap<Span, TUnion, impl std::hash::BuildHasher>,
    ) -> Type {
        if let Some(raw) = types
            .get(&e.span())
            .and_then(|raw| without_requirements(self, raw))
        {
            ty = self.to_phpstan(&raw);
        }
        let Some(receiver) = super::super::scope::method_receiver(e, &|id| self.resolved(id))
        else {
            return ty;
        };
        let prefix = format!("{receiver}::");
        let original = self.guards.iter().find_map(|g| match g {
            super::super::scope::Guard::MethodSource(key, _) if key.starts_with(&prefix) => {
                self.method_guard_source_in(key, types).map(|(ty, _)| ty)
            }
            _ => None,
        });
        if let Some(original) = &original
            && ((matches!(ty, Type::ClassString(None)) && original.is_string().yes())
                || matches!(
                    (&ty, original),
                    (Type::ObjectWithoutClass, Type::Mixed { .. })
                ))
        {
            ty = original.clone();
        }
        let classes = match &ty {
            Type::ClassString(Some(t)) => t.object_class_names(),
            _ => ty.object_class_names(),
        };
        for guard in &self.guards {
            let super::super::scope::Guard::Method(key) = guard else {
                continue;
            };
            let Some(method) = key.strip_prefix(&prefix) else {
                continue;
            };
            if !self.has_method_guard_in(key, types) {
                ty = declared::with_classes(self, || {
                    intersect(vec![
                        ty,
                        pun_phpstan::types::union(vec![Type::ObjectWithoutClass, Type::String]),
                    ])
                });
                continue;
            }
            if classes.iter().any(|n| {
                matches!(self.method_of(n, method.as_bytes()), Ok(Some(_)))
                    && self
                        .class_like(n)
                        .found()
                        .is_some_and(|k| members::method(self, k, method.as_bytes()).is_none())
            }) {
                continue;
            }
            ty = declared::with_classes(self, || {
                let specified = original
                    .as_ref()
                    .unwrap_or(&ty)
                    .clone()
                    .require_method(method);
                intersect(vec![specified, ty])
            });
        }
        ty
    }

    pub(in super::super) fn can_call_methods(&self, t: &Type) -> Tri {
        self.capable(t, true)
    }

    pub(super) fn can_access_properties(&self, t: &Type) -> Tri {
        self.capable(t, false)
    }

    fn capable(&self, t: &Type, methods: bool) -> Tri {
        t.query_members(&|t| match t {
            Type::Object(n, _, _) if methods && n.eq_ignore_ascii_case("stdClass") => Tri::No,
            Type::Object(..)
            | Type::Static(..)
            | Type::This(_)
            | Type::EnumCase(..)
            | Type::ObjectWithoutClass
            | Type::ObjectShape(_)
            | Type::Closure(_) => Tri::Yes,
            Type::Accessory(Accessory::HasMethod(_)) if methods => Tri::Yes,
            Type::Callable(_) | Type::Iterable(..) => Tri::Maybe,
            _ => Tri::No,
        })
    }

    fn class_of(t: &Type) -> Option<&str> {
        match t {
            Type::Object(n, _, _) | Type::Static(n, _) | Type::This(n) | Type::EnumCase(n, _) => {
                Some(n)
            }
            t => matches!(t, Type::Closure(_)).then_some("Closure"),
        }
    }

    pub(in super::super) fn method_of(
        &self,
        n: &str,
        name: &[u8],
    ) -> Result<Option<members::Method<'a>>, ()> {
        self.method_of_type(&Type::Object(n.to_string(), vec![], false), name)
    }

    pub(in super::super) fn method_of_type(
        &self,
        t: &Type,
        name: &[u8],
    ) -> Result<Option<members::Method<'a>>, ()> {
        self.reflected_method(t, name)
            .map(|m| m.map(|m| m.declaring))
    }

    pub(in super::super) fn reflected_method(
        &self,
        t: &Type,
        name: &[u8],
    ) -> Result<Option<method_reflection::Method<'a>>, ()> {
        self.extended_method(t, name, &mut HashSet::new(), false)
            .map_err(|_| ())
    }

    /// The declaring class and name strict-rules' `staticMethod.dynamicCall` reports for a call to `name` on `t`,
    /// when PHPStan reflects the method it finds as static.
    pub(in super::super) fn static_call(&self, t: &Type, name: &[u8]) -> Option<(String, String)> {
        let call = || String::from_utf8_lossy(name).into_owned();
        match self.extended_method(t, name, &mut HashSet::new(), false) {
            Ok(Some(m)) if m.is_static => Some(match m.static_display {
                Some(display) => (display, call()),
                None => (m.display, m.name),
            }),
            Ok(_) => None,
            Err(Unknown(declaring)) => declaring.map(|d| (d, call())),
        }
    }

    /// Whether [`Self::reflected_method`] finds `name` on `t` (`Ok(Some(_))`), finds none (`Ok(None)`) or cannot
    /// tell (`Err`), without building the found methods' parameters, which the answer does not depend on.
    fn finds_method(&self, t: &Type, name: &[u8]) -> Result<Option<()>, ()> {
        self.extended_method(t, name, &mut HashSet::new(), true)
            .map(|m| m.map(|_| ()))
            .map_err(|_| ())
    }

    /// `shallow` leaves each found method's display, variants and types empty: only whether one is found.
    fn extended_method(
        &self,
        t: &Type,
        name: &[u8],
        active: &mut HashSet<String>,
        shallow: bool,
    ) -> Result<Option<method_reflection::Method<'a>>, Unknown> {
        match t {
            Type::Union(ts) | Type::Benevolent(ts) => {
                let mut methods = Vec::new();
                for t in ts {
                    let Some(method) = self.extended_method(t, name, active, shallow)? else {
                        return Ok(None);
                    };
                    methods.push(method);
                }
                return Ok(method_reflection::Method::union(self, methods, name));
            }
            Type::Intersection(ts) => {
                let methods = ts
                    .iter()
                    .filter_map(|t| {
                        self.extended_method(t, name, active, shallow)
                            .ok()
                            .flatten()
                    })
                    .collect();
                return Ok(method_reflection::Method::intersection(methods, name));
            }
            Type::Template(t) => return self.extended_method(&t.bound, name, active, shallow),
            _ => {}
        }
        let Some(n) = Self::class_of(t) else {
            return Ok(None);
        };
        let Lookup::Found(k) = self.class_like(n) else {
            return if active.is_empty() {
                Err(Unknown(None))
            } else {
                Ok(None)
            };
        };
        let nominal = |class: &str| {
            declared::with_classes(self, || {
                is_super_type_of(
                    &Type::Object(class.to_string(), vec![], false),
                    &Type::Object(n.to_string(), vec![], false),
                )
                .yes()
            })
        };
        if members::method(self, k, name).is_none()
            && let Some(method) =
                self.extension_method(MethodPhase::BeforeAnnotations, t, k, name, active, shallow)?
        {
            return Ok(Some(method));
        }
        let native = members::reflected_method(self, k, name);
        if let Some(m) = native.filter(|m| {
            matches!(m, members::Method::User { declaring, .. } if declaring.kind.is_trait())
                || nominal(&m.declaring_name())
        }) {
            if shallow {
                return Ok(Some(method_reflection::Method::shallow(m)));
            }
            let mut method = method_reflection::Method::leaf(self, t, m);
            let own;
            let receiver_args = match t {
                Type::Object(receiver, actual, _) | Type::Static(receiver, actual) => {
                    Some((receiver, actual))
                }
                Type::This(receiver) => {
                    own = declared::class_templates(self, k)
                        .into_iter()
                        .map(|tt| {
                            Type::Template(Box::new(pun_phpstan::types::Template {
                                argument: true,
                                ..tt
                            }))
                        })
                        .collect::<Vec<_>>();
                    Some((receiver, &own))
                }
                _ => None,
            };
            if let members::Method::User { declaring, .. } = &method.declaring
                && let Some((receiver, actual)) = receiver_args
                && (!actual.is_empty() || matches!(t, Type::Object(..) | Type::This(_)))
            {
                let class = declared::original_name(declaring);
                let args = declared::with_classes(self, || {
                    if receiver.eq_ignore_ascii_case(&class) {
                        actual.clone()
                    } else {
                        pun_phpstan::symbols::lookup(receiver)
                            .and_then(|k| pun_phpstan::types::ancestor_args(&k, &class, actual))
                            .unwrap_or_default()
                    }
                });
                let names: Vec<_> = declared::class_templates(self, declaring)
                    .into_iter()
                    .map(|t| t.name)
                    .collect();
                let scope = format!("class {class}");
                declared::with_classes(self, || {
                    for variant in &mut method.variants.all {
                        variant.param_types = (super::declared_types(self, variant).into_iter())
                            .map(|ty| {
                                let ty = ty.unwrap_or_else(pun_phpstan::types::mixed);
                                let ty = super::properties::with_args(&ty, &scope, &names, &args);
                                params::ParamType::Typed(method_reflection::called_on(&ty, t))
                            })
                            .collect();
                    }
                });
            }
            return Ok(Some(method));
        }
        if let Some(method) =
            self.extension_method(MethodPhase::BeforeMixins, t, k, name, active, shallow)?
        {
            return Ok(Some(method));
        }
        for mixin in self.method_mixins(k, t) {
            if matches!(mixin.resolve_to_bounds(), Type::Mixed { .. } | Type::Error) {
                return Err(Unknown(None));
            }
            let key = declared::with_classes(self, || mixin.describe(Level::TypeOnly));
            if !active.insert(key.clone()) {
                continue;
            }
            let method = self.extended_method(&mixin, name, active, shallow);
            active.remove(&key);
            if let Some(mut method) = method? {
                // PHPStan's MixinMethodReflection: static when the class has a native __callStatic.
                if members::method(self, k, b"__callStatic").is_some() {
                    method.reflect(&Reflected::StaticDeclared);
                }
                return Ok(Some(method));
            }
        }
        let mut interfaces = vec![Class::User(k)];
        let mut seen = HashSet::new();
        while let Some(interface) = interfaces.pop() {
            let Class::User(interface) = interface else {
                continue;
            };
            if !interface.kind.is_interface() || !seen.insert(interface.name) {
                continue;
            }
            for required in super::super::require_tags::tag_types(
                self,
                interface,
                super::super::require_tags::EXTENDS,
            ) {
                if matches!(
                    required.resolve_to_bounds(),
                    Type::Mixed { .. } | Type::Error
                ) {
                    return Err(Unknown(None));
                }
                let key = declared::with_classes(self, || {
                    format!("require-extends {}", required.describe(Level::TypeOnly))
                });
                if !active.insert(key.clone()) {
                    continue;
                }
                let method = self.extended_method(&required, name, active, shallow);
                active.remove(&key);
                if let Some(method) = method? {
                    return Ok(Some(method));
                }
            }
            interfaces.extend(
                self.all_interfaces(Class::User(interface))
                    .into_iter()
                    .rev(),
            );
        }
        self.extension_method(MethodPhase::Last, t, k, name, active, shallow)
    }

    /// The extensions' answer for `name` at `phase` of the lookup.
    fn extension_method(
        &self,
        phase: MethodPhase,
        t: &Type,
        k: &mago_codex::metadata::class_like::ClassLikeMetadata,
        name: &[u8],
        active: &mut HashSet<String>,
        shallow: bool,
    ) -> Result<Option<method_reflection::Method<'a>>, Unknown> {
        for e in self.class_reflection_extensions() {
            match e.method(self, phase, t, k, name) {
                Some(MethodAnswer::Forward(to, reflected)) => {
                    let method = match self.extended_method(&to, name, active, shallow) {
                        Ok(method) => method.map(|mut m| {
                            m.reflect(&reflected);
                            m
                        }),
                        Err(Unknown(declaring)) => {
                            return Err(Unknown(match reflected {
                                Reflected::Declared | Reflected::StaticDeclared => declaring,
                                Reflected::EloquentBuilder(display) => display.or(declaring),
                                Reflected::Instance => None,
                            }));
                        }
                    };
                    if method.is_some() || phase == MethodPhase::Last {
                        return Ok(method);
                    }
                }
                Some(MethodAnswer::Exists(declaring)) => return Err(Unknown(declaring)),
                None => {}
            }
        }
        Ok(None)
    }

    pub(in super::super) fn method_mixins(
        &self,
        class: &'a mago_codex::metadata::class_like::ClassLikeMetadata,
        receiver: &Type,
    ) -> Vec<Type> {
        let args = match receiver {
            Type::Object(_, args, _) | Type::Static(_, args) => args.clone(),
            _ => Vec::new(),
        };
        let mut pending = vec![(class, args)];
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        while let Some((class, actual)) = pending.pop() {
            if !seen.insert(class.name) {
                continue;
            }
            let templates = declared::class_templates(self, class);
            let names: Vec<_> = templates.iter().map(|t| t.name.clone()).collect();
            let args: Vec<_> = templates
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    actual
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| t.default.as_ref().unwrap_or(&t.bound).clone())
                })
                .collect();
            let scope = format!("class {}", declared::original_name(class));
            let replace = |t: &Type| super::properties::with_args(t, &scope, &names, &args);
            if let Some((_, entry, _)) = declared::doc_at(self, class.span) {
                let scope = declared::scope_at(self, class.span, Some(class));
                out.extend(
                    entry
                        .doc
                        .mixins
                        .iter()
                        .map(|node| replace(&pun_phpstan::resolve::resolve_in(node, &scope))),
                );
            }
            if let Some(parent) = class
                .direct_parent_class
                .and_then(|n| self.codebase.get_class_like(n.as_bytes()))
            {
                let args = declared::with_classes(self, || {
                    pun_phpstan::symbols::lookup(&declared::original_name(class))
                        .and_then(|k| {
                            pun_phpstan::types::ancestor_args(
                                &k,
                                &declared::original_name(parent),
                                &args,
                            )
                        })
                        .unwrap_or_default()
                });
                pending.push((parent, args));
            }
            #[expect(
                clippy::needless_collect,
                reason = "the iterator is not double-ended, and `rev()` needs one"
            )]
            let traits: Vec<_> = class.direct_trait_names().collect();
            pending.extend(
                traits
                    .into_iter()
                    .rev()
                    .filter_map(|n| self.codebase.get_class_like(n.as_bytes()))
                    .map(|t| (t, Vec::new())),
            );
        }
        out
    }

    pub(in super::super) fn has_method(&self, t: &Type, name: &[u8]) -> Tri {
        t.query_members(&|t| match t {
            Type::Accessory(Accessory::HasMethod(_)) => {
                t.has_method(&String::from_utf8_lossy(name))
            }
            Type::ObjectWithoutClass | Type::Iterable(..) | Type::Callable(_) => Tri::Maybe,
            t => {
                if matches!(self.finds_method(t, name), Ok(None)) {
                    Tri::No
                } else {
                    Tri::Yes
                }
            }
        })
    }

    fn classes_with(&self, t: &Type, name: &[u8]) -> usize {
        match t {
            Type::Union(ts) | Type::Benevolent(ts) | Type::Intersection(ts) => {
                ts.iter().map(|t| self.classes_with(t, name)).sum()
            }
            Type::Template(t) => self.classes_with(&t.bound, name),
            t => usize::from(matches!(self.finds_method(t, name), Ok(Some(())))),
        }
    }

    fn get_method(&self, t: &Type, name: &[u8]) -> Option<members::Method<'a>> {
        self.method_of_type(t, name).ok().flatten()
    }
}

fn without_requirements(c: &Checker, ty: &TUnion) -> Option<TUnion> {
    let mut changed: Option<TUnion> = None;
    for (i, atomic) in ty.types.iter().enumerate() {
        if let Some(atomic) = without_atomic_requirements(c, atomic) {
            changed.get_or_insert_with(|| ty.clone()).types.to_mut()[i] = atomic;
        }
    }
    changed
}

fn without_atomic_requirements(c: &Checker, atomic: &TAtomic) -> Option<TAtomic> {
    match atomic {
        TAtomic::Object(TObject::Named(named)) => {
            let more = named.intersection_types.as_ref()?;
            let class = c.codebase.get_class_like(named.name.as_bytes())?;
            let mut changed: Option<Vec<TAtomic>> = None;
            for required in class
                .require_extends
                .iter()
                .chain(&class.require_implements)
            {
                let appended = TAtomic::Object(TObject::Named(TNamedObject::new(*required)));
                let index = changed
                    .as_ref()
                    .unwrap_or(more)
                    .iter()
                    .rposition(|t| t == &appended)?;
                changed
                    .get_or_insert_with(|| more.as_ref().clone())
                    .remove(index);
            }
            changed.map(|more| {
                let mut named = named.clone();
                named.intersection_types = Some(Box::new(more));
                TAtomic::Object(TObject::Named(named))
            })
        }
        TAtomic::GenericParameter(t) => {
            let constraint = without_requirements(c, &t.constraint)?;
            let mut t = t.clone();
            t.constraint = std::sync::Arc::new(constraint);
            Some(TAtomic::GenericParameter(t))
        }
        TAtomic::Scalar(TScalar::ClassLikeString(TClassLikeString::OfType {
            kind,
            constraint,
        })) => {
            let constraint = without_atomic_requirements(c, constraint)?;
            Some(TAtomic::Scalar(TScalar::ClassLikeString(
                TClassLikeString::OfType {
                    kind: *kind,
                    constraint: std::sync::Arc::new(constraint),
                },
            )))
        }
        _ => None,
    }
}

fn describe(c: &Checker, t: &Type) -> String {
    let t = match t {
        Type::Static(n, args) => Type::Object(n.clone(), args.clone(), false),
        Type::This(n) => Type::Object(n.clone(), vec![], false),
        t => t.clone(),
    };
    declared::with_classes(c, || t.describe(Level::TypeOnly))
}

pub(super) fn non_null(t: Type) -> Type {
    if t.is_null().yes() {
        t
    } else {
        t.remove_null()
    }
}

pub(super) fn typed(object: &Expression, spans: &mut Vec<Span>) {
    if !is_this(object) {
        spans.push(object.span());
    }
}

pub(in super::super) fn method_call<'arena>(
    c: &mut Checker<'_, 'arena>,
    span: Span,
    object: &Expression<'arena>,
    selector: &ClassLikeMemberSelector<'arena>,
    args: &ArgumentList<'arena>,
    nullsafe: bool,
) {
    check_method(c, object, selector, Some((span, args)), nullsafe);
}

pub(in super::super) fn check_method<'arena>(
    c: &mut Checker<'_, 'arena>,
    object: &Expression<'arena>,
    selector: &ClassLikeMemberSelector<'arena>,
    arguments: Option<(Span, &ArgumentList<'arena>)>,
    nullsafe: bool,
) {
    if !c.checks_expression(object) {
        return;
    }
    let dynamic = match selector {
        ClassLikeMemberSelector::Identifier(name) => {
            if arguments.is_some() && is_this(object) && !c.this_retyped(object) {
                return;
            }
            check_single_method(c, object, name, arguments, nullsafe, None, false);
            return;
        }
        ClassLikeMemberSelector::Variable(variable) => &Expression::Variable(variable.clone()),
        ClassLikeMemberSelector::Expression(expression) => expression.expression,
        ClassLikeMemberSelector::Missing(_) => return,
    };
    let Some((span, _)) = arguments else {
        return;
    };
    if is_this(object) && !c.this_defined() {
        return;
    }
    let scopes = c.named_call_scopes.get(&span).cloned().unwrap_or_default();
    for scope in &scopes {
        let at = super::dynamic_names::member(selector).unwrap_or(selector.span());
        let name = LocalIdentifier {
            span: at,
            value: scope.name.as_bytes(),
        };
        let method_exists = c.in_dynamic_method_exists(object, dynamic, &scope.name.to_string());
        check_single_method(
            c,
            object,
            &name,
            arguments,
            nullsafe,
            Some(scope),
            method_exists,
        );
    }
}

fn check_single_method<'arena>(
    c: &mut Checker<'_, 'arena>,
    object: &Expression<'arena>,
    name: &LocalIdentifier,
    arguments: Option<(Span, &ArgumentList<'arena>)>,
    nullsafe: bool,
    scope: Option<&super::super::infer::NamedCallScope>,
    method_exists: bool,
) {
    let ty = match scope {
        Some(scope) => c.phpstan_type_in(object, &scope.types),
        None => c.phpstan_type(object),
    };
    let short_circuit = match scope {
        Some(scope) => scope.types.get(&object.span()),
        None => c.type_of(object),
    }
    .is_some_and(|t| t.has_nullsafe_null());
    let ty = declared::with_classes(c, || {
        ty.map(|t| {
            if nullsafe {
                non_null(t)
            } else if short_circuit && super::super::assign::contains_nullsafe(object) {
                t.remove_null()
            } else {
                t
            }
        })
    });
    let found = if arguments.is_none() {
        callable_receiver(c, object, name, false)
    } else {
        let mut unknown = Vec::new();
        let found = declared::with_classes(c, || {
            ty.and_then(|ty| {
                c.find_in(
                    c.levels(),
                    ty,
                    &|t| c.can_call_methods(t).yes() && c.has_method(t, name.value).yes(),
                    Some(&mut unknown),
                )
            })
        });
        for class in unknown {
            let method = String::from_utf8_lossy(name.value);
            c.report(
                name,
                "class.notFound",
                format!("Call to method {method}() on an unknown class {class}."),
                Some(super::super::DISCOVERING_SYMBOLS_TIP),
            );
        }
        found
    };
    let Some(t) = found else {
        return;
    };
    let method_name = String::from_utf8_lossy(name.value).into_owned();
    if !c.can_call_methods(&t).yes() || t.is_string().yes() {
        let message = format!("Cannot call method {method_name}() on {}.", describe(c, &t));
        c.report(name, "method.nonObject", message, None);
        return;
    }
    if !c.has_method(&t, name.value).yes() {
        if method_exists {
            return;
        }
        if let [n] = t.object_class_names().as_slice()
            && let Lookup::Found(k) = c.class_like(n)
        {
            let own: Vec<Type> = declared::class_templates(c, k)
                .into_iter()
                .map(|t| Type::Template(Box::new(t)))
                .collect();
            let child = declared::original_name(k);
            let mut parent = k
                .direct_parent_class
                .and_then(|p| c.codebase.get_class_like(p.as_bytes()));
            while let Some(p) = parent {
                let ancestor = declared::original_name(p);
                let args = declared::with_classes(c, || {
                    pun_phpstan::symbols::lookup(&child)
                        .and_then(|s| pun_phpstan::types::ancestor_args(&s, &ancestor, &own))
                        .unwrap_or_default()
                });
                let reflection = Type::Object(ancestor, args.clone(), false);
                if c.has_method(&reflection, name.value).yes() {
                    let method = c
                        .reflected_method(&reflection, name.value)
                        .ok()
                        .flatten()
                        .map_or(method_name, |m| m.name);
                    let display = declared::with_classes(c, || {
                        let args: Vec<String> =
                            args.iter().map(|a| a.describe(Level::TypeOnly)).collect();
                        if args.is_empty() {
                            c.display(p)
                        } else {
                            format!("{}<{}>", c.display(p), args.join(","))
                        }
                    });
                    let message =
                        format!("Call to private method {method}() of parent class {display}.");
                    c.report(name, "method.private", message, None);
                    return;
                }
                parent = p
                    .direct_parent_class
                    .and_then(|p| c.codebase.get_class_like(p.as_bytes()));
            }
        }
        let message = format!(
            "Call to an undefined method {}::{method_name}().",
            describe(c, &t)
        );
        c.report(name, "method.notFound", message, None);
        return;
    }
    let Some(mut method) = c.reflected_method(&t, name.value).ok().flatten() else {
        return;
    };
    let declaring = &method.display;
    let visibility = method.visibility();
    let forwarded =
        (c.class_reflection_extensions()).any(|e| e.calls_inaccessible_method(c, &t, &method.name));
    if !forwarded
        && !calls::can_call(
            c,
            c.class_scope(),
            &method.declaring.declaring_name(),
            visibility,
        )
    {
        let v = super::super::restricted(visibility);
        let identifier = if visibility.is_private() {
            "method.private"
        } else {
            "method.protected"
        };
        let message = format!("Call to {v} method {}() of class {declaring}.", method.name);
        c.report(name, identifier, message, None);
    }
    let Some((span, args)) = arguments else {
        if !method.compound {
            calls::method_name_case(c, name, &method.declaring, declaring, false);
        }
        return;
    };
    if scope.is_some() && !method.compound {
        calls::method_name_case(c, name, &method.declaring, declaring, false);
    }
    let subject = format!("{declaring}::{}()", method.name);
    let first_level = c.is_top_level(span);
    let counts = params::count(args);
    let selected_scope = scope.cloned().or_else(|| {
        c.named_call_scopes
            .get(&span)
            .and_then(|scopes| scopes.iter().find(|scope| !scope.filtered))
            .cloned()
    });
    if method.name.eq_ignore_ascii_case("bindTo")
        && !args.arguments.is_empty()
        && let Expression::Variable(Variable::Direct(v)) = object
        && declared::with_classes(c, || {
            is_super_type_of(&Type::Object("Closure".into(), vec![], false), &t).yes()
        })
    {
        let var = String::from_utf8_lossy(&v.name[1..]);
        method.variants =
            super::super::selector::closure_bind(c, &var, span.start.offset, 0, method.variants);
    }
    let acceptor = method.select(c, &counts, selected_scope.as_ref());
    let void_message = Some(format!("Result of method {subject} (void) is used."));
    let subject = format!("Method {subject}");
    let call = params::Call {
        kind: params::CallKind::Method,
        subject: &subject,
        scope: Some(span),
        void_message,
    };
    params::check_bound_in(
        c,
        &acceptor,
        &counts,
        name,
        call,
        first_level,
        selected_scope.as_ref(),
        !method.compound,
    );
}

pub(super) fn typed_class(class: &Expression, spans: &mut Vec<Span>) {
    if !matches!(
        class,
        Expression::Identifier(_)
            | Expression::Self_(_)
            | Expression::Static(_)
            | Expression::Parent(_)
    ) {
        spans.push(class.span());
    }
}

pub(in super::super) fn static_call<'arena>(
    c: &mut Checker<'_, 'arena>,
    call: &StaticMethodCall<'arena>,
) {
    check_static(c, call.class, &call.method, Some(call));
}

pub(in super::super) fn check_static<'arena>(
    c: &mut Checker<'_, 'arena>,
    class: &Expression<'arena>,
    selector: &ClassLikeMemberSelector<'arena>,
    call: Option<&StaticMethodCall<'arena>>,
) {
    if !c.checks_expression(class)
        || matches!(
            class,
            Expression::Identifier(_)
                | Expression::Self_(_)
                | Expression::Static(_)
                | Expression::Parent(_)
        )
    {
        return;
    }
    let ClassLikeMemberSelector::Identifier(name) = selector else {
        return;
    };
    let ty = c.phpstan_type(class).map(|t| {
        if super::super::assign::contains_nullsafe(class) {
            t.remove_null()
        } else {
            t
        }
    });
    let found = if call.is_none() {
        callable_receiver(c, class, name, true)
    } else {
        let c = &*c;
        c.find_type_to_check_of(ty, &|t| {
            c.can_call_methods(t).yes() && c.has_method(t, name.value).yes()
        })
    };
    let Some(mut t) = found else {
        return;
    };
    if let Type::ClassString(Some(g)) = &t {
        if !is_super_type_of(&Type::ObjectWithoutClass, g).yes() {
            return;
        }
        t = (**g).clone();
    } else if t.is_string().yes() {
        return;
    }
    let described = describe(c, &t);
    if let Type::Union(ts) = &t {
        t = pun_phpstan::types::union(
            ts.iter()
                .filter(|t| !t.is_string().yes())
                .cloned()
                .collect(),
        );
    }
    let method_name = String::from_utf8_lossy(name.value).into_owned();
    if !c.can_call_methods(&t).yes() {
        let message = format!("Cannot call static method {method_name}() on {described}.");
        c.report(name, "staticMethod.nonObject", message, None);
        return;
    }
    if !c.has_method(&t, name.value).yes() {
        if call.is_none()
            && c.level == 0
            && t.object_class_names().iter().any(|n| {
                c.class_like(n)
                    .found()
                    .is_some_and(|k| members::method(c, k, b"__callStatic").is_some())
            })
        {
            return;
        }
        let message = format!("Call to an undefined static method {described}::{method_name}().");
        c.report(name, "staticMethod.notFound", message, None);
        return;
    }
    let method = c.get_method(&t, name.value);
    let Some(method) = method else { return };
    let declaring = method.declaring_display(c);
    let declared = method.name();
    let forwards =
        |n: &String, e: &dyn super::super::class_reflection::ClassReflectionExtension| {
            (c.class_like(n).found())
                .is_some_and(|k| e.static_forward(c, k, &method.declaring_name()).is_some())
        };
    // IntersectionType::getMethod(): one member of an intersection (the model of `Model&Publishable`) finds it
    let names = t.object_class_names();
    let forwarded = c.class_reflection_extensions().any(|e| {
        if matches!(t, Type::Intersection(_)) {
            names.iter().any(|n| forwards(n, e))
        } else {
            names.iter().all(|n| forwards(n, e))
        }
    });
    let forwarding: Vec<&String> = names
        .iter()
        .filter(|n| c.class_reflection_extensions().any(|e| forwards(n, e)))
        .collect();
    if forwarded
        && let Some(call) = call
        && let [model] = forwarding.as_slice()
        && let Some(k) = c.class_like(model).found()
    {
        forwarded_static_call(c, k, name, call);
        return;
    }
    if !method.is_static() && !forwarded {
        let in_instance_method = c.functions.last().is_some_and(|f| {
            f.kind == super::super::FunctionScopeKind::Method { is_static: false }
        });
        let in_class = c.class_scope().is_some_and(|s| {
            t.object_class_names().iter().all(|n| {
                s.name.as_bytes().eq_ignore_ascii_case(n.as_bytes())
                    || c.codebase.is_instance_of(s.name.as_bytes(), n.as_bytes())
            })
        });
        if !in_instance_method || !in_class {
            if declared.eq_ignore_ascii_case("loadhtml") && declaring == "DOMDocument" {
                return;
            }
            let message = format!("Static call to instance method {declaring}::{declared}().");
            c.report(name, "method.staticCall", message, None);
            return;
        }
    }
    let kind = if method.is_static() {
        "static method"
    } else {
        "method"
    };
    let visibility = method.visibility();
    if !calls::can_call(c, c.class_scope(), &method.declaring_name(), visibility) {
        let v = super::super::restricted(visibility);
        let identifier = if visibility.is_private() {
            "staticMethod.private"
        } else {
            "staticMethod.protected"
        };
        let message = format!("Call to {v} {kind} {declared}() of class {declaring}.");
        c.report(name, identifier, message, None);
    }
    let Some(call) = call else {
        calls::method_name_case(c, name, &method, &declaring, true);
        return;
    };
    if c.classes_with(&t, name.value) != 1 {
        return;
    }
    let subject = format!("{declaring}::{declared}()");
    let display_kind = if method.is_static() {
        "Static method"
    } else {
        "Method"
    };
    let first_level = c.is_top_level(call.span());
    let counts = params::count(&call.argument_list);
    params::check(
        c,
        &params::select(c, method.variants(), &counts),
        &counts,
        name,
        params::Call {
            kind: params::CallKind::StaticMethod,
            subject: &format!("{display_kind} {subject}"),
            scope: None,
            void_message: Some(format!("Result of {kind} {subject} (void) is used.")),
        },
        first_level,
    );
}

pub(in super::super) fn forwarded_static_call<'arena>(
    c: &mut Checker<'_, 'arena>,
    model: &mago_codex::metadata::class_like::ClassLikeMetadata,
    name: &LocalIdentifier,
    call: &StaticMethodCall<'arena>,
) -> bool {
    let own = Type::Object(declared::original_name(model), vec![], false);
    let Ok(Some(declared)) = c.method_of_type(&own, name.value) else {
        return false;
    };
    let declaring = declared.declaring_name();
    let Some(t) =
        (c.class_reflection_extensions()).find_map(|e| e.static_forward(c, model, &declaring))
    else {
        return false;
    };
    let Some(method) = c.reflected_method(&t, name.value).ok().flatten() else {
        return false;
    };
    let subject = format!("{}::{}()", describe(c, &t), method.name);
    let first_level = c.is_top_level(call.span());
    let counts = params::count(&call.argument_list);
    let selected_scope = c
        .named_call_scopes
        .get(&call.span())
        .and_then(|scopes| scopes.iter().find(|scope| !scope.filtered))
        .cloned();
    let acceptor = method.select(c, &counts, selected_scope.as_ref());
    let void_message = Some(format!("Result of static method {subject} (void) is used."));
    let subject = format!("Static method {subject}");
    let check = params::Call {
        kind: params::CallKind::StaticMethod,
        subject: &subject,
        scope: Some(call.span()),
        void_message,
    };
    params::check_bound_in(
        c,
        &acceptor,
        &counts,
        name,
        check,
        first_level,
        selected_scope.as_ref(),
        !method.compound,
    );
    true
}

fn callable_receiver(
    c: &mut Checker,
    e: &Expression,
    name: &LocalIdentifier,
    static_call: bool,
) -> Option<Type> {
    let mut ty = c.phpstan_type(e)?;
    if super::super::assign::contains_nullsafe(e)
        && c.type_of(e).is_some_and(|t| t.has_nullsafe_null())
    {
        ty = ty.remove_null();
    }
    let mut unknown = Vec::new();
    let found = c.find_in(
        c.levels(),
        ty,
        &|t| c.can_call_methods(t).yes() && c.has_method(t, name.value).yes(),
        Some(&mut unknown),
    );
    for class in unknown {
        let method = String::from_utf8_lossy(name.value);
        let kind = if static_call {
            "static method"
        } else {
            "method"
        };
        c.report(
            name,
            "class.notFound",
            format!("Call to {kind} {method}() on an unknown class {class}."),
            Some(super::super::DISCOVERING_SYMBOLS_TIP),
        );
    }
    found
}
