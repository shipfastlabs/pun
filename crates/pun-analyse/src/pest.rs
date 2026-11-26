//! pestphp/pest-plugin-phpstan's `TestClosureThisTypeExtension`, on with `[extensions] pest = "on"`
//! (`pun_config::extensions::PEST`).
//!
//! Pest declares `@param-closure-this TestCall` on `test()`, `it()`, `describe()` and the hooks, so without the
//! extension `$this` in a test is `Pest\PendingCalls\TestCall`, which knows none of the test case's methods. At run
//! time `$this` is the test case the file is bound to, so the extension types it as that class: the class that
//! `pest()->extend(X::class)->in(...)` or `uses(X::class)->in(...)` in `tests/Pest.php` binds to the file's directory,
//! else `PHPUnit\Framework\TestCase`, as `PestTestCaseType::resolve()` does.
//!
//! It also ports `TestCaseDynamicPropertyTypeExtension` with `PestHookPropertyReader`: `$this->name` read in a test is
//! the union of the types assigned to it in the file's `beforeEach()` closures, when it has no type of its own.
//! `beforeEach()` is analysed before the tests that follow it, so a test reads what its hooks assigned.
//!
//! Not ported: bindings made in the test file itself, traits bound with `->use()` (the class alone is the type), and
//! a `beforeEach()` that comes after the tests that read its properties.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use mago_allocator::LocalArena;
use mago_analyzer::plugin::PluginRegistry;
use mago_analyzer::plugin::context::{InvocationInfo, ProviderContext};
use mago_analyzer::plugin::provider::function::{FunctionReturnTypeProvider, FunctionTarget};
use mago_analyzer::plugin::provider::{Provider, ProviderMeta};
use mago_analyzer::settings::Settings;
use mago_codex::metadata::CodebaseMetadata;
use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::atomic::object::TObject;
use mago_codex::ttype::atomic::object::named::TNamedObject;
use mago_codex::ttype::combiner::CombinerOptions;
use mago_codex::ttype::union::TUnion;
use mago_codex::ttype::{combine_union_types, wrap_atomic};
use mago_database::file::{File, FileType};
use mago_names::ResolvedNames;
use mago_names::resolver::NameResolver;
use mago_span::{HasSpan, Span};
use mago_syntax::cst::{
    Access, Argument, ArgumentList, BinaryOperator, Call, ClassLikeConstantSelector,
    ClassLikeMemberSelector, Expression, Literal, MagicConstant, Program, Statement,
};
use mago_syntax::parser::parse_file;
use mago_word::{Word, ascii_lowercase_word, word};
use pun_phpstan::types::Type;
use pun_phpstan::{Array, Value};

use crate::Project;

/// The class `$this` is when no binding covers a file.
const DEFAULT_TEST_CASE: &str = "PHPUnit\\Framework\\TestCase";

/// The functions whose closure `TestClosureThisTypeExtension` types: `PEST_TEST_FUNCTIONS` and `PEST_HOOK_FUNCTIONS`.
const FUNCTIONS: &[&[u8]] = &[
    b"it",
    b"test",
    b"describe",
    b"beforeEach",
    b"afterEach",
    b"beforeAll",
    b"afterAll",
];

/// One `->in()` target of a binding: the path's components, where `*` matches any one directory name.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Target(Vec<String>);

impl Target {
    fn of(path: &Path) -> Target {
        let mut parts: Vec<String> = Vec::new();
        for component in path.components() {
            match component {
                Component::ParentDir => {
                    parts.pop();
                }
                Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
                Component::RootDir | Component::Prefix(_) | Component::CurDir => {}
            }
        }
        Target(parts)
    }

    /// Whether `file` is this target or lies under it.
    fn covers(&self, file: &Target) -> bool {
        file.0.len() >= self.0.len()
            && (self.0.iter().zip(&file.0)).all(|(pattern, part)| pattern == "*" || pattern == part)
    }
}

/// The test case classes `tests/Pest.php` binds, each to its `->in()` targets.
#[derive(Clone, Debug, Default)]
struct Bindings(Vec<(Target, String)>);

/// On with the switch. `load()` reads `tests/Pest.php`.
#[derive(Clone, Debug)]
pub struct Pest {
    root: PathBuf,
    bindings: Arc<Bindings>,
}

impl Pest {
    /// On when the parameters hold the `pest` structure `pun_config` writes with the switch.
    pub fn from_params(root: &Path, params: &Array) -> Option<Pest> {
        params.get_str("pest").and_then(Value::as_array)?;
        Some(Pest {
            root: root.to_path_buf(),
            bindings: Arc::default(),
        })
    }
}

