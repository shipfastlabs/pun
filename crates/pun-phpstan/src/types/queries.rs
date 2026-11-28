use super::{
    Accessory, NeverKind, Tri, Type, array_key, has_method, intersect, is_instance_of,
    is_super_type_of, iterable_arg, mixed, static_object, static_of, string_is_callable, union,
};
use crate::symbols;

impl Type {
    pub fn array(key: Type, item: Type) -> Type {
        let key = if key.is_benevolent_array_key() {
            mixed()
        } else if matches!(key, Type::StrictMixed) {
            Type::Union(vec![Type::String, Type::Int])
        } else {
            key
        };
        Type::Array(Box::new(key), Box::new(item))
    }

    pub fn list(item: Type) -> Type {
        Type::Intersection(vec![
            Type::array(Type::IntRange(Some(0), None), item),
            Type::Accessory(Accessory::List),
        ])
    }

    pub(super) fn is_benevolent_array_key(&self) -> bool {
        matches!(self, Type::Benevolent(types) if types.len() == 2 && types.contains(&Type::Int) && types.contains(&Type::String))
    }

    pub fn int_range(min: Option<i64>, max: Option<i64>) -> Type {
        match (min, max) {
            (Some(a), Some(b)) if a > b => Type::Never(NeverKind::Implicit),
            (Some(a), Some(b)) if a == b => Type::ConstInt(a),
            (None, None) => Type::Int,
            (None, Some(i64::MIN)) => Type::ConstInt(i64::MIN),
            (Some(i64::MAX), None) => Type::ConstInt(i64::MAX),
            _ => Type::IntRange(min, max),
        }
    }

    pub(super) fn int_bounds(&self) -> Option<(Option<i64>, Option<i64>)> {
        match self {
            Type::Int => Some((None, None)),
            Type::IntRange(min, max) => Some((*min, *max)),
            Type::ConstInt(v) => Some((Some(*v), Some(*v))),
            _ => None,
        }
    }

    pub(super) fn is_implicit_never(&self) -> bool {
        matches!(self.unreasoned(), Type::Never(NeverKind::Implicit))
    }

    pub fn is_never(&self) -> bool {
        matches!(self.unreasoned(), Type::Never(_))
    }

    pub(super) fn is_plain_implicit_mixed(&self) -> bool {
        matches!(self, Type::Mixed { explicit: false })
    }

    pub(super) fn is_compound(&self) -> bool {
        matches!(
            self,
            Type::Mixed { .. }
                | Type::StrictMixed
                | Type::SubtractedMixed { .. }
                | Type::Error
                | Type::Never(_)
                | Type::Reasoned(..)
                | Type::LateArrayShape(_)
                | Type::IntRange(..)
                | Type::Iterable(..)
                | Type::Callable(_)
                | Type::Union(_)
                | Type::Benevolent(_)
                | Type::Intersection(_)
                | Type::Accessory(_)
                | Type::Conditional(_)
                | Type::Template(_)
        )
    }

    fn query(&self, f: &dyn Fn(&Type) -> Tri) -> Tri {
        self.query_mixed(f, Tri::Maybe)
    }

    pub fn query_members(&self, f: &dyn Fn(&Type) -> Tri) -> Tri {
        self.query_mixed(f, Tri::Yes)
    }

    fn query_mixed(&self, f: &dyn Fn(&Type) -> Tri, mixed: Tri) -> Tri {
        match self {
            Type::Reasoned(t, _) => t.query_mixed(f, mixed),
            Type::SubtractedObject { object, .. } => object.query_mixed(f, mixed),
            Type::LateArrayShape(s) => s.resolve().query_mixed(f, mixed),
            Type::Union(types) => {
                Tri::extreme_identity(types.iter().map(|t| t.query_mixed(f, mixed)))
            }
            Type::Benevolent(types) => types
                .iter()
                .map(|t| t.query_mixed(f, mixed))
                .fold(Tri::No, Tri::or),
            Type::Intersection(types) => {
                Tri::max_min(types.iter().map(|t| t.query_mixed(f, mixed)))
            }
            Type::Template(t) => t.bound.query_mixed(f, mixed),
            Type::Conditional(c) => c.resolve().query_mixed(f, mixed),
            Type::Mixed { .. } | Type::SubtractedMixed { .. } | Type::Error => mixed,
            Type::Never(_) => Tri::No,
            _ => f(self),
        }
    }

