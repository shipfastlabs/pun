use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::named_argument::deferred_local_ignores as local_ignores;
use mago_allocator::LocalArena;
use mago_codex::metadata::CodebaseMetadata;
use mago_codex::symbol::{SymbolIdentifier, SymbolKind};
use mago_database::file::{File, FileId};
use mago_names::ResolvedNames;
use mago_span::Span;
use mago_syntax::cst::Program;
use mago_syntax::walker::MutWalker;
use pun_phpstan::comments::lines_to_ignore;
use pun_phpstan::ignore::{
    Error, fix_all_line_errors, ignores_without_comments, parse_errors, unmatched_line_errors,
};

use super::{
    Checker, anonymous, arrays, assign, class_names, infer::infer, property_access, reflection,
    signatures, syntax,
};
use crate::Project;
use crate::codebase::{Sources, parse};
use crate::relative_path::RelativePath;

pub(crate) struct Natives {
    pub(super) classes: super::overriding::NativeClasses,
    pub(super) throws: super::types::throw_points::ThrowData,
    pub(super) curl_options: Vec<(u32, HashMap<i64, String>)>,
}

impl Natives {
    /// Loaded on its own thread while the codebase is built: it needs only the PHP version.
    pub(crate) fn load(php_version: u32) -> Natives {
        Natives {
            classes: super::overriding::natives(php_version),
            throws: super::types::throw_points::ThrowData::load(),
            curl_options: super::selector::curl_options(),
        }
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) struct Switches: u64 {
        const REPORT_IGNORES_WITHOUT_COMMENTS = 1 << 0;
        const REPORT_UNMATCHED_IGNORES = 1 << 1;
        const CHECK_CLASS_CASE_SENSITIVITY = 1 << 2;
        const CHECK_INTERNAL_CLASS_CASE_SENSITIVITY = 1 << 3;
        const CHECK_IMPORTED_CLASS_NAME_CASE = 1 << 4;
        const CHECK_FUNCTION_NAME_CASE = 1 << 5;
        const CHECK_ARGUMENT_TYPES = 1 << 6;
        const CHECK_ARGUMENTS_PASSED_BY_REFERENCE = 1 << 7;
        const CHECK_THIS_ONLY = 1 << 8;
        const CHECK_ADVANCED_ISSET = 1 << 9;
        const CHECK_EXPLICIT_MIXED = 1 << 10;
        const CHECK_IMPLICIT_MIXED = 1 << 11;
        const TREAT_PHP_DOC_TYPES_AS_CERTAIN = 1 << 12;
        const GENERIC_ITERABLE_CLASSES = 1 << 13;
        const NEW_STATIC_IN_ABSTRACT_CLASS_STATIC_METHOD = 1 << 14;
        const INTERNAL_TAG = 1 << 15;
        const PRECISE_UNUSED_PARAMETER_LINE = 1 << 16;
        const INTERNAL_MEMBERS = 1 << 17;
        const DEPRECATIONS = 1 << 18;
        const BLEEDING_EDGE = 1 << 19;
        const UNUSED_LABEL = 1 << 20;
        const SWITCH_CONDITION = 1 << 21;
        const SORT_WITHOUT_EFFECT = 1 << 22;
        const NON_STRINGABLE_NAMES = 1 << 23;
        const CLI_ARGUMENTS_VARIABLES_REGISTERED = 1 << 24;
        const CHECK_MAYBE_UNDEFINED_VARIABLES = 1 << 25;
        const POLLUTE_SCOPE_WITH_LOOP_INITIAL_ASSIGNMENTS = 1 << 26;
        const POLLUTE_SCOPE_WITH_ALWAYS_ITERABLE_FOREACH = 1 << 27;
        const POLLUTE_SCOPE_WITH_BLOCK = 1 << 28;
        const METHOD_PURITY_OVERRIDE = 1 << 29;
        const UNINITIALIZED_PROPERTIES = 1 << 30;
        const MISSING_CALLABLE_SIGNATURE = 1 << 31;
        const TEMPLATE_ARGUMENTS = 1 << 32;
        const WRONG_PHP_DOC_TYPE_IN_VAR_TAG = 1 << 33;
        const MAYBES_IN_METHOD_SIGNATURES = 1 << 34;
        const STATIC_METHOD_SIGNATURES = 1 << 35;
    }
}

