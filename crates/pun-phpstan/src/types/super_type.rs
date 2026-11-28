use super::{
    Accessory, Conditional, ConstArray, Param, Signature, Subject, TemplateMap, Tri, Type, accepts,
    canonical_class, infer_template_types, larastan, mixed, resolve_template_types, static_object,
    static_of, union, with_class_args,
};
use crate::symbols;

pub fn is_super_type_of(a: &Type, b: &Type) -> Tri {
    use Type as T;
    if let T::Reasoned(t, _) = b {
        return is_super_type_of(a, t);
    }
    if let T::LateArrayShape(s) = b {
        return is_super_type_of(a, &s.resolve());
    }
    if let Some(b) = super::larastan::resolve_late(b) {
        return is_super_type_of(a, &b);
    }
    if let Some(a) = super::larastan::resolve_late(a) {
        return is_super_type_of(&a, b);
    }
    if let T::SubtractedObject { object, excluded } = b
        && matches!(a, T::Object(..) | T::Static(..) | T::This(_))
    {
        return if is_super_type_of(excluded, a).yes() {
            Tri::No
        } else {
            is_super_type_of(a, object)
        };
    }
    match a {
        T::Reasoned(t, _) => is_super_type_of(t, b),
        T::LateArrayShape(s) => {
            if b.is_never() {
                Tri::Yes
            } else {
                is_super_type_of(&s.resolve(), b).and(Tri::Maybe)
            }
        }
        T::Mixed { .. }
        | T::StrictMixed
        | T::Error
        | T::Wildcard
        | T::OffsetAccess(..)
        | T::Projection(..) => Tri::Yes,
        T::SubtractedMixed { excluded, .. } => mixed::is_super(excluded, b),
        T::SubtractedObject { object, excluded } => {
            if b.is_compound() {
                return is_sub_type_of(b, a);
            }
            let outside = is_super_type_of(excluded, b).negate();
            if outside.no() {
                return Tri::No;
            }
            let positive = match b {
                T::SubtractedObject { object, excluded } => {
                    if is_super_type_of(excluded, a).yes() {
                        return Tri::No;
                    }
                    object.as_ref()
                }
                b => b,
            };
            is_super_type_of(object, positive).and(outside)
        }
        T::Never(_) => match b {
            T::Never(_) => Tri::Yes,
            T::Template(_) => Tri::Maybe,
            _ => Tri::No,
        },
        T::Conditional(c) => conditional_super(c, b),
        T::Union(types) | T::Benevolent(types) => {
            if matches!(
                b,
                T::Union(_) | T::Benevolent(_) | T::Iterable(..) | T::Never(_) | T::IntRange(..)
            ) || matches!(b, T::Template(tpl) if matches!(tpl.bound, T::Benevolent(_)))
            {
                return is_sub_type_of(b, a);
            }
            let mut result = Tri::No;
            for t in types {
                let r = is_super_type_of(t, b);
                if r.yes() {
                    return r;
                }
                result = result.or(r);
            }
            let whole = matches!(b, T::Conditional(_))
                || matches!(b, T::Template(tpl) if matches!(tpl.bound, T::Union(_) | T::Benevolent(_)));
            if whole {
                result = result.or(is_sub_type_of(b, a));
            }
            result
        }
        T::Intersection(types) => {
            if matches!(b, T::Intersection(_)) && a == b {
                return Tri::Yes;
            }
            if b.is_never() {
                return Tri::Yes;
            }
            types
                .iter()
                .map(|t| is_super_type_of(t, b))
                .fold(Tri::Yes, Tri::and)
        }
        T::Template(tpl) => {
            if matches!(b, T::Template(_) | T::Intersection(_)) {
                return is_sub_type_of(b, a);
            }
            if b.is_never() {
                return Tri::Yes;
            }
            is_super_type_of(&tpl.bound, b).and(Tri::Maybe)
        }
        T::Int | T::ConstInt(_) | T::IntRange(..)
            if matches!(b, T::Int | T::ConstInt(_) | T::IntRange(..)) =>
        {
            match (a, b) {
                (T::Int, _) => Tri::Yes,
                (T::ConstInt(x), T::ConstInt(y)) => Tri::from(x == y),
                (T::ConstInt(x), T::IntRange(min, max)) => {
                    if min.is_none_or(|m| m <= *x) && max.is_none_or(|m| *x <= m) {
                        Tri::Maybe
                    } else {
                        Tri::No
                    }
                }
                (_, T::Int) => Tri::Maybe,
                _ => int_range_super(a, b),
            }
        }
        _ if b.is_compound() && !(matches!(a, T::Callable(_)) && matches!(b, T::Callable(_))) => {
            if let T::Accessory(acc) = a
                && (matches!(acc, Accessory::List | Accessory::NonEmptyArray))
                && a == b
            {
                return Tri::Yes;
            }
            is_sub_type_of(b, a)
        }
        T::NonexistentParent => Tri::from(matches!(b, T::NonexistentParent)),
        T::IntRange(..) | T::Int | T::ConstInt(_) => Tri::No,
        T::Null => Tri::from(matches!(b, T::Null)),
        T::Void => Tri::from(matches!(b, T::Void)),
        T::Bool => Tri::from(matches!(b, T::Bool | T::ConstBool(_))),
        T::ConstBool(x) => match b {
            T::ConstBool(y) => Tri::from(x == y),
            T::Bool => Tri::Maybe,
            _ => Tri::No,
        },
        T::Float => Tri::from(matches!(b, T::Float | T::ConstFloat(_))),
        T::ConstFloat(_) => match b {
            T::ConstFloat(_) => Tri::from(a == b),
            T::Float => Tri::Maybe,
            _ => Tri::No,
        },
        T::String | T::StringAcceptingStringable | T::StringNotAcceptingStringable => {
            Tri::from(matches!(
                b,
                T::String
                    | T::StringAcceptingStringable
                    | T::StringNotAcceptingStringable
                    | T::ConstString(_)
                    | T::ClassString(_)
            ))
        }
        T::ConstString(x) => match b {
            T::ClassString(Some(g)) if matches!(**g, T::Mixed { .. }) => Tri::Maybe,
            T::ClassString(Some(g)) => {
                let named = T::Object(String::from_utf8_lossy(x).into_owned(), vec![], false);
                let r = match &**g {
                    T::Template(tpl) => is_super_type_of(&tpl.bound, &named),
                    T::Static(c, args) => is_super_type_of(&static_of(c, args), &named),
                    g => is_super_type_of(g, &named),
                };
                if r.yes() { Tri::Maybe } else { Tri::No }
            }
            T::ClassString(None) => Tri::from(
                std::str::from_utf8(x)
                    .ok()
                    .and_then(symbols::lookup)
                    .is_some(),
            )
            .and(Tri::Maybe)
            .or(Tri::No),
            T::ConstString(y) => Tri::from(x == y),
            T::String | T::StringAcceptingStringable | T::StringNotAcceptingStringable => {
                Tri::Maybe
            }
            _ => Tri::No,
        },
        T::ClassString(None) => b.is_class_string(),
        T::ClassString(Some(g)) => match b {
            T::ConstString(s) => {
                if matches!(**g, T::Mixed { .. }) {
                    return Tri::Yes;
                }
                let g = match &**g {
                    T::Template(t) => &t.bound,
                    g => g,
                };
                let r = is_super_type_of(
                    g,
                    &T::Object(String::from_utf8_lossy(s).into_owned(), vec![], false),
                );
                if b.is_class_string().yes() {
                    r
                } else {
                    r.and(Tri::Maybe)
                }
            }
            T::ClassString(Some(h)) => is_super_type_of(g, h),
            T::ClassString(None) => is_super_type_of(g, &T::ObjectWithoutClass),
            T::String => Tri::Maybe,
            _ => Tri::No,
        },
        T::Resource => Tri::from(matches!(b, T::Resource)),
        T::ObjectWithoutClass => match b {
            T::ObjectWithoutClass | T::ObjectShape(_) => Tri::Yes,
            _ => Tri::from(!b.object_class_names().is_empty()),
        },
        T::Object(class, args, exact) => object_super(class, args, b, true, *exact),
        T::This(class) if !matches!(b, T::This(_)) => {
            is_super_type_of(&T::Static(class.clone(), vec![]), b).and(Tri::Maybe)
        }
        T::Static(class, _) | T::This(class) => {
            let object = match a {
                T::Static(_, args) => static_of(class, args),
                _ => static_object(class),
            };
            match b {
                T::Static(..) | T::This(_) => is_super_type_of(&object, b),
                T::ObjectWithoutClass => Tri::Maybe,
                T::Object(c, _, exact) => {
                    let r = is_super_type_of(&object, b);
                    if r.yes() && (*exact || symbols::lookup(c).is_some_and(|c| c.is_final)) {
                        r
                    } else {
                        r.and(Tri::Maybe)
                    }
                }
                _ => Tri::No,
            }
        }
        T::Array(_, item) => match b {
            T::Array(..) | T::ConstArray(_) => {
                let result = is_super_type_of(item, &b.iterable_value_type()).and(
                    is_super_type_of(&a.iterable_key_type(), &b.iterable_key_type()),
                );
                if result.no()
                    && matches!(b, T::ConstArray(_))
                    && !b.is_iterable_at_least_once().yes()
                {
                    Tri::Maybe
                } else {
                    result
                }
            }
            _ => Tri::No,
        },
        T::ConstArray(ca) => match b {
            T::ConstArray(cb) => const_array_super(ca, cb),
            T::Array(..) => {
                if ca.keys.is_empty() {
                    return Tri::Maybe;
                }
                let key = is_super_type_of(&ca.iterable_key_type(), &b.iterable_key_type());
                if key.no() {
                    return key;
                }
                Tri::Maybe.and(key).and(is_super_type_of(
                    &ca.iterable_value_type(),
                    &b.iterable_value_type(),
                ))
            }
            _ => Tri::No,
        },
        T::Callable(sig) => {
            let callable = b.is_callable();
            match sig {
                Some(ours) if !callable.no() => callable.and(signature_super(ours, b, None)),
                _ => callable,
            }
        }
        T::Closure(ours) => match b {
            T::Closure(_) => signature_super(ours, b, None),
            T::Object(c, _, _) if canonical_class(c) == "Closure" => Tri::Maybe,
            _ => object_super("Closure", &[], b, true, false),
        },
        T::EnumCase(class, _) => match b {
            T::EnumCase(..) => Tri::from(a == b),
            T::Object(c, _, _) if canonical_class(c) == canonical_class(class) => Tri::Maybe,
            T::ObjectWithoutClass => Tri::Maybe,
            _ => Tri::No,
        },
        T::ObjectShape(props) => match b {
            T::NonexistentParent => Tri::from(props.iter().all(|(_, _, optional)| *optional)),
            _ if b.object_class_names().iter().any(|c| {
                symbols::lookup(c).is_some_and(|c| {
                    c.is_final
                        && !c.allows_dynamic_properties
                        && props
                            .iter()
                            .any(|(name, _, optional)| !optional && !c.properties.contains(name))
                })
            }) =>
            {
                Tri::No
            }
            T::ObjectWithoutClass => Tri::Maybe,
            T::ObjectShape(others) => {
                let mut result = Tri::Yes;
                for (name, ty, optional) in props {
                    let Some((_, other, other_optional)) = others.iter().find(|(n, ..)| n == name)
                    else {
                        if !optional {
                            result = Tri::No;
                        }
                        continue;
                    };
                    if *other_optional && !optional {
                        result = result.and(Tri::Maybe);
                        continue;
                    }
                    let property = is_super_type_of(ty, other);
                    if property.no() {
                        return Tri::No;
                    }
                    result = result.and(property);
                }
                result
            }
            T::Object(..) | T::Static(..) | T::This(_)
                if b.object_class_names().iter().any(|c| c == "stdClass") =>
            {
                Tri::Maybe
            }
            T::Object(..) | T::Static(..) | T::This(_) => {
                super::properties::shape_takes(props, b, false, &is_super_type_of)
                    .unwrap_or(Tri::Maybe)
            }
            _ => Tri::No,
        },
        T::Iterable(key, value) => b
            .is_iterable()
            .and(is_super_type_of(value, &b.iterable_value_type()))
            .and(is_super_type_of(key, &b.iterable_key_type())),
        T::Accessory(acc) => accessory_super(acc, b),
    }
}

