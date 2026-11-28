use std::sync::Arc;

use mago_analyzer::plugin::context::{InvocationInfo, ProviderContext};
use mago_analyzer::plugin::provider::function::{FunctionReturnTypeProvider, FunctionTarget};
use mago_analyzer::plugin::provider::method::{MethodReturnTypeProvider, MethodTarget};
use mago_analyzer::plugin::provider::{Provider, ProviderMeta};
use mago_codex::metadata::CodebaseMetadata;
use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::atomic::array::key::ArrayKey;
use mago_codex::ttype::atomic::array::keyed::KnownItems;
use mago_codex::ttype::atomic::object::TObject;
use mago_codex::ttype::atomic::object::named::TNamedObject;
use mago_codex::ttype::atomic::scalar::TScalar;
use mago_codex::ttype::atomic::scalar::int::TInteger;
use mago_codex::ttype::combiner::CombinerOptions;
use mago_codex::ttype::expander::StaticClassType;
use mago_codex::ttype::union::TUnion;
use mago_codex::ttype::{
    add_union_type, get_bool, get_mixed, get_mixed_keyed_array, get_null,
    get_specialized_template_type, get_string,
};
use mago_span::{HasSpan, Span};
use mago_syntax::cst::{
    BinaryOperator, Call, Expression, Return, Statement, UnaryPrefixOperator, Variable,
};

use crate::larastan::app::App;

static META: ProviderMeta = ProviderMeta::new(
    "pun-larastan-app",
    "pun-larastan-app",
    "Larastan's app extensions",
);

const GUARD: &str = "Illuminate\\Contracts\\Auth\\Guard";
const STATEFUL_GUARD: &str = "Illuminate\\Contracts\\Auth\\StatefulGuard";
const ENUMERABLE: &str = "Illuminate\\Support\\Enumerable";
const COLLECTION: &str = "Illuminate\\Support\\Collection";
const ELOQUENT_COLLECTION: &str = "Illuminate\\Database\\Eloquent\\Collection";

fn object(codebase: &CodebaseMetadata, class: &str) -> TAtomic {
    let name = codebase.get_class_like(class.as_bytes()).map_or_else(
        || mago_word::word(class.trim_start_matches('\\')),
        |k| k.original_name,
    );
    TAtomic::Object(TObject::Named(TNamedObject::new(name)))
}

fn auth_models(codebase: &CodebaseMetadata, models: Vec<String>) -> Option<TUnion> {
    if models.is_empty() {
        return None;
    }
    let mut types: Vec<TAtomic> = models.iter().map(|m| object(codebase, m)).collect();
    types.push(TAtomic::Null);
    Some(TUnion::from_vec(types))
}

fn stateful_guard(codebase: &CodebaseMetadata) -> TUnion {
    let TAtomic::Object(TObject::Named(mut guard)) = object(codebase, GUARD) else {
        unreachable!()
    };
    guard.intersection_types = Some(Box::new(vec![object(codebase, STATEFUL_GUARD)]));
    TUnion::from_atomic(TAtomic::Object(TObject::Named(guard)))
}

fn constant_string(context: &ProviderContext<'_, '_, '_>, span: Span) -> Option<String> {
    let ty = context.get_expression_type(&span)?;
    match ty.types.as_ref() {
        [TAtomic::Scalar(TScalar::String(s))] => s
            .get_known_literal_value()
            .map(|v| String::from_utf8_lossy(v).into_owned()),
        _ => None,
    }
}

pub(crate) struct Functions(pub(crate) Arc<App>, pub(crate) bool);

impl Provider for Functions {
    fn meta() -> &'static ProviderMeta {
        &META
    }
}

impl FunctionReturnTypeProvider for Functions {
    fn targets() -> FunctionTarget {
        FunctionTarget::ExactMultiple(&[b"auth", b"now", b"today", b"__", b"trans", b"collect"])
    }

    fn get_return_type(
        &self,
        context: &ProviderContext<'_, '_, '_>,
        invocation: &InvocationInfo<'_, '_, '_>,
    ) -> Option<TUnion> {
        let codebase = context.codebase();
        let name = invocation.function_name().to_ascii_lowercase();
        match name.as_str() {
            "auth" if invocation.has_no_arguments() => {
                let factory = "Illuminate\\Contracts\\Auth\\Factory";
                let class = self
                    .0
                    .resolve(factory)
                    .filter(|c| codebase.class_exists(c.as_bytes()));
                Some(TUnion::from_atomic(object(
                    codebase,
                    class.unwrap_or(factory),
                )))
            }
            "auth" => Some(stateful_guard(codebase)),
            "now" | "today" => Some(TUnion::from_atomic(object(codebase, &self.0.date_class))),
            "collect" => collect(context, invocation, self.1),
            "__" if invocation.has_no_arguments() => Some(get_null()),
            "trans" if invocation.has_no_arguments() => Some(TUnion::from_atomic(object(
                codebase,
                "Illuminate\\Contracts\\Translation\\Translator",
            ))),
            "__" | "trans" => Some(translation()),
            _ => None,
        }
    }
}

fn translation() -> TUnion {
    let mut ty = TUnion::from_vec(
        get_mixed_keyed_array()
            .types
            .iter()
            .chain(get_string().types.iter())
            .cloned()
            .collect(),
    );
    ty.set_ignore_nullable_issues(true);
    ty.set_ignore_falsable_issues(true);
    ty
}

pub(crate) struct Methods(pub(crate) Arc<App>, pub(crate) Vec<u8>);