/// The class names `uses(...)`, `->extend(...)` or `->use(...)` names as `X::class` or a string.
fn class_arguments(names: &ResolvedNames<'_>, arguments: &ArgumentList<'_>) -> Vec<String> {
    (arguments.arguments.iter())
        .filter_map(|argument| match argument {
            Argument::Positional(p) if p.ellipsis.is_none() => Some(p.value),
            _ => None,
        })
        .filter_map(|value| match value {
            Expression::Access(Access::ClassConstant(access))
                if matches!(&access.constant, ClassLikeConstantSelector::Identifier(id) if id.value.eq_ignore_ascii_case(b"class")) =>
            {
                let Expression::Identifier(class) = access.class else {
                    return None;
                };
                Some(String::from_utf8_lossy(names.get(&class.span())).into_owned())
            }
            Expression::Literal(Literal::String(s)) => {
                Some(String::from_utf8_lossy(s.value?).trim_start_matches('\\').to_string())
            }
            _ => None,
        })
        .collect()
}

/// A path argument of `->in()`: a string, or `__DIR__` joined to one. Relative paths are relative to `dir`.
fn path_argument(dir: &Path, value: &Expression<'_>) -> Option<PathBuf> {
    match value {
        Expression::Literal(Literal::String(s)) => {
            let path = Path::new(std::str::from_utf8(s.value?).ok()?);
            Some(if path.is_absolute() {
                path.to_path_buf()
            } else {
                dir.join(path)
            })
        }
        Expression::MagicConstant(MagicConstant::Directory(_)) => Some(dir.to_path_buf()),
        Expression::Binary(binary)
            if matches!(binary.operator, BinaryOperator::StringConcat(_)) =>
        {
            let left = path_argument(dir, binary.lhs)?;
            let Expression::Literal(Literal::String(s)) = binary.rhs else {
                return None;
            };
            let right = std::str::from_utf8(s.value?).ok()?;
            Some(PathBuf::from(format!("{}{right}", left.display())))
        }
        _ => None,
    }
}

/// The classes the chain under `->in()` binds: `uses(X)` and its methods, or `pest()` followed by `->extend(X)`,
/// `->extends(X)`, `->use(X)` or `->uses(X)`. `None` when the chain starts with neither function.
fn chain_classes(names: &ResolvedNames<'_>, expression: &Expression<'_>) -> Option<Vec<String>> {
    match expression {
        Expression::Call(Call::Function(call)) => {
            let Expression::Identifier(function) = call.function else {
                return None;
            };
            let name = function.value().rsplit(|b| *b == b'\\').next()?;
            if name.eq_ignore_ascii_case(b"uses") {
                Some(class_arguments(names, &call.argument_list))
            } else if name.eq_ignore_ascii_case(b"pest") {
                Some(Vec::new())
            } else {
                None
            }
        }
        Expression::Call(Call::Method(call)) => {
            let mut classes = chain_classes(names, call.object)?;
            if let ClassLikeMemberSelector::Identifier(method) = &call.method
                && [&b"extend"[..], b"extends", b"use", b"uses"]
                    .iter()
                    .any(|m| method.value.eq_ignore_ascii_case(m))
            {
                classes.extend(class_arguments(names, &call.argument_list));
            }
            Some(classes)
        }
        _ => None,
    }
}

/// The bindings of one `Pest.php`: each top-level `<chain>->in(...)` statement.
fn read_bindings(path: &Path) -> Vec<(Target, String)> {
    let Ok(contents) = std::fs::read(path) else {
        return Vec::new();
    };
    let dir = path.parent().unwrap_or(Path::new("/"));
    let file = File::new(
        std::borrow::Cow::Owned(path.to_string_lossy().into_owned().into_bytes()),
        FileType::Host,
        Some(path.to_path_buf()),
        std::borrow::Cow::Owned(contents),
    );
    let arena = LocalArena::new();
    let program: &Program<'_> = parse_file(&arena, &file);
    let names = NameResolver::new(&arena).resolve(program);
    let mut bindings = Vec::new();
    let mut statements: Vec<&Statement<'_>> = program.statements.iter().collect();
    while let Some(statement) = statements.pop() {
        match statement {
            Statement::Namespace(namespace) => statements.extend(namespace.statements().iter()),
            Statement::Expression(statement) => {
                let Expression::Call(Call::Method(call)) = statement.expression else {
                    continue;
                };
                let ClassLikeMemberSelector::Identifier(method) = &call.method else {
                    continue;
                };
                if !method.value.eq_ignore_ascii_case(b"in") {
                    continue;
                }
                let Some(classes) = chain_classes(&names, call.object) else {
                    continue;
                };
                for argument in &call.argument_list.arguments {
                    let Argument::Positional(p) = argument else {
                        continue;
                    };
                    let Some(target) = path_argument(dir, p.value) else {
                        continue;
                    };
                    let target = Target::of(&target);
                    bindings.extend(classes.iter().map(|c| (target.clone(), c.clone())));
                }
            }
            _ => {}
        }
    }
    bindings
}