    fn query_c(&self, f: &dyn Fn(&Type) -> Tri) -> Tri {
        self.query(&|t| {
            if matches!(t, Type::Callable(_)) {
                Tri::Maybe
            } else {
                f(t)
            }
        })
    }

    fn query_with_mixed_domain(
        &self,
        query: fn(&Type) -> Tri,
        domain: fn() -> Type,
        benevolent: bool,
        f: &dyn Fn(&Type) -> Tri,
    ) -> Tri {
        match self {
            Type::Reasoned(t, _) => query(t),
            Type::SubtractedObject { object, .. } => query(object),
            Type::LateArrayShape(s) => query(&s.resolve()),
            Type::Union(types) => Tri::extreme_identity(types.iter().map(query)),
            Type::Benevolent(types) if benevolent => types.iter().map(query).fold(Tri::No, Tri::or),
            Type::Benevolent(types) => Tri::extreme_identity(types.iter().map(query)),
            Type::Intersection(types) => Tri::max_min(types.iter().map(query)),
            Type::Template(t) if matches!(&t.bound, Type::SubtractedMixed { .. }) => Tri::Maybe,
            Type::Template(t) => query(&t.bound),
            Type::Conditional(c) => query(&c.resolve()),
            Type::SubtractedMixed { excluded, .. } => {
                if is_super_type_of(excluded, &domain()).yes() {
                    Tri::No
                } else {
                    Tri::Maybe
                }
            }
            Type::Mixed { .. } | Type::Error => Tri::Maybe,
            _ => f(self),
        }
    }

    fn query_c_with_mixed_domain(
        &self,
        query: fn(&Type) -> Tri,
        domain: fn() -> Type,
        benevolent: bool,
        f: &dyn Fn(&Type) -> Tri,
    ) -> Tri {
        self.query_with_mixed_domain(query, domain, benevolent, &|t| {
            if matches!(t, Type::Callable(_)) {
                Tri::Maybe
            } else {
                f(t)
            }
        })
    }

    pub(super) fn is_callable_array(&self) -> bool {
        matches!(self, Type::Intersection(_)) && self.is_callable().yes() && self.is_array().yes()
    }

    pub fn is_string(&self) -> Tri {
        match self {
            Type::Union(ts) | Type::Benevolent(ts) => {
                return Tri::extreme_identity(ts.iter().map(Type::is_string));
            }
            Type::Intersection(ts) => return Tri::max_min(ts.iter().map(Type::is_string)),
            Type::Template(t) if matches!(&t.bound, Type::SubtractedMixed { .. }) => {
                return Tri::Maybe;
            }
            Type::Template(t) => return t.bound.is_string(),
            Type::Conditional(c) => return c.resolve().is_string(),
            _ => {}
        }
        self.query_c_with_mixed_domain(Self::is_string, || Type::String, false, &|t| {
            if matches!(
                t,
                Type::Accessory(Accessory::HasMethod(_) | Accessory::HasOffsetValue(..))
            ) {
                return Tri::Maybe;
            }
            Tri::from(matches!(
                t,
                Type::String
                    | Type::ConstString(_)
                    | Type::ClassString(_)
                    | Type::StringAcceptingStringable
                    | Type::StringNotAcceptingStringable
                    | Type::Accessory(
                        Accessory::NonEmptyString
                            | Accessory::NonFalsyString
                            | Accessory::NumericString
                            | Accessory::LiteralString
                            | Accessory::LowercaseString
                            | Accessory::UppercaseString
                            | Accessory::DecimalIntString { .. }
                            | Accessory::ModelProperty(_)
                            | Accessory::ViewString
                    )
            ))
        })
    }

