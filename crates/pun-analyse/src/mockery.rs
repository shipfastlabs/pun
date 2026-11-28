//! phpstan/phpstan-mockery 2.0.0's PHPStan extension (`extension.neon`), on with `[extensions] mockery = "on"`
//! (`pun_config::extensions::MOCKERY`). It has no rules, only type knowledge:
//!
//! - `stubs/MockInterface.stub`, bundled in `data/phpstan-mockery` (MIT, with its license file);
//! - `StubMethodsClassReflectionExtension` (every method of `Allows` / `Expects` returns `Mockery\Expectation`): a
//!   bundled file declares both interfaces, and each gets a method for every name the analysed files call;
//! - `MockDynamicReturnTypeExtension` and `MockDynamicNamedMockReturnTypeExtension` (`Mockery::mock()`, `spy()`,
//!   `namedMock()`) and `StubDynamicReturnTypeExtension` (`MockInterface::allows()` / `expects()`);
//! - `TypeNodeResolverExtension` when `mockery.convertUnionToIntersectionType` is true: a PHPDoc union of two types,
//!   one of them `Mockery\MockInterface`, is their intersection.

use std::path::{Path, PathBuf};

use mago_analyzer::plugin::PluginRegistry;
use mago_analyzer::plugin::context::{InvocationInfo, ProviderContext};
use mago_analyzer::plugin::provider::method::{MethodReturnTypeProvider, MethodTarget};
use mago_analyzer::plugin::provider::{Provider, ProviderMeta};
use mago_analyzer::settings::{PhpDocUnionResolver, Settings};
use mago_codex::metadata::CodebaseMetadata;
use mago_codex::metadata::ttype::TypeMetadata;
use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::atomic::object::TObject;
use mago_codex::ttype::atomic::object::named::TNamedObject;
use mago_codex::ttype::atomic::reference::TReference;
use mago_codex::ttype::expander::StaticClassType;
use mago_codex::ttype::union::TUnion;
use mago_codex::ttype::{get_never, wrap_atomic};
use mago_names::ResolvedNames;
use mago_span::Span;
use mago_syntax::cst::Program;
use mago_word::{Word, ascii_lowercase_word, word};
use pun_phpstan::types::Type;
use pun_phpstan::{Array, Value};

use crate::Project;

const MOCK_INTERFACE: &str = "Mockery\\MockInterface";
/// upstream's stub file, from the project root.
const STUB: &str = "vendor/phpstan/phpstan-mockery/stubs/MockInterface.stub";
/// `Allows` and `Expects`, which PHPStan autoloads from phpstan-mockery's `src/`: scanned, not a stub, as a stub only
/// changes a class declared elsewhere. The path is Pun's own, so it is never on disk.
const INTERFACES: &str = "vendor/phpstan/phpstan-mockery/pun/StubInterfaces.php";
const BUNDLED: [(&str, &[u8]); 2] = [
    (
        STUB,
        include_bytes!("../data/phpstan-mockery/MockInterface.stub"),
    ),
    (
        INTERFACES,
        include_bytes!("../data/phpstan-mockery/StubInterfaces.php"),
    ),
];

/// On with the switch. `pun_config` always sets `mockery.convertUnionToIntersectionType: true`, as extension.neon
/// does (`pun migrate` reports a project's `false` as unsupported), so `TypeNodeResolverExtension` always runs.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Mockery;

impl Mockery {
    /// On when the parameters hold extension.neon's `mockery` structure.
    pub fn from_params(params: &Array) -> Option<Mockery> {
        params.get_str("mockery").and_then(Value::as_array)?;
        Some(Mockery)
    }

    /// The file that declares `Allows` and `Expects`, to scan.
    pub(crate) fn scan_files(root: &Path) -> PathBuf {
        root.join(INTERFACES)
    }
}

/// Whether a PHPDoc type atom, read before population (so a class is a reference), is plain `Mockery\MockInterface`.
fn is_mock_interface(atomic: &TAtomic) -> bool {
    matches!(atomic, TAtomic::Reference(TReference::Symbol { name, intersection_types: None, .. })
        if name.as_bytes().eq_ignore_ascii_case(MOCK_INTERFACE.as_bytes()))
}

/// Whether `child` is `parent` or extends or implements it, read from the direct parents so that it also holds
/// before the codebase is populated.
fn extends(codebase: &CodebaseMetadata, child: Word, parent: Word) -> bool {
    let mut all = Vec::new();
    crate::carbon::ancestors(codebase, ascii_lowercase_word(child.as_bytes()), &mut all);
    all.contains(&ascii_lowercase_word(parent.as_bytes()))
}