/// Whether `class` is a class (not a trait or an interface) the codebase knows.
fn is_class(codebase: &CodebaseMetadata, class: &str) -> bool {
    (codebase
        .class_likes
        .get(&ascii_lowercase_word(class.as_bytes())))
    .is_some_and(|metadata| !metadata.kind.is_trait() && !metadata.kind.is_interface())
}

/// The test case class a file's test and hook closures are bound to: `PestTestCaseType::resolve()`.
fn bound_class<'b>(bindings: &'b Bindings, path: &Path, codebase: &CodebaseMetadata) -> &'b str {
    let file = Target::of(path);
    (bindings.0.iter())
        .filter(|(target, _)| target.covers(&file))
        .map(|(_, class)| class.as_str())
        .find(|class| is_class(codebase, class))
        .unwrap_or(DEFAULT_TEST_CASE)
}

impl Pest {
    /// The test case class `$this` is in the file's test and hook closures.
    pub(crate) fn test_case(&self, path: &Path, codebase: &CodebaseMetadata) -> String {
        bound_class(&self.bindings, path, codebase).to_string()
    }

    /// Whether `function` is one whose closure Pest binds to the test case.
    pub(crate) fn binds_closure(function: &str) -> bool {
        let name = function.rsplit('\\').next().unwrap_or(function);
        FUNCTIONS
            .iter()
            .any(|f| name.as_bytes().eq_ignore_ascii_case(f))
    }
}

/// `$this` in the test and hook closures of a file: `TestClosureThisTypeExtension`.
struct ClosureThis(Arc<Bindings>);

impl Provider for ClosureThis {
    fn meta() -> &'static ProviderMeta
    where
        Self: Sized,
    {
        static META: ProviderMeta = ProviderMeta::new(
            "pest-closure-this",
            "Pest test closure $this",
            "Types $this in Pest test and hook closures as the bound test case",
        );
        &META
    }
}

impl FunctionReturnTypeProvider for ClosureThis {
    fn targets() -> FunctionTarget
    where
        Self: Sized,
    {
        FunctionTarget::exact_multiple(FUNCTIONS)
    }

    fn get_return_type(
        &self,
        _: &ProviderContext<'_, '_, '_>,
        _: &InvocationInfo<'_, '_, '_>,
    ) -> Option<TUnion> {
        None
    }

    fn get_parameter_closure_this_type(
        &self,
        context: &ProviderContext<'_, '_, '_>,
        _: &InvocationInfo<'_, '_, '_>,
        parameter: &[u8],
    ) -> Option<TUnion> {
        if parameter.strip_prefix(b"$").unwrap_or(parameter) != b"closure" {
            return None;
        }
        let path = context.source_file().path.as_deref()?;
        let class = bound_class(&self.0, path, context.codebase());
        let name: Word = word(class.as_bytes());
        Some(wrap_atomic(TAtomic::Object(TObject::Named(
            TNamedObject::new(name),
        ))))
    }
}

/// The property `$this->name` names, when the expression is one.
fn this_property<'a>(expression: &'a Expression<'_>) -> Option<&'a [u8]> {
    let Expression::Access(Access::Property(access)) = expression else {
        return None;
    };
    let Expression::Variable(mago_syntax::cst::Variable::Direct(variable)) = access.object else {
        return None;
    };
    let ClassLikeMemberSelector::Identifier(name) = &access.property else {
        return None;
    };
    (variable.name == b"$this").then_some(name.value)
}

/// One file's `beforeEach()` closures and the types they assign to `$this` properties:
/// `TestCaseDynamicPropertyTypeExtension` reading `PestHookPropertyReader`.
#[derive(Default)]
struct HookProperties {
    hooks: Mutex<Vec<Span>>,
    assigned: Mutex<HashMap<Vec<u8>, TUnion>>,
}

impl HookProperties {
    fn in_hook(&self, span: Span) -> bool {
        (self.hooks.lock().expect("not poisoned").iter()).any(|hook| {
            hook.file_id == span.file_id && hook.start <= span.start && span.end <= hook.end
        })
    }
}