impl Provider for Methods {
    fn meta() -> &'static ProviderMeta {
        &META
    }
}

impl MethodReturnTypeProvider for Methods {
    fn targets() -> &'static [MethodTarget] {
        static TARGETS: [MethodTarget; 1] = [MethodTarget::any_class(b"*")];
        &TARGETS
    }

    fn get_return_type(
        &self,
        context: &ProviderContext<'_, '_, '_>,
        class: &[u8],
        method: &[u8],
        invocation: &InvocationInfo<'_, '_, '_>,
    ) -> Option<TUnion> {
        let codebase = context.codebase();
        let target = invocation.inner().target.get_method_context();
        let receiver = target.map_or(class, |t| t.class_like_metadata.name.as_bytes());
        let is = |parent: &str| {
            receiver.eq_ignore_ascii_case(parent.as_bytes())
                || codebase.is_instance_of(receiver, parent.as_bytes())
        };
        let method_name = String::from_utf8_lossy(method).to_ascii_lowercase();
        match method_name.as_str() {
            "factory"
                if codebase.is_instance_of(receiver, b"Illuminate\\Database\\Eloquent\\Model") =>
            {
                return super::factories::model_factory(context, receiver, invocation);
            }
            _ if is("Illuminate\\Database\\Eloquent\\Factories\\Factory") => {
                let span = self.receiver(invocation, method);
                return super::factories::factory_method(context, method, invocation, span);
            }
            "user"
                if is("Illuminate\\Support\\Facades\\Auth")
                    || is("Illuminate\\Auth\\AuthManager") =>
            {
                return auth_models(codebase, self.0.auth_models(None));
            }
            "user" if is("Illuminate\\Http\\Request") => {
                let declaring = codebase.get_declaring_method_class(receiver, b"user")?;
                if !declaring
                    .as_bytes()
                    .eq_ignore_ascii_case(b"Illuminate\\Http\\Request")
                {
                    return None;
                }
                let guard = match invocation.arguments().as_slice() {
                    [argument] => constant_string(context, argument.value()?.span()),
                    _ => None,
                };
                return auth_models(codebase, self.0.auth_models(guard.as_deref()));
            }
            "user" if is(GUARD) => {
                let guard = self
                    .receiver_argument(invocation)
                    .and_then(|a| constant_string(context, a));
                return auth_models(codebase, self.0.auth_models(guard.as_deref()));
            }
            "get" if is("Illuminate\\Contracts\\Translation\\Translator") => {
                return Some(translation());
            }
            "guard"
                if receiver.eq_ignore_ascii_case(b"Illuminate\\Support\\Facades\\Auth")
                    || class.eq_ignore_ascii_case(b"Illuminate\\Support\\Facades\\Auth") =>
            {
                let class = match invocation.arguments().as_slice() {
                    [] => self.0.guard_class(None),
                    [argument, ..] => constant_string(context, argument.value()?.span())
                        .and_then(|guard| self.0.guard_class(Some(&guard))),
                };
                return Some(match class {
                    Some(class) => TUnion::from_atomic(object(codebase, class)),
                    None => stateful_guard(codebase),
                });
            }
            "create"
            | "createfromdate"
            | "createfromtime"
            | "createfromtimestring"
            | "createfromtimestamp"
            | "createfromtimestampms"
            | "createfromtimestamputc"
            | "createmidnightdate"
            | "fromserialized"
            | "gettestnow"
            | "instance"
            | "maxvalue"
            | "minvalue"
            | "now"
            | "parse"
            | "today"
            | "tomorrow"
            | "yesterday"
            | "createfromformat"
            | "createsafe"
            | "make"
                if is("Illuminate\\Support\\Facades\\Date") =>
            {
                let date = object(codebase, &self.0.date_class);
                return Some(TUnion::from_vec(match method_name.as_str() {
                    "gettestnow" | "make" => vec![date, TAtomic::Null],
                    "createfromformat" | "createsafe" if self.0.laravel_12 => {
                        vec![date, TAtomic::Null]
                    }
                    "createfromformat" | "createsafe" => {
                        vec![date, TAtomic::Scalar(TScalar::r#false())]
                    }
                    _ => vec![date],
                }));
            }
            "safe" if is("Illuminate\\Foundation\\Http\\FormRequest") => {
                return form_request_safe(context, invocation);
            }
            "disk" | "drive" | "cloud" if is("Illuminate\\Support\\Facades\\Storage") => {
                return Some(TUnion::from_atomic(object(
                    codebase,
                    "Illuminate\\Filesystem\\FilesystemAdapter",
                )));
            }
            "filter" | "reject" | "where" if is(ENUMERABLE) => {
                return self.filter_reject(context, &method_name, invocation);
            }
            "wherenotnull" if is(ENUMERABLE) => return self.where_not_null(context, invocation),
            "map" | "mapwithkeys" if is(ELOQUENT_COLLECTION) => {
                if let Some(ty) = eloquent_collection_map(context, &method_name, invocation) {
                    return Some(ty);
                }
            }
            "environment"
                if is("Illuminate\\Foundation\\Application")
                    || is("Illuminate\\Contracts\\Foundation\\Application")
                    || is("Illuminate\\Support\\Facades\\App") =>
            {
                return Some(if invocation.has_no_arguments() {
                    get_string()
                } else {
                    get_bool()
                });
            }
            _ => {}
        }
        let k = codebase.get_class_like(receiver)?;
        if !k.kind.is_interface()
            || !k
                .original_name
                .as_bytes()
                .starts_with(b"Illuminate\\Contracts")
            || codebase.method_exists(receiver, method)
        {
            return None;
        }
        let concrete = self
            .0
            .resolve(&String::from_utf8_lossy(k.original_name.as_bytes()))?;
        let concrete = codebase.get_class_like(concrete.as_bytes())?;
        if !codebase.method_exists(concrete.name.as_bytes(), method) {
            return None;
        }
        super::forwards::method_type(
            codebase,
            concrete.name.as_bytes(),
            &TNamedObject::new(concrete.original_name),
            method,
            Some((context, invocation)),
        )
    }
}

fn form_request_safe(
    context: &ProviderContext<'_, '_, '_>,
    invocation: &InvocationInfo<'_, '_, '_>,
) -> Option<TUnion> {
    use mago_codex::ttype::atomic::array::TArray;
    use mago_codex::ttype::atomic::array::key::ArrayKey;
    use mago_codex::ttype::atomic::array::keyed::TKeyedArray;
    let argument = invocation.arguments().into_iter().next()?.value()?;
    let ty = context.get_expression_type(argument)?;
    let values: Vec<&TUnion> = match ty.types.as_ref() {
        [TAtomic::Array(TArray::List(list))] if list.element_type.is_never() => list
            .known_elements
            .iter()
            .flat_map(|k| k.values())
            .map(|(_, v)| v)
            .collect(),
        [TAtomic::Array(TArray::Keyed(keyed))] if keyed.parameters.is_none() => keyed
            .known_items
            .iter()
            .flat_map(|k| k.values())
            .map(|(_, v)| v)
            .collect(),
        _ => return None,
    };
    let items: KnownItems = values
        .iter()
        .flat_map(|v| v.types.iter())
        .filter_map(|a| match a {
            TAtomic::Scalar(TScalar::String(s)) => s.get_known_literal_value(),
            _ => None,
        })
        .map(|key| {
            (
                ArrayKey::String(mago_word::Word::new(key)),
                (false, get_mixed()),
            )
        })
        .collect();
    let non_empty = !items.is_empty();
    Some(TUnion::from_atomic(TAtomic::Array(TArray::Keyed(
        TKeyedArray::new()
            .with_known_items(items)
            .with_non_empty(non_empty),
    ))))
}

fn benevolent_array_key() -> TUnion {
    let mut ty = TUnion::from_vec(vec![
        TAtomic::Scalar(TScalar::int()),
        TAtomic::Scalar(TScalar::string()),
    ]);
    ty.set_ignore_nullable_issues(true);
    ty.set_ignore_falsable_issues(true);
    ty
}

pub(super) fn generic(codebase: &CodebaseMetadata, class: &str, arguments: Vec<TUnion>) -> TUnion {
    let TAtomic::Object(TObject::Named(object)) = object(codebase, class) else {
        unreachable!()
    };
    TUnion::from_atomic(TAtomic::Object(TObject::Named(
        TNamedObject::new_with_type_parameters(object.name, Some(arguments)),
    )))
}

fn active_template(
    codebase: &CodebaseMetadata,
    object: &TNamedObject,
    defining: &[u8],
    name: &str,
) -> Option<TUnion> {
    let metadata = codebase.get_class_like(object.name.as_bytes())?;
    let defining = codebase.get_class_like(defining)?;
    defining.get_template_type(mago_word::word(name))?;
    get_specialized_template_type(
        codebase,
        mago_word::word(name),
        defining.name,
        metadata,
        object.type_parameters.as_deref(),
    )
}

fn reflection_declaring_class(
    codebase: &CodebaseMetadata,
    class: &[u8],
    method: &[u8],
) -> Option<mago_word::Word> {
    let k = codebase.get_class_like(class)?;
    k.appearing_method_ids
        .get(&mago_word::word(method.to_ascii_lowercase()))
        .map(|id| id.get_class_name())
}

fn bound(t: TUnion) -> TUnion {
    match t.types.as_ref() {
        [TAtomic::GenericParameter(g)] => (*g.constraint).clone(),
        _ => t,
    }
}

fn collect(
    context: &ProviderContext<'_, '_, '_>,
    invocation: &InvocationInfo<'_, '_, '_>,
    infer_from_usage: bool,
) -> Option<TUnion> {
    let codebase = context.codebase();
    let default = || {
        generic(
            codebase,
            COLLECTION,
            vec![benevolent_array_key(), get_mixed()],
        )
    };
    let Some(argument) = invocation.get_argument(0, &[b"value"]) else {
        return (!infer_from_usage).then(default);
    };
    let ty = context.get_expression_type(argument)?;
    if infer_from_usage
        && ty
            .types
            .iter()
            .all(|t| matches!(t, TAtomic::Array(a) if a.is_empty()))
    {
        return None;
    }
    let objects: Vec<&TNamedObject> = ty
        .types
        .iter()
        .filter_map(|t| match t {
            TAtomic::Object(TObject::Named(n)) => Some(n),
            _ => None,
        })
        .collect();
    if let Some(first) = objects.first() {
        let all = |parent: &str| {
            objects.len() == ty.types.len()
                && objects
                    .iter()
                    .all(|o| codebase.is_instance_of(o.name.as_bytes(), parent.as_bytes()))
        };
        if all(ENUMERABLE) {
            let key = if codebase
                .is_instance_of(first.name.as_bytes(), ELOQUENT_COLLECTION.as_bytes())
                || first
                    .name
                    .as_bytes()
                    .eq_ignore_ascii_case(ELOQUENT_COLLECTION.as_bytes())
            {
                mago_codex::ttype::get_int()
            } else {
                benevolent_array_key()
            };
            let model = active_template(codebase, first, first.name.as_bytes(), "TModel")?;
            return Some(generic(codebase, COLLECTION, vec![key, model]));
        }
        if all("Traversable") {
            let metadata = codebase.get_class_like(first.name.as_bytes())?;
            let templates: Vec<TUnion> = metadata
                .template_types
                .iter()
                .filter_map(|(name, _)| {
                    active_template(
                        codebase,
                        first,
                        first.name.as_bytes(),
                        &String::from_utf8_lossy(name.as_bytes()),
                    )
                })
                .collect();
            return Some(match <[TUnion; 1]>::try_from(templates) {
                Ok([value]) => generic(codebase, COLLECTION, vec![benevolent_array_key(), value]),
                Err(templates) => generic(codebase, COLLECTION, templates),
            });
        }
    }
    let scalars: Vec<TAtomic> = ty
        .types
        .iter()
        .filter(|t| !matches!(t, TAtomic::Null))
        .cloned()
        .collect();
    if !scalars.is_empty()
        && ty
            .types
            .iter()
            .all(|t| matches!(t, TAtomic::Null | TAtomic::Scalar(_)))
    {
        let key = TUnion::from_atomic(TAtomic::Scalar(TScalar::literal_int(0)));
        return Some(generic(
            codebase,
            COLLECTION,
            vec![key, TUnion::from_vec(scalars)],
        ));
    }
    let empty = !ty.types.is_empty()
        && ty.types.iter().all(|t| match t {
            TAtomic::Array(a) => a.is_empty(),
            _ => false,
        });
    empty.then(default)
}

fn without_int(t: &TInteger, value: i64) -> Vec<TAtomic> {
    let int = |i: TInteger| TAtomic::Scalar(TScalar::Integer(i));
    match *t {
        TInteger::Unspecified => vec![int(TInteger::To(value - 1)), int(TInteger::From(value + 1))],
        TInteger::From(from) if from == value => vec![int(TInteger::From(value + 1))],
        TInteger::To(to) if to == value => vec![int(TInteger::To(value - 1))],
        _ => vec![int(*t)],
    }
}

fn remove_falsey(ty: &TUnion) -> TUnion {
    use mago_codex::ttype::atomic::mixed::truthiness::TMixedTruthiness;
    TUnion::from_vec(
        ty.types
            .iter()
            .filter(|t| !t.is_falsy())
            .flat_map(|t| match t.clone() {
                TAtomic::Scalar(TScalar::Bool(b)) if b.is_general() => {
                    vec![TAtomic::Scalar(TScalar::r#true())]
                }
                TAtomic::Scalar(TScalar::Integer(i)) => without_int(&i, 0),
                TAtomic::Scalar(TScalar::String(mut s)) if !s.is_known_literal() => {
                    s.is_truthy = true;
                    s.is_non_empty = true;
                    vec![TAtomic::Scalar(TScalar::String(s))]
                }
                TAtomic::Mixed(m) => {
                    vec![TAtomic::Mixed(m.with_truthiness(TMixedTruthiness::Truthy))]
                }
                t => vec![t],
            })
            .collect(),
    )
}

fn remove_truthy(ty: &TUnion) -> TUnion {
    TUnion::from_vec(
        ty.types
            .iter()
            .filter(|t| !t.is_truthy())
            .flat_map(|t| match t {
                _ if t.is_falsy() => vec![t.clone()],
                TAtomic::Scalar(TScalar::Bool(_)) => vec![TAtomic::Scalar(TScalar::r#false())],
                TAtomic::Scalar(TScalar::Integer(_)) => {
                    vec![TAtomic::Scalar(TScalar::literal_int(0))]
                }
                TAtomic::Scalar(TScalar::Float(_)) => {
                    vec![TAtomic::Scalar(TScalar::literal_float(0.0))]
                }
                TAtomic::Scalar(TScalar::String(s)) => {
                    let zero = TAtomic::Scalar(TScalar::literal_string(mago_word::word("0")));
                    if s.is_non_empty {
                        vec![zero]
                    } else {
                        vec![
                            TAtomic::Scalar(TScalar::literal_string(mago_word::word(""))),
                            zero,
                        ]
                    }
                }
                TAtomic::Mixed(_) => [
                    TScalar::literal_int(0),
                    TScalar::literal_float(0.0),
                    TScalar::literal_string(mago_word::word("")),
                    TScalar::literal_string(mago_word::word("0")),
                    TScalar::r#false(),
                ]
                .into_iter()
                .map(TAtomic::Scalar)
                .chain(
                    mago_codex::ttype::get_empty_keyed_array()
                        .types
                        .iter()
                        .cloned(),
                )
                .chain([TAtomic::Null])
                .collect(),
                t => vec![t.clone()],
            })
            .collect(),
    )
}

fn narrow(
    context: &ProviderContext<'_, '_, '_>,
    expression: &Expression<'_>,
    name: &[u8],
    ty: TUnion,
    truthy: bool,
) -> TUnion {
    use mago_codex::ttype::comparator::{ComparisonResult, atomic_comparator};
    let codebase = context.codebase();
    let is_var = |e: &Expression<'_>| matches!(e.unparenthesized(), Expression::Variable(Variable::Direct(d)) if d.name == name);
    // the offsets of `$name['a']['b']` (`['a', 'b']`), none for `$name` itself: PHPStan's TypeSpecifier narrows an
    // array shape's offset as it narrows a variable
    let target = |e: &Expression<'_>| -> Option<Vec<ArrayKey>> {
        let mut path = Vec::new();
        let mut e = e.unparenthesized();
        loop {
            match e {
                Expression::Variable(Variable::Direct(d)) if d.name == name => {
                    path.reverse();
                    return Some(path);
                }
                Expression::ArrayAccess(access) => {
                    let index = context.get_expression_type(access.index)?;
                    path.push(match index.get_single_literal_string_value() {
                        Some(s) => ArrayKey::String(mago_word::word(s)),
                        None => ArrayKey::Integer(index.get_single_literal_int_value()?),
                    });
                    e = access.array.unparenthesized();
                }
                _ => return None,
            }
        }
    };
    let contained = |a: &TAtomic, b: &TAtomic| {
        atomic_comparator::is_contained_by(codebase, a, b, true, &mut ComparisonResult::default())
    };
    let intersect = |ty: TUnion, with: &TAtomic| {
        let kept: Vec<TAtomic> = ty
            .types
            .iter()
            .filter_map(|t| {
                if contained(t, with) {
                    Some(t.clone())
                } else {
                    contained(with, t).then(|| with.clone())
                }
            })
            .collect();
        if kept.is_empty() {
            mago_codex::ttype::get_never()
        } else {
            TUnion::from_vec(kept)
        }
    };
    let remove = |ty: TUnion, what: &TAtomic| {
        let kept: Vec<TAtomic> = ty
            .types
            .iter()
            .filter(|t| !contained(t, what))
            .flat_map(|t| match (t.clone(), what) {
                (TAtomic::Scalar(TScalar::String(mut s)), TAtomic::Scalar(TScalar::String(w)))
                    if !s.is_known_literal()
                        && w.get_known_literal_value() == Some(b"".as_slice()) =>
                {
                    s.is_non_empty = true;
                    vec![TAtomic::Scalar(TScalar::String(s))]
                }
                (
                    TAtomic::Scalar(TScalar::Integer(i)),
                    TAtomic::Scalar(TScalar::Integer(TInteger::Literal(v))),
                ) => without_int(&i, *v),
                (t, _) => vec![t],
            })
            .collect();
        if kept.is_empty() {
            mago_codex::ttype::get_never()
        } else {
            TUnion::from_vec(kept)
        }
    };
    match expression.unparenthesized() {
        Expression::UnaryPrefix(unary) if matches!(unary.operator, UnaryPrefixOperator::Not(_)) => {
            narrow(context, unary.operand, name, ty, !truthy)
        }
        Expression::Binary(binary) => match &binary.operator {
            BinaryOperator::And(_)
            | BinaryOperator::LowAnd(_)
            | BinaryOperator::Or(_)
            | BinaryOperator::LowOr(_) => {
                let and = matches!(
                    binary.operator,
                    BinaryOperator::And(_) | BinaryOperator::LowAnd(_)
                );
                let first = narrow(context, binary.lhs, name, ty.clone(), and);
                let both = narrow(context, binary.rhs, name, first, truthy);
                if and == truthy {
                    return both;
                }
                let either = narrow(context, binary.lhs, name, ty, !and);
                add_union_type(either, &both, codebase, CombinerOptions::default())
            }
            BinaryOperator::LessThan(_)
            | BinaryOperator::LessThanOrEqual(_)
            | BinaryOperator::GreaterThan(_)
            | BinaryOperator::GreaterThanOrEqual(_) => {
                let literal = |e: &Expression<'_>| match context
                    .get_expression_type(e)
                    .map(|t| t.types.as_ref())
                {
                    Some([TAtomic::Scalar(TScalar::Integer(TInteger::Literal(n)))]) => Some(*n),
                    _ => None,
                };
                let less = matches!(
                    binary.operator,
                    BinaryOperator::LessThan(_) | BinaryOperator::LessThanOrEqual(_)
                );
                let inclusive = matches!(
                    binary.operator,
                    BinaryOperator::LessThanOrEqual(_) | BinaryOperator::GreaterThanOrEqual(_)
                );
                let (n, less, inclusive) = if is_var(binary.lhs)
                    && let Some(n) = literal(binary.rhs)
                {
                    (n, less, inclusive)
                } else if is_var(binary.rhs)
                    && let Some(n) = literal(binary.lhs)
                {
                    (n, !less, inclusive)
                } else {
                    return ty;
                };
                let (less, inclusive) = if truthy {
                    (less, inclusive)
                } else {
                    (!less, !inclusive)
                };
                let kept: Vec<TAtomic> = ty
                    .types
                    .iter()
                    .filter_map(|t| match t {
                        TAtomic::Scalar(TScalar::Integer(i)) => match (less, inclusive) {
                            (true, true) => i.to_less_than_or_equal(n),
                            (true, false) => i.to_less_than(n),
                            (false, true) => i.to_greater_than_or_equal(n),
                            (false, false) => i.to_greater_than(n),
                        }
                        .map(|i| TAtomic::Scalar(TScalar::Integer(i))),
                        t => Some(t.clone()),
                    })
                    .collect();
                if kept.is_empty() {
                    mago_codex::ttype::get_never()
                } else {
                    TUnion::from_vec(kept)
                }
            }
            BinaryOperator::Identical(_) | BinaryOperator::NotIdentical(_) => {
                let (path, other) = if let Some(path) = target(binary.lhs) {
                    (path, binary.rhs)
                } else if let Some(path) = target(binary.rhs) {
                    (path, binary.lhs)
                } else {
                    return ty;
                };
                let Some([literal]) = context.get_expression_type(other).map(|t| t.types.as_ref())
                else {
                    return ty;
                };
                let literal = literal.clone();
                let single = literal.is_null()
                    || matches!(&literal, TAtomic::Scalar(TScalar::String(s)) if s.is_known_literal())
                    || matches!(
                        &literal,
                        TAtomic::Scalar(TScalar::Integer(TInteger::Literal(_)))
                    )
                    || matches!(&literal, TAtomic::Scalar(TScalar::Bool(b)) if !b.is_general());
                if !single {
                    return ty;
                }
                let identical = matches!(binary.operator, BinaryOperator::Identical(_)) == truthy;
                at_offsets(ty, &path, &|ty| {
                    if identical {
                        intersect(ty, &literal)
                    } else {
                        remove(ty, &literal)
                    }
                })
            }
            _ => ty,
        },
        Expression::Call(Call::Function(call)) => {
            let Expression::Identifier(function) = call.function else {
                return ty;
            };
            let function = function
                .value()
                .rsplit(|b| *b == b'\\')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            let checked = match function.as_slice() {
                b"is_int" | b"is_integer" | b"is_long" => vec![TAtomic::Scalar(TScalar::int())],
                b"is_string" => vec![TAtomic::Scalar(TScalar::string())],
                b"is_float" | b"is_double" => vec![TAtomic::Scalar(TScalar::float())],
                b"is_bool" => vec![TAtomic::Scalar(TScalar::bool())],
                b"is_null" => vec![TAtomic::Null],
                b"is_object" => vec![TAtomic::Object(TObject::Any)],
                // IsScalarFunctionTypeSpecifyingExtension's `bool|float|int|string`, IsArrayFunctionTypeSpecifyingExtension's
                // `array<mixed, mixed>`
                b"is_scalar" => vec![
                    TAtomic::Scalar(TScalar::bool()),
                    TAtomic::Scalar(TScalar::float()),
                    TAtomic::Scalar(TScalar::int()),
                    TAtomic::Scalar(TScalar::string()),
                ],
                b"is_array" => {
                    let mixed = Arc::new(TUnion::from_atomic(TAtomic::Mixed(
                        mago_codex::ttype::atomic::mixed::TMixed::new().with_is_explicit(true),
                    )));
                    vec![TAtomic::Array(
                        mago_codex::ttype::atomic::array::TArray::Keyed(
                            mago_codex::ttype::atomic::array::keyed::TKeyedArray::new_with_parameters(
                                mixed.clone(),
                                mixed,
                            ),
                        ),
                    )]
                }
                _ => return ty,
            };
            match call.argument_list.arguments.as_slice() {
                [argument] => {
                    let Some(path) = target(argument.value()) else {
                        return ty;
                    };
                    at_offsets(ty, &path, &|ty| {
                        if truthy {
                            let mut kept = checked
                                .iter()
                                .map(|checked| intersect(ty.clone(), checked))
                                .filter(|t| !t.is_never());
                            match kept.next() {
                                Some(first) => kept.fold(first, |acc, t| {
                                    add_union_type(acc, &t, codebase, CombinerOptions::default())
                                }),
                                None => mago_codex::ttype::get_never(),
                            }
                        } else {
                            checked.iter().fold(ty, remove)
                        }
                    })
                }
                _ => ty,
            }
        }
        Expression::Construct(mago_syntax::cst::Construct::Isset(isset)) if truthy => {
            let mut ty = ty;
            for value in &isset.values {
                let Expression::ArrayAccess(access) = value.unparenthesized() else {
                    return ty;
                };
                if !is_var(access.array) {
                    return ty;
                }
                let Some(index) = context.get_expression_type(access.index) else {
                    return ty;
                };
                let key = if let Some(s) = index.get_single_literal_string_value() {
                    ArrayKey::String(mago_word::word(s))
                } else if let Some(i) = index.get_single_literal_int_value() {
                    ArrayKey::Integer(i)
                } else {
                    return ty;
                };
                ty = has_non_null_offset(&ty, &key);
            }
            ty
        }
        e if truthy => match target(e) {
            Some(path) => at_offsets(ty, &path, &|ty| remove_falsey(&ty)),
            None => ty,
        },
        _ => ty,
    }
}

/// `ty` with `f` applied to the value at `path` in each of its array shapes; a shape whose value becomes `never` is
/// dropped. An empty `path` applies `f` to `ty`.
fn at_offsets(ty: TUnion, path: &[ArrayKey], f: &dyn Fn(TUnion) -> TUnion) -> TUnion {
    use mago_codex::ttype::atomic::array::TArray;
    let Some((key, rest)) = path.split_first() else {
        return f(ty);
    };
    let mut kept = Vec::with_capacity(ty.types.len());
    for atomic in ty.types.iter() {
        let TAtomic::Array(TArray::Keyed(keyed)) = atomic else {
            kept.push(atomic.clone());
            continue;
        };
        let Some((optional, value)) = keyed.known_items.as_ref().and_then(|items| items.get(key))
        else {
            kept.push(atomic.clone());
            continue;
        };
        let value = at_offsets(value.clone(), rest, f);
        if value.is_never() {
            continue;
        }
        let mut keyed = keyed.clone();
        if let Some(items) = keyed.known_items.as_mut() {
            Arc::make_mut(items).insert(*key, (*optional, value));
        }
        kept.push(TAtomic::Array(TArray::Keyed(keyed)));
    }
    if kept.is_empty() {
        mago_codex::ttype::get_never()
    } else {
        ty.clone_with_types(kept)
    }
}

fn has_non_null_offset(ty: &TUnion, key: &ArrayKey) -> TUnion {
    use mago_codex::ttype::atomic::array::TArray;
    let mut kept = Vec::with_capacity(ty.types.len());
    for atomic in ty.types.iter() {
        let TAtomic::Array(TArray::Keyed(keyed)) = atomic else {
            kept.push(atomic.clone());
            continue;
        };
        let Some(item) = keyed.known_items.as_ref().and_then(|items| items.get(key)) else {
            if keyed.parameters.is_some() || keyed.known_items.is_none() {
                kept.push(atomic.clone());
            }
            continue;
        };
        let value = item.1.to_non_nullable();
        if value.is_never() {
            continue;
        }
        let mut keyed = keyed.clone();
        if let Some(items) = keyed.known_items.as_mut() {
            Arc::make_mut(items).insert(*key, (false, value));
        }
        keyed.non_empty = true;
        kept.push(TAtomic::Array(TArray::Keyed(keyed)));
    }
    if kept.is_empty() {
        mago_codex::ttype::get_never()
    } else {
        ty.clone_with_types(kept)
    }
}

fn eloquent_collection_map(
    context: &ProviderContext<'_, '_, '_>,
    method: &str,
    invocation: &InvocationInfo<'_, '_, '_>,
) -> Option<TUnion> {
    let codebase = context.codebase();
    let arguments = invocation.arguments();
    let [argument] = arguments.as_slice() else {
        return None;
    };
    let callable = context.get_expression_type(argument.value()?)?;
    let [
        mago_codex::ttype::atomic::TAtomic::Callable(
            mago_codex::ttype::atomic::callable::TCallable::Signature(signature),
        ),
    ] = callable.types.as_ref()
    else {
        return None;
    };
    let target = invocation.inner().target.get_method_context()?;
    let StaticClassType::Object(TObject::Named(called_on)) = &target.class_type else {
        return None;
    };
    let mut key = active_template(codebase, called_on, ELOQUENT_COLLECTION.as_bytes(), "TKey")?;
    active_template(
        codebase,
        called_on,
        ELOQUENT_COLLECTION.as_bytes(),
        "TModel",
    )?;
    let mut value = signature
        .get_return_type()
        .cloned()
        .unwrap_or_else(get_mixed);
    if method == "mapwithkeys" {
        let parameters: Vec<_> = value
            .types
            .iter()
            .map(|a| mago_codex::ttype::get_iterable_parameters(a, codebase))
            .collect::<Option<_>>()?;
        let (keys, values): (Vec<_>, Vec<_>) = parameters.into_iter().unzip();
        key = super::helpers::union(codebase, keys);
        value = super::helpers::union(codebase, values);
    }
    let model = b"Illuminate\\Database\\Eloquent\\Model";
    let is_model = |a: &TAtomic| match a {
        TAtomic::Object(TObject::Named(n)) if codebase.is_instance_of(n.name.as_bytes(), model) => {
            Some(true)
        }
        TAtomic::Object(TObject::Named(n)) => codebase
            .get_class_like(n.name.as_bytes())
            .is_some_and(|c| !c.kind.is_interface())
            .then_some(false),
        TAtomic::Object(_) | TAtomic::Mixed(_) | TAtomic::GenericParameter(_) => None,
        _ => Some(false),
    };
    let verdicts: Vec<Option<bool>> = value.types.iter().map(is_model).collect();
    let models = !verdicts.is_empty() && verdicts.iter().all(|v| *v == Some(true));
    let no_models = verdicts.iter().all(|v| *v == Some(false));
    if models {
        let is_generic = codebase
            .get_class_like(called_on.name.as_bytes())
            .is_some_and(|k| !k.template_types.is_empty());
        if !is_generic {
            return Some(TUnion::from_atomic(TAtomic::Object(TObject::Named(
                TNamedObject::new(called_on.name),
            ))));
        }
        let class = String::from_utf8_lossy(called_on.name.as_bytes()).into_owned();
        return Some(generic(codebase, &class, vec![key, value]));
    }
    let generic_receiver = codebase
        .get_class_like(called_on.name.as_bytes())
        .is_some_and(|k| !k.template_types.is_empty());
    let open_key = key
        .types
        .iter()
        .any(|a| matches!(a, TAtomic::GenericParameter(_)));
    if method == "map" && no_models && generic_receiver && !open_key {
        return None;
    }
    Some(generic(codebase, COLLECTION, vec![key, value]))
}

impl Methods {
    fn filter_reject(
        &self,
        context: &ProviderContext<'_, '_, '_>,
        method: &str,
        invocation: &InvocationInfo<'_, '_, '_>,
    ) -> Option<TUnion> {
        let codebase = context.codebase();
        let receiver = self.receiver(invocation, method.as_bytes())?;
        let called_on = context.get_expression_type(&receiver)?;
        let first = called_on.types.iter().find_map(|t| match t {
            TAtomic::Object(TObject::Named(n)) => Some(n),
            _ => None,
        })?;
        if codebase
            .get_class_like(first.name.as_bytes())?
            .template_types
            .is_empty()
        {
            return None;
        }
        let declaring =
            reflection_declaring_class(codebase, first.name.as_bytes(), method.as_bytes())?;
        let key = active_template(codebase, first, declaring.as_bytes(), "TKey")?;
        let key = if key
            .types
            .iter()
            .any(|t| mago_analyzer::template_arguments::as_marker(t).is_some())
        {
            key
        } else {
            bound(key)
        };
        let unresolved = active_template(codebase, first, declaring.as_bytes(), "TValue")?;
        let value = bound(unresolved.clone());
        let keep = |narrowed: TUnion| {
            if narrowed == value
                && unresolved
                    .types
                    .iter()
                    .any(|t| mago_analyzer::template_arguments::as_marker(t).is_some())
            {
                unresolved.clone()
            } else {
                narrowed
            }
        };
        let class = String::from_utf8_lossy(first.name.as_bytes()).into_owned();
        let Some(callback) = invocation.get_argument(0, &[b"callback", b"key"]) else {
            return match method {
                "filter" => Some(generic(
                    codebase,
                    &class,
                    vec![key, keep(remove_falsey(&value))],
                )),
                "reject" => Some(generic(
                    codebase,
                    &class,
                    vec![key, keep(remove_truthy(&value))],
                )),
                _ => None,
            };
        };
        let condition = match callback.unparenthesized() {
            Expression::Closure(closure) if !closure.parameter_list.parameters.is_empty() => {
                match closure.body.statements.as_slice() {
                    [Statement::Return(Return { value: Some(e), .. })] => {
                        Some((closure.parameter_list.parameters.first()?.variable.name, *e))
                    }
                    _ => None,
                }
            }
            Expression::ArrowFunction(arrow) if !arrow.parameter_list.parameters.is_empty() => {
                Some((
                    arrow.parameter_list.parameters.first()?.variable.name,
                    arrow.expression,
                ))
            }
            _ if method == "where" => return None,
            _ => None,
        };
        let narrowed = match condition {
            Some((name, condition)) => {
                narrow(context, condition, name, value.clone(), method != "reject")
            }
            None => value.clone(),
        };
        Some(generic(codebase, &class, vec![key, keep(narrowed)]))
    }

    fn where_not_null(
        &self,
        context: &ProviderContext<'_, '_, '_>,
        invocation: &InvocationInfo<'_, '_, '_>,
    ) -> Option<TUnion> {
        let codebase = context.codebase();
        let receiver = self.receiver(invocation, b"whereNotNull")?;
        let first = context
            .get_expression_type(&receiver)?
            .types
            .iter()
            .find_map(|t| match t {
                TAtomic::Object(TObject::Named(n)) => Some(n.clone()),
                _ => None,
            })?;
        let declaring =
            reflection_declaring_class(codebase, first.name.as_bytes(), b"whereNotNull")?;
        let template = |name: &str| active_template(codebase, &first, declaring.as_bytes(), name);
        let key = template("TKey")?;
        let value = template("TValue").or_else(|| template("TModel"))?;
        let keyed = invocation
            .get_argument(0, &[b"key"])
            .and_then(|a| context.get_expression_type(a))
            .is_some_and(|t| {
                !t.types.is_empty() && t.types.iter().all(|t| !t.is_null() && !t.is_mixed())
            });
        let value = TUnion::from_vec(
            value
                .types
                .iter()
                .filter(|t| !t.is_null())
                .filter(|t| {
                    !keyed
                        || !matches!(
                            t,
                            TAtomic::Scalar(
                                TScalar::String(_)
                                    | TScalar::Integer(_)
                                    | TScalar::Float(_)
                                    | TScalar::Bool(_)
                            )
                        )
                })
                .cloned()
                .collect(),
        );
        let class = String::from_utf8_lossy(first.name.as_bytes()).into_owned();
        Some(generic(codebase, &class, vec![key, value]))
    }

    fn receiver(&self, invocation: &InvocationInfo<'_, '_, '_>, method: &[u8]) -> Option<Span> {
        let call = invocation.span();
        let arguments = invocation.inner().arguments_source.span();
        let before = self
            .1
            .get(call.start.offset as usize..arguments.start.offset as usize)?
            .trim_ascii_end();
        let name = before.len().checked_sub(method.len())?;
        if !before[name..].eq_ignore_ascii_case(method) {
            return None;
        }
        let before = before[..name].trim_ascii_end().strip_suffix(b"->")?;
        let before = before.strip_suffix(b"?").unwrap_or(before).trim_ascii_end();
        Some(Span::new(
            call.file_id,
            call.start,
            call.start.forward(before.len() as u32),
        ))
    }

    fn receiver_argument(&self, invocation: &InvocationInfo<'_, '_, '_>) -> Option<Span> {
        let call = invocation.span();
        let arguments = invocation.inner().arguments_source.span();
        let before = self
            .1
            .get(call.start.offset as usize..arguments.start.offset as usize)?
            .trim_ascii_end();
        let before = before
            .strip_suffix(b"user")?
            .trim_ascii_end()
            .strip_suffix(b"->")?;
        let before = before.strip_suffix(b"?").unwrap_or(before).trim_ascii_end();
        let close = before.len().checked_sub(1).filter(|&i| before[i] == b')')?;
        let mut depth = 0usize;
        let mut open = None;
        for i in (0..close).rev() {
            match before[i] {
                b')' | b']' => depth += 1,
                b'(' | b'[' if depth > 0 => depth -= 1,
                b'(' => {
                    open = Some(i);
                    break;
                }
                b',' if depth == 0 => return None,
                _ => {}
            }
        }
        let open = open?;
        let inner = &before[open + 1..close];
        let start = open + 1 + (inner.len() - inner.trim_ascii_start().len());
        let end = open + 1 + inner.trim_ascii_end().len();
        if start >= end {
            return None;
        }
        Some(Span::new(
            call.file_id,
            call.start.forward(start as u32),
            call.start.forward(end as u32),
        ))
    }
}