/// `TypeCombinator::intersect(MockInterface, $other)` for the members `TypeNodeResolverExtension` sees: a class
/// keeps the narrower of the two (or both), `object` gives `MockInterface`, and a type that is no object
/// (null, a scalar, an array) gives never. `None` leaves the union as it is.
fn intersect_mock(codebase: &CodebaseMetadata, mock: &TAtomic, other: &TAtomic) -> Option<TUnion> {
    let interface = word(MOCK_INTERFACE.as_bytes());
    match other {
        TAtomic::Reference(TReference::Symbol { name, .. }) => {
            if extends(codebase, *name, interface) {
                return Some(wrap_atomic(other.clone()));
            }
            if extends(codebase, interface, *name) {
                return Some(wrap_atomic(mock.clone()));
            }
            let class = codebase
                .class_likes
                .get(&ascii_lowercase_word(name.as_bytes()));
            if class.is_some_and(|k| k.flags.is_final() && !k.kind.is_interface()) {
                return Some(get_never());
            }
            Some(wrap_atomic(TAtomic::Reference(TReference::Symbol {
                name: interface,
                parameters: None,
                variances: None,
                intersection_types: Some(Box::new(vec![other.clone()])),
            })))
        }
        TAtomic::Object(TObject::Any) => Some(wrap_atomic(mock.clone())),
        TAtomic::Null | TAtomic::Scalar(_) | TAtomic::Array(_) => Some(get_never()),
        _ => None,
    }
}

/// The method names `->name(` calls in `contents`, lowercased.
fn called_methods(contents: &[u8], out: &mut std::collections::BTreeSet<Vec<u8>>) {
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
    let mut at = 0;
    while let Some(arrow) = memchr::memmem::find(&contents[at..], b"->") {
        let start = at + arrow + 2;
        let end = start + contents[start..].iter().take_while(|&&b| ident(b)).count();
        let after = &contents[end..];
        let open = after.iter().position(|b| !b.is_ascii_whitespace());
        if end > start
            && !contents[start].is_ascii_digit()
            && open.is_some_and(|i| after[i] == b'(')
        {
            out.insert(contents[start..end].to_ascii_lowercase());
        }
        at = start;
    }
}

/// `StubMethodsClassReflectionExtension::hasMethod()` is true for every name on `Allows` and `Expects`. Pun needs the
/// names up front: each interface gets a copy of its `__pun_stub()` for every method name the analysed files call,
/// and the template itself goes.
fn stub_methods(project: &Project, codebase: &mut CodebaseMetadata) {
    use rayon::prelude::*;
    let names = (project.analysed.par_iter())
        .filter_map(|f| crate::io::read(f).ok())
        .map(|contents| {
            let mut names = std::collections::BTreeSet::new();
            called_methods(&contents, &mut names);
            names
        })
        .reduce(std::collections::BTreeSet::new, |mut a, b| {
            a.extend(b);
            a
        });
    let template = ascii_lowercase_word(b"__pun_stub");
    for stub in [
        b"phpstan\\mockery\\type\\allows".as_slice(),
        b"phpstan\\mockery\\type\\expects",
    ] {
        let class = word(stub);
        // Both are bundled and scanned whenever the extension is on
        let method = (codebase.function_likes.remove(&(class, template))).expect("bundled");
        let k = codebase.class_likes.get_mut(&class).expect("bundled");
        k.methods.remove(&template);
        for name in &names {
            let lower = word(name);
            let mut copy = (*method).clone();
            copy.name = lower;
            copy.original_name = lower;
            k.methods.insert(lower);
            codebase
                .function_likes
                .insert((class, lower), Box::new(copy));
        }
    }
}

/// `TypeNodeResolverExtension::resolve()` on a PHPDoc union that resolved to `union`: with exactly two members, one
/// of them `Mockery\MockInterface`, the union is their intersection.
pub(crate) fn union_to_intersection(codebase: &CodebaseMetadata, union: &TUnion) -> Option<TUnion> {
    let [a, b] = union.types.as_ref() else {
        return None;
    };
    if is_mock_interface(a) {
        intersect_mock(codebase, a, b)
    } else if is_mock_interface(b) {
        intersect_mock(codebase, b, a)
    } else {
        None
    }
}

fn convert(codebase: &CodebaseMetadata, metadata: &mut Option<TypeMetadata>) -> bool {
    if let Some(t) = metadata.as_mut().filter(|t| t.from_docblock)
        && let Some(intersection) = union_to_intersection(codebase, &t.type_union)
    {
        t.type_union = intersection;
        return true;
    }
    false
}

static META: ProviderMeta =
    ProviderMeta::new("pun-mockery", "pun", "phpstan-mockery's mock return types");