impl Provider for HookProperties {
    fn meta() -> &'static ProviderMeta
    where
        Self: Sized,
    {
        static META: ProviderMeta = ProviderMeta::new(
            "pest-hook-properties",
            "Pest beforeEach properties",
            "Types $this->name in a Pest test as what the file's beforeEach() closures assign to it",
        );
        &META
    }
}

impl mago_analyzer::plugin::ExpressionHook for HookProperties {
    fn before_expression(
        &self,
        expr: &Expression<'_>,
        _: &mut mago_analyzer::plugin::HookContext<'_, '_>,
    ) -> mago_analyzer::plugin::HookResult<mago_analyzer::plugin::ExpressionHookResult> {
        if let Expression::Call(Call::Function(call)) = expr
            && let Expression::Identifier(function) = call.function
            && (function.value().rsplit(|b| *b == b'\\').next())
                .is_some_and(|name| name.eq_ignore_ascii_case(b"beforeEach"))
        {
            let closures =
                (call.argument_list.arguments.iter()).map(|argument| argument.value().span());
            self.hooks.lock().expect("not poisoned").extend(closures);
        }
        Ok(mago_analyzer::plugin::ExpressionHookResult::Continue)
    }

    fn after_expression(
        &self,
        expr: &Expression<'_>,
        context: &mut mago_analyzer::plugin::HookContext<'_, '_>,
    ) -> mago_analyzer::plugin::HookResult<()> {
        if let Expression::Assignment(assignment) = expr
            && let Some(name) = this_property(assignment.lhs)
            && self.in_hook(expr.span())
        {
            let Some(ty) = context.get_expression_type(assignment.rhs).cloned() else {
                return Ok(());
            };
            if ty.is_mixed() || ty.is_never() {
                return Ok(());
            }
            let codebase = context.codebase();
            let mut assigned = self.assigned.lock().expect("not poisoned");
            let entry = assigned.entry(name.to_vec());
            match entry {
                std::collections::hash_map::Entry::Occupied(mut previous) => {
                    let combined = combine_union_types(
                        previous.get(),
                        &ty,
                        codebase,
                        CombinerOptions::default(),
                    );
                    previous.insert(combined);
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(ty);
                }
            }
            return Ok(());
        }
        let Some(name) = this_property(expr) else {
            return Ok(());
        };
        if context
            .get_expression_type(expr)
            .is_some_and(|t| !t.is_mixed())
        {
            return Ok(());
        }
        let assigned = self
            .assigned
            .lock()
            .expect("not poisoned")
            .get(name)
            .cloned();
        if let Some(ty) = assigned {
            context.set_expression_type(expr, ty);
        }
        Ok(())
    }
}

impl crate::AnalyseExtension for Pest {
    fn stub_files(&self, _: &Path) -> Vec<PathBuf> {
        Vec::new()
    }

    fn bundled_stub(&self, _: &Path) -> Option<&'static [u8]> {
        None
    }

    fn class_aliases(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    fn prepare_codebase(&self, _: &Project, _: &mut CodebaseMetadata) {}

    fn load(&mut self, _: &Project, _: &CodebaseMetadata) {
        self.bindings = Arc::new(Bindings(read_bindings(&self.root.join("tests/Pest.php"))));
    }

    fn register(&self, registry: &mut PluginRegistry, _: &[u8], _: &ResolvedNames<'_>) {
        registry.register_function_provider(ClosureThis(Arc::clone(&self.bindings)));
        registry.register_expression_hook(HookProperties::default());
    }

    fn settings(&self, _: &mut Settings) {}

    fn typed_expressions<'arena>(
        &self,
        _: &ResolvedNames<'arena>,
        _: &Program<'arena>,
    ) -> Vec<Span> {
        Vec::new()
    }

    fn type_node(&self, _: &CodebaseMetadata, _: &str, _: &[Type]) -> Option<Type> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::Target;
    use std::path::Path;

    #[test]
    fn a_target_covers_the_files_under_it_and_a_star_matches_one_directory() {
        let target = Target::of(Path::new("/app/tests/../app-modules/*/tests/Feature"));
        assert!(target.covers(&Target::of(Path::new(
            "/app/app-modules/Identity/tests/Feature/A/LoginTest.php"
        ))));
        assert!(!target.covers(&Target::of(Path::new(
            "/app/app-modules/Identity/tests/Unit/LoginTest.php"
        ))));
        assert!(!target.covers(&Target::of(Path::new(
            "/app/app-modules/tests/Feature/LoginTest.php"
        ))));
    }
}
