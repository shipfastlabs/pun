use crate::problem::{Fix, Safety};
use mago_codex::metadata::function_like::FunctionLikeMetadata;
use mago_codex::metadata::parameter::FunctionLikeParameterMetadata;
use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::atomic::object::TObject;
use mago_codex::ttype::atomic::scalar::TScalar;
use mago_codex::ttype::union::TUnion;
use mago_syntax::cst::{
    Access, Argument, ArgumentList, AssignmentOperator, ClassLikeMemberSelector, Expression,
    FunctionCall, FunctionLikeReturnTypeHint, MethodBody, MethodCall, Node, NodeKind,
    PartialArgument, PartialArgumentList, StaticMethodCall, Variable,
};
use pun_phpstan::resolve::resolve_str;
use pun_phpstan::types::{
    Accessory, NeverKind, StrictMixed, Type, accepts_with, decide_type, intersect, mixed,
};

use crate::rules::phpunit::require_strict_types_in_tests::{
    declare_strict_types, has_strict_types,
};
use crate::rules::support::names::resolve_function;
use crate::rules::support::statement_edits::contains_html;
use crate::rules::types::compare::classes::with_classes;
use crate::rules::types::compare::mapper::{TypeScope, expr_type, from_mago};
use crate::rules::types::compare::{Object, RType};
use crate::rules::types::facts::{Facts, facts, is_builtin_class, is_builtin_function};
use crate::rules::types::names::resolve_class_name;
use crate::rules::types::{any_scoped, class_scope, returns_scoped};
use crate::{Context, Report, Rule};

pub struct AddSafeStrictTypes;

impl Rule for AddSafeStrictTypes {
    fn id(&self) -> &'static str {
        "lint/statements/add-safe-strict-types"
    }

    fn min_php_version(&self) -> crate::PhpVersion {
        crate::PhpVersion::from_id(70000)
    }

    fn targets(&self) -> Option<&'static [NodeKind]> {
        Some(&[NodeKind::Program])
    }

    fn visit<'ast, 'arena>(
        &self,
        node: Node<'ast, 'arena>,
        ancestors: &[Node<'ast, 'arena>],
        ctx: &Context<'_>,
        out: &mut Vec<Report>,
    ) {
        let Node::Program(program) = node else { return };
        if has_strict_types(&program.statements)
            || contains_html(ctx, node, ancestors)
            || has_dynamic_call(node)
        {
            return;
        }
        let facts = facts(ctx);
        if !with_classes(facts.documented(), || {
            is_file_strict_type_safe(&facts, node)
        }) {
            return;
        }
        let Some((at, edit)) = declare_strict_types(ctx, &program.statements) else {
            return;
        };
        out.push(Report {
            span: at,
            message: "This file never relies on scalar type coercion: declare strict types."
                .to_string(),
            help: None,
            fix: Some(Fix {
                safety: Safety::Safe,
                edits: vec![edit],
            }),
        });
    }
}

/// A call that `is_node_safe` rejects without types (a nullsafe or dynamic
/// call), checked first so such a file skips type analysis.
fn has_dynamic_call(program: Node<'_, '_>) -> bool {
    let mut stack = vec![program];
    while let Some(node) = stack.pop() {
        let dynamic = match node {
            Node::FunctionCall(c) => !matches!(c.function, Expression::Identifier(_)),
            Node::MethodCall(c) => !matches!(c.method, ClassLikeMemberSelector::Identifier(_)),
            Node::StaticMethodCall(c) => {
                !matches!(c.method, ClassLikeMemberSelector::Identifier(_))
            }
            Node::NullSafeMethodCall(_) => true,
            _ => false,
        };
        if dynamic {
            return true;
        }
        node.visit_children(|child| stack.push(child));
    }
    false
}

fn is_file_strict_type_safe(facts: &Facts, program: Node<'_, '_>) -> bool {
    let mut stack = vec![(program, 0usize)];
    let mut path: Vec<Node<'_, '_>> = Vec::new();
    while let Some((node, depth)) = stack.pop() {
        path.truncate(depth);
        if !is_node_safe(facts, node, &path) {
            return false;
        }
        path.push(node);
        let mut children = Vec::new();
        node.visit_children(|child| children.push(child));
        stack.extend(children.into_iter().rev().map(|c| (c, depth + 1)));
    }
    true
}

