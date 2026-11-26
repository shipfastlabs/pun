use std::path::{Path, PathBuf};

use pun_phpstan::Array;
use pun_phpstan::Key;
use pun_phpstan::Value;
use pun_phpstan::ignore::IgnoredErrors;
use pun_phpstan::php::FileExcluder;
use pun_tokens::PhpVersion;
use rayon::prelude::*;
use serde_json::Value as Json;

use crate::relative_path::{RelativePath, absolute};
use crate::{AnalyseExtension, display, php_files};

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct ProjectOptions: u64 {
        const CHECK_MAGIC_DIR_IN_INCLUDE = 1 << 0;
        const USE_PATH_CONSTANTS_AS_CONSTANT_STRING = 1 << 1;
        const REPORT_UNMATCHED_IGNORES = 1 << 2;
        const REPORT_IGNORES_WITHOUT_COMMENTS = 1 << 3;
        const CHECK_CLASS_CASE_SENSITIVITY = 1 << 4;
        const CHECK_INTERNAL_CLASS_CASE_SENSITIVITY = 1 << 5;
        const CHECK_IMPORTED_CLASS_NAME_CASE = 1 << 6;
        const CHECK_FUNCTION_NAME_CASE = 1 << 7;
        const CLI_ARGUMENTS_VARIABLES_REGISTERED = 1 << 8;
        const CHECK_MAYBE_UNDEFINED_VARIABLES = 1 << 9;
        const POLLUTE_SCOPE_WITH_LOOP_INITIAL_ASSIGNMENTS = 1 << 10;
        const POLLUTE_SCOPE_WITH_ALWAYS_ITERABLE_FOREACH = 1 << 11;
        const POLLUTE_SCOPE_WITH_BLOCK = 1 << 12;
        const CHECK_GENERIC_ITERABLE_CLASSES = 1 << 13;
        const INTERNAL_TAG = 1 << 14;
        const NEW_STATIC_IN_ABSTRACT_CLASS_STATIC_METHOD = 1 << 15;
        const PRECISE_UNUSED_PARAMETER_LINE = 1 << 17;
        const BLEEDING_EDGE = 1 << 18;
        const UNUSED_LABEL = 1 << 19;
        const SWITCH_CONDITION = 1 << 20;
        const SORT_WITHOUT_EFFECT = 1 << 21;
        const CHECK_NON_STRINGABLE_DYNAMIC_ACCESS = 1 << 22;
        const REPORT_METHOD_PURITY_OVERRIDE = 1 << 23;
        const ASSIGN_TO_BY_REF_FOREACH_EXPR = 1 << 24;
        const CHECK_DYNAMIC_CONSTANT_NAME_VALUES = 1 << 25;
        const CHECK_DATE_INTERVAL_CONSTRUCTOR = 1 << 26;
        const CHECK_PARAMETER_CASTABLE_TO_NUMBER_FUNCTIONS = 1 << 27;
        const CHECK_PRINTF_PARAMETER_TYPES = 1 << 28;
        const CHECK_STRICT_PRINTF_PLACEHOLDER_TYPES = 1 << 29;
        const FINITE_TYPES_IN_HAYSTACK = 1 << 30;
        const UNUSED_VARIABLE = 1 << 31;
        const UNUSED_PARAMETERS = 1 << 32;
        const NEW_ON_NON_OBJECT = 1 << 33;
        const CHECK_UNINITIALIZED_PROPERTIES = 1 << 34;
        const CHECK_MISSING_CALLABLE_SIGNATURE = 1 << 35;
        const DEPRECATION_RULES = 1 << 36;
        const UNRESOLVED_TEMPLATE_ARGUMENTS = 1 << 37;
        const REPORT_WRONG_PHP_DOC_TYPE_IN_VAR_TAG = 1 << 38;
        const REPORT_MAYBES_IN_METHOD_SIGNATURES = 1 << 39;
        const REPORT_STATIC_METHOD_SIGNATURES = 1 << 40;
    }
}

pub struct Project {
    pub options: ProjectOptions,
    pub analysed: Vec<PathBuf>,
    pub scanned: Vec<PathBuf>,
    pub autoloaded: Vec<Autoloaded>,
    pub php_version: PhpVersion,
    pub current_working_directory: PathBuf,
    pub include_paths: Vec<PathBuf>,
    pub ignore_errors: IgnoredErrors,
    pub level: u8,
    pub check_function_argument_types: Option<bool>,
    pub check_arguments_passed_by_reference: Option<bool>,
    pub check_this_only: Option<bool>,
    pub check_advanced_isset: Option<bool>,
    pub check_explicit_mixed: Option<bool>,
    pub check_implicit_mixed: Option<bool>,
    pub treat_php_doc_types_as_certain: Option<bool>,
    pub only_files: bool,
    pub dynamic_constant_types: Vec<(String, String)>,
    pub strict_rules: StrictRules,
    pub exceptions: Exceptions,
    pub too_wide: TooWide,
    pub relative_path: RelativePath,
    pub larastan: Option<crate::Larastan>,
    pub carbon: Option<crate::Carbon>,
    /// `parameters.phpat` with the test classes, when phpat is on; [`Project::load_phpat`] reads the tests.
    pub phpat_config: Option<crate::phpat::Config>,
    pub phpat: Option<std::sync::Arc<crate::phpat::Rules>>,
    /// `[analyse] php-rules`: the neon files whose rules the PHP workers run; [`Project::start_php_rules`] prepares them.
    pub php_rules_config: Vec<String>,
    /// `[analyse] php-rules-workers`: the most PHP workers that run them; `None` is the number of cores, at most 8.
    pub php_rules_workers: Option<usize>,
    /// `[analyse] php-rules-paths`: the absolute paths the workers check; empty is every analysed file.
    pub php_rules_paths: Vec<PathBuf>,
    /// The rule classes Pun ports, which the workers leave to the native analysis.
    pub php_rules_native: Vec<String>,
    pub php_rules: Option<std::sync::Arc<crate::PhpRules>>,
    pub type_aliases: Vec<(String, String)>,
    pub stubs: Vec<PathBuf>,
    pub reported: Option<Reported>,
    /// The identifiers an enabled extension's config entries give its errors (disallowed-calls' `errorIdentifier`):
    /// kept although no rule claims them, as an extension's errors without an identifier are.
    pub extension_identifiers: std::collections::HashSet<String>,
    pub method_rules: MethodRules,
}

