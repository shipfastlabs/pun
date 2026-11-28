use std::borrow::Cow;
use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use crate::problem::Span;
use mago_allocator::LocalArena;
use mago_analyzer::Analyzer;
use mago_analyzer::analysis_result::AnalysisResult;
use mago_analyzer::artifacts::AnalysisArtifacts;
use mago_analyzer::plugin::{PluginRegistry, create_registry_with_plugins};
use mago_analyzer::settings::Settings;
use mago_codex::metadata::CodebaseMetadata;
use mago_codex::metadata::flags::MetadataFlags;
use mago_codex::populator::populate_codebase_parallel;
use mago_codex::reference::SymbolReferences;
use mago_codex::scanner::scan_program;
use mago_codex::signature_builder::build_file_signature;
use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::atomic::object::TObject;
use mago_codex::ttype::atomic::scalar::TScalar;
use mago_codex::ttype::union::TUnion;
use mago_codex::ttype::{
    get_float, get_int, get_int_or_float, get_int_or_string, get_mixed_keyed_array, get_object,
    get_string,
};
use mago_database::file::{File, FileId, FileType};
use mago_names::ResolvedNames;
use mago_names::resolver::NameResolver;
use mago_php_version::PHPVersion;
use mago_prelude::Prelude;
use mago_span::HasSpan;
use mago_syntax::cst::{
    Access, BinaryOperator, Call, ClassLikeConstantSelector, ClassLikeMemberSelector, Expression,
    FunctionCall, Node, Program, Sequence, Trivia, TriviaKind, UnaryPrefixOperator, Variable,
};
use mago_syntax::parser::parse_file;
use rayon::prelude::*;

use crate::Context;

static PRELUDE: LazyLock<(CodebaseMetadata, SymbolReferences)> = LazyLock::new(|| {
    let prelude = Prelude::build();
    (prelude.metadata, prelude.symbol_references)
});

static PLUGINS: LazyLock<PluginRegistry> = LazyLock::new(|| {
    let mut registry = create_registry_with_plugins(&[], false);
    super::union_method::register(&mut registry);
    registry
});

pub struct ProjectCodebase {
    native: Arc<CodebaseMetadata>,
    documented: Arc<CodebaseMetadata>,
}

static PROJECT: RwLock<Option<Arc<ProjectCodebase>>> = RwLock::new(None);

pub fn set_project_codebase(codebase: Option<Arc<ProjectCodebase>>) {
    *PROJECT
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = codebase;
}

type PendingScan = Box<dyn FnOnce() -> Arc<ProjectCodebase> + Send>;

static PENDING: Mutex<Option<PendingScan>> = Mutex::new(None);

pub fn set_project_scan(build: impl FnOnce() -> Arc<ProjectCodebase> + Send + 'static) {
    *PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(build));
}

/// What linting a file read besides the file itself, for a result cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reads {
    /// The project's codebase (its other files and Composer's packages), composer.json or a Pest.php.
    pub project: bool,
    /// A note went to stderr (a contained Mago panic), which a cached result would not print again.
    pub noted: bool,
}

thread_local! {
    static READS: std::cell::Cell<Reads> = const { std::cell::Cell::new(Reads { project: false, noted: false }) };
}

/// Whether any note went to stderr in this process: one printed by the project scan, on another thread, concerns
/// every file that reads the project.
static NOTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Runs `f` (the linting of one file) and returns what it read. Nested calls, as when rayon runs another file's
/// work on this thread while `f` waits, keep their reads apart.
pub fn tracking<T>(f: impl FnOnce() -> T) -> (T, Reads) {
    let outer = READS.replace(Reads::default());
    let value = f();
    let mut reads = READS.replace(outer);
    reads.noted |= reads.project && NOTED.load(std::sync::atomic::Ordering::Relaxed);
    (value, reads)
}

pub(crate) fn note_project_read() {
    READS.set(Reads {
        project: true,
        ..READS.get()
    });
}

/// Mago's analyser settings for the lint facts. Rector runs PHPStan without Larastan, so a method found on a named
/// `@mixin` binds `$this`/`static` to the mixin object, as MixinMethodsClassReflectionExtension does
/// (`Model::query()->whereExists(...)` is a query builder there, not an Eloquent one).
fn settings(version: PHPVersion) -> Settings {
    Settings {
        mixin_receiver_binding: false,
        ..Settings::new(version)
    }
}

