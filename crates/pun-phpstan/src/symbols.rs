//! PHPStan 2.3.5's reflection of built-in functions, classes and constants (`data/symbols.json`, written by
//! `scripts/export-phpstan-symbols.php`). Types are kept as the strings PHPStan's `toPhpDocNode()` printed, in
//! the syntax [`crate::resolve`] reads back.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, LazyLock};

use std::fmt;
use std::marker::PhantomData;

use serde::Deserialize;
use serde::de::{Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};

use crate::scoped::{Family, Scoped};

pub use pun_tokens::PhpVersion;

const PHP85: PhpVersion = PhpVersion::new(8, 5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ByRef {
    No,
    CreatesNewVariable,
    ReadsArgument,
}

#[derive(Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: String,
    pub optional: bool,
    pub variadic: bool,
    pub by_ref: ByRef,
    pub default: Option<String>,
    pub allowed_constants: Option<ParameterAllowedConstants>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ParameterAllowedConstants {
    pub bitmask: bool,
    pub constants: Vec<String>,
    #[serde(rename = "exclusiveGroups")]
    pub exclusive_groups: Vec<Vec<String>>,
}

pub struct ConstantReflection {
    pub name: String,
    pub builtin: bool,
}

pub struct AllowedConstantsResult<'a> {
    pub disallowed: Vec<&'a ConstantReflection>,
    pub violated_groups: Vec<Vec<&'a str>>,
    pub bitmask_not_allowed: bool,
}

impl AllowedConstantsResult<'_> {
    pub fn is_ok(&self) -> bool {
        self.disallowed.is_empty() && self.violated_groups.is_empty() && !self.bitmask_not_allowed
    }
}

impl ParameterAllowedConstants {
    pub fn check<'a>(&self, constants: &'a [ConstantReflection]) -> AllowedConstantsResult<'a> {
        let names: Vec<_> = constants
            .iter()
            .filter(|c| c.builtin)
            .map(|c| c.name.as_str())
            .collect();
        AllowedConstantsResult {
            disallowed: constants
                .iter()
                .filter(|c| c.builtin && !self.constants.contains(&c.name))
                .collect(),
            violated_groups: if self.bitmask {
                self.exclusive_groups
                    .iter()
                    .filter_map(|group| {
                        let matched: Vec<_> = names
                            .iter()
                            .copied()
                            .filter(|name| group.iter().any(|c| c == name))
                            .collect();
                        (matched.len() >= 2).then_some(matched)
                    })
                    .collect()
            } else {
                Vec::new()
            },
            bitmask_not_allowed: !self.bitmask && constants.len() > 1,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Variant {
    pub params: Vec<Param>,
    pub ret: String,
    pub signature: Option<Signature>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Signature {
    pub phpdoc_return: String,
    pub native_return: String,
    pub templates: Vec<(String, String)>,
    pub resolved_templates: Vec<(String, String)>,
    pub variances: Vec<(String, String)>,
}

#[derive(Debug, PartialEq)]
pub struct Function {
    pub name: String,
    pub side_effects: String,
    pub deprecated: bool,
    pub variants: Vec<Variant>,
    pub named: Option<Vec<Variant>>,
    pub is_static: bool,
    pub doc: Option<MethodDoc>,
}

#[derive(Debug, PartialEq, Default)]
pub struct MethodDoc {
    pub params: HashMap<usize, String>,
    pub ret: Option<String>,
}

#[derive(Debug, PartialEq)]
pub struct Template {
    pub name: String,
    pub bound: String,
    pub default: Option<String>,
    pub variance: &'static str,
}

#[derive(Debug, PartialEq)]
pub struct Class {
    pub name: String,
    pub ancestors: Vec<String>,
    pub templates: Vec<Template>,
    pub cases: Vec<String>,
    pub case_values: Vec<(String, String)>,
    pub is_interface: bool,
    pub is_trait: bool,
    pub is_enum: bool,
    pub is_final: bool,
    pub properties: Vec<String>,
    pub allows_dynamic_properties: bool,
    pub constants: Vec<(String, String)>,
    pub methods: HashMap<String, Function>,
    pub iterable: Option<(String, String)>,
    pub generics: HashMap<String, Vec<String>>,
    pub property_docs: HashMap<String, String>,
}

pub struct Symbols {
    pub functions: HashMap<String, Function>,
    pub classes: HashMap<String, Class>,
    pub constants: HashMap<String, String>,
}

struct Entries<T>(Vec<(String, T)>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Entries<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
            type Value = Entries<T>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a map, or [] for an empty one")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Entries<T>, A::Error> {
                let mut out = Vec::new();
                while let Some(e) = m.next_entry()? {
                    out.push(e);
                }
                Ok(Entries(out))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut s: A) -> Result<Entries<T>, A::Error> {
                while s.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Entries(Vec::new()))
            }
        }
        d.deserialize_any(V(PhantomData))
    }
}