impl Conditional {
    pub fn resolve(&self) -> Type {
        if let Some(t) = self.resolved.0.get() {
            return t.clone();
        }
        let t = self.compute();
        let _ = self.resolved.0.set(t.clone());
        t
    }

    fn compute(&self) -> Type {
        let Subject::Type(subject) = &self.subject else {
            return union(vec![self.then.clone(), self.otherwise.clone()]);
        };
        let (yes, no) = if self.negated {
            (&self.otherwise, &self.then)
        } else {
            (&self.then, &self.otherwise)
        };
        match is_super_type_of(&self.target, subject) {
            Tri::Yes => yes.clone(),
            Tri::No => no.clone(),
            Tri::Maybe => union(vec![self.then.clone(), self.otherwise.clone()]),
        }
    }

    pub fn is_resolvable(&self) -> bool {
        let Subject::Type(subject) = &self.subject else {
            return false;
        };
        !contains_template(subject) && !contains_template(&self.target)
            || !is_super_type_of(&self.target, subject).maybe()
    }
}

pub(crate) fn contains_template(t: &Type) -> bool {
    let found = std::cell::Cell::new(matches!(t, Type::Template(_)));
    t.traverse(&|x| {
        found.set(found.get() || contains_template(x));
        x.clone()
    });
    found.get()
}