fn is_node_safe(facts: &Facts, node: Node<'_, '_>, ancestors: &[Node<'_, '_>]) -> bool {
    let at = At { facts, ancestors };
    // `has_dynamic_call` has ruled out the calls these patterns leave out.
    match node {
        Node::FunctionCall(FunctionCall {
            function: Expression::Identifier(id),
            argument_list,
        }) => at
            .function_parameters(id)
            .is_some_and(|p| at.are_args_safe(&args(argument_list), &p)),
        Node::MethodCall(MethodCall {
            object,
            method: ClassLikeMemberSelector::Identifier(name),
            argument_list,
            ..
        }) => (at.single_class(object))
            .and_then(|class| at.method_parameters(&class, name.value))
            .is_some_and(|p| at.are_args_safe(&args(argument_list), &p)),
        Node::StaticMethodCall(StaticMethodCall {
            class,
            method: ClassLikeMemberSelector::Identifier(name),
            argument_list,
            ..
        }) => (at.class_of_reference(class))
            .and_then(|class| at.method_parameters(&class, name.value))
            .is_some_and(|p| at.are_args_safe(&args(argument_list), &p)),
        Node::Instantiation(new) => {
            let arguments = new.argument_list.as_ref().map_or_else(Vec::new, args);
            (at.class_of_reference(new.class))
                .and_then(|class| at.method_parameters(&class, b"__construct"))
                .is_some_and(|p| at.are_args_safe(&arguments, &p))
        }
        Node::AnonymousClass(class) => {
            let name = facts.anonymous_class_name(mago_span::HasSpan::span(class));
            let arguments = class
                .argument_list
                .as_ref()
                .map_or(Some(Vec::new()), partial_args);
            match (arguments, at.method_parameters(&name, b"__construct")) {
                (Some(arguments), Some(p)) => at.are_args_safe(&arguments, &p),
                _ => false,
            }
        }
        Node::Attribute(attribute) => {
            let class = resolve_class_name(ancestors, &attribute.name);
            let arguments = attribute
                .argument_list
                .as_ref()
                .map_or(Some(Vec::new()), partial_args);
            match (arguments, at.method_parameters(&class, b"__construct")) {
                (Some(arguments), Some(p)) => at.are_args_safe(&arguments, &p),
                _ => false,
            }
        }
        Node::Function(f) => {
            at.are_returns_safe(f.return_type_hint.as_ref(), Returns::Block(&f.body))
        }
        Node::Method(m) => match &m.body {
            MethodBody::Concrete(body) => {
                at.are_returns_safe(m.return_type_hint.as_ref(), Returns::Block(body))
            }
            MethodBody::Abstract(_) => true,
        },
        Node::Closure(c) => {
            at.are_returns_safe(c.return_type_hint.as_ref(), Returns::Block(&c.body))
        }
        Node::ArrowFunction(a) => at.are_returns_safe(
            a.return_type_hint.as_ref(),
            Returns::Expression(a.expression),
        ),
        Node::Assignment(a) if matches!(a.operator, AssignmentOperator::Assign(_)) => {
            at.is_property_assign_safe(a.lhs, a.rhs)
        }
        _ => true,
    }
}

struct Arg<'a> {
    value: &'a Expression<'a>,
    name: Option<&'a [u8]>,
    unpack: bool,
}

fn args<'a>(list: &'a ArgumentList<'a>) -> Vec<Arg<'a>> {
    (list.arguments.iter())
        .map(|a| match a {
            Argument::Positional(p) => Arg {
                value: p.value,
                name: None,
                unpack: p.ellipsis.is_some(),
            },
            Argument::Named(n) => Arg {
                value: n.value,
                name: Some(n.name.value),
                unpack: false,
            },
        })
        .collect()
}

fn partial_args<'a>(list: &'a PartialArgumentList<'a>) -> Option<Vec<Arg<'a>>> {
    (list.arguments.iter())
        .map(|a| {
            Some(Arg {
                value: a.value()?,
                name: match a {
                    PartialArgument::Named(n) => Some(n.name.value),
                    _ => None,
                },
                unpack: matches!(a, PartialArgument::Positional(p) if p.ellipsis.is_some()),
            })
        })
        .collect()
}

struct Param {
    name: Vec<u8>,
    r#type: Type,
    variadic: bool,
}

enum Returns<'a> {
    Block(&'a mago_syntax::cst::Block<'a>),
    Expression(&'a Expression<'a>),
}

struct At<'a, 'ast, 'arena> {
    facts: &'a Facts,
    ancestors: &'a [Node<'ast, 'arena>],
}

