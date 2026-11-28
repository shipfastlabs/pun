use crate::symbols;

macro_rules! description {
    ($($part:expr),+ $(,)?) => {
        [$(AsRef::<[u8]>::as_ref(&$part)),+].concat()
    };
}

mod accepts;
mod casts;
mod classes;
mod const_array;
mod decide;
mod describe;
mod extraction;
mod intersect;
mod larastan;
mod late_shape;
mod mixed;
mod properties;
mod queries;
mod reasons;
mod sort;
mod super_type;
mod templates;
mod to_phpdoc;
mod traverse;
mod union;
mod unresolvable;
mod verbosity;
pub use accepts::{StrictMixed, accepts, accepts_with};
pub use casts::{Cast, cast_is_error};
pub use classes::ancestor_args;
pub use const_array::{ARRAY_COUNT_LIMIT, ShapeBuilder, oversized_array};
pub use decide::decide_type;
pub use describe::float_to_string;
pub use intersect::intersect;
pub use larastan::{StringQuery, with_larastan_late_types, with_larastan_strings};
pub use late_shape::LateArrayShape;
pub use properties::{FoundProperty, ObjectProperty, with_object_properties};
pub use reasons::accepts_reasons;
pub use super_type::{is_super_type_of, to_benevolent};
pub use templates::{
    TemplateMap, infer_template_types, remove_final_by_keyword_overrides, resolve_template_types,
};
pub use to_phpdoc::{to_phpdoc, to_phpdoc_fully_qualified};
pub use union::{remove_object, sort_types, union};
pub use unresolvable::{PropertyReflection, with_property_reflections};
pub use verbosity::recommended_level;

use classes::{
    canonical_class, has_method, is_instance_of, iterable_arg, static_object, static_of,
    string_is_callable, with_class_args,
};
use const_array::is_valid_identifier;
use describe::{display_bytes, print_templates};
pub(crate) use super_type::contains_template;
use super_type::{
    callable_signatures, object_super, signature_super, specialize_signature, valid_variance,
};
use to_phpdoc::to_phpdoc_bytes;
use union::{compare_in_union, compare_strings, intersect_object_exclusion};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tri {
    No = 0,
    Maybe = 1,
    Yes = 3,
}