fn conditional_super(c: &Conditional, b: &Type) -> Tri {
    if let Type::Conditional(other) = b
        && std::mem::discriminant(&c.subject) == std::mem::discriminant(&other.subject)
    {
        return is_super_type_of(&c.then, &other.then)
            .and(is_super_type_of(&c.otherwise, &other.otherwise));
    }
    if b.is_never() {
        return Tri::Yes;
    }
    let b = match b {
        Type::Conditional(other) => other.resolve(),
        b => b.clone(),
    };
    let r = is_super_type_of(&c.resolve(), &b);
    if c.is_resolvable() {
        r
    } else {
        r.and(Tri::Maybe)
    }
}

pub(super) fn signature_super(ours: &Signature, b: &Type, accept: Option<bool>) -> Tri {
    let variants = callable_signatures(b);
    variants
        .iter()
        .map(|theirs| variant_super(ours, &specialize_signature(ours, theirs), accept))
        .reduce(Tri::or)
        .expect("a variant")
}

pub(super) fn specialize_signature(ours: &Signature, theirs: &Signature) -> Signature {
    let mut map = TemplateMap::default();
    for (i, param) in theirs.params.iter().enumerate() {
        let received = if param.variadic {
            Some(union(
                ours.params.iter().skip(i).map(|p| p.ty.clone()).collect(),
            ))
        } else {
            ours.params.get(i).map(|p| p.ty.clone())
        };
        if let Some(received) = received {
            map = map.union(infer_template_types(&param.ty, &received));
        }
    }
    let map = map.with_errors(&theirs.templates.iter().collect::<Vec<_>>());
    Signature {
        params: theirs
            .params
            .iter()
            .map(|p| Param {
                ty: resolve_template_types(&p.ty, &map),
                ..p.clone()
            })
            .collect(),
        ret: resolve_template_types(&theirs.ret, &map),
        templates: theirs.templates.clone(),
    }
}