fn note_stderr() {
    READS.set(Reads {
        noted: true,
        ..READS.get()
    });
    NOTED.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn project_codebase(
    files: &[(String, String)],
    php_version: crate::PhpVersion,
) -> Arc<ProjectCodebase> {
    project_codebase_autoloaded(files, php_version, &|_| None)
}

pub fn project_codebase_autoloaded(
    files: &[(String, String)],
    php_version: crate::PhpVersion,
    autoload: &(dyn Fn(&[u8]) -> Option<(String, String)> + Sync),
) -> Arc<ProjectCodebase> {
    let version = php_version.to_mago();
    let stubbed = |native: bool| {
        let mut codebase = PRELUDE.0.clone();
        stub_preg_match(&mut codebase, version, native);
        codebase
    };
    let kind = |path: &str| match is_vendor_path(path) {
        true => FileType::Vendored,
        false => FileType::Host,
    };
    let ((mut codebase, mut documented), scans) = rayon::join(
        || rayon::join(|| stubbed(true), || stubbed(false)),
        || {
            (files.par_iter())
                .map(|(path, code)| Scanned::new(path, code, kind(path), version))
                .collect::<Vec<_>>()
        },
    );
    let mut prefetched = prefetch(&scans, version, autoload);
    let mut named = Vec::new();
    for ((path, code), scanned) in files.iter().zip(scans) {
        named.extend(scanned.add(path, code, &mut codebase, &mut documented));
    }
    let mut asked: HashSet<Vec<u8>> = HashSet::new();
    while let Some(name) = named.pop() {
        if !asked.insert(name.clone()) || codebase.get_class_like(&name).is_some() {
            continue;
        }
        let loaded = (prefetched.found.remove(&name)).unwrap_or_else(|| autoload(&name));
        if let Some((path, code)) = loaded {
            let scanned = (prefetched.scans.remove(&path))
                .unwrap_or_else(|| Scanned::new(&path, &code, FileType::Vendored, version));
            named.extend(scanned.add(&path, &code, &mut codebase, &mut documented));
        }
    }
    // What the walk did not take: the populated facts need the memory.
    drop(prefetched);
    let populated = |mut codebase: CodebaseMetadata| {
        let mut references = PRELUDE.1.clone();
        populate_codebase_parallel(
            &mut codebase,
            &mut references,
            Default::default(),
            Default::default(),
        );
        Arc::new(codebase)
    };
    let (native, documented) = rayon::join(|| populated(codebase), || populated(documented));
    Arc::new(ProjectCodebase { native, documented })
}

struct Scanned {
    id: FileId,
    native: CodebaseMetadata,
    documented: CodebaseMetadata,
    names: Vec<Vec<u8>>,
}

impl Scanned {
    fn new(path: &str, code: &str, kind: FileType, version: PHPVersion) -> Self {
        let file = mago_file(path, code, kind);
        let arena = LocalArena::new();
        let program = parse_file(&arena, &file);
        let names = NameResolver::new(&arena).resolve(program);
        let documented = scan_symbols(&arena, &file, program, &names, version);
        let trivia: Vec<Trivia<'_>> = (program.trivia.iter())
            .map(|t| match t.kind {
                TriviaKind::DocBlockComment => Trivia {
                    kind: TriviaKind::MultiLineComment,
                    ..*t
                },
                _ => *t,
            })
            .collect();
        let mut native_program = program.clone();
        native_program.trivia = Sequence::from_slice(&trivia);
        let mut native = scan_symbols(&arena, &file, &native_program, &names, version);
        // PHPStan's native type of a backed enum's `from()` and `tryFrom()` is `mixed`; Mago declares
        // them as returning the enum.
        let factories: Vec<_> = (native.function_likes.keys())
            .filter(|(class, method)| {
                (method.as_bytes().eq_ignore_ascii_case(b"from")
                    || method.as_bytes().eq_ignore_ascii_case(b"tryfrom"))
                    && native
                        .get_class_like(class.as_bytes())
                        .is_some_and(|c| c.kind.is_enum())
            })
            .copied()
            .collect();
        for id in factories {
            let method = native.function_likes.get_mut(&id).unwrap();
            method.return_type_declaration_metadata = None;
            method.return_type_metadata = None;
        }
        Self {
            id: file.id,
            native,
            documented,
            names: (names.iter())
                .map(|(_, _, name, _)| name.to_ascii_lowercase())
                .collect(),
        }
    }

    fn add(
        self,
        path: &str,
        code: &str,
        native: &mut CodebaseMetadata,
        documented: &mut CodebaseMetadata,
    ) -> Vec<Vec<u8>> {
        remember(self.id, path, code);
        remember_phar(self.id, path);
        native.extend(self.native);
        documented.extend(self.documented);
        self.names
    }
}

#[derive(Default)]
struct Prefetched {
    found: HashMap<Vec<u8>, Option<(String, String)>>,
    scans: HashMap<String, Scanned>,
}

#[derive(Default)]
struct Prefetching {
    declared: HashSet<Vec<u8>>,
    seen: HashSet<Vec<u8>>,
    paths: HashSet<String>,
    found: Prefetched,
}

impl Prefetching {
    fn unknown<'a>(&mut self, scans: impl Iterator<Item = &'a Scanned> + Clone) -> Vec<Vec<u8>> {
        for s in scans.clone() {
            (self.declared).extend(s.native.class_likes.keys().map(|k| k.as_bytes().to_vec()));
        }
        (scans.flat_map(|s| &s.names))
            .filter(|n| !self.declared.contains(*n) && PRELUDE.0.get_class_like(n).is_none())
            .filter(|n| self.seen.insert((*n).clone()))
            .cloned()
            .collect()
    }
}