    pub fn is_integer(&self) -> Tri {
        self.query(&|t| {
            Tri::from(matches!(
                t,
                Type::Int | Type::ConstInt(_) | Type::IntRange(..)
            ))
        })
    }

    pub fn is_float(&self) -> Tri {
        self.query_with_mixed_domain(Self::is_float, || Type::Float, false, &|t| {
            Tri::from(matches!(t, Type::Float | Type::ConstFloat(_)))
        })
    }

    pub fn is_boolean(&self) -> Tri {
        self.query_with_mixed_domain(Self::is_boolean, || Type::Bool, false, &|t| {
            Tri::from(matches!(t, Type::Bool | Type::ConstBool(_)))
        })
    }

    pub fn is_object(&self) -> Tri {
        self.query_c_with_mixed_domain(Self::is_object, || Type::ObjectWithoutClass, true, &|t| {
            match t {
                Type::NonexistentParent
                | Type::ObjectWithoutClass
                | Type::Object(..)
                | Type::Static(..)
                | Type::This(_)
                | Type::EnumCase(..)
                | Type::ObjectShape(_)
                | Type::Closure(_) => Tri::Yes,
                Type::Iterable(..)
                | Type::Accessory(Accessory::HasMethod(_) | Accessory::HasOffsetValue(..)) => {
                    Tri::Maybe
                }
                _ => Tri::No,
            }
        })
    }

    pub(super) fn is_class_string(&self) -> Tri {
        self.query_c(&|t| match t {
            Type::ClassString(_) => Tri::Yes,
            Type::Accessory(Accessory::HasMethod(_)) => Tri::Maybe,
            Type::String
            | Type::StringAcceptingStringable
            | Type::StringNotAcceptingStringable
            | Type::Accessory(
                Accessory::NonEmptyString
                | Accessory::NonFalsyString
                | Accessory::LiteralString
                | Accessory::ModelProperty(_)
                | Accessory::ViewString,
            ) => Tri::Maybe,
            Type::ConstString(s) => Tri::from(
                std::str::from_utf8(s)
                    .ok()
                    .and_then(symbols::lookup)
                    .is_some(),
            ),
            _ => Tri::No,
        })
    }

    pub fn is_array(&self) -> Tri {
        self.query_c_with_mixed_domain(
            Self::is_array,
            || Type::array(mixed(), mixed()),
            false,
            &|t| {
                if matches!(t, Type::Iterable(..)) {
                    return Tri::Maybe;
                }
                if matches!(t, Type::Accessory(Accessory::HasOffsetValue(..))) {
                    return Tri::Maybe;
                }
                Tri::from(matches!(
                    t,
                    Type::Array(..)
                        | Type::ConstArray(_)
                        | Type::Accessory(
                            Accessory::List | Accessory::NonEmptyArray | Accessory::OversizedArray
                        )
                ))
            },
        )
    }

    pub fn is_constant_array(&self) -> Tri {
        if self.is_callable_array() {
            return Tri::Yes;
        }
        self.query_c(&|t| match t {
            Type::ConstArray(_) => Tri::Yes,
            Type::Array(..)
            | Type::Accessory(
                Accessory::List
                | Accessory::NonEmptyArray
                | Accessory::OversizedArray
                | Accessory::HasOffsetValue(..),
            ) => Tri::Maybe,
            _ => Tri::No,
        })
    }