pub(super) fn callable_signatures(b: &Type) -> Vec<Signature> {
    let of = |f: &symbols::Function| -> Vec<Signature> {
        f.variants
            .iter()
            .map(|v| Signature {
                params: v
                    .params
                    .iter()
                    .map(|p| Param {
                        name: p.name.clone(),
                        ty: crate::resolve::resolve_str(&p.ty, None),
                        optional: p.optional,
                        by_ref: p.by_ref != symbols::ByRef::No,
                        variadic: p.variadic,
                    })
                    .collect(),
                ret: crate::resolve::resolve_str(&v.ret, None),
                templates: vec![],
            })
            .collect()
    };
    let method = |class: &str, method: &str| -> Vec<Signature> {
        symbols::lookup(class)
            .and_then(|c| {
                std::iter::once(c.name.clone())
                    .chain(c.ancestors.iter().cloned())
                    .find_map(|n| {
                        symbols::lookup(&n)?
                            .methods
                            .get(&method.to_ascii_lowercase())
                            .map(of)
                    })
            })
            .unwrap_or_default()
    };
    let mut variants = match b {
        Type::Callable(Some(s)) | Type::Closure(s) => vec![(**s).clone()],
        Type::ConstString(name)
            if let Some(f) = std::str::from_utf8(name).ok().and_then(symbols::function) =>
        {
            of(f)
        }
        Type::ConstString(name) => std::str::from_utf8(name)
            .ok()
            .and_then(|name| name.split_once("::"))
            .map_or_default(|(c, m)| method(c, m)),
        Type::ConstArray(a) => a.callable_parts().map_or_default(|(class, _, names)| {
            names.into_iter().flat_map(|m| method(class, m)).collect()
        }),
        _ => Vec::new(),
    };
    if variants.is_empty() {
        variants.push(Signature {
            params: vec![],
            ret: mixed(),
            templates: vec![],
        });
    }
    variants
}