/// symbols.json as scripts/export-phpstan-symbols.php writes it, read straight into these types (a `serde_json::Value`
/// of the 6 MB file costs as much again).
#[derive(Deserialize)]
struct RawParam {
    name: String,
    #[serde(rename = "type")]
    ty: String,
    optional: bool,
    variadic: bool,
    #[serde(rename = "byRef")]
    by_ref: String,
    default: Option<String>,
    #[serde(rename = "allowedConstants")]
    allowed_constants: Option<ParameterAllowedConstants>,
}

#[derive(Deserialize)]
struct RawVariant {
    params: Vec<RawParam>,
    #[serde(rename = "return")]
    ret: String,
    signature: Option<RawSignature>,
}

#[derive(Deserialize)]
struct RawSignature {
    #[serde(rename = "phpDocReturn")]
    phpdoc_return: String,
    #[serde(rename = "nativeReturn")]
    native_return: String,
    templates: Entries<String>,
    #[serde(rename = "resolvedTemplates")]
    resolved_templates: Entries<String>,
    variances: Entries<String>,
}

#[derive(Deserialize)]
struct RawFunction {
    name: String,
    #[serde(rename = "sideEffects")]
    side_effects: String,
    deprecated: bool,
    variants: Vec<RawVariant>,
    named: Option<Vec<RawVariant>>,
    #[serde(rename = "static", default)]
    is_static: bool,
    doc: Option<RawDoc>,
}

#[derive(Deserialize)]
struct RawDoc {
    params: Entries<String>,
    #[serde(rename = "return")]
    ret: Option<String>,
}

#[derive(Deserialize)]
struct RawClass {
    name: String,
    ancestors: Vec<String>,
    templates: Entries<(String, Option<String>)>,
    variances: Option<Entries<String>>,
    cases: Vec<String>,
    interface: bool,
    #[serde(rename = "final")]
    is_final: bool,
    properties: Vec<String>,
    constants: Entries<String>,
    methods: Entries<RawFunction>,
    iterable: Option<(String, String)>,
    generics: Option<Entries<Vec<String>>>,
    #[serde(rename = "propertyDocs")]
    property_docs: Option<Entries<String>>,
}

#[derive(Deserialize)]
struct RawPhp85 {
    functions: Entries<Option<RawFunction>>,
    classes: Entries<Option<RawClass>>,
}

#[derive(Deserialize)]
struct RawSymbols {
    functions: Entries<RawFunction>,
    classes: Entries<RawClass>,
    constants: Entries<String>,
    php85: RawPhp85,
}

fn variants(v: Vec<RawVariant>) -> Vec<Variant> {
    v.into_iter()
        .map(|v| Variant {
            params: v
                .params
                .into_iter()
                .map(|p| Param {
                    name: p.name,
                    ty: p.ty,
                    optional: p.optional,
                    variadic: p.variadic,
                    by_ref: match p.by_ref.as_str() {
                        "no" => ByRef::No,
                        "createsNewVariable" => ByRef::CreatesNewVariable,
                        _ => ByRef::ReadsArgument,
                    },
                    default: p.default,
                    allowed_constants: p.allowed_constants,
                })
                .collect(),
            ret: v.ret,
            signature: v.signature.map(|s| Signature {
                phpdoc_return: s.phpdoc_return,
                native_return: s.native_return,
                templates: s.templates.0,
                resolved_templates: s.resolved_templates.0,
                variances: s.variances.0,
            }),
        })
        .collect()
}

fn read_function(v: RawFunction) -> Function {
    Function {
        name: v.name,
        side_effects: v.side_effects,
        deprecated: v.deprecated,
        variants: variants(v.variants),
        named: v.named.map(variants),
        is_static: v.is_static,
        doc: v.doc.map(|d| MethodDoc {
            params: d
                .params
                .0
                .into_iter()
                .filter_map(|(i, t)| Some((i.parse().ok()?, t)))
                .collect(),
            ret: d.ret,
        }),
    }
}

fn read_class(v: RawClass) -> Class {
    let variances = v.variances.map_or_default(|e| e.0);
    Class {
        name: v.name,
        ancestors: v.ancestors,
        templates: v
            .templates
            .0
            .into_iter()
            .map(|(name, (bound, default))| Template {
                variance: variances.iter().find(|(n, _)| *n == name).map_or(
                    "invariant",
                    |(_, v)| {
                        if v == "covariant" {
                            "covariant"
                        } else {
                            "contravariant"
                        }
                    },
                ),
                name,
                bound,
                default,
            })
            .collect(),
        is_enum: !v.cases.is_empty(),
        cases: v.cases,
        case_values: Vec::new(),
        is_interface: v.interface,
        is_trait: false,
        is_final: v.is_final,
        properties: v.properties,
        allows_dynamic_properties: false,
        constants: v.constants.0,
        methods: v
            .methods
            .0
            .into_iter()
            .map(|(k, m)| {
                const PREFIX: &str = "ps_unreserve_prefix_";
                let mut f = read_function(m);
                if f.name.to_ascii_lowercase().starts_with(PREFIX) {
                    f.name = f.name[PREFIX.len()..].to_string();
                }
                (
                    k.strip_prefix(PREFIX)
                        .map_or_else(|| k.clone(), str::to_string),
                    f,
                )
            })
            .collect(),
        iterable: v.iterable,
        generics: v.generics.map_or_default(|g| g.0.into_iter().collect()),
        property_docs: v
            .property_docs
            .map_or_default(|g| g.0.into_iter().collect()),
    }
}