pub(crate) struct Checks {
    switches: Switches,
    natives: Natives,
    patched_docs: HashMap<Span, Span>,
    classes: super::declared::ClassCache,
    anonymous_names: super::anonymous::Cache,
    trait_properties: super::readonly_init::Cache,
    php_version: u32,
    pub(super) level: u8,
    relative_path: RelativePath,
    larastan: Option<super::Larastan>,
    mockery: Option<crate::Mockery>,
    pest: Option<crate::Pest>,
    type_coverage: Option<super::TypeCoverage>,
    internal: Option<super::internal::Names>,
    pub(super) type_aliases: Vec<(String, String)>,
    reported: Option<crate::project::Reported>,
    extension_identifiers: HashSet<String>,
    disallowed: Option<super::Disallowed>,
    method_rules: crate::MethodRules,
    pub(super) strict: crate::StrictRules,
    pub(super) exceptions: crate::Exceptions,
    too_wide: crate::TooWide,
    include_file: super::include_file::Settings,
    toggled: Toggled,
    pub(super) function_likes: HashMap<FileId, Vec<(Span, SymbolIdentifier)>>,
}

#[derive(Default)]
pub(super) struct Toggled {
    pub(super) on: Toggles,
    pub(super) constant_types: Option<Vec<(String, String)>>,
    pub(super) printf_types: Option<bool>,
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Default)]
    pub(super) struct Toggles: u8 {
        const BY_REF_FOREACH = 1;
        const DATE_INTERVAL = 1 << 1;
        const CASTABLE_TO_NUMBER = 1 << 2;
        const FINITE_HAYSTACK = 1 << 3;
        const UNUSED_VARIABLE = 1 << 4;
        const UNUSED_PARAMETERS = 1 << 5;
        const UNUSED_FLOW = 1 << 6;
        const NEW_ON_NON_OBJECT = 1 << 7;
    }
}

fn patched_docs(codebase: &CodebaseMetadata) -> HashMap<Span, Span> {
    let functions = codebase
        .patch_function_likes
        .iter()
        .filter(|(_, p)| p.has_docblock)
        .filter_map(|(key, patch)| Some((codebase.function_likes.get(key)?.span, patch.span)));
    let classes = codebase
        .patch_class_likes
        .iter()
        .filter(|(_, p)| p.has_docblock)
        .filter_map(|(key, patch)| Some((codebase.class_likes.get(key)?.span, patch.span)));
    functions
        .chain(classes)
        .filter(|(original, patch)| original != patch)
        .collect()
}

impl Checks {
    /// Drops the errors of rules that are off. Pun's own rules always give an identifier, so an error without one
    /// comes from an extension that is on (type-coverage's measure mode), as one with an identifier its config entry
    /// names does.
    pub(crate) fn keep_reported(&self, errors: &mut Vec<Error>) {
        if let Some(reported) = &self.reported {
            errors.retain(|e| {
                e.identifier.as_deref().is_none_or(|id| {
                    id == "phpstan.parse"
                        || id.starts_with("larastan.collected.")
                        || id == super::type_coverage::COLLECTED
                        || self.extension_identifiers.contains(id)
                        || reported(id)
                })
            });
        }
    }

    fn extensions(&self) -> impl Iterator<Item = &dyn crate::AnalyseExtension> {
        (self
            .larastan
            .iter()
            .map(|l| l as &dyn crate::AnalyseExtension))
        .chain(
            self.mockery
                .iter()
                .map(|m| m as &dyn crate::AnalyseExtension),
        )
        .chain(self.pest.iter().map(|p| p as &dyn crate::AnalyseExtension))
    }
}