fn variant_super(ours: &Signature, theirs: &Signature, accept: Option<bool>) -> Tri {
    let compare = |a: &Type, b: &Type, strict: bool| match accept {
        Some(_) => accepts::accepts(a, b, strict),
        None => is_super_type_of(a, b),
    };
    let mut their_params: Vec<&Param> = theirs.params.iter().collect();
    if let Some(last) = theirs.params.last()
        && last.variadic
    {
        while their_params.len() < ours.params.len() {
            their_params.push(last);
        }
    }
    let mut result = Tri::Yes;
    for (i, theirs) in their_params.iter().enumerate() {
        let Some(ours) = ours.params.get(i) else {
            if !theirs.optional {
                result = result.and(Tri::No);
            }
            continue;
        };
        if ours.optional && !theirs.optional {
            result = result.and(Tri::No);
        }
        result = result.and(compare(&theirs.ty, &ours.ty, accept.unwrap_or(true)));
    }
    if accept.is_none() && theirs.params.len() < ours.params.len() {
        result = result.and(Tri::Maybe);
    }
    result.and(compare(&ours.ret, &theirs.ret, true))
}

pub fn to_benevolent(t: Type) -> Type {
    match t {
        Type::Union(types) => Type::Benevolent(types),
        t => t,
    }
}

fn accessory_super(acc: &Accessory, b: &Type) -> Tri {
    match acc {
        Accessory::NonEmptyString => {
            if b.is_non_falsy_string().yes() {
                Tri::Yes
            } else {
                b.is_non_empty_string()
            }
        }
        Accessory::NonFalsyString => b.is_non_falsy_string(),
        Accessory::NumericString => b.is_numeric_string(),
        Accessory::LiteralString => b.is_literal_string(),
        Accessory::LowercaseString => b.is_lowercase_string(),
        Accessory::UppercaseString => b.is_uppercase_string(),
        Accessory::DecimalIntString { inverse } => decimal_int_super(*inverse, b),
        Accessory::List => b.is_list(),
        Accessory::NonEmptyArray => b.is_array().and(b.is_iterable_at_least_once()),
        Accessory::HasMethod(name) => b.has_method(name),
        Accessory::HasOffsetValue(key, value) => offset_value_relation(key, value, b, false),
        Accessory::ModelProperty(t) => larastan::model_property_super(t, b),
        Accessory::ViewString => larastan::view_string_super(b),
    }
}

fn offset_value_relation(key: &Type, value: &Type, other: &Type, reverse: bool) -> Tri {
    let (has, item) = match other {
        Type::Accessory(Accessory::HasOffsetValue(k, v)) if k.as_ref() == key => {
            (Tri::Yes, v.as_ref().clone())
        }
        Type::Accessory(Accessory::HasOffsetValue(..)) => (Tri::Maybe, mixed()),
        Type::Array(k, v) => (is_super_type_of(k, key).and(Tri::Maybe), v.as_ref().clone()),
        Type::ConstArray(a) => {
            if let Some(i) = a.keys.iter().position(|k| k == key) {
                (
                    if a.is_optional_key(i) {
                        Tri::Maybe
                    } else {
                        Tri::Yes
                    },
                    a.values[i].clone(),
                )
            } else if let Some((k, v)) = &a.unsealed {
                (is_super_type_of(k, key).and(Tri::Maybe), v.clone())
            } else {
                return Tri::No;
            }
        }
        _ => {
            let object = matches!(
                other,
                Type::Object(..) | Type::ObjectWithoutClass | Type::Static(..) | Type::This(_)
            );
            return other
                .is_array()
                .or(other.is_string())
                .or(Tri::from(object))
                .and(Tri::Maybe);
        }
    };
    if reverse {
        has.and(
            if matches!(other, Type::Accessory(Accessory::HasOffsetValue(..))) {
                Tri::Yes
            } else {
                Tri::Maybe
            },
        )
        .and(is_super_type_of(&item, value))
    } else {
        has.and(is_super_type_of(value, &item))
    }
}