impl At<'_, '_, '_> {
    fn are_args_safe(&self, args: &[Arg<'_>], parameters: &[Param]) -> bool {
        args.iter().enumerate().all(|(position, arg)| {
            if arg.unpack {
                return false;
            }
            let parameter = match arg.name {
                Some(name) => parameters.iter().find(|p| p.name == name),
                None => parameters
                    .get(position)
                    .or_else(|| parameters.last().filter(|p| p.variadic)),
            };
            let Some(parameter) = parameter else {
                return false;
            };
            is_type_safe_for_strict_mode(
                &parameter.r#type,
                &expr_type(self.facts, self.ancestors, arg.value, true),
            )
        })
    }

    fn are_returns_safe(
        &self,
        hint: Option<&FunctionLikeReturnTypeHint<'_>>,
        returns: Returns<'_>,
    ) -> bool {
        let Some(hint) = hint else {
            return true;
        };
        let declared = declared_type(
            &TypeScope::new(self.ancestors, self.facts.documented()).map_hint(&hint.hint),
        );
        // `self` and `parent` map to a never that accepts nothing (Rector's `SelfStaticType`), not the `never` type.
        if matches!(declared, Type::Mixed { .. })
            || matches!(declared, Type::Never(kind) if kind != NeverKind::NonAccepting)
            || declared.is_void().yes()
        {
            return true;
        }
        let values: Vec<&Expression<'_>> = match returns {
            Returns::Block(body) => returns_scoped(body)
                .into_iter()
                .filter_map(|r| r.value)
                .collect(),
            Returns::Expression(e) => {
                let yields = matches!(e, Expression::Yield(_))
                    || any_scoped(Node::Expression(e), &mut |n| matches!(n, Node::Yield(_)));
                if yields { vec![] } else { vec![e] }
            }
        };
        values.into_iter().all(|value| {
            is_type_safe_for_strict_mode(
                &declared,
                &expr_type(self.facts, self.ancestors, value, true),
            )
        })
    }

    fn is_property_assign_safe(&self, var: &Expression<'_>, value: &Expression<'_>) -> bool {
        let (class, name) = match var {
            Expression::Access(Access::Property(p)) => {
                let ClassLikeMemberSelector::Identifier(id) = &p.property else {
                    return false;
                };
                (self.single_class(p.object), [b"$", id.value].concat())
            }
            Expression::Access(Access::StaticProperty(p)) => {
                let Variable::Direct(v) = &p.property else {
                    return false;
                };
                (self.class_of_reference(p.class), v.name.to_vec())
            }
            _ => return true,
        };
        let Some(property) = class.and_then(|c| {
            self.facts
                .codebase
                .get_declaring_property(c.as_bytes(), &name)
        }) else {
            return false;
        };
        let declared = property
            .type_declaration_metadata
            .as_ref()
            .map_or_else(mixed, |t| from_mago(&t.type_union));
        is_type_safe_for_strict_mode(
            &declared,
            &expr_type(self.facts, self.ancestors, value, true),
        )
    }

    fn single_class(&self, expr: &Expression<'_>) -> Option<String> {
        // Rector's PropertyFetchTypeResolver types a fetch on a known class's property by the property's readable type,
        // so an `instanceof` that narrowed `$event->account` does not reach the call made on it.
        if let Expression::Access(Access::Property(p)) = crate::syntax::unparenthesize(expr)
            && let ClassLikeMemberSelector::Identifier(id) = &p.property
            && let Some(class) = self.single_class(p.object)
            && let Some(t) = (self.facts.documented())
                .get_declaring_property(class.as_bytes(), &[b"$", id.value].concat())
                .and_then(|property| property.type_metadata.as_ref())
        {
            return match t.type_union.types.as_ref() {
                [TAtomic::Object(TObject::Named(n))] if n.intersection_types.is_none() => {
                    Some(n.name.to_string())
                }
                [TAtomic::Object(TObject::Enum(e))] => Some(e.name.to_string()),
                _ => None,
            };
        }
        let span = mago_span::HasSpan::span(expr);
        let span = crate::problem::Span::new(span.start.offset as usize, span.end.offset as usize);
        self.facts.native_type(span)?;
        let t = self.facts.full_type(span)?;
        match t.types.as_ref() {
            [TAtomic::Object(TObject::Named(n))] if n.intersection_types.is_none() => {
                Some(n.name.to_string())
            }
            [TAtomic::Object(TObject::Enum(e))] => Some(e.name.to_string()),
            _ => None,
        }
    }

    fn class_of_reference(&self, class: &Expression<'_>) -> Option<String> {
        let scope = || class_scope(self.ancestors);
        match class {
            Expression::Identifier(id) => Some(resolve_class_name(self.ancestors, id)),
            Expression::Self_(_) | Expression::Static(_) => scope()?.name,
            Expression::Parent(_) => scope()?.parent_class,
            other => self.single_class(other),
        }
    }

    fn function_parameters(&self, id: &mago_syntax::cst::Identifier<'_>) -> Option<Vec<Param>> {
        let (resolved, explicit) = resolve_function(self.ancestors, id);
        let written = id.value().strip_prefix(b"\\").unwrap_or(id.value());
        let codebase = &self.facts.codebase;
        let name =
            if explicit || codebase.get_function(&resolved).is_some() || written.contains(&b'\\') {
                resolved
            } else {
                written.to_vec()
            };
        if is_builtin_function(&name) {
            return builtin_parameters(pun_phpstan::symbols::function(&String::from_utf8_lossy(
                &name,
            ))?);
        }
        let native = codebase.get_function(&name)?;
        Some(user_parameters(
            native,
            self.facts.documented().get_function(&name),
        ))
    }

    fn method_parameters(&self, class: &str, method: &[u8]) -> Option<Vec<Param>> {
        let documented = self.facts.documented();
        documented.get_class_like(class.as_bytes())?;
        let declaring = documented.get_declaring_method_class(class.as_bytes(), method)?;
        if is_builtin_class(declaring.as_bytes()) {
            let builtin = pun_phpstan::symbols::class(&declaring.to_string())?;
            return builtin_parameters(
                builtin
                    .methods
                    .get(&String::from_utf8_lossy(method).to_ascii_lowercase())?,
            );
        }
        let doc = documented.get_declaring_method(class.as_bytes(), method)?;
        let native = self
            .facts
            .codebase
            .get_declaring_method(class.as_bytes(), method);
        Some(user_parameters(native.unwrap_or(doc), Some(doc)))
    }
}