impl Checks {
    pub(crate) fn of(project: &Project, codebase: &CodebaseMetadata, natives: Natives) -> Checks {
        let internal_members = project.level >= 2
            && project
                .options
                .contains(crate::ProjectOptions::INTERNAL_TAG);
        let mut function_likes: HashMap<FileId, Vec<(Span, SymbolIdentifier)>> = HashMap::new();
        for (id, f) in codebase.function_likes.iter() {
            function_likes
                .entry(f.span.file_id)
                .or_default()
                .push((f.span, *id));
        }
        let mut larastan = project.larastan.clone();
        if let Some(l) = &mut larastan {
            crate::AnalyseExtension::load(l, project, codebase);
        }
        let mut pest = project.pest.clone();
        if let Some(p) = &mut pest {
            crate::AnalyseExtension::load(p, project, codebase);
        }
        let mut switches = Switches::empty();
        switches.set(
            Switches::CHECK_CLASS_CASE_SENSITIVITY,
            project
                .options
                .contains(crate::ProjectOptions::CHECK_CLASS_CASE_SENSITIVITY),
        );
        switches.set(
            Switches::CHECK_INTERNAL_CLASS_CASE_SENSITIVITY,
            project
                .options
                .contains(crate::ProjectOptions::CHECK_INTERNAL_CLASS_CASE_SENSITIVITY),
        );
        switches.set(
            Switches::CHECK_IMPORTED_CLASS_NAME_CASE,
            project
                .options
                .contains(crate::ProjectOptions::CHECK_IMPORTED_CLASS_NAME_CASE),
        );
        switches.set(
            Switches::CHECK_FUNCTION_NAME_CASE,
            project
                .options
                .contains(crate::ProjectOptions::CHECK_FUNCTION_NAME_CASE),
        );
        switches.set(
            Switches::CHECK_ARGUMENT_TYPES,
            project
                .check_function_argument_types
                .unwrap_or(project.level >= 5),
        );
        switches.set(
            Switches::CHECK_ARGUMENTS_PASSED_BY_REFERENCE,
            project
                .check_arguments_passed_by_reference
                .unwrap_or(project.level >= 5),
        );
        switches.set(
            Switches::CHECK_THIS_ONLY,
            project.check_this_only.unwrap_or(project.level < 2),
        );
        switches.set(
            Switches::CHECK_ADVANCED_ISSET,
            project.check_advanced_isset.unwrap_or(project.level >= 4),
        );
        switches.set(
            Switches::CHECK_EXPLICIT_MIXED,
            project.check_explicit_mixed.unwrap_or(project.level >= 9),
        );
        switches.set(
            Switches::CHECK_IMPLICIT_MIXED,
            project.check_implicit_mixed.unwrap_or(project.level >= 10),
        );
        switches.set(
            Switches::TREAT_PHP_DOC_TYPES_AS_CERTAIN,
            project.treat_php_doc_types_as_certain.unwrap_or(true),
        );
        switches.set(
            Switches::GENERIC_ITERABLE_CLASSES,
            project
                .options
                .contains(crate::ProjectOptions::CHECK_GENERIC_ITERABLE_CLASSES),
        );
        switches.set(
            Switches::NEW_STATIC_IN_ABSTRACT_CLASS_STATIC_METHOD,
            project
                .options
                .contains(crate::ProjectOptions::NEW_STATIC_IN_ABSTRACT_CLASS_STATIC_METHOD),
        );
        switches.set(
            Switches::REPORT_IGNORES_WITHOUT_COMMENTS,
            project
                .options
                .contains(crate::ProjectOptions::REPORT_IGNORES_WITHOUT_COMMENTS),
        );
        switches.set(
            Switches::REPORT_UNMATCHED_IGNORES,
            project
                .options
                .contains(crate::ProjectOptions::REPORT_UNMATCHED_IGNORES),
        );
        switches.set(Switches::INTERNAL_MEMBERS, internal_members);
        switches.set(
            Switches::INTERNAL_TAG,
            project
                .options
                .contains(crate::ProjectOptions::INTERNAL_TAG),
        );
        switches.set(
            Switches::PRECISE_UNUSED_PARAMETER_LINE,
            project
                .options
                .contains(crate::ProjectOptions::PRECISE_UNUSED_PARAMETER_LINE),
        );
        switches.set(
            Switches::DEPRECATIONS,
            project
                .options
                .contains(crate::ProjectOptions::DEPRECATION_RULES),
        );
        switches.set(
            Switches::BLEEDING_EDGE,
            project
                .options
                .contains(crate::ProjectOptions::BLEEDING_EDGE),
        );
        switches.set(
            Switches::UNUSED_LABEL,
            project.level >= 4
                && project
                    .options
                    .contains(crate::ProjectOptions::UNUSED_LABEL),
        );
        switches.set(
            Switches::SWITCH_CONDITION,
            project
                .options
                .contains(crate::ProjectOptions::SWITCH_CONDITION),
        );
        switches.set(
            Switches::SORT_WITHOUT_EFFECT,
            project
                .options
                .contains(crate::ProjectOptions::SORT_WITHOUT_EFFECT),
        );
        switches.set(
            Switches::NON_STRINGABLE_NAMES,
            project
                .options
                .contains(crate::ProjectOptions::CHECK_NON_STRINGABLE_DYNAMIC_ACCESS),
        );
        switches.set(
            Switches::CLI_ARGUMENTS_VARIABLES_REGISTERED,
            project
                .options
                .contains(crate::ProjectOptions::CLI_ARGUMENTS_VARIABLES_REGISTERED),
        );
        switches.set(
            Switches::CHECK_MAYBE_UNDEFINED_VARIABLES,
            project
                .options
                .contains(crate::ProjectOptions::CHECK_MAYBE_UNDEFINED_VARIABLES),
        );
        switches.set(
            Switches::POLLUTE_SCOPE_WITH_LOOP_INITIAL_ASSIGNMENTS,
            project
                .options
                .contains(crate::ProjectOptions::POLLUTE_SCOPE_WITH_LOOP_INITIAL_ASSIGNMENTS),
        );
        switches.set(
            Switches::POLLUTE_SCOPE_WITH_ALWAYS_ITERABLE_FOREACH,
            project
                .options
                .contains(crate::ProjectOptions::POLLUTE_SCOPE_WITH_ALWAYS_ITERABLE_FOREACH),
        );
        switches.set(
            Switches::POLLUTE_SCOPE_WITH_BLOCK,
            project
                .options
                .contains(crate::ProjectOptions::POLLUTE_SCOPE_WITH_BLOCK),
        );
        switches.set(
            Switches::METHOD_PURITY_OVERRIDE,
            project
                .options
                .contains(crate::ProjectOptions::REPORT_METHOD_PURITY_OVERRIDE),
        );
        switches.set(
            Switches::UNINITIALIZED_PROPERTIES,
            project
                .options
                .contains(crate::ProjectOptions::CHECK_UNINITIALIZED_PROPERTIES),
        );
        switches.set(
            Switches::MISSING_CALLABLE_SIGNATURE,
            project
                .options
                .contains(crate::ProjectOptions::CHECK_MISSING_CALLABLE_SIGNATURE),
        );
        switches.set(
            Switches::TEMPLATE_ARGUMENTS,
            project
                .options
                .contains(crate::ProjectOptions::UNRESOLVED_TEMPLATE_ARGUMENTS),
        );
        for (switch, option) in [
            (
                Switches::WRONG_PHP_DOC_TYPE_IN_VAR_TAG,
                crate::ProjectOptions::REPORT_WRONG_PHP_DOC_TYPE_IN_VAR_TAG,
            ),
            (
                Switches::MAYBES_IN_METHOD_SIGNATURES,
                crate::ProjectOptions::REPORT_MAYBES_IN_METHOD_SIGNATURES,
            ),
            (
                Switches::STATIC_METHOD_SIGNATURES,
                crate::ProjectOptions::REPORT_STATIC_METHOD_SIGNATURES,
            ),
        ] {
            switches.set(switch, project.options.contains(option));
        }
        Checks {
            switches,
            natives,
            level: project.level,
            php_version: project.php_version.id(),
            relative_path: project.relative_path.clone(),
            larastan,
            mockery: project.mockery.clone(),
            pest,
            type_coverage: project.type_coverage,
            internal: (internal_members
                || project
                    .options
                    .contains(crate::ProjectOptions::DEPRECATION_RULES))
            .then(|| {
                super::internal::Names::of(
                    codebase,
                    project.php_version.id(),
                    internal_members,
                    project
                        .options
                        .contains(crate::ProjectOptions::DEPRECATION_RULES),
                )
            }),
            type_aliases: project.type_aliases.clone(),
            reported: project.reported.clone(),
            extension_identifiers: project.extension_identifiers.clone(),
            disallowed: (!project.disallowed.is_empty()).then(|| project.disallowed.clone()),
            method_rules: project.method_rules,
            patched_docs: patched_docs(codebase),
            classes: Default::default(),
            anonymous_names: Default::default(),
            trait_properties: Default::default(),
            strict: project.strict_rules,
            exceptions: project.exceptions.clone(),
            too_wide: project.too_wide,
            include_file: super::include_file::Settings {
                cwd: project.current_working_directory.clone(),
                include_paths: project.include_paths.clone(),
                magic_dir: project
                    .options
                    .contains(crate::ProjectOptions::CHECK_MAGIC_DIR_IN_INCLUDE),
                path_constants: project
                    .options
                    .contains(crate::ProjectOptions::USE_PATH_CONSTANTS_AS_CONSTANT_STRING),
            },
            toggled: Toggled {
                on: {
                    let mut on = Toggles::empty();
                    on.set(
                        Toggles::BY_REF_FOREACH,
                        project.level >= 1
                            && project
                                .options
                                .contains(crate::ProjectOptions::ASSIGN_TO_BY_REF_FOREACH_EXPR),
                    );
                    on.set(
                        Toggles::DATE_INTERVAL,
                        project.level >= 5
                            && project
                                .options
                                .contains(crate::ProjectOptions::CHECK_DATE_INTERVAL_CONSTRUCTOR),
                    );
                    on.set(
                        Toggles::CASTABLE_TO_NUMBER,
                        project.level >= 5
                            && project.options.contains(
                                crate::ProjectOptions::CHECK_PARAMETER_CASTABLE_TO_NUMBER_FUNCTIONS,
                            ),
                    );
                    on.set(
                        Toggles::FINITE_HAYSTACK,
                        project.level >= 4
                            && project
                                .options
                                .contains(crate::ProjectOptions::FINITE_TYPES_IN_HAYSTACK),
                    );
                    on.set(
                        Toggles::UNUSED_VARIABLE,
                        project.level >= 4
                            && project
                                .options
                                .contains(crate::ProjectOptions::UNUSED_VARIABLE),
                    );
                    on.set(
                        Toggles::UNUSED_PARAMETERS,
                        project.level >= 4
                            && project
                                .options
                                .contains(crate::ProjectOptions::UNUSED_PARAMETERS),
                    );
                    on.set(
                        Toggles::UNUSED_FLOW,
                        project
                            .options
                            .contains(crate::ProjectOptions::UNUSED_PARAMETERS),
                    );
                    on.set(
                        Toggles::NEW_ON_NON_OBJECT,
                        project
                            .options
                            .contains(crate::ProjectOptions::NEW_ON_NON_OBJECT),
                    );
                    on
                },
                constant_types: (project.level >= 2
                    && project
                        .options
                        .contains(crate::ProjectOptions::CHECK_DYNAMIC_CONSTANT_NAME_VALUES))
                .then(|| project.dynamic_constant_types.clone()),
                printf_types: (project.level >= 5
                    && project
                        .options
                        .contains(crate::ProjectOptions::CHECK_PRINTF_PARAMETER_TYPES))
                .then_some(
                    project
                        .options
                        .contains(crate::ProjectOptions::CHECK_STRICT_PRINTF_PLACEHOLDER_TYPES),
                ),
            },
            function_likes,
        }
    }
}