fn prefetch(
    scans: &[Scanned],
    version: PHPVersion,
    autoload: &(dyn Fn(&[u8]) -> Option<(String, String)> + Sync),
) -> Prefetched {
    let state = Mutex::new(Prefetching::default());
    let names = (state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
    .unknown(scans.iter());
    let look_up = |name: Vec<u8>| Lookup {
        name,
        state: &state,
        version,
        autoload,
    };
    rayon::scope(|scope| {
        for name in names {
            let lookup = look_up(name);
            scope.spawn(move |scope| lookup.run(scope));
        }
    });
    (state
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
    .found
}

struct Lookup<'a> {
    name: Vec<u8>,
    state: &'a Mutex<Prefetching>,
    version: PHPVersion,
    autoload: &'a (dyn Fn(&[u8]) -> Option<(String, String)> + Sync),
}

impl<'a> Lookup<'a> {
    fn run(self, scope: &rayon::Scope<'a>) {
        let file = (self.autoload)(&self.name);
        let lock = || {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        };
        let new = match &file {
            Some((path, code)) if lock().paths.insert(path.clone()) => {
                Some((path.clone(), code.clone()))
            }
            _ => None,
        };
        lock().found.found.insert(self.name, file);
        let Some((path, code)) = new else {
            return;
        };
        let scanned = Scanned::new(&path, &code, FileType::Vendored, self.version);
        let mut state = lock();
        let names = state.unknown(std::iter::once(&scanned));
        state.found.scans.insert(path, scanned);
        drop(state);
        for name in names {
            let lookup = Lookup { name, ..self };
            scope.spawn(move |scope| lookup.run(scope));
        }
    }
}

fn stub_preg_match(codebase: &mut CodebaseMetadata, version: PHPVersion, native: bool) {
    const STUB: &str = include_str!("../../../data/phpstan-preg-match.stub");
    const NATIVE: &str = include_str!("../../../data/phpstan-preg-match-native.stub");
    let file = mago_file(
        "phpstan/stubs/core.stub",
        if native { NATIVE } else { STUB },
        FileType::Host,
    );
    let arena = LocalArena::new();
    let program = parse_file(&arena, &file);
    let names = NameResolver::new(&arena).resolve(program);
    let scanned = scan_program(&arena, &file, program, &names, version);
    for (key, stub) in scanned.function_likes {
        let builtin = codebase
            .function_likes
            .get_mut(&key)
            .expect("a built-in function");
        builtin.template_types.clone_from(&stub.template_types);
        for parameter in &mut builtin.parameters {
            let name = parameter.name.0.as_bytes();
            if name != b"$flags" && name != b"$matches" {
                continue;
            }
            if let Some(from) = stub.parameters.iter().find(|p| p.name.0.as_bytes() == name) {
                parameter.type_metadata = from.type_metadata.clone();
                parameter.out_type = from.out_type.clone();
            }
        }
    }
}

static VENDOR: LazyLock<RwLock<HashSet<FileId>>> = LazyLock::new(Default::default);

pub(crate) fn is_vendor_file(file: FileId) -> bool {
    (VENDOR
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
    .contains(&file)
}

static PHPSTAN_PHAR: LazyLock<RwLock<HashSet<FileId>>> = LazyLock::new(Default::default);

fn remember_phar(file: FileId, path: &str) {
    if path.contains("phpstan.phar") {
        (PHPSTAN_PHAR
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
        .insert(file);
    }
}

pub(crate) fn is_phpstan_phar_file(file: FileId) -> bool {
    (PHPSTAN_PHAR
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
    .contains(&file)
}

static SOURCES_BY_ID: LazyLock<RwLock<HashMap<FileId, Arc<str>>>> = LazyLock::new(Default::default);

fn remember_source(file: FileId, code: &str) {
    (SOURCES_BY_ID
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
    .insert(file, Arc::from(code));
}

thread_local! {
    static SOURCES: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

pub fn with_sources<R>(sources: Vec<(String, String)>, f: impl FnOnce() -> R) -> R {
    SOURCES.with(|s| *s.borrow_mut() = sources);
    let result = f();
    SOURCES.with(|s| s.borrow_mut().clear());
    result
}

struct Analysis {
    artifacts: Option<AnalysisArtifacts>,
    untyped: HashSet<(u32, u32)>,
    declared: HashMap<(u32, u32), TUnion>,
}

pub(crate) struct Facts {
    pub codebase: Arc<CodebaseMetadata>,
    documented: Arc<CodebaseMetadata>,
    path: String,
    native_source: String,
    source: String,
    version: PHPVersion,
    artifacts: OnceCell<Analysis>,
    documented_artifacts: OnceCell<Option<AnalysisArtifacts>>,
    dead_code: bool,
    file: OnceCell<File>,
}

impl Facts {
    pub(crate) fn implicit_array_writes(&self) -> Vec<((u32, u32), String, bool)> {
        let file = self.file();
        let arena = LocalArena::new();
        let program = parse_file(&arena, file);
        let names = NameResolver::new(&arena).resolve(program);
        let found = Arc::new(Mutex::new(HashMap::new()));
        let mut registry = create_registry_with_plugins(&[], false);
        super::union_method::register(&mut registry);
        registry
            .register_expression_hook(super::implicit_array_observer::Observer(Arc::clone(&found)));
        let analyzer = Analyzer::new(
            &arena,
            file,
            &names,
            &self.codebase,
            &registry,
            settings(self.version),
        );
        let mut result = AnalysisResult::new(SymbolReferences::new());
        // An analysis error only ends the walk early, so what the observer saw until then stands; after a panic the
        // observations may be half-recorded, so none are kept.
        let analysed = contained("analyser", file, || {
            analyzer.analyze_with_artifacts(program, &mut result)
        })
        .is_some();
        let found = found
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut out: Vec<_> = (found.iter())
            .filter(|_| analysed)
            .filter(|(_, (_, state))| *state != super::implicit_array_observer::YES)
            .map(|(span, (name, state))| {
                (
                    *span,
                    name.clone(),
                    *state == super::implicit_array_observer::MAYBE,
                )
            })
            .collect();
        out.sort();
        out
    }

    pub(crate) fn native_type(&self, span: Span) -> Option<&TUnion> {
        let analysis = self.artifacts.get_or_init(|| self.analyse());
        let (start, end) = (span.start as u32, span.end as u32);
        if analysis.untyped.contains(&(start, end)) {
            return None;
        }
        if let Some(t) = analysis.declared.get(&(start, end)) {
            return Some(t);
        }
        analysis
            .artifacts
            .as_ref()?
            .get_keep_void_expression_type(&mago_span::Span::dummy(start, end))
    }

    pub(crate) fn is_untyped(&self, span: Span) -> bool {
        let analysis = self.artifacts.get_or_init(|| self.analyse());
        analysis
            .untyped
            .contains(&(span.start as u32, span.end as u32))
    }

    fn builtin_return<'s>(
        &'s self,
        names: &ResolvedNames<'_>,
        call: &FunctionCall<'_>,
    ) -> Option<&'s TUnion> {
        self.codebase
            .get_function(&self.builtin_name(names, call)?)?
            .return_type_declaration_metadata
            .as_ref()
            .map(|t| &t.type_union)
    }

    fn builtin_name(&self, names: &ResolvedNames<'_>, call: &FunctionCall<'_>) -> Option<Vec<u8>> {
        let Expression::Identifier(id) = call.function else {
            return None;
        };
        let resolved = names.get(id);
        let name = if self.codebase.get_function(resolved).is_some() {
            resolved
        } else {
            let written = id.value();
            if written.contains(&b'\\') && !written.starts_with(b"\\") {
                return None;
            }
            written.rsplit(|b| *b == b'\\').next()?
        };
        PRELUDE.0.get_function(name)?;
        Some(name.to_vec())
    }

    fn builtin_operand(
        &self,
        names: &ResolvedNames<'_>,
        operand: &Expression<'_>,
        plus: bool,
    ) -> Option<bool> {
        let Expression::Call(Call::Function(call)) = crate::syntax::unparenthesize(operand) else {
            return None;
        };
        let t = self.builtin_return(names, call)?;
        let error = (t.types.iter()).any(|a| match a {
            TAtomic::Scalar(s) => s.is_any_string(),
            TAtomic::Array(_) => !plus,
            other => matches!(other, TAtomic::Object(_) | TAtomic::Reference(_)),
        });
        let float = t
            .types
            .iter()
            .any(|a| matches!(a, TAtomic::Scalar(s) if s.is_float()));
        let wide = t.is_mixed()
            || (float
                && t.types
                    .iter()
                    .any(|a| !matches!(a, TAtomic::Scalar(s) if s.is_float())));
        (error || wide).then_some(error)
    }

    pub(crate) fn text(&self, span: mago_span::Span) -> Option<&str> {
        (span.file_id == self.file().id)
            .then(|| {
                self.native_source
                    .get(span.start.offset as usize..span.end.offset as usize)
            })
            .flatten()
    }

    pub(crate) fn documented(&self) -> &CodebaseMetadata {
        &self.documented
    }

    pub(crate) fn source_of(&self, file: FileId) -> Option<Arc<str>> {
        (SOURCES_BY_ID
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
        .get(&file)
        .cloned()
    }

    pub(crate) fn anonymous_class_name(&self, span: mago_span::Span) -> String {
        mago_codex::get_anonymous_class_name(self.file(), span).to_string()
    }

    /// The file as Mago sees it: rules ask for its names and lines at many nodes, and building it indexes its lines.
    fn file(&self) -> &File {
        (self.file).get_or_init(|| mago_file(&self.path, &self.native_source, FileType::Host))
    }

    pub(crate) fn full_type(&self, span: Span) -> Option<&TUnion> {
        let full = (self.documented_artifacts()?.expression_types)
            .get(&(span.start as u32, span.end as u32))?;
        match self.native_type(span) {
            Some(native) if full.is_mixed() && full.by_reference() && !native.is_mixed() => {
                Some(native)
            }
            _ => Some(full.as_ref()),
        }
    }

    fn documented_artifacts(&self) -> Option<&AnalysisArtifacts> {
        (self.documented_artifacts)
            .get_or_init(|| self.analyse_documented())
            .as_ref()
    }

    fn analyse_documented(&self) -> Option<AnalysisArtifacts> {
        let file = mago_file(&self.path, &self.source, FileType::Host);
        let arena = LocalArena::new();
        let program = parse_file(&arena, &file);
        let names = NameResolver::new(&arena).resolve(program);
        let settings = settings(self.version);
        let analyzer = Analyzer::new(&arena, &file, &names, &self.documented, &PLUGINS, settings);
        let mut result = AnalysisResult::new(SymbolReferences::new());
        contained("analyser", &file, || {
            analyzer.analyze_with_artifacts(program, &mut result)
        })
        .and_then(Result::ok)
    }

    fn analyse(&self) -> Analysis {
        let file = self.file();
        let arena = LocalArena::new();
        let program = parse_file(&arena, file);
        let mut untyped = HashSet::new();
        let names = NameResolver::new(&arena).resolve(program);
        let mut functions: Vec<(u32, u32)> = Vec::new();
        let mut assigned: Vec<(&[u8], u32, Option<&TUnion>)> = Vec::new();
        // the end of a plain `$v = ...` and the span of its value
        let mut copies: HashMap<u32, (u32, u32)> = HashMap::new();
        let mut builtin_calls: Vec<(mago_span::Span, Option<mago_span::Span>)> = Vec::new();
        let mut widened: Vec<(u32, u32)> = Vec::new();

        any_node(Node::Program(program), &mut |n| match n {
            Node::Function(_) | Node::Method(_) | Node::Closure(_) | Node::ArrowFunction(_) => {
                let span = n.span();
                functions.push((span.start.offset, span.end.offset));
            }
            Node::Assignment(a) => {
                if let Expression::Variable(Variable::Direct(v)) = a.lhs {
                    let declared = match a.rhs {
                        Expression::Call(Call::Function(call)) if a.operator.is_assign() => {
                            self.builtin_return(&names, call)
                        }
                        _ => None,
                    };
                    assigned.push((v.name, a.span().end.offset, declared));
                    if a.operator.is_assign() {
                        let value = crate::syntax::unparenthesize(a.rhs).span();
                        copies.insert(a.span().end.offset, (value.start.offset, value.end.offset));
                    }
                }
            }
            Node::Binary(b) if b.operator.is_arithmetic() => {
                let plus = matches!(b.operator, BinaryOperator::Addition(_));
                let operands = [b.lhs, b.rhs].map(|o| self.builtin_operand(&names, o, plus));
                let span = (b.span().start.offset, b.span().end.offset);
                if operands.contains(&Some(true)) {
                    untyped.insert(span);
                } else if operands.contains(&Some(false)) {
                    widened.push(span);
                }
            }
            Node::UnaryPrefix(u) if u.operator.is_arithmetic() => {
                let span = (u.span().start.offset, u.span().end.offset);
                match self.builtin_operand(&names, u.operand, false) {
                    Some(true) => {
                        untyped.insert(span);
                    }
                    Some(false) => widened.push(span),
                    None => {}
                }
            }
            Node::Trait(_) => {
                let mut anonymous = Vec::new();
                any_node(n, &mut |c| {
                    if let Node::AnonymousClass(c) = c {
                        anonymous.push((c.span().start.offset, c.span().end.offset));
                    }
                });
                let own = |s: mago_span::Span| {
                    !(anonymous.iter()).any(|(a, b)| *a <= s.start.offset && s.end.offset <= *b)
                };
                any_node(n, &mut |e| {
                    if let Node::Expression(e) = e
                        && own(e.span())
                        && from_this(e)
                    {
                        let span = e.span();
                        untyped.insert((span.start.offset, span.end.offset));
                    }
                });
                any_node(n, &mut |m| {
                    if let Node::Method(_) = m
                        && own(m.span())
                    {
                        let mut assignments = Vec::new();
                        any_node(m, &mut |a| {
                            if let Node::Assignment(a) = a {
                                assignments.push(a);
                            }
                        });
                        let mut writes: Vec<(&[u8], u32, bool)> = Vec::new();
                        for a in assignments {
                            let mut targets: Vec<&[u8]> = Vec::new();
                            any_node(Node::Expression(a.lhs), &mut |t| {
                                if let Node::DirectVariable(v) = t {
                                    targets.push(v.name);
                                }
                            });
                            if !matches!(
                                a.lhs,
                                Expression::Variable(Variable::Direct(_))
                                    | Expression::Array(_)
                                    | Expression::List(_)
                            ) {
                                continue;
                            }
                            let mut reads_this = false;
                            for v in passed_on(a.rhs) {
                                let last = (writes.iter())
                                    .filter(|(name, end, _)| {
                                        *name == v.name && *end <= v.span.start.offset
                                    })
                                    .max_by_key(|(_, end, _)| *end);
                                reads_this |= matches!(last, Some((_, _, true)));
                            }
                            let this = a.operator.is_assign()
                                && own(a.span())
                                && (from_this(a.rhs) || reads_this);
                            writes.extend(
                                targets.into_iter().map(|t| (t, a.span().end.offset, this)),
                            );
                        }
                        if !writes.iter().any(|w| w.2) {
                            return;
                        }
                        any_node(m, &mut |r| {
                            let Node::DirectVariable(v) = r else { return };
                            let at = v.span.start.offset;
                            let last = (writes.iter())
                                .filter(|(name, end, _)| *name == v.name && *end <= at)
                                .max_by_key(|(_, end, _)| *end);
                            if let Some((_, _, true)) = last {
                                untyped.insert((at, v.span.end.offset));
                            }
                        });
                    }
                });
            }
            Node::FunctionCall(call) => {
                if let Some(name) = self.builtin_name(&names, call) {
                    let argument = (name.eq_ignore_ascii_case(b"get_class"))
                        .then(|| {
                            call.argument_list
                                .arguments
                                .first()
                                .map(|a| a.value().span())
                        })
                        .flatten();
                    builtin_calls.push((call.span(), argument));
                }
            }
            _ => {}
        });
        let mut declared: HashMap<(u32, u32), TUnion> = HashMap::new();
        for span in widened {
            declared.insert(span, get_int_or_float());
        }
        if assigned.iter().any(|(_, _, t)| t.is_some()) {
            any_node(Node::Program(program), &mut |n| {
                let Node::DirectVariable(v) = n else {
                    return;
                };
                let at = v.span.start.offset;
                let function = functions
                    .iter()
                    .filter(|(s, e)| *s <= at && at <= *e)
                    .min_by_key(|(s, e)| e - s)
                    .copied()
                    .unwrap_or((0, u32::MAX));
                let last = assigned
                    .iter()
                    .filter(|(name, after, _)| {
                        *name == v.name && function.0 <= *after && *after <= at
                    })
                    .max_by_key(|(_, after, _)| *after);
                if let Some((_, _, Some(t))) = last {
                    declared.insert((at, v.span.end.offset), (*t).clone());
                }
            });
        }
        any_node(Node::Program(program), &mut |n| {
            let Node::Binary(b) = n else { return };
            if !b.operator.is_concatenation() {
                return;
            }
            let array = |o: &Expression<'_>| {
                let t = match crate::syntax::unparenthesize(o) {
                    Expression::Call(Call::Function(call)) => self.builtin_return(&names, call),
                    Expression::Variable(Variable::Direct(v)) => {
                        declared.get(&(v.span.start.offset, v.span.end.offset))
                    }
                    _ => None,
                };
                t.is_some_and(|t| t.types.iter().any(|a| matches!(a, TAtomic::Array(_))))
            };
            if array(b.lhs) || array(b.rhs) {
                untyped.insert((b.span().start.offset, b.span().end.offset));
            }
        });
        let analyzer = Analyzer::new(
            &arena,
            file,
            &names,
            &self.codebase,
            &PLUGINS,
            Settings {
                analyze_dead_code: self.dead_code,
                ..settings(self.version)
            },
        );
        let mut result = AnalysisResult::new(SymbolReferences::new());
        let artifacts = contained("analyser", file, || {
            analyzer.analyze_with_artifacts(program, &mut result)
        })
        .and_then(Result::ok);
        if let Some(artifacts) = &artifacts {
            let unknown = |t: &TUnion| {
                t.types.iter().any(|t| match t {
                    TAtomic::Object(TObject::Named(object)) => self
                        .codebase
                        .get_class_like(object.name.as_bytes())
                        .is_none(),
                    TAtomic::Reference(_) | TAtomic::Never => true,
                    _ => false,
                })
            };
            any_node(Node::Program(program), &mut |n| {
                let (object, span) = match n {
                    Node::PropertyAccess(a) => (a.object, a.span()),
                    Node::NullSafePropertyAccess(a) => (a.object, a.span()),
                    Node::MethodCall(c) => (c.object, c.span()),
                    Node::NullSafeMethodCall(c) => (c.object, c.span()),
                    _ => return,
                };
                let o = crate::syntax::unparenthesize(object).span();
                let Some(t) = (artifacts.expression_types).get(&(o.start.offset, o.end.offset))
                else {
                    return;
                };
                // PHPStan gives an enum's `name` and `value` properties a mixed native type
                // (`PhpClassReflectionExtension::createProperty()`).
                let property = match n {
                    Node::PropertyAccess(a) => Some(&a.property),
                    Node::NullSafePropertyAccess(a) => Some(&a.property),
                    _ => None,
                };
                let enum_property = matches!(
                    property,
                    Some(ClassLikeMemberSelector::Identifier(id)) if matches!(id.value, b"name" | b"value")
                ) && !t.types.is_empty()
                    && t.types.iter().all(|t| match t {
                        TAtomic::Object(TObject::Enum(_)) => true,
                        TAtomic::Object(TObject::Named(object)) => (self.codebase)
                            .get_class_like(object.name.as_bytes())
                            .is_some_and(|c| c.kind.is_enum()),
                        _ => false,
                    });
                if unknown(t) || enum_property {
                    untyped.insert((span.start.offset, span.end.offset));
                }
            });
            let open = |class: &[u8], constant: &[u8]| {
                (self.documented.get_class_like(class)).is_some_and(|c| !c.flags.is_final())
                    && (self.documented.get_class_constant(class, constant))
                        .is_some_and(|k| !k.flags.is_final() && k.type_metadata.is_none())
            };
            let mut scopes: Vec<(mago_span::Span, Option<&[u8]>)> = Vec::new();
            any_node(Node::Program(program), &mut |n| match n {
                Node::Class(c) => scopes.push((c.span(), Some(names.get(&c.name)))),
                Node::Interface(i) => scopes.push((i.span(), Some(names.get(&i.name)))),
                Node::AnonymousClass(c) => scopes.push((c.span(), None)),
                Node::ClassConstantAccess(a) => {
                    let ClassLikeConstantSelector::Identifier(constant) = &a.constant else {
                        return;
                    };
                    if constant.value.eq_ignore_ascii_case(b"class") {
                        return;
                    }
                    let span = a.span();
                    let mixed = match a.class {
                        Expression::Static(_) => (scopes.iter().rev())
                            .find(|(s, _)| {
                                s.start.offset <= span.start.offset
                                    && span.end.offset <= s.end.offset
                            })
                            .and_then(|(_, class)| *class)
                            .is_some_and(|class| open(class, constant.value)),
                        Expression::Self_(_)
                        | Expression::Parent(_)
                        | Expression::Identifier(_) => false,
                        object => {
                            let o = object.span();
                            (artifacts
                                .expression_types
                                .get(&(o.start.offset, o.end.offset)))
                            .is_some_and(|t| {
                                t.types.iter().any(|t| match t {
                                    TAtomic::Object(TObject::Named(n)) => {
                                        open(n.name.as_bytes(), constant.value)
                                    }
                                    _ => false,
                                })
                            })
                        }
                    };
                    if mixed {
                        untyped.insert((span.start.offset, span.end.offset));
                    }
                }
                _ => {}
            });
            any_node(Node::Program(program), &mut |n| {
                if let Node::UnaryPrefix(u) = n
                    && matches!(
                        u.operator,
                        UnaryPrefixOperator::StringCast(..) | UnaryPrefixOperator::BinaryCast(..)
                    )
                {
                    let key = (u.span().start.offset, u.span().end.offset);
                    if (artifacts.expression_types.get(&key)).is_none_or(|t| {
                        !t.types
                            .iter()
                            .all(|a| matches!(a, TAtomic::Scalar(s) if s.is_any_string()))
                    }) {
                        declared.insert(key, get_string());
                    }
                }
            });
            let mut operations = Vec::new();
            any_node(Node::Program(program), &mut |n| {
                if let Node::Binary(b) = n
                    && (b.operator.is_arithmetic() || b.operator.is_bitwise())
                {
                    operations.push(b);
                }
            });
            for b in operations.into_iter().rev() {
                let key = |e: &Expression<'_>| {
                    let s = crate::syntax::unparenthesize(e).span();
                    (s.start.offset, s.end.offset)
                };
                let kind = |e: &Expression<'_>| -> Option<Operand> {
                    if untyped.contains(&key(e)) {
                        return None;
                    }
                    let t = (declared.get(&key(e))).or_else(|| {
                        artifacts
                            .expression_types
                            .get(&key(e))
                            .map(std::convert::AsRef::as_ref)
                    })?;
                    operand(t)
                };
                let (Some(left), Some(right)) = (kind(b.lhs), kind(b.rhs)) else {
                    continue;
                };
                if let Some(t) = mixed_math(&b.operator, left, right) {
                    declared.insert((b.span().start.offset, b.span().end.offset), t);
                }
            }
        }
        if let Some(documented) = (!builtin_calls.is_empty())
            .then(|| self.documented_artifacts())
            .flatten()
        {
            let key = |s: mago_span::Span| (s.start.offset, s.end.offset);
            let t = |s: mago_span::Span| documented.expression_types.get(&key(s));
            for (call, argument) in builtin_calls {
                if argument.is_some_and(|a| {
                    t(a).is_none_or(|t| t.types.iter().any(|t| matches!(t, TAtomic::Mixed(_))))
                }) {
                    untyped.insert(key(call));
                } else if let Some(t) = t(call) {
                    declared.insert(key(call), TUnion::clone(t));
                }
            }
        }
        if !untyped.is_empty() {
            let mut around = Vec::new();
            any_node(Node::Program(program), &mut |n| match n {
                Node::Parenthesized(p) => around.push((p.span(), false, vec![p.expression])),
                Node::Conditional(c) => around.push((
                    c.span(),
                    false,
                    vec![c.then.unwrap_or(c.condition), c.r#else],
                )),
                Node::Binary(b) if b.operator.is_arithmetic() => {
                    around.push((b.span(), true, vec![b.lhs, b.rhs]));
                }
                Node::UnaryPrefix(u) if u.operator.is_arithmetic() => {
                    around.push((u.span(), true, vec![u.operand]));
                }
                _ => {}
            });
            for (span, arithmetic, parts) in around.into_iter().rev() {
                let key = |s: mago_span::Span| (s.start.offset, s.end.offset);
                if untyped.contains(&key(span)) {
                    continue;
                }
                if !parts.iter().any(|p| untyped.contains(&key(p.span()))) {
                    if let (false, [inner]) = (arithmetic, &parts[..])
                        && let Some(t) = declared.get(&key(inner.span())).cloned()
                    {
                        declared.insert(key(span), t);
                    }
                    continue;
                }
                if arithmetic {
                    declared.insert(key(span), get_int_or_float());
                } else {
                    untyped.insert(key(span));
                }
            }
        }
        // A variable assigned a value with a mixed native type has a mixed native type itself, until it is assigned again.
        if !untyped.is_empty() {
            let mut copied = Vec::new();
            any_node(Node::Program(program), &mut |n| {
                let Node::DirectVariable(v) = n else {
                    return;
                };
                let at = v.span.start.offset;
                let function = (functions.iter())
                    .filter(|(s, e)| *s <= at && at <= *e)
                    .min_by_key(|(s, e)| e - s)
                    .copied()
                    .unwrap_or((0, u32::MAX));
                let last = (assigned.iter())
                    .filter(|(name, after, _)| {
                        *name == v.name && function.0 <= *after && *after <= at
                    })
                    .max_by_key(|(_, after, _)| *after);
                if let Some((_, after, _)) = last
                    && copies
                        .get(after)
                        .is_some_and(|value| untyped.contains(value))
                {
                    copied.push((at, v.span.end.offset));
                }
            });
            untyped.extend(copied);
        }
        let mut holders: Vec<(&[u8], mago_span::Span, Vec<u8>)> = Vec::new();
        any_node(Node::Program(program), &mut |n| {
            let Node::IssetConstruct(isset) = n else {
                return;
            };
            for value in &isset.values {
                let Expression::ArrayAccess(access) = crate::syntax::unparenthesize(value) else {
                    continue;
                };
                let (Expression::Variable(Variable::Direct(v)), Expression::Literal(literal)) =
                    (crate::syntax::unparenthesize(access.array), access.index)
                else {
                    continue;
                };
                let key = match literal {
                    mago_syntax::cst::Literal::String(s) => s.value.map(<[u8]>::to_vec),
                    mago_syntax::cst::Literal::Integer(i) => {
                        i.value.map(|i| i.to_string().into_bytes())
                    }
                    _ => None,
                };
                if let Some(key) = key {
                    holders.push((v.name, v.span, key));
                }
            }
        });
        if let (false, Some(native), Some(documented)) = (
            holders.is_empty(),
            artifacts.as_ref(),
            self.documented_artifacts(),
        ) {
            let key = |s: mago_span::Span| (s.start.offset, s.end.offset);
            let mixed = |t: &TUnion| t.types.iter().all(|a| matches!(a, TAtomic::Mixed(_)));
            let has_key = |t: &TUnion, k: &[u8]| {
                t.types.iter().any(|a| match a {
                    TAtomic::Array(mago_codex::ttype::atomic::array::TArray::Keyed(keyed)) => {
                        (keyed.known_items.iter().flat_map(|i| i.iter())).any(
                            |(name, (optional, _))| {
                                !optional && match name {
                                    mago_codex::ttype::atomic::array::key::ArrayKey::String(w) => {
                                        w.as_bytes() == k
                                    }
                                    mago_codex::ttype::atomic::array::key::ArrayKey::Integer(i) => {
                                        i.to_string().as_bytes() == k
                                    }
                                    _ => false,
                                }
                            },
                        )
                    }
                    _ => false,
                })
            };
            let within = |s: mago_span::Span| {
                (functions.iter())
                    .filter(|(a, b)| *a <= s.start.offset && s.end.offset <= *b)
                    .min_by_key(|(a, b)| b - a)
                    .copied()
            };
            let holders: Vec<_> = (holders.into_iter())
                .filter(|(_, s, _)| {
                    documented
                        .expression_types
                        .get(&key(*s))
                        .is_some_and(|t| !mixed(t))
                })
                .collect();
            any_node(Node::Program(program), &mut |n| {
                let Node::DirectVariable(v) = n else { return };
                let narrowed = holders.iter().any(|(name, s, k)| {
                    *name == v.name
                        && s.end.offset <= v.span.start.offset
                        && within(*s) == within(v.span)
                        && documented
                            .expression_types
                            .get(&key(v.span))
                            .is_some_and(|t| has_key(t, k))
                });
                if narrowed
                    && !declared.contains_key(&key(v.span))
                    && native
                        .expression_types
                        .get(&key(v.span))
                        .is_some_and(|t| mixed(t))
                {
                    declared.insert(key(v.span), get_mixed_keyed_array());
                }
            });
        }
        if let Some(native) = artifacts.as_ref() {
            let regions = isset_property_regions(program);
            if !regions.is_empty() {
                let key = |s: mago_span::Span| (s.start.offset, s.end.offset);
                let mixed = |t: &TUnion| t.types.iter().all(|a| matches!(a, TAtomic::Mixed(_)));
                any_node(Node::Program(program), &mut |n| {
                    let Node::DirectVariable(v) = n else { return };
                    let inside = regions.iter().any(|(name, from, to)| {
                        *name == v.name && *from <= v.span.start.offset && v.span.end.offset <= *to
                    });
                    if inside
                        && !declared.contains_key(&key(v.span))
                        && native
                            .expression_types
                            .get(&key(v.span))
                            .is_some_and(|t| mixed(t))
                    {
                        declared.insert(key(v.span), get_object());
                    }
                });
            }
        }
        Analysis {
            artifacts,
            untyped,
            declared,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Operand {
    Mixed,
    Number,
    Int,
    Float,
}

fn operand(t: &TUnion) -> Option<Operand> {
    let all =
        |f: fn(&TScalar) -> bool| (t.types.iter()).all(|a| matches!(a, TAtomic::Scalar(s) if f(s)));
    if t.is_mixed() {
        Some(Operand::Mixed)
    } else if all(TScalar::is_int) {
        Some(Operand::Int)
    } else if all(TScalar::is_float) {
        Some(Operand::Float)
    } else if all(|s| s.is_int() || s.is_float()) {
        Some(Operand::Number)
    } else {
        None
    }
}

fn mixed_math(operator: &BinaryOperator<'_>, left: Operand, right: Operand) -> Option<TUnion> {
    use Operand::*;
    if ![left, right].iter().any(|o| matches!(o, Mixed | Number)) {
        return None;
    }
    let float = left == Float || right == Float;
    let union = |types: Vec<TUnion>| {
        TUnion::new(Cow::Owned(
            types
                .into_iter()
                .flat_map(|t| t.types.into_owned())
                .collect(),
        ))
    };
    Some(match operator {
        BinaryOperator::Addition(_) if left == Mixed && right == Mixed => {
            union(vec![get_float(), get_int(), get_mixed_keyed_array()])
        }
        BinaryOperator::Exponentiation(_) if left == Mixed => union(vec![get_float(), get_int()]),
        BinaryOperator::Exponentiation(_) => return None,
        BinaryOperator::Modulo(_) => get_int(),
        BinaryOperator::LeftShift(_) | BinaryOperator::RightShift(_) if float => get_int(),
        BinaryOperator::BitwiseAnd(_)
        | BinaryOperator::BitwiseOr(_)
        | BinaryOperator::BitwiseXor(_) => match (left, right) {
            (Mixed, Mixed) => get_int_or_string(),
            _ => get_int(),
        },
        _ if float => get_float(),
        _ => get_int_or_float(),
    })
}

fn passed_on<'a>(e: &'a Expression<'a>) -> Vec<&'a mago_syntax::cst::DirectVariable<'a>> {
    match e {
        Expression::Variable(Variable::Direct(v)) => vec![v],
        Expression::Parenthesized(p) => passed_on(p.expression),
        Expression::ArrayAccess(a) => passed_on(a.array),
        Expression::Conditional(c) => c
            .then
            .into_iter()
            .chain([c.r#else])
            .flat_map(passed_on)
            .collect(),
        Expression::Binary(b) if matches!(b.operator, BinaryOperator::NullCoalesce(_)) => {
            [b.lhs, b.rhs].into_iter().flat_map(passed_on).collect()
        }
        Expression::Array(a) => a
            .elements
            .iter()
            .filter_map(|e| e.get_value())
            .flat_map(passed_on)
            .collect(),
        Expression::LegacyArray(a) => a
            .elements
            .iter()
            .filter_map(|e| e.get_value())
            .flat_map(passed_on)
            .collect(),
        _ => Vec::new(),
    }
}
fn from_this(expr: &Expression<'_>) -> bool {
    match expr {
        Expression::Variable(Variable::Direct(v)) => v.name == b"$this",
        Expression::Parenthesized(p) => from_this(p.expression),
        Expression::Access(Access::Property(p)) => from_this(p.object),
        Expression::Access(Access::NullSafeProperty(p)) => from_this(p.object),
        Expression::Call(Call::Method(c)) => from_this(c.object),
        Expression::Call(Call::NullSafeMethod(c)) => from_this(c.object),
        Expression::ArrayAccess(a) => from_this(a.array),
        Expression::Clone(c) => from_this(c.object),
        Expression::Access(Access::ClassConstant(c)) => {
            matches!(c.class, Expression::Self_(_) | Expression::Static(_))
                && !matches!(&c.constant, ClassLikeConstantSelector::Identifier(id) if id.value.eq_ignore_ascii_case(b"class"))
        }
        _ => false,
    }
}

fn any_node<'a>(node: Node<'a, 'a>, f: &mut impl FnMut(Node<'a, 'a>)) {
    let (mut stack, mut children) = (vec![node], Vec::new());
    while let Some(node) = stack.pop() {
        f(node);
        node.visit_children(|child| children.push(child));
        stack.extend(children.drain(..).rev());
    }
}

/// `code` with the inside of each doc block blanked, from its parsed `program`: the source the native facts read.
fn without_docblocks(code: &str, program: &Program<'_>) -> String {
    let mut bytes = code.as_bytes().to_vec();
    for trivia in program
        .trivia
        .iter()
        .filter(|t| t.kind == TriviaKind::DocBlockComment)
    {
        let (start, end) = (
            trivia.span.start.offset as usize,
            trivia.span.end.offset as usize,
        );
        for b in &mut bytes[start + 2..end - 2] {
            if *b != b'\n' {
                *b = b' ';
            }
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| code.to_string())
}

pub(crate) fn facts(ctx: &Context<'_>) -> Rc<Facts> {
    Rc::clone(ctx.facts.get_or_init(|| build(ctx, false)))
}

pub(crate) fn facts_with_dead_code(ctx: &Context<'_>) -> Rc<Facts> {
    Rc::clone(ctx.dead_code_facts.get_or_init(|| build(ctx, true)))
}

fn build(ctx: &Context<'_>, dead_code: bool) -> Rc<Facts> {
    let mut pending = PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    note_project_read();
    if let Some(build) = pending.take() {
        set_project_codebase(Some(build()));
    }
    drop(pending);
    let project = PROJECT
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .map(Arc::clone);
    Rc::new(scan(ctx, project, dead_code))
}

fn mago_file(path: &str, code: &str, file_type: FileType) -> File {
    File::new(
        Cow::Owned(path.as_bytes().to_vec()),
        file_type,
        None,
        Cow::Owned(code.as_bytes().to_vec()),
    )
}

fn scan_symbols<'a>(
    arena: &'a LocalArena,
    file: &File,
    program: &'a Program<'a>,
    names: &ResolvedNames<'a>,
    version: PHPVersion,
) -> CodebaseMetadata {
    let mut scanned = contained("scanner", file, || {
        scan_program(arena, file, program, names, version)
    })
    .unwrap_or_default();
    mark_anonymous_classes_final(&mut scanned);
    if file.file_type == FileType::Vendored {
        (scanned.function_likes).retain(|(class, name), _| {
            !class.as_bytes().is_empty() || PRELUDE.0.get_function(name.as_bytes()).is_none()
        });
    }
    scanned
}

/// Runs one of Mago's passes over `file`. Mago panics on some inputs; that should cost the rules reading this fact
/// their type information for the file, not the whole run, so the panic stops here, and is named on stderr because
/// those rules then go quiet on the file.
fn contained<T>(pass: &str, file: &File, run: impl FnOnce() -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
        .map_err(|panic| {
            note_stderr();
            let message = (panic.downcast_ref::<&str>().map(ToString::to_string))
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            #[expect(
                clippy::print_stderr,
                reason = "a contained panic has no other channel to the user"
            )]
            {
                eprintln!(
                    "pun: Mago's {pass} panicked on {}, so the lint rules that need its types skip that file: {message}",
                    String::from_utf8_lossy(&file.name)
                );
            }
        })
        .ok()
}

fn mark_anonymous_classes_final(scanned: &mut CodebaseMetadata) {
    for class in scanned.class_likes.values_mut() {
        if class.kind.is_class() && class.name_span.is_none() {
            class.flags |= MetadataFlags::FINAL;
        }
    }
}

fn is_vendor_path(path: &str) -> bool {
    path.contains("/vendor/")
}

static PATHS: LazyLock<RwLock<HashMap<FileId, Arc<str>>>> = LazyLock::new(Default::default);

pub(crate) fn path_of(file: FileId) -> Option<Arc<str>> {
    (PATHS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
    .get(&file)
    .cloned()
}

fn remember(id: FileId, path: &str, code: &str) {
    remember_source(id, code);
    (PATHS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
    .insert(id, Arc::from(path));
    if is_vendor_path(path) {
        (VENDOR
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
        .insert(id);
    }
}

fn refresh_closures(
    codebase: &CodebaseMetadata,
    path: &str,
    code: &str,
    version: PHPVersion,
) -> Arc<CodebaseMetadata> {
    let file = mago_file(path, code, FileType::Host);
    let arena = LocalArena::new();
    let program = parse_file(&arena, &file);
    refresh_parsed(codebase, &arena, &file, program, version)
}

fn refresh_parsed<'a>(
    codebase: &CodebaseMetadata,
    arena: &'a LocalArena,
    file: &File,
    program: &'a Program<'a>,
    version: PHPVersion,
) -> Arc<CodebaseMetadata> {
    let names = NameResolver::new(arena).resolve(program);
    let mut scanned = contained("scanner", file, || {
        scan_program(arena, file, program, &names, version)
    })
    .unwrap_or_default();
    mark_anonymous_classes_final(&mut scanned);
    scanned.set_file_signature(file.id, build_file_signature(file, program, &names));
    Arc::new(codebase.with_file_closures(file.id, scanned))
}

fn scan(ctx: &Context<'_>, project: Option<Arc<ProjectCodebase>>, dead_code: bool) -> Facts {
    let path = ctx.path.to_string_lossy().into_owned();
    let project = project.unwrap_or_else(|| {
        let mut files: Vec<(String, String)> = SOURCES.with(|s| s.borrow().clone());
        files.push((path.clone(), ctx.source.to_string()));
        project_codebase(&files, ctx.php_version)
    });
    let version = ctx.php_version.to_mago();
    // One parse of the source finds its doc blocks and refreshes the documented facts; it is dropped before the
    // native facts parse the blanked source.
    let (native_source, documented) = {
        let file = mago_file(&path, ctx.source, FileType::Host);
        let arena = LocalArena::new();
        let program = parse_file(&arena, &file);
        let native_source = without_docblocks(ctx.source, program);
        let documented = refresh_parsed(&project.documented, &arena, &file, program, version);
        (native_source, documented)
    };
    let codebase = refresh_closures(&project.native, &path, &native_source, version);
    Facts {
        codebase,
        documented,
        path,
        native_source,
        source: ctx.source.to_string(),
        version,
        artifacts: OnceCell::new(),
        documented_artifacts: OnceCell::new(),
        dead_code,
        file: OnceCell::new(),
    }
}

pub(crate) fn is_builtin_class(class: &[u8]) -> bool {
    PRELUDE.0.get_class_like(class).is_some()
}

pub(crate) fn is_builtin_function(function: &[u8]) -> bool {
    PRELUDE.0.get_function(function).is_some()
}

fn isset_property_regions<'a>(program: &'a Program<'a>) -> Vec<(&'a [u8], u32, u32)> {
    use mago_syntax::cst::{Construct, IfBody, Statement};
    fn property_holders<'a>(expr: &'a Expression<'a>, out: &mut Vec<&'a [u8]>) {
        match crate::syntax::unparenthesize(expr) {
            Expression::Construct(Construct::Isset(isset)) => {
                for value in &isset.values {
                    if let Expression::Access(Access::Property(p)) =
                        crate::syntax::unparenthesize(value)
                        && let Expression::Variable(Variable::Direct(v)) =
                            crate::syntax::unparenthesize(p.object)
                    {
                        out.push(v.name);
                    }
                }
            }
            Expression::Binary(b)
                if matches!(
                    b.operator,
                    BinaryOperator::And(_) | BinaryOperator::LowAnd(_)
                ) =>
            {
                property_holders(b.lhs, out);
                property_holders(b.rhs, out);
            }
            _ => {}
        }
    }
    fn exits(statement: &Statement<'_>) -> bool {
        match statement {
            Statement::Block(block) => block.statements.last().is_some_and(exits),
            Statement::Return(_) | Statement::Continue(_) | Statement::Break(_) => true,
            Statement::Expression(e) => matches!(e.expression, Expression::Throw(_)),
            _ => false,
        }
    }
    let mut regions: Vec<(&'a [u8], u32, u32)> = Vec::new();
    any_node(Node::Program(program), &mut |n| match n {
        Node::If(r#if) => {
            let IfBody::Statement(body) = &r#if.body else {
                return;
            };
            let mut holders = Vec::new();
            property_holders(r#if.condition, &mut holders);
            let span = body.statement.span();
            regions.extend(
                holders
                    .into_iter()
                    .map(|h| (h, span.start.offset, span.end.offset)),
            );
        }
        Node::Block(block) => {
            for (i, statement) in block.statements.iter().enumerate() {
                let Statement::If(r#if) = statement else {
                    continue;
                };
                let IfBody::Statement(body) = &r#if.body else {
                    continue;
                };
                let Expression::UnaryPrefix(not) = crate::syntax::unparenthesize(r#if.condition)
                else {
                    continue;
                };
                if !matches!(not.operator, UnaryPrefixOperator::Not(_))
                    || !body.else_if_clauses.is_empty()
                    || body.else_clause.is_some()
                    || !exits(body.statement)
                    || i + 1 == block.statements.len()
                {
                    continue;
                }
                let Expression::Construct(Construct::Isset(_)) =
                    crate::syntax::unparenthesize(not.operand)
                else {
                    continue;
                };
                let mut holders = Vec::new();
                property_holders(not.operand, &mut holders);
                let end = block.right_brace.start.offset;
                regions.extend(
                    holders
                        .into_iter()
                        .map(|h| (h, r#if.span().end.offset, end)),
                );
            }
        }
        _ => {}
    });
    let mut assigned: Vec<(&[u8], u32)> = Vec::new();
    any_node(Node::Program(program), &mut |n| {
        if let Node::Assignment(a) = n
            && let Expression::Variable(Variable::Direct(v)) = crate::syntax::unparenthesize(a.lhs)
        {
            assigned.push((v.name, v.span.start.offset));
        }
    });
    regions.retain(|(name, from, to)| {
        !assigned
            .iter()
            .any(|(n, at)| n == name && from <= at && at < to)
    });
    regions
}

#[cfg(test)]
mod tests {
    use mago_database::file::FileType;

    #[test]
    fn a_panicking_pass_yields_no_fact() {
        let file = super::mago_file("a.php", "<?php", FileType::Host);
        assert_eq!(super::contained("scanner", &file, || 1), Some(1));
        assert_eq!(
            super::contained("scanner", &file, || -> i32 { panic!("boom") }),
            None
        );
        let pass = "scanner";
        assert_eq!(
            super::contained(pass, &file, || -> i32 { panic!("{pass} failed") }),
            None
        );
    }

    /// A result cache keys a file on the project only when linting it read the project, so a read must count for
    /// the file whose linting made it, even when another file's linting runs nested on the same thread. A note on
    /// stderr makes the file it concerns, and every file that reads the project, uncacheable.
    #[test]
    fn tracks_reads_per_file() {
        use super::{Reads, note_project_read, tracking};
        let file = super::mago_file("a.php", "<?php", FileType::Host);
        let ((), outer) = tracking(|| {
            assert!(tracking(note_project_read).1.project);
            assert_eq!(
                super::contained("scanner", &file, || -> i32 { panic!("boom") }),
                None
            );
        });
        assert_eq!(
            outer,
            Reads {
                project: false,
                noted: true
            }
        );
        assert_eq!(tracking(|| ()).1, Reads::default());
        assert!(tracking(note_project_read).1.noted);
    }
}