    // PHPStan's Type::isOversizedArray(): only OversizedArrayType answers yes,
    // a constant array or a non-array answers no, any other array maybe.
    // query() answers maybe for mixed, as MixedType does. MixedType's no for a
    // subtracted type that covers oversized arrays is left out, since no caller
    // asks a mixed: two ask an intersection, and is_super_type_of() sends a
    // compound type such as mixed to is_sub_type_of() before accessory_super().
    pub(super) fn is_oversized_array(&self) -> Tri {
        self.query_c(&|t| match t {
            Type::Accessory(Accessory::OversizedArray) => Tri::Yes,
            Type::Array(..)
            | Type::Iterable(..)
            | Type::Accessory(
                Accessory::List | Accessory::NonEmptyArray | Accessory::HasOffsetValue(..),
            ) => Tri::Maybe,
            _ => Tri::No,
        })
    }

    pub fn is_constant_value(&self) -> Tri {
        self.query(&|t| {
            Tri::from(matches!(
                t,
                Type::ConstBool(_)
                    | Type::ConstInt(_)
                    | Type::ConstFloat(_)
                    | Type::ConstString(_)
                    | Type::Null
                    | Type::ConstArray(_)
            ))
        })
    }

    pub(super) fn is_constant_scalar(&self) -> bool {
        match self.scalar_class_type() {
            Type::ConstBool(_)
            | Type::ConstInt(_)
            | Type::ConstFloat(_)
            | Type::ConstString(_)
            | Type::Null => true,
            Type::Union(parts) | Type::Benevolent(parts) => {
                parts.iter().all(Self::is_constant_scalar)
            }
            _ => false,
        }
    }

    pub(super) fn scalar_class_type(&self) -> &Type {
        match self {
            Type::Template(t) => match &t.bound {
                bound @ (Type::ConstInt(_) | Type::ConstString(_) | Type::Null) => bound,
                bound @ (Type::Union(_) | Type::Benevolent(_)) => bound,
                Type::String => &Type::String,
                Type::Int | Type::IntRange(..) => &Type::Int,
                Type::Bool | Type::ConstBool(_) => &Type::Bool,
                Type::Template(_) => match t.bound.scalar_class_type() {
                    Type::String | Type::ConstString(_) => &Type::String,
                    Type::Int | Type::ConstInt(_) => &Type::Int,
                    Type::Bool => &Type::Bool,
                    Type::Null => &Type::Null,
                    bound @ (Type::Union(_) | Type::Benevolent(_)) => bound,
                    _ => self,
                },
                _ => self,
            },
            _ => self,
        }
    }

    pub fn is_list(&self) -> Tri {
        if self.is_callable_array() {
            return Tri::Yes;
        }
        self.query_c_with_mixed_domain(Self::is_list, || Type::list(mixed()), false, &|t| match t {
            Type::Accessory(Accessory::List) => Tri::Yes,
            Type::Accessory(Accessory::HasOffsetValue(key, _)) => {
                if key.is_string().yes() {
                    Tri::No
                } else {
                    Tri::Maybe
                }
            }
            Type::ConstArray(a) => a.is_list,
            Type::Array(key, _) if is_super_type_of(&Type::int_range(Some(0), None), key).no() => {
                Tri::No
            }
            Type::Array(..)
            | Type::Accessory(Accessory::NonEmptyArray | Accessory::OversizedArray)
            | Type::Iterable(..) => Tri::Maybe,
            _ => Tri::No,
        })
    }

    pub fn is_iterable(&self) -> Tri {
        if self.is_never() {
            return Tri::Yes;
        }
        self.query_c_with_mixed_domain(
            Self::is_iterable,
            || Type::Iterable(Box::new(mixed()), Box::new(mixed())),
            true,
            &|t| match t {
                Type::Array(..)
                | Type::ConstArray(_)
                | Type::Iterable(..)
                | Type::Accessory(
                    Accessory::List | Accessory::NonEmptyArray | Accessory::OversizedArray,
                ) => Tri::Yes,
                Type::Object(class, _, _) | Type::Static(class, _) | Type::This(class) => {
                    is_instance_of(class, "Traversable")
                }
                Type::ObjectWithoutClass
                | Type::ObjectShape(_)
                | Type::Accessory(Accessory::HasMethod(_) | Accessory::HasOffsetValue(..)) => {
                    Tri::Maybe
                }
                _ => Tri::No,
            },
        )
    }