pub(super) struct TraitUse {
    pub(super) class: String,
    pub(super) trait_name: String,
    pub(super) renames: HashMap<String, String>,
    pub(super) php_versions: Vec<super::scope::Guard>,
}

fn checker<'a, 'arena>(
    arena: &'arena LocalArena,
    file: &'a File,
    program: &'arena Program<'arena>,
    names: &'a ResolvedNames<'arena>,
    codebase: &'a CodebaseMetadata,
    project: &'a Checks,
    files: &'a HashMap<FileId, PathBuf>,
    sources: &'a Sources,
    trait_class: Option<&str>,
) -> Checker<'a, 'arena> {
    let php_version = project.php_version;
    let facts = &sources
        .scanned(file.id)
        .expect("the scan read every walked file")
        .facts;
    let functions = project
        .function_likes
        .get(&file.id)
        .map_or(&[][..], Vec::as_slice);
    let extensions: Vec<_> = project.extensions().collect();
    let mut typed: Vec<_> = (extensions.iter())
        .flat_map(|e| e.typed_expressions(names, program))
        .collect();
    typed.extend(super::internal::typed_expressions(
        project.internal.as_ref(),
        program,
    ));
    typed.extend(super::function_checks::typed_expressions(
        names, codebase, program,
    ));
    typed.extend(super::debug::typed_expressions(names, codebase, program));
    typed.extend(super::disallowed::typed_expressions(
        project.disallowed.is_some(),
        program,
    ));
    typed.extend(arrays::typed_expressions(program));
    typed.extend(super::names::typed_class_constants(program, project.level));
    typed.extend(super::scope::typed_method_receivers(program));
    typed.extend(super::function_callable::typed_expressions(
        program,
        project.level,
    ));
    typed.extend(super::method_callable::typed_expressions(
        program,
        project.level,
    ));
    typed.extend(super::types::typed_expressions(
        program,
        &file.contents,
        &facts.comments,
        project.level,
        project.switches.contains(Switches::CHECK_ARGUMENT_TYPES)
            || project
                .switches
                .contains(Switches::CHECK_ARGUMENTS_PASSED_BY_REFERENCE),
        project.switches.contains(Switches::CHECK_THIS_ONLY),
    ));
    typed.extend(super::strict::typed_expressions(project, codebase, program));
    typed.extend(super::var_tags::typed_expressions(
        program,
        &file.contents,
        &facts.comments,
        project.level,
    ));
    typed.extend(super::dead_code::bodies(
        program,
        project.level,
        project
            .toggled
            .on
            .intersects(Toggles::UNUSED_PARAMETERS | Toggles::UNUSED_VARIABLE),
    ));
    typed.extend(super::operators::typed_expressions(project.strict, program));
    typed.extend(super::types::throw_points::typed(
        program,
        project.level >= 4
            || project
                .exceptions
                .checks
                .contains(crate::ExceptionChecks::MISSING_CHECKED_EXCEPTION_IN_THROWS),
    ));
    let run = super::infer::Run {
        arena,
        file,
        program,
        names,
        codebase,
        php_version,
        switches: project.switches,
        comments: &facts.comments,
        functions,
    };
    let (include_typed, include_state) = super::include_file::prepare(&run, &project.include_file);
    typed.extend(include_typed);
    let calls = if project.method_rules.call_methods {
        super::named_argument::requests(program)
    } else {
        Vec::new()
    };
    let super::infer::Inferred {
        mut types,
        defined,
        origins,
        call_terminations,
        named_call_scopes,
        keep_void_types,
        native_types,
        nullsafe_method_receiver_types,
        property_offset_writes,
        variables,
        variable_writes,
        property_initializations,
    } = infer(
        &run,
        super::infer::Request {
            level: project.level,
            extra: typed,
            calls,
            extensions: &extensions,
            trait_class,
        },
    );
    if project.level >= 4 {
        let (subjects, unknown) = super::dead_code::match_subject_types(&run);
        types.extend(subjects);
        for span in unknown {
            types.remove(&span);
        }
    }
    Checker {
        anonymous: anonymous::names(file, program, names, &project.relative_path),
        property_access: property_access::state(program),
        relative_path: &project.relative_path,
        file,
        types,
        keep_void_types,
        native_types,
        nullsafe_method_receiver_types,
        property_offset_writes,
        variable_writes,
        property_initializations,
        call_terminations,
        named_call_scopes,
        defined,
        variables,
        origins,
        source_types: Default::default(),
        names,
        codebase,
        classes: Vec::new(),
        guards: Vec::new(),
        marks: Vec::new(),
        vars: HashMap::new(),
        saved: Vec::new(),
        functions: Vec::new(),
        var_tag_functions: Vec::new(),
        in_exists_call: 0,
        top_level: HashSet::new(),
        piped: HashSet::new(),
        rebound: HashMap::new(),
        pest_test_case: (project.pest.as_ref())
            .zip(file.path.as_deref())
            .map(|(pest, path)| pest.test_case(path, codebase)),
        label: String::from_utf8_lossy(&file.name).into_owned(),
        trait_context: None,
        trait_uses: Vec::new(),
        renamed: None,
        php_version,
        builtins: reflection::builtins(php_version),
        natives: &project.natives,
        resolving_aliases: Default::default(),
        trait_lines: Vec::new(),
        frames: Vec::new(),
        namespaces: Vec::new(),
        magic_defaults: HashSet::new(),
        walk: super::Walk::FIRST_STATEMENT,
        declares_not_first: Vec::new(),
        decls: Vec::new(),
        immutable: HashMap::new(),
        files,
        sources,
        facts,
        function_likes: &project.function_likes,
        patched_docs: &project.patched_docs,
        class_cache: &project.classes,
        anonymous_names: &project.anonymous_names,
        trait_properties: &project.trait_properties,
        syntax: syntax::State::default(),
        assign: assign::State::default(),
        signatures: signatures::State::default(),
        class_names: class_names::State::default(),
        arrays: arrays::State::default(),
        standalone_throws: HashSet::new(),
        level: project.level,
        switches: project.switches,
        larastan: project.larastan.as_ref(),
        pest: project.pest.as_ref(),
        type_coverage: project.type_coverage.as_ref(),
        larastan_state: Default::default(),
        internal: project.internal.as_ref(),
        unreachable: Default::default(),
        conditions: Default::default(),
        strict: project.strict,
        exceptions: &project.exceptions,
        too_wide: project.too_wide,
        include_settings: &project.include_file,
        include_state,
        closure_bind: 0,
        toggled: &project.toggled,
        by_ref_foreach: Vec::new(),
        method_rules: project.method_rules,
        disallowed: project.disallowed.as_ref(),
        errors: Vec::new(),
    }
}