fn decimal_int_super(inverse: bool, b: &Type) -> Tri {
    let decimal = b.is_decimal_int_string();
    b.is_string()
        .and(if inverse { decimal.negate() } else { decimal })
}

fn int_range_super(a: &Type, b: &Type) -> Tri {
    let ((min, max), (bmin, bmax)) = (
        a.int_bounds().unwrap_or_default(),
        b.int_bounds().unwrap_or_default(),
    );
    let disjoint = matches!((min, bmax), (Some(x), Some(y)) if x > y)
        || matches!((max, bmin), (Some(x), Some(y)) if x < y);
    if disjoint {
        return Tri::No;
    }
    let lower = min.is_none() || bmin.is_some_and(|v| min.expect("min") <= v);
    let upper = max.is_none() || bmax.is_some_and(|v| max.expect("max") >= v);
    if lower && upper { Tri::Yes } else { Tri::Maybe }
}

pub(super) fn object_super(class: &str, args: &[Type], b: &Type, strict: bool, exact: bool) -> Tri {
    let names = match b {
        Type::Closure(_) => vec!["Closure".to_string()],
        b => b.object_class_names(),
    };
    if names.is_empty() && !matches!(b, Type::ObjectWithoutClass) {
        return Tri::No;
    }
    if let Type::ObjectWithoutClass = b {
        return Tri::Maybe;
    }
    let that = &names[0];
    let input_exact = matches!(b, Type::Object(_, _, true));
    let result = match (symbols::lookup(class), symbols::lookup(that)) {
        (Some(this), Some(other)) => {
            if this.name == other.name {
                if class == that && exact && !this.is_final && !input_exact {
                    Tri::Maybe
                } else {
                    Tri::Yes
                }
            } else if this.is_trait || other.is_trait {
                Tri::No
            } else if !exact && !this.is_final && other.ancestors.contains(&this.name) {
                Tri::Yes
            } else if (!input_exact && !other.is_final && this.ancestors.contains(&other.name))
                || (this.is_interface && !other.is_final && !input_exact)
                || (other.is_interface && !this.is_final && !exact)
            {
                Tri::Maybe
            } else {
                Tri::No
            }
        }
        _ => {
            if that == class {
                Tri::Yes
            } else {
                Tri::Maybe
            }
        }
    };
    if args.is_empty() || result.no() {
        return result;
    }
    let Type::Object(b_class, b_args, _) = b else {
        return result;
    };
    let strict = strict && !b_args.is_empty();
    let (Some(c), class) = (symbols::lookup(b_class), canonical_class(class)) else {
        return result;
    };
    let ancestor: Vec<Type> = if c.name == class {
        if b_args.is_empty() {
            return result.and(Tri::Maybe);
        }
        b_args.clone()
    } else if c.ancestors.contains(&class) {
        let Some(generic) = c.generics.get(&class) else {
            return result;
        };
        generic
            .iter()
            .map(|s| with_class_args(&crate::resolve::resolve_str(s, Some(&c.name)), &c, b_args))
            .collect()
    } else {
        return result;
    };
    if ancestor.len() != args.len() {
        return Tri::No;
    }
    let declared = symbols::lookup(&class);
    args.iter()
        .zip(&ancestor)
        .enumerate()
        .map(|(i, (x, y))| {
            valid_variance(
                x,
                y,
                declared
                    .as_ref()
                    .and_then(|c| c.templates.get(i))
                    .map_or("invariant", |t| t.variance),
                strict,
            )
        })
        .fold(Tri::Yes, Tri::and)
}