    pub fn is_cloneable(&self) -> Tri {
        match self {
            Type::Reasoned(t, _) => t.is_cloneable(),
            Type::LateArrayShape(s) => s.resolve().is_cloneable(),
            Type::Mixed { .. } | Type::SubtractedMixed { .. } | Type::Error | Type::Never(_) => {
                Tri::Yes
            }
            Type::Template(t) => t.bound.is_cloneable(),
            _ => self.query(&|t| match t {
                Type::Object(..)
                | Type::ObjectWithoutClass
                | Type::ObjectShape(_)
                | Type::Static(..)
                | Type::This(_)
                | Type::EnumCase(..)
                | Type::Closure(_) => Tri::Yes,
                Type::Callable(_)
                | Type::Iterable(..)
                | Type::Accessory(Accessory::HasMethod(_) | Accessory::HasOffsetValue(..)) => {
                    Tri::Maybe
                }
                _ => Tri::No,
            }),
        }
    }

    pub fn is_iterable_at_least_once(&self) -> Tri {
        match self {
            _ if self.is_callable_array() => Tri::Yes,
            Type::Intersection(types) => {
                let iterable: Vec<&Type> = types.iter().filter(|t| !t.is_iterable().no()).collect();
                if iterable.is_empty() {
                    return Tri::No;
                }
                Tri::max_min(iterable.into_iter().map(Type::is_iterable_at_least_once))
            }
            _ => self.query_with_mixed_domain(
                Self::is_iterable_at_least_once,
                || Type::Iterable(Box::new(mixed()), Box::new(mixed())),
                true,
                &|t| match t {
                    Type::Never(_) => Tri::Maybe,
                    Type::ConstArray(a) => a.is_iterable_at_least_once(),
                    Type::Accessory(Accessory::NonEmptyArray | Accessory::HasOffsetValue(..)) => {
                        Tri::Yes
                    }
                    Type::Array(..)
                    | Type::Iterable(..)
                    | Type::Accessory(Accessory::List | Accessory::OversizedArray) => Tri::Maybe,
                    Type::Object(..)
                    | Type::Static(..)
                    | Type::This(_)
                    | Type::ObjectWithoutClass
                    | Type::Callable(_) => Tri::Maybe,
                    _ => Tri::No,
                },
            ),
        }
    }

    pub(super) fn is_non_empty_string(&self) -> Tri {
        self.query_c(&|t| match t {
            Type::Accessory(
                Accessory::NonEmptyString
                | Accessory::NonFalsyString
                | Accessory::NumericString
                | Accessory::DecimalIntString { inverse: false },
            ) => Tri::Yes,
            Type::ConstString(s) => Tri::from(!s.is_empty()),
            Type::ClassString(_) => Tri::Yes,
            Type::String
            | Type::StringAcceptingStringable
            | Type::StringNotAcceptingStringable
            | Type::Accessory(_) => Tri::Maybe,
            _ => Tri::No,
        })
    }

    pub(super) fn is_non_falsy_string(&self) -> Tri {
        self.query_c(&|t| match t {
            Type::Accessory(Accessory::NonFalsyString) => Tri::Yes,
            Type::ConstString(s) => Tri::from(!s.is_empty() && s.as_slice() != b"0"),
            Type::ClassString(_) => Tri::Yes,
            Type::String
            | Type::StringAcceptingStringable
            | Type::StringNotAcceptingStringable
            | Type::Accessory(_) => Tri::Maybe,
            _ => Tri::No,
        })
    }

    pub fn is_numeric_string(&self) -> Tri {
        self.query(&|t| match t {
            Type::Accessory(
                Accessory::NumericString | Accessory::DecimalIntString { inverse: false },
            ) => Tri::Yes,
            Type::ConstString(s) => Tri::from(
                std::str::from_utf8(s)
                    .ok()
                    .and_then(crate::value::numeric)
                    .is_some(),
            ),
            Type::ClassString(_) => Tri::No,
            Type::String
            | Type::StringAcceptingStringable
            | Type::StringNotAcceptingStringable
            | Type::Accessory(_) => Tri::Maybe,
            _ => Tri::No,
        })
    }