pub(crate) fn check_file(
    path: &Path,
    codebase: &CodebaseMetadata,
    project: &Checks,
    files: &HashMap<FileId, PathBuf>,
    sources: &Sources,
) -> (Vec<Error>, usize) {
    let Some(file) = sources.get(path) else {
        return (Vec::new(), 0);
    };
    let arena = LocalArena::new();
    let (program, names) = parse(&arena, file);
    let errors = crate::parser::errors(
        file,
        program,
        crate::codebase::php_version(project.php_version),
    );
    if !errors.is_empty() {
        return (errors, 0);
    }
    let mut checker = checker(
        &arena, file, program, &names, codebase, project, files, sources, None,
    );
    walk(&mut checker, program, project);
    let name = checker.file_name();
    let (mut lines, parse) = lines_to_ignore(&file.contents);
    let mut unmatched = lines.clone();
    unmatched.retain(|line, _| {
        !checker
            .trait_lines
            .iter()
            .any(|(a, b)| a <= line && line <= b)
    });
    let mut errors = std::mem::take(&mut checker.errors);
    errors.extend(parse_errors(&name, &parse));
    project.keep_reported(&mut errors);
    let mut out = Vec::new();
    if project
        .switches
        .contains(Switches::REPORT_IGNORES_WITHOUT_COMMENTS)
    {
        out.extend(ignores_without_comments(&name, &mut lines, &mut unmatched));
    }
    let (kept, ignored) = local_ignores(&name, errors, &mut lines, &mut unmatched);
    let mut fixable_ignored = ignored.iter().filter(|e| e.fixable).count();
    out.extend(kept);
    fix_all_line_errors(&name, Some(&file.contents), &mut out);
    if project
        .switches
        .contains(Switches::REPORT_UNMATCHED_IGNORES)
    {
        out.extend(unmatched_line_errors(
            &name,
            Some(&file.contents),
            &unmatched,
        ));
    }
    enum TraitWalk {
        Enter(TraitUse),
        Exit(String),
    }
    let mut pending: Vec<_> = std::mem::take(&mut checker.trait_uses)
        .into_iter()
        .map(TraitWalk::Enter)
        .collect();
    let mut active = HashSet::new();
    while let Some(step) = pending.pop() {
        let u = match step {
            TraitWalk::Enter(u) => u,
            TraitWalk::Exit(name) => {
                active.remove(&name);
                continue;
            }
        };
        let trait_name = u.trait_name.to_ascii_lowercase();
        if !active.insert(trait_name.clone()) {
            continue;
        }
        let (errors, uses, ignored) =
            in_trait_context(&name, &u, codebase, project, files, sources);
        out.extend(errors);
        fixable_ignored += ignored;
        pending.push(TraitWalk::Exit(trait_name));
        pending.extend(uses.into_iter().map(TraitWalk::Enter));
    }
    (out, fixable_ignored)
}