fn builtin_parameters(function: &pun_phpstan::symbols::Function) -> Option<Vec<Param>> {
    let variant = function.variants.first()?;
    Some(
        (variant.params.iter())
            .map(|p| Param {
                name: p.name.trim_start_matches('$').as_bytes().to_vec(),
                r#type: resolve_str(&p.ty, None),
                variadic: p.variadic,
            })
            .collect(),
    )
}

fn user_parameters(
    native: &FunctionLikeMetadata,
    documented: Option<&FunctionLikeMetadata>,
) -> Vec<Param> {
    (native.parameters.iter())
        .map(|p| {
            let doc = documented
                .and_then(|d| d.parameters.iter().find(|q| q.name == p.name))
                .and_then(FunctionLikeParameterMetadata::get_type_metadata)
                .filter(|t| t.from_docblock)
                .map(|t| doc_type(&t.type_union));
            let declared = p
                .type_declaration_metadata
                .as_ref()
                .map_or_else(mixed, |t| from_mago(&t.type_union));
            Param {
                name: p.name.0.as_bytes()[1..].to_vec(),
                r#type: decide_type(declared, doc),
                variadic: p.flags.is_variadic(),
            }
        })
        .collect()
}

fn doc_type(t: &TUnion) -> Type {
    let members: Vec<Type> = (t.types.iter())
        .map(|a| match a {
            TAtomic::Scalar(TScalar::String(s)) if s.literal.is_none() => {
                let accessory = if s.is_numeric {
                    Some(Accessory::NumericString)
                } else if s.is_truthy {
                    Some(Accessory::NonFalsyString)
                } else if s.is_non_empty {
                    Some(Accessory::NonEmptyString)
                } else {
                    None
                };
                accessory.map_or(Type::String, |accessory| {
                    intersect(vec![Type::String, Type::Accessory(accessory)])
                })
            }
            a => from_mago(&TUnion::from_atomic(a.clone())),
        })
        .collect();
    match members.len() {
        1 => members.into_iter().next().expect("one member"),
        _ => pun_phpstan::types::union(members),
    }
}

fn declared_type(t: &RType) -> Type {
    match t {
        RType::Object(Object::SelfStatic(_) | Object::ParentStatic(_)) => {
            Type::Never(NeverKind::NonAccepting)
        }
        RType::Union(types) => Type::Union(types.iter().map(declared_type).collect()),
        other => other.to_super(),
    }
}

fn is_type_safe_for_strict_mode(declared: &Type, value: &Type) -> bool {
    accepts_with(
        declared,
        value,
        true,
        StrictMixed {
            explicit: true,
            implicit: true,
        },
    )
    .yes()
}