/// `MockDynamicReturnTypeExtension` (`mock`, `spy`) and `MockDynamicNamedMockReturnTypeExtension` (`namedMock`).
struct Mocks;

impl Provider for Mocks {
    fn meta() -> &'static ProviderMeta {
        &META
    }
}

impl MethodReturnTypeProvider for Mocks {
    fn targets() -> &'static [MethodTarget] {
        static TARGETS: [MethodTarget; 3] = [
            MethodTarget::exact(b"Mockery", b"mock"),
            MethodTarget::exact(b"Mockery", b"spy"),
            MethodTarget::exact(b"Mockery", b"namedMock"),
        ];
        &TARGETS
    }

    fn get_return_type(
        &self,
        context: &ProviderContext<'_, '_, '_>,
        _: &[u8],
        method: &[u8],
        invocation: &InvocationInfo<'_, '_, '_>,
    ) -> Option<TUnion> {
        let mut arguments = invocation.arguments();
        if method.eq_ignore_ascii_case(b"namedmock") && arguments.len() > 1 {
            arguments.remove(0);
        }
        let codebase = context.codebase();
        let mut names: Vec<Word> = Vec::new();
        for argument in arguments {
            let Some(value) = argument
                .value()
                .and_then(|v| context.get_expression_type(v))
                .and_then(|t| {
                    (t.get_single_literal_string_value().map(<[u8]>::to_vec)).or_else(|| {
                        t.get_single_class_string_value()
                            .map(|w| w.as_bytes().to_vec())
                    })
                })
            else {
                continue;
            };
            for name in mocked_names(&value) {
                names.push(word(&name));
            }
        }
        // TypeCombinator::intersect() keeps the narrowest classes
        let interface = word(MOCK_INTERFACE.as_bytes());
        let mut kept: Vec<Word> = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let wider = names.iter().enumerate().any(|(j, other)| {
                (j != i
                    && !other.as_bytes().eq_ignore_ascii_case(name.as_bytes())
                    && codebase.is_instance_of(other.as_bytes(), name.as_bytes()))
                    || (j < i && other.as_bytes().eq_ignore_ascii_case(name.as_bytes()))
            });
            if !wider && !codebase.is_instance_of(interface.as_bytes(), name.as_bytes()) {
                kept.push(*name);
            }
        }
        let mut mock = TNamedObject::new(interface);
        if !kept.is_empty() {
            let parts = kept
                .into_iter()
                .map(|n| TAtomic::Object(TObject::Named(TNamedObject::new(n))));
            mock.intersection_types = Some(Box::new(parts.collect()));
        }
        Some(wrap_atomic(TAtomic::Object(TObject::Named(mock))))
    }
}

/// The class and interface names one constant string argument of `Mockery::mock()` names, as
/// `MockDynamicReturnTypeExtension::getTypeFromStaticMethodCall()` reads them.
fn mocked_names(value: &[u8]) -> Vec<Vec<u8>> {
    let mut value = value;
    if let Some(rest) = value.strip_prefix(b"alias:") {
        value = rest;
    }
    if let Some(rest) = value.strip_prefix(b"overload:") {
        value = rest;
    }
    if value.ends_with(b"]")
        && let Some(at) = value.iter().position(|&b| b == b'[')
    {
        value = &value[..at];
    }
    let names: Vec<Vec<u8>> = if value.contains(&b',') {
        let without_spaces: Vec<u8> = value.iter().copied().filter(|&b| b != b' ').collect();
        without_spaces
            .split(|&b| b == b',')
            .map(<[u8]>::to_vec)
            .collect()
    } else {
        vec![value.to_vec()]
    };
    // An ObjectType's name has no leading backslash in Mago
    (names.into_iter())
        .map(|n| n.strip_prefix(b"\\").map_or(n.clone(), <[u8]>::to_vec))
        .collect()
}

static STUB_META: ProviderMeta = ProviderMeta::new(
    "pun-mockery-stubs",
    "pun",
    "phpstan-mockery's allows() and expects() return types",
);

/// `StubDynamicReturnTypeExtension`: `allows()` / `expects()` on a mock of exactly one class or interface return
/// `Allows` / `Expects`. Otherwise Mockery's own conditional return type stays.
struct Stubs;

impl Provider for Stubs {
    fn meta() -> &'static ProviderMeta {
        &STUB_META
    }
}