    pub fn is_decimal_int_string(&self) -> Tri {
        self.query(&|t| match t {
            Type::Accessory(Accessory::DecimalIntString { inverse }) => Tri::from(!inverse),
            Type::ConstString(s) => Tri::from(
                std::str::from_utf8(s)
                    .ok()
                    .and_then(|s| s.parse::<i64>().ok())
                    .is_some_and(|i| i.to_string().as_bytes() == s),
            ),
            Type::String
            | Type::StringAcceptingStringable
            | Type::StringNotAcceptingStringable
            | Type::Accessory(
                Accessory::NonEmptyString
                | Accessory::NonFalsyString
                | Accessory::NumericString
                | Accessory::LiteralString
                | Accessory::LowercaseString
                | Accessory::UppercaseString
                | Accessory::HasMethod(_)
                | Accessory::ModelProperty(_)
                | Accessory::ViewString,
            ) => Tri::Maybe,
            _ => Tri::No,
        })
    }

    pub(super) fn is_literal_string(&self) -> Tri {
        self.query_c(&|t| match t {
            Type::Accessory(Accessory::LiteralString) | Type::ConstString(_) => Tri::Yes,
            Type::String
            | Type::StringAcceptingStringable
            | Type::StringNotAcceptingStringable
            | Type::ClassString(_)
            | Type::Accessory(_) => Tri::Maybe,
            _ => Tri::No,
        })
    }

    pub(super) fn is_lowercase_string(&self) -> Tri {
        self.query_c(&|t| match t {
            Type::Accessory(
                Accessory::LowercaseString | Accessory::DecimalIntString { inverse: false },
            ) => Tri::Yes,
            Type::ConstString(s) => Tri::from(s.to_ascii_lowercase() == *s),
            Type::String
            | Type::StringAcceptingStringable
            | Type::StringNotAcceptingStringable
            | Type::ClassString(_)
            | Type::Accessory(_) => Tri::Maybe,
            _ => Tri::No,
        })
    }

    pub(super) fn is_uppercase_string(&self) -> Tri {
        self.query_c(&|t| match t {
            Type::Accessory(
                Accessory::UppercaseString | Accessory::DecimalIntString { inverse: false },
            ) => Tri::Yes,
            Type::ConstString(s) => Tri::from(s.to_ascii_uppercase() == *s),
            Type::String
            | Type::StringAcceptingStringable
            | Type::StringNotAcceptingStringable
            | Type::ClassString(_)
            | Type::Accessory(_) => Tri::Maybe,
            _ => Tri::No,
        })
    }

    pub fn is_callable(&self) -> Tri {
        self.query_with_mixed_domain(
            Self::is_callable,
            || Type::Callable(None),
            true,
            &|t| match t {
                Type::Callable(_) | Type::Closure(_) => Tri::Yes,
                Type::Accessory(Accessory::HasMethod(n)) if n.eq_ignore_ascii_case("__invoke") => {
                    Tri::Yes
                }
                Type::Accessory(
                    Accessory::NumericString | Accessory::DecimalIntString { inverse: false },
                ) => Tri::No,
                Type::String
                | Type::StringAcceptingStringable
                | Type::StringNotAcceptingStringable
                | Type::ClassString(_)
                | Type::Accessory(_)
                | Type::Iterable(..)
                | Type::ObjectWithoutClass
                | Type::ObjectShape(_) => Tri::Maybe,
                Type::ConstString(s) => string_is_callable(s),
                Type::Array(_, item) => Tri::Maybe.and(item.is_string()),
                Type::ConstArray(a) => a.is_callable(),
                Type::Object(class, _, _) | Type::Static(class, _) | Type::This(class) => {
                    match symbols::lookup(class) {
                        Some(c) if has_method(&c, "__invoke") => Tri::Yes,
                        Some(c) if c.is_final || matches!(t, Type::Object(_, _, true)) => Tri::No,
                        _ => Tri::Maybe,
                    }
                }
                _ => Tri::No,
            },
        )
    }