pub type Reported = std::sync::Arc<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Clone, Copy)]
pub struct MethodRules {
    pub call_methods: bool,
    pub overriding_method: bool,
    pub consistent_constructor: bool,
    pub renamed_named_arguments: bool,
}

impl Default for MethodRules {
    fn default() -> Self {
        Self {
            call_methods: true,
            overriding_method: true,
            consistent_constructor: true,
            renamed_named_arguments: true,
        }
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Default)]
    pub struct StrictRules: u16 {
        const CLOSURE_USES_THIS = 1;
        const REQUIRE_PARENT_CONSTRUCTOR_CALL = 1 << 1;
        const MATCHING_INHERITED_METHOD_NAMES = 1 << 2;
        const DYNAMIC_CALL_ON_STATIC_METHOD = 1 << 3;
        const BOOLEANS_IN_CONDITIONS = 1 << 4;
        const NUMERIC_OPERANDS_IN_ARITHMETIC_OPERATORS = 1 << 5;
        const BOOLEANS_IN_LOOP_CONDITIONS = 1 << 6;
        const USELESS_CAST = 1 << 7;
        const SWITCH_CONDITIONS_MATCHING_TYPE = 1 << 8;
    }
}

impl StrictRules {
    fn from_params(params: &Array) -> StrictRules {
        let strict = params.get_str("strictRules").and_then(Value::as_array);
        let on = |k: &str| match strict.and_then(|s| s.get_str(k)) {
            Some(Value::Bool(b)) => *b,
            Some(Value::Array(a)) => a.values().all(|v| *v == Value::Bool(true)),
            _ => false,
        };
        let mut rules = StrictRules::empty();
        rules.set(StrictRules::CLOSURE_USES_THIS, on("closureUsesThis"));
        rules.set(
            StrictRules::REQUIRE_PARENT_CONSTRUCTOR_CALL,
            on("requireParentConstructorCall"),
        );
        rules.set(
            StrictRules::MATCHING_INHERITED_METHOD_NAMES,
            on("matchingInheritedMethodNames"),
        );
        rules.set(
            StrictRules::DYNAMIC_CALL_ON_STATIC_METHOD,
            on("dynamicCallOnStaticMethod"),
        );
        rules.set(
            StrictRules::BOOLEANS_IN_CONDITIONS,
            on("booleansInConditions"),
        );
        rules.set(
            StrictRules::NUMERIC_OPERANDS_IN_ARITHMETIC_OPERATORS,
            on("numericOperandsInArithmeticOperators"),
        );
        rules.set(
            StrictRules::BOOLEANS_IN_LOOP_CONDITIONS,
            on("booleansInLoopConditions"),
        );
        rules.set(StrictRules::USELESS_CAST, on("uselessCast"));
        rules.set(
            StrictRules::SWITCH_CONDITIONS_MATCHING_TYPE,
            on("switchConditionsMatchingType"),
        );
        rules
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct ExceptionChecks: u8 {
        const IMPLICIT_THROWS = 1;
        const REPORT_UNCHECKED_EXCEPTION_DEAD_CATCH = 1 << 1;
        const MISSING_CHECKED_EXCEPTION_IN_THROWS = 1 << 2;
        const TOO_WIDE_THROW_TYPE = 1 << 3;
        const TOO_WIDE_IMPLICIT_THROW_TYPE = 1 << 4;
        const THROW_TYPE_COVARIANCE = 1 << 5;
        const CHECK_TOO_WIDE_THROW_TYPES_IN_PROTECTED_AND_PUBLIC_METHODS = 1 << 6;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Exceptions {
    pub checks: ExceptionChecks,
    pub unchecked_exception_regexes: Vec<String>,
    pub unchecked_exception_classes: Vec<String>,
    pub checked_exception_regexes: Vec<String>,
    pub checked_exception_classes: Vec<String>,
}

impl Default for Exceptions {
    fn default() -> Self {
        Exceptions {
            checks: ExceptionChecks::IMPLICIT_THROWS
                | ExceptionChecks::REPORT_UNCHECKED_EXCEPTION_DEAD_CATCH
                | ExceptionChecks::TOO_WIDE_THROW_TYPE,
            unchecked_exception_regexes: Vec::new(),
            unchecked_exception_classes: vec!["Error".into()],
            checked_exception_regexes: Vec::new(),
            checked_exception_classes: Vec::new(),
        }
    }
}

impl Exceptions {
    fn from_params(params: &Array) -> Exceptions {
        let d = Exceptions::default();
        let exceptions = params.get_str("exceptions").and_then(Value::as_array);
        let check = exceptions
            .and_then(|e| e.get_str("check"))
            .and_then(Value::as_array);
        let mut checks = d.checks;
        let mut flag = |a: Option<&Array>, k: &str, which: ExceptionChecks| {
            if let Some(Value::Bool(b)) = a.and_then(|a| a.get_str(k)) {
                checks.set(which, *b);
            }
        };
        flag(
            exceptions,
            "implicitThrows",
            ExceptionChecks::IMPLICIT_THROWS,
        );
        flag(
            exceptions,
            "reportUncheckedExceptionDeadCatch",
            ExceptionChecks::REPORT_UNCHECKED_EXCEPTION_DEAD_CATCH,
        );
        flag(
            check,
            "missingCheckedExceptionInThrows",
            ExceptionChecks::MISSING_CHECKED_EXCEPTION_IN_THROWS,
        );
        flag(
            check,
            "tooWideThrowType",
            ExceptionChecks::TOO_WIDE_THROW_TYPE,
        );
        flag(
            check,
            "tooWideImplicitThrowType",
            ExceptionChecks::TOO_WIDE_IMPLICIT_THROW_TYPE,
        );
        flag(
            check,
            "throwTypeCovariance",
            ExceptionChecks::THROW_TYPE_COVARIANCE,
        );
        flag(
            Some(params),
            "checkTooWideThrowTypesInProtectedAndPublicMethods",
            ExceptionChecks::CHECK_TOO_WIDE_THROW_TYPES_IN_PROTECTED_AND_PUBLIC_METHODS,
        );
        let list = |k: &str, default: Vec<String>| match exceptions
            .and_then(|e| e.get_str(k))
            .and_then(Value::as_array)
        {
            Some(a) => a
                .values()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            None => default,
        };
        Exceptions {
            checks,
            unchecked_exception_regexes: list(
                "uncheckedExceptionRegexes",
                d.unchecked_exception_regexes,
            ),
            unchecked_exception_classes: list(
                "uncheckedExceptionClasses",
                d.unchecked_exception_classes,
            ),
            checked_exception_regexes: list("checkedExceptionRegexes", d.checked_exception_regexes),
            checked_exception_classes: list("checkedExceptionClasses", d.checked_exception_classes),
        }
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Default)]
    pub struct TooWide: u8 {
        const REPORT_BOOL = 1;
        const REPORT_NESTED = 1 << 1;
        const PARAM_OUT_IN_PROTECTED_AND_PUBLIC_METHODS = 1 << 2;
    }
}

pub(crate) const RUNTIME_PHP_VERSION: PhpVersion = PhpVersion::from_id(80511);

fn strings<'a>(params: &'a Array, key: &str) -> Vec<&'a str> {
    params
        .get_str(key)
        .and_then(Value::as_array)
        .map(|a| a.values().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// `parameters.phpat`, which Pun also gives the `tests` list phpstan.neon's `phpat.test` services name.
fn phpat_config(params: &Array) -> Option<crate::phpat::Config> {
    let p = params.get_str("phpat").and_then(Value::as_array)?;
    let flag = |k: &str| p.get_str(k) == Some(&Value::Bool(true));
    Some(crate::phpat::Config {
        tests: strings(p, "tests")
            .into_iter()
            .map(str::to_string)
            .collect(),
        ignore_doc_comments: flag("ignore_doc_comments"),
        ignore_built_in_classes: flag("ignore_built_in_classes"),
        show_rule_names: flag("show_rule_names"),
    })
}

impl Project {
    /// Reads the phpat tests `phpat_config` names, once the analysed and scanned files are final. A test Pun can't
    /// read statically is an error that names it.
    pub fn load_phpat(&mut self) -> Result<(), String> {
        let Some(config) = &self.phpat_config else {
            return Ok(());
        };
        let rules = crate::phpat::Rules::load(self, config)?;
        self.phpat = Some(std::sync::Arc::new(rules));
        Ok(())
    }

    /// Prepares the php-rules workers when `php_rules_config` names neon files, with `paths` (or `php_rules_paths`,
    /// which the workers then keep to) as PHPStan's analysed paths. They start when a file needs them; a worker that can't start fails the run ([`crate::PhpRules::failure`]).
    /// `cache` is the result cache's file and the caller's part of its key ([`crate::PhpRules::start`]).
    pub fn start_php_rules(&mut self, paths: &[PathBuf], cache: Option<(PathBuf, &str)>) {
        if self.php_rules_config.is_empty() {
            return;
        }
        let neon: Vec<PathBuf> = self.php_rules_config.iter().map(PathBuf::from).collect();
        let workers = (self.php_rules_workers).unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map_or(1, std::num::NonZeroUsize::get)
                .min(8)
        });
        let root = &self.current_working_directory;
        let (paths, only) = if self.php_rules_paths.is_empty() {
            (paths, false)
        } else {
            (&self.php_rules_paths[..], true)
        };
        let native = &self.php_rules_native;
        let rules = crate::PhpRules::start(root, &neon, native, paths, only, workers, cache);
        self.php_rules = Some(std::sync::Arc::new(rules));
    }