impl MethodReturnTypeProvider for Stubs {
    fn targets() -> &'static [MethodTarget] {
        static TARGETS: [MethodTarget; 2] = [
            MethodTarget::exact(b"Mockery\\MockInterface", b"allows"),
            MethodTarget::exact(b"Mockery\\MockInterface", b"expects"),
        ];
        &TARGETS
    }

    fn get_return_type(
        &self,
        _: &ProviderContext<'_, '_, '_>,
        _: &[u8],
        method: &[u8],
        invocation: &InvocationInfo<'_, '_, '_>,
    ) -> Option<TUnion> {
        let target = invocation.inner().target.get_method_context()?;
        // A DynamicMethodReturnTypeExtension: a static call (`MockInterface::allows()`), whose class type is a name, keeps
        // Mockery's type
        let mut names = match &target.class_type {
            StaticClassType::Object(object) => {
                TAtomic::Object(object.clone()).get_all_object_names()
            }
            _ => return None,
        };
        names.retain(|n| n.as_bytes() != MOCK_INTERFACE.as_bytes());
        names.sort_unstable();
        names.dedup();
        if names.len() != 1 {
            return None;
        }
        let stub: &[u8] = if method.eq_ignore_ascii_case(b"allows") {
            b"PHPStan\\Mockery\\Type\\Allows"
        } else {
            b"PHPStan\\Mockery\\Type\\Expects"
        };
        Some(wrap_atomic(TAtomic::Object(TObject::Named(
            TNamedObject::new(word(stub)),
        ))))
    }
}

impl crate::AnalyseExtension for Mockery {
    fn stub_files(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join(STUB)]
    }

    fn bundled_stub(&self, path: &Path) -> Option<&'static [u8]> {
        BUNDLED
            .iter()
            .find(|(p, _)| path.ends_with(p))
            .map(|(_, contents)| *contents)
    }

    fn class_aliases(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// `TypeNodeResolverExtension` on every PHPDoc property, parameter and return type the scanner read.
    fn prepare_codebase(&self, project: &Project, codebase: &mut CodebaseMetadata) {
        stub_methods(project, codebase);
        let mut properties = Vec::new();
        for (class, k) in &codebase.class_likes {
            for (name, p) in k.properties.iter().chain(&k.magic_properties) {
                let mut types = [p.type_metadata.clone(), p.write_type_metadata.clone()];
                let changed = types
                    .iter_mut()
                    .fold(false, |c, t| convert(codebase, t) | c);
                if changed {
                    properties.push((*class, *name, types));
                }
            }
        }
        for (class, name, [read, write]) in properties {
            let k = codebase
                .class_likes
                .get_mut(&class)
                .expect("collected above");
            for p in [
                k.properties.get_mut(&name),
                k.magic_properties.get_mut(&name),
            ]
            .into_iter()
            .flatten()
            {
                p.type_metadata.clone_from(&read);
                p.write_type_metadata.clone_from(&write);
            }
        }
        let mut functions = Vec::new();
        for (key, f) in &codebase.function_likes {
            let mut f2 = None::<mago_codex::metadata::function_like::FunctionLikeMetadata>;
            let mut ret = f.return_type_metadata.clone();
            if convert(codebase, &mut ret) {
                f2.get_or_insert_with(|| (**f).clone()).return_type_metadata = ret;
            }
            for (i, p) in f.parameters.iter().enumerate() {
                let mut t = p.type_metadata.clone();
                if convert(codebase, &mut t) {
                    f2.get_or_insert_with(|| (**f).clone()).parameters[i].type_metadata = t;
                }
            }
            if let Some(f2) = f2 {
                functions.push((*key, f2));
            }
        }
        for (key, f) in functions {
            codebase.function_likes.insert(key, Box::new(f));
        }
    }

    fn load(&mut self, _: &Project, _: &CodebaseMetadata) {}

    fn register(&self, registry: &mut PluginRegistry, _: &[u8], _: &ResolvedNames<'_>) {
        registry.register_method_provider(Mocks);
        registry.register_method_provider(Stubs);
    }

    fn settings(&self, settings: &mut Settings) {
        settings.phpdoc_union_resolver = Some(PhpDocUnionResolver(union_to_intersection));
    }

    fn typed_expressions<'arena>(
        &self,
        _: &ResolvedNames<'arena>,
        _: &Program<'arena>,
    ) -> Vec<Span> {
        Vec::new()
    }

    /// `TypeNodeResolverExtension` for Pun's own rules' PHPDoc types.
    fn type_node(&self, _: &CodebaseMetadata, name: &str, args: &[Type]) -> Option<Type> {
        let is_mock = |t: &Type| t.object_class_names() == [MOCK_INTERFACE];
        match (name, args) {
            ("|", [a, b]) if is_mock(a) || is_mock(b) => {
                Some(pun_phpstan::types::intersect(vec![a.clone(), b.clone()]))
            }
            _ => None,
        }
    }
}