    pub fn has_method(&self, name: &str) -> Tri {
        self.query(&|t| match t {
            Type::Accessory(Accessory::HasMethod(n)) => {
                if n.eq_ignore_ascii_case(name) {
                    Tri::Yes
                } else {
                    Tri::Maybe
                }
            }
            Type::Object(n, _, _) | Type::Static(n, _) | Type::This(n) | Type::EnumCase(n, _) => {
                match symbols::lookup(n) {
                    Some(c) if has_method(&c, &name.to_ascii_lowercase()) => Tri::Yes,
                    Some(c) if c.is_final || matches!(t, Type::Object(_, _, true)) => Tri::No,
                    _ => Tri::Maybe,
                }
            }
            Type::ClassString(_)
            | Type::ObjectWithoutClass
            | Type::ObjectShape(_)
            | Type::Iterable(..)
            | Type::Callable(_) => Tri::Maybe,
            Type::ConstString(_) if t.is_class_string().yes() => Tri::Maybe,
            _ => Tri::No,
        })
    }

    pub fn require_method(self, name: &str) -> Type {
        let accessory = Type::Accessory(Accessory::HasMethod(name.to_string()));
        if self.is_string().yes() {
            return if self.is_class_string().yes() {
                intersect(vec![self, accessory])
            } else {
                self
            };
        }
        intersect(vec![
            self,
            union(vec![
                intersect(vec![Type::ObjectWithoutClass, accessory]),
                Type::ClassString(None),
            ]),
        ])
    }