pub(super) fn valid_variance(a: &Type, b: &Type, declared: &str, strict: bool) -> Tri {
    match (a, b) {
        (Type::Wildcard, _) => Tri::Yes,
        (_, Type::Wildcard) => Tri::No,
        (Type::Projection(c, a), b) => {
            let b = match b {
                Type::Projection(d, _) if d != c => return Tri::No,
                Type::Projection(_, b) => &**b,
                b => b,
            };
            if matches!(b, Type::Never(_))
                || (!strict
                    && (matches!(**a, Type::Mixed { .. } | Type::SubtractedMixed { .. })
                        || matches!(b, Type::Mixed { .. } | Type::SubtractedMixed { .. })))
            {
                return Tri::Yes;
            }
            if *c {
                is_super_type_of(a, b)
            } else {
                is_super_type_of(b, a)
            }
        }
        (_, Type::Projection(..)) => Tri::No,
        (_, Type::Never(_)) => Tri::Yes,
        // A narrowed `mixed~T` is still PHPStan's `MixedType` here.
        (Type::Mixed { .. } | Type::SubtractedMixed { .. }, _)
        | (_, Type::Mixed { .. } | Type::SubtractedMixed { .. })
            if !strict =>
        {
            Tri::Yes
        }
        (Type::Benevolent(_), _) if !strict && !is_super_type_of(a, b).no() => Tri::Yes,
        (_, Type::Benevolent(_)) if !strict && !is_super_type_of(b, a).no() => Tri::Yes,
        _ if declared == "covariant" => is_super_type_of(a, b),
        _ if declared == "contravariant" => is_super_type_of(b, a),
        (Type::Template(a), Type::Template(b)) if a.name == b.name && a.scope == b.scope => {
            Tri::Yes
        }
        (Type::Mixed { .. } | Type::SubtractedMixed { .. }, _) => Tri::Yes,
        (Type::Benevolent(_), _) if !is_super_type_of(a, b).no() => Tri::Yes,
        (_, Type::Benevolent(_)) if !is_super_type_of(b, a).no() => Tri::Maybe,
        (_, Type::Mixed { .. } | Type::SubtractedMixed { .. }) => Tri::Maybe,
        _ => Tri::from(a == b || implicit_mixed(a) == implicit_mixed(b)),
    }
}

/// For PHPStan's `Type::equals()`: MixedType::equals() ignores whether a `mixed` is explicit, so `array` equals
/// `array<mixed>`.
fn implicit_mixed(t: &Type) -> Type {
    match t {
        Type::Mixed { .. } => Type::Mixed { explicit: false },
        Type::SubtractedMixed { excluded, .. } => Type::SubtractedMixed {
            explicit: false,
            excluded: Box::new(implicit_mixed(excluded)),
        },
        t => t.traverse(&implicit_mixed),
    }
}

fn const_array_super(a: &ConstArray, b: &ConstArray) -> Tri {
    if a.keys.is_empty() {
        return b.is_iterable_at_least_once().negate();
    }
    let mut result = Tri::Yes;
    for (i, key) in a.keys.iter().enumerate() {
        let has = match b.keys.iter().position(|k| k == key) {
            None => Tri::No,
            Some(j) if b.is_optional_key(j) => Tri::Maybe,
            Some(_) => Tri::Yes,
        };
        if has.no() {
            if !a.is_optional_key(i) {
                return Tri::No;
            }
            continue;
        } else if has.maybe() && !a.is_optional_key(i) {
            result = result.and(Tri::Maybe);
        }
        let j = b.keys.iter().position(|k| k == key).expect("offset");
        let r = is_super_type_of(&a.values[i], &b.values[j]);
        if r.no() {
            return r;
        }
        result = result.and(r);
    }
    result
}