    pub fn from_config(root: &Path, params: &Array) -> Project {
        let extensions: Vec<String> = strings(params, "fileExtensions")
            .into_iter()
            .map(str::to_string)
            .collect();
        let excludes = params.get_str("excludePaths").and_then(Value::as_array);
        let group = |k: &str| {
            excludes
                .and_then(|e| e.get_str(k))
                .and_then(Value::as_array)
                .map(|a| a.values().filter_map(Value::as_str).collect::<Vec<_>>())
                .unwrap_or_default()
        };
        let both = FileExcluder::new(group("analyseAndScan"));
        let analyse_only = FileExcluder::new(group("analyse"));
        let mut analysed = Vec::new();
        let mut scanned = Vec::new();
        let paths: Vec<String> = strings(params, "paths")
            .into_iter()
            .map(|p| absolute(Path::new(p)))
            .collect();
        for path in strings(params, "paths") {
            let p = Path::new(path);
            let found = if p.is_file() {
                vec![p.to_path_buf()]
            } else {
                php_files(p, &extensions)
            };
            for f in found {
                let name = display(&f);
                if both.is_excluded(&name) {
                    continue;
                }
                if analyse_only.is_excluded(&name) {
                    scanned.push(f);
                } else {
                    analysed.push(f);
                }
            }
        }
        // PHPStan runs the bootstrap files, so their symbols exist; Pun reads them like the scanned files
        for name in ["scanFiles", "bootstrapFiles"] {
            scanned.extend(strings(params, name).into_iter().map(PathBuf::from));
        }
        for dir in strings(params, "scanDirectories") {
            scanned.extend(php_files(Path::new(dir), &extensions));
        }
        let (composer, autoloaded) = composer_files(root, &extensions);
        scanned.extend(composer);
        scanned.retain(|f| !both.is_excluded(&display(f)));
        let php_version = params
            .get_str("phpVersion")
            .and_then(|v| match v {
                Value::Array(a) => a.get_str("min"),
                v => Some(v),
            })
            .and_then(|v| match v {
                Value::Int(i) => u32::try_from(*i).ok().map(PhpVersion::from_id),
                _ => None,
            })
            .unwrap_or(RUNTIME_PHP_VERSION);
        let flag = |k: &str| params.get_str(k) == Some(&Value::Bool(true));
        let optional_flag = |k: &str| match params.get_str(k) {
            Some(Value::Bool(value)) => Some(*value),
            _ => None,
        };
        let toggle = |k: &str| {
            params
                .get_str("featureToggles")
                .and_then(Value::as_array)
                .and_then(|t| t.get_str(k))
                == Some(&Value::Bool(true))
        };
        let report_unmatched_ignores = flag("reportUnmatchedIgnoredErrors");
        let empty = Array::new();
        let ignore_errors = params
            .get_str("ignoreErrors")
            .and_then(Value::as_array)
            .unwrap_or(&empty);
        let level = match params.get_str("level") {
            Some(Value::Int(i)) => u8::try_from(*i).unwrap_or(10).min(10),
            Some(Value::Str(s)) if s == "max" => 10,
            _ => 0,
        };
        let mut options = ProjectOptions::empty();
        options.set(
            ProjectOptions::CHECK_MAGIC_DIR_IN_INCLUDE,
            toggle("magicDirInInclude"),
        );
        options.set(
            ProjectOptions::USE_PATH_CONSTANTS_AS_CONSTANT_STRING,
            flag("usePathConstantsAsConstantString"),
        );
        options.set(
            ProjectOptions::REPORT_IGNORES_WITHOUT_COMMENTS,
            flag("reportIgnoresWithoutComments"),
        );
        options.set(
            ProjectOptions::REPORT_UNMATCHED_IGNORES,
            report_unmatched_ignores,
        );
        options.set(
            ProjectOptions::CHECK_CLASS_CASE_SENSITIVITY,
            flag("checkClassCaseSensitivity"),
        );
        options.set(
            ProjectOptions::CHECK_INTERNAL_CLASS_CASE_SENSITIVITY,
            flag("checkInternalClassCaseSensitivity"),
        );
        options.set(
            ProjectOptions::CHECK_IMPORTED_CLASS_NAME_CASE,
            toggle("checkImportedClassNameCase"),
        );
        options.set(
            ProjectOptions::CHECK_FUNCTION_NAME_CASE,
            flag("checkFunctionNameCase"),
        );
        options.set(
            ProjectOptions::CLI_ARGUMENTS_VARIABLES_REGISTERED,
            flag("cliArgumentsVariablesRegistered"),
        );
        options.set(
            ProjectOptions::CHECK_MAYBE_UNDEFINED_VARIABLES,
            flag("checkMaybeUndefinedVariables"),
        );
        options.set(
            ProjectOptions::POLLUTE_SCOPE_WITH_LOOP_INITIAL_ASSIGNMENTS,
            !matches!(
                params.get_str("polluteScopeWithLoopInitialAssignments"),
                Some(Value::Bool(false))
            ),
        );
        options.set(
            ProjectOptions::POLLUTE_SCOPE_WITH_ALWAYS_ITERABLE_FOREACH,
            !matches!(
                params.get_str("polluteScopeWithAlwaysIterableForeach"),
                Some(Value::Bool(false))
            ),
        );
        options.set(
            ProjectOptions::POLLUTE_SCOPE_WITH_BLOCK,
            !matches!(
                params.get_str("polluteScopeWithBlock"),
                Some(Value::Bool(false))
            ),
        );
        options.set(
            ProjectOptions::CHECK_GENERIC_ITERABLE_CLASSES,
            toggle("checkGenericIterableClasses"),
        );
        options.set(ProjectOptions::INTERNAL_TAG, toggle("internalTag"));
        options.set(
            ProjectOptions::NEW_STATIC_IN_ABSTRACT_CLASS_STATIC_METHOD,
            toggle("newStaticInAbstractClassStaticMethod"),
        );
        options.set(ProjectOptions::BLEEDING_EDGE, toggle("bleedingEdge"));
        options.set(ProjectOptions::UNUSED_LABEL, toggle("unusedLabel"));
        options.set(
            ProjectOptions::SWITCH_CONDITION,
            toggle("switchConditionAlwaysFalse"),
        );
        options.set(
            ProjectOptions::SORT_WITHOUT_EFFECT,
            toggle("sortWithoutEffect"),
        );
        options.set(
            ProjectOptions::CHECK_NON_STRINGABLE_DYNAMIC_ACCESS,
            toggle("checkNonStringableDynamicAccess"),
        );
        options.set(
            ProjectOptions::REPORT_METHOD_PURITY_OVERRIDE,
            toggle("reportMethodPurityOverride"),
        );
        options.set(
            ProjectOptions::ASSIGN_TO_BY_REF_FOREACH_EXPR,
            toggle("assignToByRefForeachExpr"),
        );
        options.set(
            ProjectOptions::CHECK_DYNAMIC_CONSTANT_NAME_VALUES,
            toggle("checkDynamicConstantNameValues"),
        );
        options.set(
            ProjectOptions::CHECK_DATE_INTERVAL_CONSTRUCTOR,
            toggle("checkDateIntervalConstructor"),
        );
        options.set(
            ProjectOptions::CHECK_PARAMETER_CASTABLE_TO_NUMBER_FUNCTIONS,
            toggle("checkParameterCastableToNumberFunctions"),
        );
        options.set(
            ProjectOptions::CHECK_PRINTF_PARAMETER_TYPES,
            toggle("checkPrintfParameterTypes"),
        );
        options.set(
            ProjectOptions::CHECK_STRICT_PRINTF_PLACEHOLDER_TYPES,
            flag("checkStrictPrintfPlaceholderTypes"),
        );
        options.set(
            ProjectOptions::FINITE_TYPES_IN_HAYSTACK,
            toggle("finiteTypesInHaystack"),
        );
        options.set(ProjectOptions::UNUSED_VARIABLE, toggle("unusedVariable"));
        options.set(
            ProjectOptions::UNUSED_PARAMETERS,
            toggle("unusedParameters"),
        );
        options.set(ProjectOptions::NEW_ON_NON_OBJECT, toggle("newOnNonObject"));
        options.set(
            ProjectOptions::UNRESOLVED_TEMPLATE_ARGUMENTS,
            toggle("unresolvedTemplateArguments"),
        );
        options.set(
            ProjectOptions::CHECK_UNINITIALIZED_PROPERTIES,
            flag("checkUninitializedProperties"),
        );
        options.set(
            ProjectOptions::CHECK_MISSING_CALLABLE_SIGNATURE,
            flag("checkMissingCallableSignature"),
        );
        options.set(
            ProjectOptions::DEPRECATION_RULES,
            flag("deprecationRulesInstalled"),
        );
        options.set(
            ProjectOptions::REPORT_WRONG_PHP_DOC_TYPE_IN_VAR_TAG,
            flag("reportWrongPhpDocTypeInVarTag"),
        );
        options.set(
            ProjectOptions::REPORT_MAYBES_IN_METHOD_SIGNATURES,
            flag("reportMaybesInMethodSignatures"),
        );
        options.set(
            ProjectOptions::REPORT_STATIC_METHOD_SIGNATURES,
            flag("reportStaticMethodSignatures"),
        );
        options.set(
            ProjectOptions::PRECISE_UNUSED_PARAMETER_LINE,
            toggle("reportPreciseLineForUnusedFunctionParameter"),
        );
        let mut project = Project {
            options,
            analysed,
            scanned,
            autoloaded,
            php_version,
            current_working_directory: root.to_path_buf(),
            include_paths: vec![PathBuf::from(".")],
            ignore_errors: IgnoredErrors::new(ignore_errors, report_unmatched_ignores)
                .with_config_files(params)
                // The configured paths are the analysed ones, so the judge never asks about excludePaths.
                .with_analysed_paths(&paths, &paths, None),
            level,
            check_function_argument_types: optional_flag("checkFunctionArgumentTypes"),
            check_arguments_passed_by_reference: optional_flag("checkArgumentsPassedByReference"),
            check_this_only: optional_flag("checkThisOnly"),
            check_advanced_isset: optional_flag("checkAdvancedIsset"),
            check_explicit_mixed: optional_flag("checkExplicitMixed"),
            check_implicit_mixed: optional_flag("checkImplicitMixed"),
            treat_php_doc_types_as_certain: optional_flag("treatPhpDocTypesAsCertain"),
            only_files: strings(params, "paths")
                .iter()
                .all(|p| Path::new(p).is_file()),
            dynamic_constant_types: params
                .get_str("dynamicConstantNames")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter(|(k, _)| matches!(k, Key::Str(_)))
                        .filter_map(|(k, v)| Some((k.as_string(), v.as_str()?.to_string())))
                        .collect()
                })
                .unwrap_or_default(),
            strict_rules: StrictRules::from_params(params),
            exceptions: Exceptions::from_params(params),
            too_wide: {
                let mut too_wide = TooWide::empty();
                too_wide.set(TooWide::REPORT_BOOL, toggle("reportTooWideBool"));
                too_wide.set(TooWide::REPORT_NESTED, toggle("reportNestedTooWideType"));
                too_wide.set(
                    TooWide::PARAM_OUT_IN_PROTECTED_AND_PUBLIC_METHODS,
                    flag("checkTooWideParameterOutInProtectedAndPublicMethods"),
                );
                too_wide
            },
            relative_path: RelativePath::of(&paths),
            larastan: crate::Larastan::from_params(root, params),
            carbon: crate::Carbon::from_params(root, params),
            phpat_config: phpat_config(params),
            phpat: None,
            php_rules_config: strings(params, "punPhpRules")
                .into_iter()
                .map(str::to_string)
                .collect(),
            php_rules_workers: match params.get_str("punPhpRulesWorkers") {
                Some(Value::Int(n)) => usize::try_from(*n).ok(),
                _ => None,
            },
            php_rules_paths: strings(params, "punPhpRulesPaths")
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            php_rules_native: strings(params, "punPhpRulesNative")
                .into_iter()
                .map(str::to_string)
                .collect(),
            php_rules: None,
            type_aliases: params
                .get_str("typeAliases")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|(k, v)| Some((k.as_string(), v.as_str()?.to_string())))
                        .collect()
                })
                .unwrap_or_default(),
            stubs: strings(params, "stubFiles")
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            reported: None,
            extension_identifiers: std::collections::HashSet::new(),
            method_rules: MethodRules::default(),
        };
        let stubs: Vec<_> = (project.extensions())
            .flat_map(|e| e.stub_files(root))
            .collect();
        project.stubs.extend(stubs);
        project
    }

    pub(crate) fn extensions(&self) -> impl Iterator<Item = &dyn AnalyseExtension> + Clone {
        (self.larastan.iter().map(|l| l as &dyn AnalyseExtension))
            .chain(self.carbon.iter().map(|c| c as &dyn AnalyseExtension))
    }

    pub fn new(analysed: Vec<PathBuf>, scanned: Vec<PathBuf>) -> Project {
        let paths: Vec<String> = analysed.iter().map(|p| absolute(p)).collect();
        let mut options = ProjectOptions::empty();
        options.set(ProjectOptions::CHECK_MAGIC_DIR_IN_INCLUDE, false);
        options.set(ProjectOptions::USE_PATH_CONSTANTS_AS_CONSTANT_STRING, false);
        options.set(ProjectOptions::REPORT_UNMATCHED_IGNORES, true);
        options.set(ProjectOptions::REPORT_IGNORES_WITHOUT_COMMENTS, false);
        options.set(ProjectOptions::CHECK_CLASS_CASE_SENSITIVITY, false);
        options.set(ProjectOptions::CHECK_INTERNAL_CLASS_CASE_SENSITIVITY, false);
        options.set(ProjectOptions::CHECK_IMPORTED_CLASS_NAME_CASE, false);
        options.set(ProjectOptions::CHECK_FUNCTION_NAME_CASE, false);
        options.set(ProjectOptions::CLI_ARGUMENTS_VARIABLES_REGISTERED, true);
        options.set(ProjectOptions::CHECK_MAYBE_UNDEFINED_VARIABLES, false);
        options.set(
            ProjectOptions::POLLUTE_SCOPE_WITH_LOOP_INITIAL_ASSIGNMENTS,
            true,
        );
        options.set(
            ProjectOptions::POLLUTE_SCOPE_WITH_ALWAYS_ITERABLE_FOREACH,
            true,
        );
        options.set(ProjectOptions::POLLUTE_SCOPE_WITH_BLOCK, true);
        options.set(ProjectOptions::CHECK_GENERIC_ITERABLE_CLASSES, false);
        options.set(ProjectOptions::INTERNAL_TAG, false);
        options.set(
            ProjectOptions::NEW_STATIC_IN_ABSTRACT_CLASS_STATIC_METHOD,
            false,
        );
        options.set(ProjectOptions::BLEEDING_EDGE, false);
        options.set(ProjectOptions::UNUSED_LABEL, false);
        options.set(ProjectOptions::SWITCH_CONDITION, false);
        options.set(ProjectOptions::SORT_WITHOUT_EFFECT, false);
        options.set(ProjectOptions::CHECK_NON_STRINGABLE_DYNAMIC_ACCESS, false);
        options.set(ProjectOptions::REPORT_METHOD_PURITY_OVERRIDE, false);
        options.set(ProjectOptions::ASSIGN_TO_BY_REF_FOREACH_EXPR, false);
        options.set(ProjectOptions::CHECK_DYNAMIC_CONSTANT_NAME_VALUES, false);
        options.set(ProjectOptions::CHECK_DATE_INTERVAL_CONSTRUCTOR, false);
        options.set(
            ProjectOptions::CHECK_PARAMETER_CASTABLE_TO_NUMBER_FUNCTIONS,
            false,
        );
        options.set(ProjectOptions::CHECK_PRINTF_PARAMETER_TYPES, false);
        options.set(ProjectOptions::CHECK_STRICT_PRINTF_PLACEHOLDER_TYPES, false);
        options.set(ProjectOptions::FINITE_TYPES_IN_HAYSTACK, false);
        options.set(ProjectOptions::UNUSED_VARIABLE, false);
        options.set(ProjectOptions::UNUSED_PARAMETERS, false);
        options.set(ProjectOptions::NEW_ON_NON_OBJECT, false);
        options.set(ProjectOptions::UNRESOLVED_TEMPLATE_ARGUMENTS, false);
        options.set(ProjectOptions::CHECK_UNINITIALIZED_PROPERTIES, false);
        options.set(ProjectOptions::CHECK_MISSING_CALLABLE_SIGNATURE, false);
        options.set(ProjectOptions::DEPRECATION_RULES, false);
        options.set(ProjectOptions::PRECISE_UNUSED_PARAMETER_LINE, false);
        Project {
            options,
            relative_path: RelativePath::of(&paths),
            analysed,
            scanned,
            autoloaded: Vec::new(),
            php_version: RUNTIME_PHP_VERSION,
            current_working_directory: std::env::current_dir().unwrap_or_default(),
            include_paths: vec![PathBuf::from(".")],
            ignore_errors: IgnoredErrors::new(&Array::new(), true),
            level: 0,
            check_function_argument_types: None,
            check_arguments_passed_by_reference: None,
            check_this_only: None,
            check_advanced_isset: None,
            check_explicit_mixed: None,
            check_implicit_mixed: None,
            treat_php_doc_types_as_certain: None,
            only_files: true,
            dynamic_constant_types: Vec::new(),
            strict_rules: StrictRules::default(),
            exceptions: Exceptions::default(),
            too_wide: TooWide::default(),
            larastan: None,
            carbon: None,
            phpat_config: None,
            phpat: None,
            php_rules_config: Vec::new(),
            php_rules_workers: None,
            php_rules_paths: Vec::new(),
            php_rules_native: Vec::new(),
            php_rules: None,
            type_aliases: Vec::new(),
            stubs: Vec::new(),
            reported: None,
            extension_identifiers: std::collections::HashSet::new(),
            method_rules: MethodRules::default(),
        }
    }
}