fn load(version: PhpVersion) -> Symbols {
    let data: RawSymbols =
        serde_json::from_str(include_str!("../data/symbols.json")).expect("symbols.json");
    let mut symbols = Symbols {
        functions: data
            .functions
            .0
            .into_iter()
            .map(|(k, v)| (k, read_function(v)))
            .collect(),
        classes: data
            .classes
            .0
            .into_iter()
            .map(|(k, v)| (k, read_class(v)))
            .collect(),
        constants: data.constants.0.into_iter().collect(),
    };
    if version >= PHP85 {
        for (name, f) in data.php85.functions.0 {
            symbols.functions.remove(&name);
            if let Some(f) = f {
                symbols.functions.insert(name, read_function(f));
            }
        }
        for (name, c) in data.php85.classes.0 {
            symbols.classes.remove(&name);
            if let Some(c) = c {
                symbols.classes.insert(name, read_class(c));
            }
        }
    }
    symbols
}

static PHP84_SYMBOLS: LazyLock<Symbols> = LazyLock::new(|| load(PhpVersion::new(8, 4)));
static PHP85_SYMBOLS: LazyLock<Symbols> = LazyLock::new(|| load(PHP85));

/// PHPStan's built-in symbols for `version`: 8.5's from 8.5 on, 8.4's below it.
pub fn symbols(version: PhpVersion) -> &'static Symbols {
    if version >= PHP85 {
        &PHP85_SYMBOLS
    } else {
        &PHP84_SYMBOLS
    }
}

pub fn class(name: &str) -> Option<&'static Class> {
    symbols(PhpVersion::new(8, 4))
        .classes
        .get(&name.trim_start_matches('\\').to_ascii_lowercase())
}

#[derive(Clone)]
pub enum ClassRef {
    Builtin(&'static Class),
    User(Arc<Class>),
}

impl Deref for ClassRef {
    type Target = Class;
    fn deref(&self) -> &Class {
        match self {
            ClassRef::Builtin(c) => c,
            ClassRef::User(c) => c,
        }
    }
}

struct Provider;
impl Family for Provider {
    type Of<'a> = dyn Fn(&str) -> Option<Arc<Class>> + 'a;
}

thread_local! {
    static PROVIDER: Scoped<Provider> = Scoped::new();
}

pub fn with_classes<R>(provider: &dyn Fn(&str) -> Option<Arc<Class>>, f: impl FnOnce() -> R) -> R {
    PROVIDER.with(|p| p.set(provider, f))
}

struct Declared;
impl Family for Declared {
    type Of<'a> = dyn Fn(&str) -> bool + 'a;
}

thread_local! {
    static DECLARED: Scoped<Declared> = Scoped::new();
}

/// Runs `f` with `declared` telling whether the analysed code declares a class: PHPStan finds such a class in the
/// analysed files before the stubs, so it shadows a built-in class of the same name that PHP lacks at runtime
/// (com_dotnet's `VARIANT` on Linux).
pub fn with_declared_classes<R>(declared: &dyn Fn(&str) -> bool, f: impl FnOnce() -> R) -> R {
    DECLARED.with(|d| d.set(declared, f))
}

pub fn lookup(name: &str) -> Option<ClassRef> {
    let name = name.trim_start_matches('\\');
    if let Some(c) = class(name)
        && !DECLARED
            .with(|d| d.get(|declared| declared(name)))
            .unwrap_or(false)
    {
        return Some(ClassRef::Builtin(c));
    }
    PROVIDER
        .with(|p| p.get(|provider| provider(name)))
        .flatten()
        .map(ClassRef::User)
}

struct FunctionProvider;
impl Family for FunctionProvider {
    type Of<'a> = dyn Fn(&str) -> bool + 'a;
}

thread_local! {
    static FUNCTIONS: Scoped<FunctionProvider> = Scoped::new();
}

pub fn with_functions<R>(provider: &dyn Fn(&str) -> bool, f: impl FnOnce() -> R) -> R {
    FUNCTIONS.with(|p| p.set(provider, f))
}

pub fn has_function(name: &str) -> bool {
    if function(name).is_some() {
        return true;
    }
    FUNCTIONS
        .with(|p| p.get(|provider| provider(name.trim_start_matches('\\'))))
        .unwrap_or(false)
}

pub fn function(name: &str) -> Option<&'static Function> {
    symbols(PhpVersion::new(8, 4))
        .functions
        .get(&name.trim_start_matches('\\').to_ascii_lowercase())
}