fn is_sub_type_of(a: &Type, b: &Type) -> Tri {
    use Type as T;
    match a {
        T::StrictMixed => Tri::from(matches!(
            b,
            T::StrictMixed | T::Mixed { .. } | T::SubtractedMixed { .. } | T::Error
        ))
        .or(Tri::Maybe),
        T::Mixed { .. } | T::Error => Tri::Maybe,
        T::SubtractedMixed { excluded, .. } => {
            if is_super_type_of(excluded, b).yes() {
                Tri::No
            } else {
                Tri::Maybe
            }
        }
        T::Union(types) | T::Benevolent(types) => {
            Tri::extreme_identity(types.iter().map(|t| is_super_type_of(b, t)))
        }
        T::Intersection(types) => Tri::max_min(types.iter().map(|t| is_super_type_of(b, t))),
        T::IntRange(min, max) => match b {
            T::Int | T::ConstInt(_) | T::IntRange(..) | T::Intersection(_) => {
                is_super_type_of(b, a)
            }
            T::Union(types) | T::Benevolent(types) => {
                if let (Some(min), Some(max)) = (min, max) {
                    let covered = types
                        .iter()
                        .filter(|t| matches!(t, T::ConstInt(v) if v >= min && v <= max))
                        .count();
                    if covered as i64 == max - min + 1 {
                        return Tri::Yes;
                    }
                }
                types
                    .iter()
                    .map(|t| is_sub_type_of(a, t))
                    .fold(Tri::No, Tri::or)
            }
            _ => Tri::No,
        },
        T::Iterable(..) => {
            if matches!(b, T::Intersection(_) | T::Union(_) | T::Benevolent(_)) {
                return is_super_type_of(
                    b,
                    &T::Union(vec![
                        T::array(a.iterable_key_type(), a.iterable_value_type()),
                        T::Object("Traversable".into(), vec![], false),
                    ]),
                );
            }
            if b.is_constant_array().yes() && b.is_iterable_at_least_once().no() {
                return Tri::Maybe;
            }
            let limit = if matches!(b, T::Iterable(..)) {
                Tri::Yes
            } else {
                Tri::Maybe
            };
            limit
                .and(b.is_iterable())
                .and(is_super_type_of(
                    &b.iterable_value_type(),
                    &a.iterable_value_type(),
                ))
                .and(is_super_type_of(
                    &b.iterable_key_type(),
                    &a.iterable_key_type(),
                ))
        }
        T::Callable(_) => b.is_callable().and(Tri::Maybe),
        T::Accessory(acc) => {
            let same = matches!(b, T::Accessory(x) if x == acc);
            match acc {
                Accessory::NonFalsyString
                    if matches!(b, T::Accessory(Accessory::NonEmptyString)) =>
                {
                    Tri::Yes
                }
                Accessory::DecimalIntString { inverse: false }
                    if matches!(
                        b,
                        T::Accessory(
                            Accessory::NumericString
                                | Accessory::LowercaseString
                                | Accessory::UppercaseString
                        )
                    ) =>
                {
                    Tri::Yes
                }
                _ => {
                    let q = match acc {
                        Accessory::NonEmptyString => b.is_non_empty_string(),
                        Accessory::NonFalsyString => b.is_non_falsy_string(),
                        Accessory::NumericString => b.is_numeric_string(),
                        Accessory::LiteralString => b.is_literal_string(),
                        Accessory::LowercaseString => b.is_lowercase_string(),
                        Accessory::UppercaseString => b.is_uppercase_string(),
                        Accessory::DecimalIntString { inverse } => decimal_int_super(*inverse, b),
                        Accessory::NonEmptyArray => b.is_array().and(b.is_iterable_at_least_once()),
                        Accessory::List => b.is_list(),
                        Accessory::HasMethod(name) => b.has_method(name),
                        Accessory::HasOffsetValue(key, value) => {
                            return offset_value_relation(key, value, b, true);
                        }
                        Accessory::ModelProperty(t) => {
                            return match b {
                                T::Accessory(Accessory::ModelProperty(other)) => {
                                    is_super_type_of(other, t)
                                }
                                _ => b.is_string().and(Tri::Maybe),
                            };
                        }
                        Accessory::ViewString => b.is_string(),
                    };
                    q.and(if same { Tri::Yes } else { Tri::Maybe })
                }
            }
        }
        T::Template(tpl) => match b {
            T::Template(other) => {
                let r = is_super_type_of(&other.bound, &tpl.bound);
                if other.scope == tpl.scope && other.name == tpl.name {
                    r
                } else {
                    r.and(Tri::Maybe)
                }
            }
            _ => is_super_type_of(b, &tpl.bound),
        },
        T::Conditional(c) => {
            let r = c.resolve();
            if r.is_compound() {
                is_sub_type_of(&r, b)
            } else {
                is_super_type_of(b, &r)
            }
        }
        _ => Tri::Yes,
    }
}