    pub fn object_class_names(&self) -> Vec<String> {
        match self {
            Type::SubtractedObject { object, .. } => object.object_class_names(),
            Type::Object(class, _, _)
            | Type::Static(class, _)
            | Type::This(class)
            | Type::EnumCase(class, _) => vec![class.clone()],
            Type::Closure(_) => vec!["Closure".to_string()],
            Type::Template(t) => t.bound.object_class_names(),
            Type::Union(types) | Type::Benevolent(types) | Type::Intersection(types) => {
                let mut names = Vec::new();
                for t in types {
                    let inner = t.object_class_names();
                    if inner.is_empty()
                        && (matches!(self, Type::Union(_))
                            || matches!(self, Type::Benevolent(_)) && t.is_object().yes())
                    {
                        return Vec::new();
                    }
                    for name in inner {
                        if !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }
                names
            }
            _ => vec![],
        }
    }

    pub fn iterable_key_type(&self) -> Type {
        match self {
            Type::Reasoned(t, _) => t.iterable_key_type(),
            Type::SubtractedObject { object, .. } => object.iterable_key_type(),
            Type::LateArrayShape(s) => s.resolve().iterable_key_type(),
            Type::NonexistentParent => Type::Error,
            Type::StrictMixed => self.clone(),
            Type::Template(t) if matches!(t.bound, Type::StrictMixed) => self.clone(),
            _ if self.is_callable_array() => {
                Type::Union(vec![Type::ConstInt(0), Type::ConstInt(1)])
            }
            Type::Array(key, _) => {
                if matches!(**key, Type::Mixed { .. }) {
                    array_key()
                } else {
                    (**key).clone()
                }
            }
            Type::ConstArray(a) => a.iterable_key_type(),
            Type::Iterable(key, _) => (**key).clone(),
            Type::Accessory(Accessory::List) => Type::int_range(Some(0), None),
            Type::Intersection(types) => {
                intersect(types.iter().map(Type::iterable_key_type).collect())
            }
            Type::Union(types) => union(types.iter().map(Type::iterable_key_type).collect()),
            Type::Template(t) => t.bound.iterable_key_type(),
            Type::Object(class, args, _) => iterable_arg(class, args, true),
            Type::Static(class, args) => static_of(class, args).iterable_key_type(),
            Type::This(class) => static_object(class).iterable_key_type(),
            Type::Never(_) => Type::Never(NeverKind::Implicit),
            _ => mixed(),
        }
    }

    pub fn iterable_value_type(&self) -> Type {
        match self {
            Type::Reasoned(t, _) => t.iterable_value_type(),
            Type::SubtractedObject { object, .. } => object.iterable_value_type(),
            Type::LateArrayShape(s) => s.resolve().iterable_value_type(),
            Type::NonexistentParent => Type::Error,
            Type::StrictMixed => self.clone(),
            Type::Template(t) if matches!(t.bound, Type::StrictMixed) => self.clone(),
            Type::Intersection(types) if self.is_callable_array() => intersect(vec![
                intersect(types.iter().map(Type::iterable_value_type).collect()),
                Type::Union(vec![
                    Type::ObjectWithoutClass,
                    Type::Intersection(vec![
                        Type::String,
                        Type::Accessory(Accessory::NonFalsyString),
                    ]),
                ]),
            ]),
            Type::Array(_, value) | Type::Iterable(_, value) => (**value).clone(),
            Type::ConstArray(a) => a.iterable_value_type(),
            Type::Intersection(types) => {
                intersect(types.iter().map(Type::iterable_value_type).collect())
            }
            Type::Union(types) => union(types.iter().map(Type::iterable_value_type).collect()),
            Type::Template(t) => t.bound.iterable_value_type(),
            Type::Object(class, args, _) => {
                let value = iterable_arg(class, args, false);
                if value.is_implicit_mixed()
                    && (is_instance_of(class, "Iterator").yes()
                        || is_instance_of(class, "IteratorAggregate").yes())
                {
                    Type::Mixed { explicit: true }
                } else {
                    value
                }
            }
            Type::Static(class, args) => static_of(class, args).iterable_value_type(),
            Type::This(class) => static_object(class).iterable_value_type(),
            Type::Never(_) => Type::Never(NeverKind::Implicit),
            _ => mixed(),
        }
    }
}

impl Type {
    pub fn is_null(&self) -> Tri {
        self.query_with_mixed_domain(Self::is_null, || Type::Null, false, &|t| {
            Tri::from(matches!(t, Type::Null))
        })
    }

    pub fn is_void(&self) -> Tri {
        if matches!(self, Type::Intersection(_)) {
            return Tri::No;
        }
        self.query_with_mixed_domain(Self::is_void, || Type::Void, true, &|t| {
            Tri::from(matches!(t, Type::Void))
        })
    }

    pub fn is_implicit_mixed(&self) -> bool {
        matches!(
            self,
            Type::Mixed { explicit: false }
                | Type::SubtractedMixed {
                    explicit: false,
                    ..
                }
        )
    }

    pub fn add_null(self) -> Type {
        if self.is_null().no() {
            union(vec![self, Type::Null])
        } else {
            self
        }
    }

    pub fn remove_null(self) -> Type {
        match self {
            Type::Null => Type::Never(NeverKind::Implicit),
            Type::Union(types) if types.iter().any(|t| matches!(t, Type::Null)) => union(
                types
                    .into_iter()
                    .filter(|t| !matches!(t, Type::Null))
                    .collect(),
            ),
            Type::Benevolent(types) if types.iter().any(|t| matches!(t, Type::Null)) => {
                match union(
                    types
                        .into_iter()
                        .filter(|t| !matches!(t, Type::Null))
                        .collect(),
                ) {
                    Type::Union(types) => Type::Benevolent(types),
                    t => t,
                }
            }
            t => t,
        }
    }

    pub fn resolve_to_bounds(&self) -> Type {
        match self {
            Type::Template(t) => t.bound.resolve_to_bounds(),
            t => t.traverse(&Type::resolve_to_bounds),
        }
    }
}