impl Tri {
    pub fn from(b: bool) -> Tri {
        if b { Tri::Yes } else { Tri::No }
    }
    pub fn yes(self) -> bool {
        self == Tri::Yes
    }
    pub fn no(self) -> bool {
        self == Tri::No
    }
    pub fn maybe(self) -> bool {
        self == Tri::Maybe
    }
    pub fn and(self, o: Tri) -> Tri {
        self.min(o)
    }
    pub fn or(self, o: Tri) -> Tri {
        self.max(o)
    }
    pub fn negate(self) -> Tri {
        match self {
            Tri::Yes => Tri::No,
            Tri::No => Tri::Yes,
            Tri::Maybe => Tri::Maybe,
        }
    }
    pub fn extreme_identity(results: impl IntoIterator<Item = Tri>) -> Tri {
        let mut it = results.into_iter();
        let first = it.next().unwrap_or(Tri::No);
        if it.all(|r| r == first) {
            first
        } else {
            Tri::Maybe
        }
    }
    pub fn max_min(results: impl IntoIterator<Item = Tri>) -> Tri {
        let mut min = Tri::Yes;
        for r in results {
            if r.yes() {
                return r;
            }
            min = min.and(r);
        }
        min
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    TypeOnly,
    Value,
    Precise,
}

impl Level {
    fn handle<T>(self, type_only: impl FnOnce() -> T, value: impl FnOnce() -> T) -> T {
        match self {
            Level::TypeOnly => type_only(),
            _ => value(),
        }
    }
    fn handle3<T>(
        self,
        type_only: impl FnOnce() -> T,
        value: impl FnOnce() -> T,
        precise: impl FnOnce() -> T,
    ) -> T {
        match self {
            Level::TypeOnly => type_only(),
            Level::Value => value(),
            Level::Precise => precise(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NeverKind {
    Implicit,
    Explicit,
    NonAccepting,
}

#[derive(Clone, Debug)]
pub enum Accessory {
    NonEmptyString,
    NonFalsyString,
    NumericString,
    LiteralString,
    LowercaseString,
    UppercaseString,
    DecimalIntString { inverse: bool },
    List,
    NonEmptyArray,
    OversizedArray,
    HasMethod(String),
    HasOffsetValue(Box<Type>, Box<Type>),
    ModelProperty(Box<Type>),
    ViewString,
}

impl PartialEq for Accessory {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::HasMethod(a), Self::HasMethod(b)) => a.eq_ignore_ascii_case(b),
            (Self::HasOffsetValue(ak, av), Self::HasOffsetValue(bk, bv)) => ak == bk && av == bv,
            (Self::ModelProperty(a), Self::ModelProperty(b)) => a == b,
            (Self::DecimalIntString { inverse: a }, Self::DecimalIntString { inverse: b }) => {
                a == b
            }
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConstArray {
    pub keys: Vec<Type>,
    pub values: Vec<Type>,
    pub next_auto_indexes: Vec<i64>,
    pub optional: Vec<usize>,
    pub is_list: Tri,
    pub unsealed: Option<(Type, Type)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Type,
    pub optional: bool,
    pub by_ref: bool,
    pub variadic: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Signature {
    pub params: Vec<Param>,
    pub ret: Type,
    pub templates: Vec<Template>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Subject {
    Parameter(String),
    Type(Type),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Conditional {
    pub subject: Subject,
    pub target: Type,
    pub then: Type,
    pub otherwise: Type,
    pub negated: bool,
    /// `resolve()`'s result, kept like PHPStan's `LateResolvableTypeTrait::$result`. Without it a chain of nested
    /// conditionals resolves exponentially often (every query `union` runs on a branch resolves it again).
    pub resolved: Resolved,
}

/// A cache that every equality ignores, so a resolved and an unresolved conditional still compare equal.
#[derive(Clone, Debug, Default)]
pub struct Resolved(std::sync::OnceLock<Type>);

impl PartialEq for Resolved {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

#[derive(Clone, Debug)]
pub struct Template {
    pub name: String,
    pub scope: String,
    pub bound: Type,
    pub default: Option<Type>,
    pub argument: bool,
}

impl PartialEq for Template {
    fn eq(&self, other: &Self) -> bool {
        self.scope == other.scope
            && self.name == other.name
            && self.bound == other.bound
            && self.default == other.default
    }
}

#[derive(Debug)]
pub enum Traversal {
    Retain,
    Replace(Type),
}

impl Traversal {
    pub fn into_type(self, original: &Type) -> Type {
        match self {
            Self::Retain => original.clone(),
            Self::Replace(ty) => ty,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Type {
    Mixed {
        explicit: bool,
    },
    StrictMixed,
    SubtractedMixed {
        explicit: bool,
        excluded: Box<Type>,
    },
    Never(NeverKind),
    Error,
    Reasoned(Box<Type>, String),
    LateArrayShape(Box<LateArrayShape>),
    Void,
    Null,
    Bool,
    ConstBool(bool),
    Int,
    ConstInt(i64),
    IntRange(Option<i64>, Option<i64>),
    Float,
    ConstFloat(f64),
    String,
    ConstString(Vec<u8>),
    ClassString(Option<Box<Type>>),
    StringAcceptingStringable,
    StringNotAcceptingStringable,
    Resource,
    ObjectWithoutClass,
    NonexistentParent,
    Object(String, Vec<Type>, bool),
    SubtractedObject {
        object: Box<Type>,
        excluded: Box<Type>,
    },
    Static(String, Vec<Type>),
    This(String),
    Array(Box<Type>, Box<Type>),
    ConstArray(Box<ConstArray>),
    Iterable(Box<Type>, Box<Type>),
    Callable(Option<Box<Signature>>),
    Closure(Box<Signature>),
    Union(Vec<Type>),
    Benevolent(Vec<Type>),
    Intersection(Vec<Type>),
    Accessory(Accessory),
    Conditional(Box<Conditional>),
    Template(Box<Template>),
    ObjectShape(Vec<(String, Type, bool)>),
    EnumCase(String, String),
    Wildcard,
    Projection(bool, Box<Type>),
    OffsetAccess(Box<Type>, Box<Type>),
}

pub fn variance_name(covariant: bool) -> &'static str {
    if covariant {
        "covariant"
    } else {
        "contravariant"
    }
}

pub fn mixed() -> Type {
    Type::Mixed { explicit: false }
}

fn array_key() -> Type {
    Type::Benevolent(vec![Type::Int, Type::String])
}