pub struct Autoloaded {
    pub path: PathBuf,
    pub class: Vec<u8>,
    pub psr0: bool,
}

enum Source {
    Psr4(String, PathBuf),
    Psr0(PathBuf),
    Classmap(PathBuf),
    File(PathBuf),
}

fn composer_files(root: &Path, extensions: &[String]) -> (Vec<PathBuf>, Vec<Autoloaded>) {
    let read =
        |p: PathBuf| -> Option<Json> { serde_json::from_slice(&std::fs::read(p).ok()?).ok() };
    let mut sources = Vec::new();
    let mut add = |base: &Path, autoload: &Json| {
        for kind in ["psr-4", "psr-0"] {
            for (prefix, dirs) in autoload[kind].as_object().into_iter().flatten() {
                let dirs = match dirs {
                    Json::Array(a) => a.iter().filter_map(Json::as_str).collect(),
                    d => d.as_str().into_iter().collect::<Vec<_>>(),
                };
                for d in dirs {
                    let dir = base.join(d);
                    sources.push(if kind == "psr-4" {
                        Source::Psr4(prefix.clone(), dir)
                    } else {
                        Source::Psr0(dir)
                    });
                }
            }
        }
        for entry in autoload["classmap"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
        {
            sources.push(Source::Classmap(base.join(entry)));
        }
        for f in autoload["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
        {
            sources.push(Source::File(base.join(f)));
        }
    };
    if let Some(composer) = read(root.join("composer.json")) {
        add(root, &composer["autoload"]);
        add(root, &composer["autoload-dev"]);
    }
    let installed_dir = root.join("vendor/composer");
    if let Some(installed) = read(installed_dir.join("installed.json")) {
        let packages = installed.get("packages").unwrap_or(&installed);
        for p in packages.as_array().into_iter().flatten() {
            if let Some(path) = p["install-path"].as_str() {
                add(&installed_dir.join(path), &p["autoload"]);
            }
        }
    }
    let found: Vec<(Vec<PathBuf>, Vec<Autoloaded>)> = sources
        .into_par_iter()
        .map(|source| match source {
            Source::Psr4(prefix, d) => (
                Vec::new(),
                by_class(&d, prefix.as_bytes(), false, extensions),
            ),
            Source::Psr0(d) => (Vec::new(), by_class(&d, b"", true, extensions)),
            Source::Classmap(p) if p.is_dir() => (php_files(&p, extensions), Vec::new()),
            Source::File(p) => {
                let mut files = vec![p];
                required_files(0, &mut files);
                (files, Vec::new())
            }
            Source::Classmap(p) => (vec![p], Vec::new()),
        })
        .collect();
    let mut out = Vec::new();
    let mut autoloaded = Vec::new();
    for (files, classes) in found {
        out.extend(files);
        autoloaded.extend(classes);
    }
    for class in ["InstalledVersions.php", "ClassLoader.php"] {
        let path = installed_dir.join(class);
        if path.is_file() {
            out.push(path);
        }
    }
    (out, autoloaded)
}

/// Composer's `files` autoloads run when PHPStan boots, so a file one of them `require`s or `include`s defines its
/// functions too. Follows the paths PHP resolves without running anything: `__DIR__ . '/functions.php'`, and
/// `__DIR__ . "/{$file}"` for each `.php` string literal in the file, as pest-plugin-laravel's Autoload.php requires a
/// literal list of files in a loop. `files[at]` is the file read; each file it loads is appended and followed in turn.
fn required_files(at: usize, files: &mut Vec<PathBuf>) {
    use pun_tokens::kind::Kind::{
        T_COMMENT, T_CONSTANT_ENCAPSED_STRING, T_CURLY_OPEN, T_DIR, T_DOC_COMMENT,
        T_ENCAPSED_AND_WHITESPACE, T_INCLUDE, T_INCLUDE_ONCE, T_REQUIRE, T_REQUIRE_ONCE,
        T_VARIABLE, T_WHITESPACE,
    };
    let path = files[at].clone();
    let (Ok(code), Some(dir)) = (std::fs::read(&path), path.parent()) else {
        return;
    };
    let tokens: Vec<_> = (pun_tokens::tokenize(&code).into_iter())
        .filter(|t| !matches!(t.kind, Some(T_WHITESPACE | T_COMMENT | T_DOC_COMMENT)))
        .collect();
    let literal = |t: &pun_tokens::RawToken| -> Option<String> {
        let text = std::str::from_utf8(t.text).ok()?;
        let inner = text.strip_prefix('\'').and_then(|t| t.strip_suffix('\''));
        let inner = inner.or_else(|| text.strip_prefix('"').and_then(|t| t.strip_suffix('"')));
        (t.kind == Some(T_CONSTANT_ENCAPSED_STRING))
            .then_some(inner?)
            .filter(|s| !s.contains(['\\', '$']))
            .map(str::to_owned)
    };
    let names: Vec<String> = (tokens.iter().filter_map(literal))
        .filter(|s| s.ends_with(".php"))
        .collect();
    let mut found = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if !matches!(
            token.kind,
            Some(T_REQUIRE | T_REQUIRE_ONCE | T_INCLUDE | T_INCLUDE_ONCE)
        ) {
            continue;
        }
        let mut rest = tokens[i + 1..]
            .iter()
            .skip_while(|t| t.kind.is_none() && t.text == b"(");
        if rest.next().is_none_or(|t| t.kind != Some(T_DIR))
            || rest.next().is_none_or(|t| t.text != b".")
        {
            continue;
        }
        let Some(next) = rest.next() else { continue };
        if let Some(tail) = literal(next) {
            found.push(format!("{}{tail}", dir.display()));
            continue;
        }
        if next.text != b"\"" {
            continue;
        }
        // `"<before>{$file}<after>"` or `"<before>$file<after>"`
        let (mut before, mut after, mut variables) = (String::new(), String::new(), 0);
        for t in rest.by_ref().take_while(|t| t.text != b"\"") {
            match t.kind {
                Some(T_ENCAPSED_AND_WHITESPACE) if variables == 0 => {
                    before.push_str(&String::from_utf8_lossy(t.text));
                }
                Some(T_ENCAPSED_AND_WHITESPACE) => after.push_str(&String::from_utf8_lossy(t.text)),
                Some(T_VARIABLE) => variables += 1,
                Some(T_CURLY_OPEN) | None if matches!(t.text, b"{" | b"}") => {}
                _ => variables = 2,
            }
        }
        if variables == 1 {
            found.extend(
                names
                    .iter()
                    .map(|name| format!("{}{before}{name}{after}", dir.display())),
            );
        }
    }
    for file in found.into_iter().map(PathBuf::from) {
        if file.is_file() && !files.contains(&file) {
            files.push(file);
            required_files(files.len() - 1, files);
        }
    }
}

fn by_class(dir: &Path, prefix: &[u8], psr0: bool, extensions: &[String]) -> Vec<Autoloaded> {
    php_files(dir, extensions)
        .into_iter()
        .map(|path| {
            let mut class = prefix.to_ascii_lowercase();
            let rel = path.strip_prefix(dir).unwrap_or(&path).with_extension("");
            for (i, part) in rel.iter().enumerate() {
                if i > 0 {
                    class.push(b'\\');
                }
                class.extend(part.as_encoded_bytes().iter().map(u8::to_ascii_lowercase));
            }
            Autoloaded { path, class, psr0 }
        })
        .collect()
}