fn in_trait_context(
    class_file: &str,
    u: &TraitUse,
    codebase: &CodebaseMetadata,
    project: &Checks,
    files: &HashMap<FileId, PathBuf>,
    sources: &Sources,
) -> (Vec<Error>, Vec<TraitUse>, usize) {
    let class = u.class.as_str();
    let Some(t) = codebase
        .get_class_like(u.trait_name.as_bytes())
        .filter(|t| t.kind == SymbolKind::Trait)
    else {
        return (Vec::new(), Vec::new(), 0);
    };
    let Some(file) = files
        .get(&t.span.file_id)
        .and_then(|p| sources.get(p).cloned())
    else {
        return (Vec::new(), Vec::new(), 0);
    };
    let arena = LocalArena::new();
    let (program, names) = parse(&arena, &file);
    let mut checker = checker(
        &arena,
        &file,
        program,
        &names,
        codebase,
        project,
        files,
        sources,
        Some(class),
    );
    let display = codebase
        .get_class_like(class.as_bytes())
        .map_or_else(|| class.to_string(), |k| checker.display(k));
    let context = if class.starts_with("{anonymous-class:") {
        display
    } else {
        format!("class {display}")
    };
    checker.label = format!("{} (in context of {context})", checker.file_name());
    checker.guards.clone_from(&u.php_versions);
    checker.trait_context = Some(super::TraitContext {
        class_file: class_file.to_string(),
        class: class.to_string(),
        trait_name: String::from_utf8_lossy(t.name.as_bytes()).into_owned(),
        renames: u.renames.clone(),
    });
    walk(&mut checker, program, project);
    let label = checker.label.clone();
    let (mut lines, _) = lines_to_ignore(&file.contents);
    lines.retain(|line, _| {
        checker
            .trait_lines
            .iter()
            .any(|(a, b)| a <= line && line <= b)
    });
    let mut unmatched = lines.clone();
    let mut errors = std::mem::take(&mut checker.errors);
    project.keep_reported(&mut errors);
    let (kept, ignored) = local_ignores(&label, errors, &mut lines, &mut unmatched);
    (
        kept,
        checker.trait_uses,
        ignored.iter().filter(|e| e.fixable).count(),
    )
}

fn walk<'arena>(
    checker: &mut super::Checker<'_, 'arena>,
    program: &'arena Program<'arena>,
    project: &Checks,
) {
    let codebase = checker.codebase;
    let nodes = |name: &str, args: &[pun_phpstan::types::Type]| {
        (project.extensions()).find_map(|e| e.type_node(codebase, name, args))
    };
    pun_phpstan::resolve::with_type_aliases(&project.type_aliases, || {
        pun_phpstan::resolve::with_type_nodes(&nodes, || checker.walk_program(program, &mut ()));
    });
}
