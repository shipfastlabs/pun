mod appends;
mod calls;
mod classes;
mod collections;
mod collectors;
mod config;
mod console;
mod dispatch;
mod extensions;
mod factories;
mod forwards;
mod helpers;
mod macros;
mod octane;
mod reflection;
mod relations;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mago_codex::metadata::class_like::ClassLikeMetadata;
use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::atomic::object::TObject;
use mago_codex::ttype::atomic::object::named::TNamedObject;
use mago_codex::ttype::union::TUnion;
use mago_span::{HasSpan, Span};
use mago_syntax::cst::*;
use mago_syntax::walker::MutWalker;
use pun_phpstan::{Array, Value};

use super::Checker;

pub(super) use crate::laravel::snake;
pub(crate) use appends::property;
pub(crate) use calls::{function_call, method_call, static_method_call};
pub(crate) use classes::{class, method};
pub(crate) use collectors::finish;
pub(super) use config::Types as ConfigTypes;
pub(super) use console::Types as ConsoleTypes;
pub(super) use extensions::{Functions as AppFunctions, Methods as AppMethods};
pub(super) use factories::has_magic_method as factory_magic_method;
pub(crate) use forwards::late_builder;
pub(super) use forwards::{
    ModelForwards, ScopeStaticCalls, Tap, builder_name, facade_mixin_method, facade_root_lacks,
    with_method_class,
};
pub(super) use helpers::{
    AbortIf, ContainerOffsets, Functions as HelperFunctions, Methods as HelperMethods,
};
pub(super) use macros::{Macros, macro_method};

pub(super) fn where_segments(finder: &str) -> Vec<&str> {
    let bytes = finder.as_bytes();
    let mut out = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < bytes.len() {
        let delimiter = [&b"And"[..], b"Or"].into_iter().find(|d| {
            bytes[i..].starts_with(d) && bytes.get(i + d.len()).is_some_and(u8::is_ascii_uppercase)
        });
        match delimiter {
            Some(d) => {
                out.push(&finder[start..i]);
                i += d.len();
                start = i;
            }
            None => i += 1,
        }
    }
    out.push(&finder[start..]);
    out
}

pub(super) const IDENTIFIERS: &[&str] = &[
    "larastan.uselessConstructs.with",
    "larastan.uselessConstructs.value",
    "larastan.noEnvCallsOutsideOfConfig",
    "larastan.noModelMake",
    "larastan.deferrableServiceProvider.missingProvides",
    "larastan.noAuthFacadeInRequestScope",
    "larastan.noAuthHelperInRequestScope",
    "larastan.noPublicModelScopeMethod",
    "larastan.noPublicModelAccessorMethod",
    "larastan.octaneCompatibility",
    "rules.modelAppends",
    "larastan.jobs.noConstructor",
    "larastan.events.noConstructor",
    "larastan.console.undefinedArgument",
    "larastan.console.undefinedOption",
    "larastan.configCollection",
    "larastan.unnecessaryEnumerableToArrayCall",
    "larastan.relationExistence",
    "larastan.noUnnecessaryCollectionCall",
    "larastan.unusedViews",
    "larastan.missingTranslations",
];

const MODEL: &[u8] = b"illuminate\\database\\eloquent\\model";
const REQUEST: &[u8] = b"illuminate\\http\\request";

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct LarastanSwitches: u16 {
        const NO_ENV_CALLS_OUTSIDE_OF_CONFIG = 1;
        const NO_MODEL_MAKE = 1 << 1;
        const CHECK_OCTANE_COMPATIBILITY = 1 << 2;
        const CHECK_MODEL_METHOD_VISIBILITY = 1 << 3;
        const CHECK_AUTH_CALLS_WHEN_IN_REQUEST_SCOPE = 1 << 4;
        const CHECK_MODEL_APPENDS = 1 << 5;
        const PARSE_MODEL_CASTS_METHOD = 1 << 6;
        const CHECK_MODEL_PROPERTIES = 1 << 7;
        const NO_UNNECESSARY_COLLECTION_CALL = 1 << 8;
        const NO_UNNECESSARY_ENUMERABLE_TO_ARRAY_CALLS = 1 << 9;
        const CHECK_UNUSED_VIEWS = 1 << 10;
        const CHECK_MISSING_TRANSLATIONS = 1 << 11;
        const CHECK_CONFIG_TYPES = 1 << 12;
        const GENERALIZE_ENV_RETURN_TYPE = 1 << 13;
        const INFER_FROM_USAGE = 1 << 14;
    }
}

#[derive(Clone, Debug)]
pub struct Larastan {
    pub root: PathBuf,
    pub config_directories: Vec<String>,
    pub switches: LarastanSwitches,
    pub(crate) database_properties: HashMap<String, HashSet<String>>,
    pub(crate) table_columns: HashMap<String, HashSet<String>>,
    pub no_unnecessary_collection_call_methods: Vec<String>,
    pub view_directories: Vec<PathBuf>,
    pub translation_directories: Vec<PathBuf>,
    pub(crate) configs: std::sync::Arc<crate::larastan::config::Configs>,
    pub(crate) models: std::sync::Arc<crate::laravel::Models>,
    pub(crate) app: std::sync::Arc<crate::larastan::app::App>,
    pub bootstrap_files: Vec<PathBuf>,
    /// MigrationHelper's and SquashedMigrationHelper's directories
    pub migration_directories: Vec<PathBuf>,
    pub schema_directories: Vec<PathBuf>,
    pub(crate) macros: std::sync::Arc<crate::larastan::macros::Macros>,
    pub(crate) commands: std::sync::Arc<Vec<crate::larastan::console::Command>>,
    /// The Eloquent builder's `$passthru` for the project's Laravel version (BuilderHelper::getPassthru()).
    pub(crate) passthru: std::sync::Arc<Vec<&'static str>>,
}

impl Larastan {
    pub fn new(root: &Path) -> Larastan {
        Larastan {
            root: root.to_path_buf(),
            config_directories: vec![root.join("config").to_string_lossy().into_owned()],
            switches: LarastanSwitches::NO_ENV_CALLS_OUTSIDE_OF_CONFIG
                | LarastanSwitches::NO_MODEL_MAKE
                | LarastanSwitches::CHECK_MODEL_APPENDS
                | LarastanSwitches::NO_UNNECESSARY_COLLECTION_CALL,
            database_properties: HashMap::new(),
            table_columns: HashMap::new(),
            commands: Default::default(),
            view_directories: Vec::new(),
            translation_directories: Vec::new(),
            no_unnecessary_collection_call_methods: collections::should_handle(&[], &[]),
            configs: Default::default(),
            bootstrap_files: Vec::new(),
            migration_directories: vec![root.join("database/migrations")],
            schema_directories: vec![root.join("database/schema")],
            macros: Default::default(),
            app: Default::default(),
            models: Default::default(),
            passthru: Arc::new(forwards::passthru(None)),
        }
    }

    pub fn from_params(root: &Path, params: &Array) -> Option<Larastan> {
        params.get_str("noModelMake")?;
        let flag = |k: &str, default: bool| match params.get_str(k) {
            Some(Value::Bool(b)) => *b,
            _ => default,
        };
        let mut larastan = Larastan::new(&app_root(root));
        let directories: Vec<String> = params
            .get_str("configDirectories")
            .and_then(Value::as_array)
            .map(|a| {
                a.values()
                    .filter_map(Value::as_str)
                    .map(|d| root.join(d).to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        if !directories.is_empty() {
            larastan.config_directories = directories;
        }
        larastan.switches.set(
            LarastanSwitches::NO_ENV_CALLS_OUTSIDE_OF_CONFIG,
            flag("noEnvCallsOutsideOfConfig", true),
        );
        larastan
            .switches
            .set(LarastanSwitches::NO_MODEL_MAKE, flag("noModelMake", true));
        larastan.switches.set(
            LarastanSwitches::CHECK_OCTANE_COMPATIBILITY,
            flag("checkOctaneCompatibility", false),
        );
        larastan.switches.set(
            LarastanSwitches::CHECK_MODEL_METHOD_VISIBILITY,
            flag("checkModelMethodVisibility", false),
        );
        larastan.switches.set(
            LarastanSwitches::CHECK_AUTH_CALLS_WHEN_IN_REQUEST_SCOPE,
            flag("checkAuthCallsWhenInRequestScope", false),
        );
        larastan.switches.set(
            LarastanSwitches::CHECK_MODEL_APPENDS,
            flag("checkModelAppends", true),
        );
        larastan.switches.set(
            LarastanSwitches::PARSE_MODEL_CASTS_METHOD,
            flag("parseModelCastsMethod", false),
        );
        larastan.switches.set(
            LarastanSwitches::CHECK_MODEL_PROPERTIES,
            flag("checkModelProperties", false),
        );
        larastan.switches.set(
            LarastanSwitches::CHECK_CONFIG_TYPES,
            flag("checkConfigTypes", false),
        );
        larastan.switches.set(
            LarastanSwitches::GENERALIZE_ENV_RETURN_TYPE,
            flag("generalizeEnvReturnType", false),
        );
        larastan.switches.set(
            LarastanSwitches::INFER_FROM_USAGE,
            params
                .get_str("featureToggles")
                .and_then(Value::as_array)
                .and_then(|t| t.get_str("unresolvedTemplateArguments"))
                == Some(&Value::Bool(true)),
        );
        larastan.switches.set(
            LarastanSwitches::NO_UNNECESSARY_ENUMERABLE_TO_ARRAY_CALLS,
            flag("noUnnecessaryEnumerableToArrayCalls", false),
        );
        larastan.switches.set(
            LarastanSwitches::NO_UNNECESSARY_COLLECTION_CALL,
            flag("noUnnecessaryCollectionCall", true),
        );
        larastan.switches.set(
            LarastanSwitches::CHECK_UNUSED_VIEWS,
            flag("checkUnusedViews", false),
        );
        larastan.switches.set(
            LarastanSwitches::CHECK_MISSING_TRANSLATIONS,
            flag("checkMissingTranslations", false),
        );
        let names = |k: &str| -> Vec<String> {
            params
                .get_str(k)
                .and_then(Value::as_array)
                .map(|a| {
                    a.values()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        };
        // MigrationHelper::getMigrationDirectories() and SquashedMigrationHelper::getSchemaDirectories(): the
        // configured paths, else the app's `database_path()` directory, none when the scan is off
        let scanned = |paths: &str, default: &[PathBuf], disabled: &str| {
            if flag(disabled, false) {
                return Vec::new();
            }
            let paths = names(paths);
            let paths = if paths.is_empty() {
                default
                    .iter()
                    .map(|d| d.to_string_lossy().into_owned())
                    .collect()
            } else {
                paths
            };
            crate::laravel::resolve_directories(root, &paths)
        };
        larastan.migration_directories = scanned(
            "databaseMigrationsPath",
            &larastan.migration_directories,
            "disableMigrationScan",
        );
        larastan.schema_directories = scanned(
            "squashedMigrationsPath",
            &larastan.schema_directories,
            "disableSchemaScan",
        );
        larastan.bootstrap_files = names("bootstrapFiles")
            .iter()
            .map(|f| root.join(f))
            .collect();
        larastan.view_directories = names("viewDirectories")
            .iter()
            .map(|d| root.join(d))
            .collect();
        larastan.translation_directories = names("translationDirectories")
            .iter()
            .map(|d| root.join(d))
            .collect();
        larastan.no_unnecessary_collection_call_methods = collections::should_handle(
            &names("noUnnecessaryCollectionCallOnly"),
            &names("noUnnecessaryCollectionCallExcept"),
        );
        Some(larastan)
    }
}

impl crate::AnalyseExtension for Larastan {
    fn stub_files(&self, root: &Path) -> Vec<PathBuf> {
        crate::larastan::stubs::files(root)
    }

    fn bundled_stub(&self, path: &Path) -> Option<&'static [u8]> {
        crate::larastan::stubs::bundled(path)
    }

    fn class_aliases(&self) -> Vec<(String, String)> {
        crate::larastan::aliases::load(&self.root)
    }

    fn prepare_codebase(
        &self,
        _: &crate::Project,
        codebase: &mut mago_codex::metadata::CodebaseMetadata,
    ) {
        crate::larastan::app::App::load(&self.root).contract_mixins(codebase);
        crate::larastan::types::generic_eloquent_builders(codebase);
    }

    fn load(
        &mut self,
        project: &crate::Project,
        codebase: &mago_codex::metadata::CodebaseMetadata,
    ) {
        let app = crate::Laravel {
            root: &self.root,
            parse_casts_method: self
                .switches
                .contains(LarastanSwitches::PARSE_MODEL_CASTS_METHOD),
            migrations: &self.migration_directories,
            schemas: &self.schema_directories,
        };
        (self.database_properties, self.table_columns) =
            crate::laravel::database_properties(app, project, codebase);
        self.models = Arc::new(crate::laravel::models(app, project, codebase));
        self.commands = Arc::new(crate::larastan::console::commands(project, codebase));
        self.app = Arc::new(crate::larastan::app::App::load(&self.root));
        self.passthru = Arc::new(forwards::passthru(
            crate::larastan::stubs::laravel_version(&self.root).as_deref(),
        ));
        self.macros = Arc::new(crate::larastan::macros::Macros::load(
            &self.bootstrap_files,
            &project.analysed,
            &project.scanned,
            &project.autoloaded,
            &self.root,
            codebase,
            &self.app,
        ));
        if self.switches.contains(LarastanSwitches::CHECK_CONFIG_TYPES) {
            self.configs = Arc::new(crate::larastan::config::Configs::load(
                &self.config_directories,
                &self.root,
                codebase,
                self.switches
                    .contains(LarastanSwitches::GENERALIZE_ENV_RETURN_TYPE),
            ));
        }
    }

    fn register(
        &self,
        registry: &mut mago_analyzer::plugin::PluginRegistry,
        contents: &[u8],
        names: &mago_names::ResolvedNames<'_>,
    ) {
        registry.register_method_provider(Macros(Arc::clone(&self.app), Arc::clone(&self.macros)));
        if self.switches.contains(LarastanSwitches::CHECK_CONFIG_TYPES) {
            registry.register_function_provider(ConfigTypes(Arc::clone(&self.configs)));
            registry.register_method_provider(ConfigTypes(Arc::clone(&self.configs)));
        }
        registry.register_method_provider(HelperMethods(Arc::clone(&self.app)));
        registry
            .register_method_provider(ModelForwards(contents.to_vec(), Arc::clone(&self.passthru)));
        registry.register_method_provider(ConsoleTypes(Arc::clone(&self.commands)));
        registry.register_method_provider(AppMethods(Arc::clone(&self.app), contents.to_vec()));
        registry.register_function_provider(AppFunctions(
            Arc::clone(&self.app),
            self.switches.contains(LarastanSwitches::INFER_FROM_USAGE),
        ));
        registry.register_function_provider(Tap);
        registry.register_function_assertion_provider(AbortIf);
        registry.register_function_provider(HelperFunctions {
            app: Arc::clone(&self.app),
            generalize_env: self
                .switches
                .contains(LarastanSwitches::GENERALIZE_ENV_RETURN_TYPE),
        });
        registry.register_expression_hook(ContainerOffsets(Arc::clone(&self.app)));
        registry.register_expression_hook(forwards::LateBuilders);
        registry.register_expression_hook(ScopeStaticCalls(
            names
                .iter()
                .map(|(start, _, name, _)| (start, name.to_vec()))
                .collect(),
        ));
        crate::laravel::register_models(registry, Arc::clone(&self.models));
    }

    fn settings(&self, settings: &mut mago_analyzer::settings::Settings) {
        settings.mixin_receiver_binding = true;
    }

    fn typed_expressions<'arena>(
        &self,
        names: &mago_names::ResolvedNames<'arena>,
        program: &Program<'arena>,
    ) -> Vec<Span> {
        let mut wanted = Wanted {
            larastan: self,
            names,
            spans: Vec::new(),
        };
        wanted.walk_program(program, &mut ());
        wanted.spans
    }

    fn type_node(
        &self,
        codebase: &mago_codex::metadata::CodebaseMetadata,
        name: &str,
        args: &[pun_phpstan::types::Type],
    ) -> Option<pun_phpstan::types::Type> {
        type_node(codebase, self, name, args)
    }
}

fn app_root(root: &Path) -> PathBuf {
    let skeleton = root.join("vendor/orchestra/testbench-core/laravel");
    if !root.join("bootstrap/app.php").is_file() && skeleton.join("bootstrap/app.php").is_file() {
        skeleton
    } else {
        root.to_path_buf()
    }
}

#[derive(Default)]
pub(crate) struct State {
    request: crate::bitset::BitSet,
}

fn has_database_property(c: &Checker, model: &str, column: &str) -> bool {
    c.larastan.is_some_and(|larastan| {
        !crate::laravel::has_accessor(c.codebase, model, column)
            && larastan
                .database_properties
                .get(&model.to_ascii_lowercase())
                .is_some_and(|p| p.contains(column))
            && !c
                .codebase
                .get_class_like(model.as_bytes())
                .is_some_and(|k| appends::property_tag(k, column))
    })
}

fn has_model_property(c: &Checker, k: &ClassLikeMetadata, property: &[u8]) -> bool {
    if !is_model(c, k) {
        return false;
    }
    let name =
        String::from_utf8_lossy(property.strip_prefix(b"$").unwrap_or(property)).into_owned();
    let class = k.original_name.as_str_lossy().into_owned();
    if crate::laravel::accessor_type(c.codebase, &class, &name).is_some()
        || !k.flags.is_abstract() && has_database_property(c, &class, &name)
    {
        return true;
    }
    if appends::property_tag(k, &name) {
        return false;
    }
    let methods = match name.strip_suffix("_count") {
        Some(relation) => vec![crate::laravel::camel(relation), relation.to_string()],
        None => vec![name.clone()],
    };
    methods.iter().any(|m| {
        c.codebase
            .get_declaring_method(k.name.as_bytes(), m.as_bytes())
            .and_then(|f| f.return_type_metadata.as_ref())
            .is_some_and(|t| {
                is_object_of(
                    c,
                    &t.type_union,
                    b"illuminate\\database\\eloquent\\relations\\relation",
                )
            })
    })
}

pub(crate) fn has_scope(c: &Checker, model: &str, method: &[u8]) -> bool {
    is_scope(c.codebase, model.as_bytes(), method)
}

pub(crate) fn is_scope(
    codebase: &mago_codex::metadata::CodebaseMetadata,
    model: &[u8],
    method: &[u8],
) -> bool {
    let Some(k) = codebase.get_class_like(model) else {
        return false;
    };
    let attribute = codebase
        .get_declaring_method(k.name.as_bytes(), method)
        .is_some_and(|m| {
            m.method_metadata
                .as_ref()
                .is_some_and(|mm| !mm.visibility.is_public())
                && m.attributes.iter().any(|a| {
                    a.name
                        .as_bytes()
                        .eq_ignore_ascii_case(b"Illuminate\\Database\\Eloquent\\Attributes\\Scope")
                })
        });
    let mut scope = b"scope".to_vec();
    scope.extend(method.iter().take(1).map(u8::to_ascii_uppercase));
    scope.extend(method.iter().skip(1));
    attribute
        || codebase
            .get_declaring_method(k.name.as_bytes(), &scope)
            .is_some()
}

pub(crate) fn is_hidden_scope(
    c: &Checker,
    k: &ClassLikeMetadata,
    method: &[u8],
    scope: Option<&ClassLikeMetadata>,
) -> bool {
    is_model(c, k)
        && is_scope(c.codebase, k.name.as_bytes(), method)
        && c.codebase
            .get_declaring_method(k.name.as_bytes(), method)
            .is_some_and(|m| {
                m.attributes.iter().any(|a| {
                    a.name
                        .as_bytes()
                        .eq_ignore_ascii_case(b"Illuminate\\Database\\Eloquent\\Attributes\\Scope")
                })
            })
        && !scope.is_some_and(|s| {
            is_a(
                c,
                s.name.as_bytes(),
                &k.name.as_bytes().to_ascii_lowercase(),
            )
        })
}

pub(crate) fn is_a(c: &Checker, class: &[u8], parent: &[u8]) -> bool {
    class.eq_ignore_ascii_case(parent)
        || c.codebase
            .is_instance_of(&class.to_ascii_lowercase(), parent)
}

pub(crate) fn is_model(c: &Checker, k: &ClassLikeMetadata) -> bool {
    class_is(c, k, MODEL)
}

fn class_is(c: &Checker, k: &ClassLikeMetadata, parent: &[u8]) -> bool {
    is_a(c, k.name.as_bytes(), parent)
}

fn object_names(ty: &TUnion) -> Option<Vec<&[u8]>> {
    named_objects(ty).map(|objects| objects.iter().map(|n| n.name.as_bytes()).collect())
}

fn named_objects(ty: &TUnion) -> Option<Vec<&TNamedObject>> {
    ty.types
        .iter()
        .map(|t| match t {
            TAtomic::Object(TObject::Named(n)) => Some(n),
            _ => None,
        })
        .collect()
}

fn is_object_of(c: &Checker, ty: &TUnion, class: &[u8]) -> bool {
    object_names(ty)
        .is_some_and(|names| !names.is_empty() && names.iter().all(|n| is_a(c, n, class)))
}

fn request_parameter(c: &Checker, parameters: &FunctionLikeParameterList) -> bool {
    parameters.parameters.iter().any(|p| {
        p.variable.name == b"$request"
            && p.default_value
                .as_ref()
                .is_none_or(|d| !matches!(d.value, Expression::Literal(Literal::Null(_))))
            && match &p.hint {
                Some(Hint::Identifier(id)) => is_a(c, c.names.get(&id.span()), REQUEST),
                _ => false,
            }
    })
}

impl Checker<'_, '_> {
    pub(super) fn larastan_enter_function(&mut self, parameters: &FunctionLikeParameterList) {
        let request = self.larastan.is_some() && request_parameter(self, parameters);
        self.larastan_state.request.push(request);
    }

    pub(super) fn larastan_enter_closure(&mut self, f: &Closure) {
        let outer = self.larastan_state.request.last().unwrap_or(false);
        let used = f
            .use_clause
            .as_ref()
            .is_some_and(|u| u.variables.iter().any(|v| v.variable.name == b"$request"));
        let request = self.larastan.is_some()
            && (request_parameter(self, &f.parameter_list) || (used && outer));
        self.larastan_state.request.push(request);
    }

    pub(super) fn larastan_leave_function(&mut self) {
        self.larastan_state.request.pop();
    }

    fn request_in_scope(&self) -> bool {
        self.larastan_state.request.last().unwrap_or(false)
    }
}

struct Wanted<'l, 'n, 'arena> {
    larastan: &'l Larastan,
    names: &'n mago_names::ResolvedNames<'arena>,
    spans: Vec<Span>,
}

impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for Wanted<'_, '_, 'arena> {
    fn walk_in_function_call(&mut self, call: &'ast FunctionCall<'arena>, (): &mut ()) {
        let Expression::Identifier(id) = call.function else {
            return;
        };
        let name = calls::written_name(self.names, id);
        let args = &call.argument_list.arguments;
        let wanted = if name.eq_ignore_ascii_case("with") {
            args.get(1)
        } else if name.eq_ignore_ascii_case("value") {
            args.first()
        } else {
            None
        };
        self.spans.extend(wanted.map(|a| a.value().span()));
    }
    fn walk_in_static_method_call(&mut self, call: &'ast StaticMethodCall<'arena>, (): &mut ()) {
        if self
            .larastan
            .switches
            .contains(LarastanSwitches::CHECK_CONFIG_TYPES)
        {
            self.spans.extend(config::typed(self.names, call));
        }
        if let Some(arg) = relations::relation_call(&call.method, &call.argument_list) {
            self.spans.push(arg.span());
            if !matches!(call.class, Expression::Identifier(_)) {
                self.spans.push(call.class.span());
            }
        }
        if let ClassLikeMemberSelector::Identifier(m) = &call.method
            && dispatch::conditional(m.value)
            && let Some(first) = call.argument_list.arguments.first()
        {
            self.spans.push(first.value().span());
        }
        if self
            .larastan
            .switches
            .contains(LarastanSwitches::NO_MODEL_MAKE)
            && matches!(&call.method, ClassLikeMemberSelector::Identifier(m) if m.value == b"make")
            && !matches!(
                call.class,
                Expression::Identifier(_)
                    | Expression::Self_(_)
                    | Expression::Static(_)
                    | Expression::Parent(_)
            )
        {
            self.spans.push(call.class.span());
        }
    }
    fn walk_in_method_call(&mut self, call: &'ast MethodCall<'arena>, (): &mut ()) {
        if self
            .larastan
            .switches
            .contains(LarastanSwitches::NO_UNNECESSARY_ENUMERABLE_TO_ARRAY_CALLS)
            && collections::to_array(call).is_some()
        {
            self.spans.push(call.object.span());
        }
        if let Some(arg) = relations::relation_call(&call.method, &call.argument_list) {
            self.spans.extend([arg.span(), call.object.span()]);
        }
        if self
            .larastan
            .switches
            .contains(LarastanSwitches::NO_UNNECESSARY_COLLECTION_CALL)
        {
            self.spans.extend(collections::unnecessary_typed(call));
        }
        if collectors::typed_method_call(self.larastan, call).is_some() {
            self.spans.push(call.object.span());
        }
        if let Some((_, arg)) = console::input_call(call) {
            self.spans.extend([call.object.span(), arg.span()]);
        }
        if self
            .larastan
            .switches
            .contains(LarastanSwitches::CHECK_OCTANE_COMPATIBILITY)
            && octane::is_binding(call)
        {
            self.spans.push(call.object.span());
        }
    }
}

thread_local! {
    static RESOLVE_UNRESOLVABLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn resolving_unresolvable<R>(f: impl FnOnce() -> R) -> R {
    let previous = RESOLVE_UNRESOLVABLE.with(|r| r.replace(true));
    let result = f();
    RESOLVE_UNRESOLVABLE.with(|r| r.set(previous));
    result
}

fn with_key(
    collection: pun_phpstan::types::Type,
    key: &pun_phpstan::types::Type,
) -> pun_phpstan::types::Type {
    match collection {
        pun_phpstan::types::Type::Object(name, mut args, nullable) if args.len() == 2 => {
            args[0] = key.clone();
            pun_phpstan::types::Type::Object(name, args, nullable)
        }
        c => c,
    }
}

fn late_of(
    codebase: &mago_codex::metadata::CodebaseMetadata,
    name: &str,
    args: &[pun_phpstan::types::Type],
) -> pun_phpstan::types::Type {
    use pun_phpstan::types::{NeverKind, Type};
    fn late(t: &Type) -> bool {
        let found = std::cell::Cell::new(matches!(t, Type::Static(..)));
        t.traverse(&|inner| {
            found.set(found.get() || late(inner));
            inner.clone()
        });
        found.get() || super::declared::has_template(t)
    }
    // a model from a receiver's call-site variance (`Builder<covariant Model>`) is the model itself: a type projection
    // gives its type to the template it resolves (GenericObjectType::getTypeArgument())
    let args: Vec<Type> = (args.iter())
        .map(|t| match t {
            Type::Projection(_, inner) => (**inner).clone(),
            t => t.clone(),
        })
        .collect();
    let args = args.as_slice();
    if args.iter().any(late) {
        return Type::Object(name.into(), args.to_vec(), false);
    }
    resolve_late(codebase, name, args).unwrap_or_else(|| match name {
        // RelationOfType::getResult(): no relation found (a collection of `mixed`) is `Relation<mixed, mixed, mixed>`
        "relation-of" => Type::Object(
            "Illuminate\\Database\\Eloquent\\Relations\\Relation".into(),
            vec![pun_phpstan::types::mixed(); 3],
            false,
        ),
        _ => Type::Never(NeverKind::Implicit),
    })
}

/// BuilderOfType::getResult() of a template: the builder of the template's bound, generic in the template itself.
fn template_builder(
    codebase: &mago_codex::metadata::CodebaseMetadata,
    t: &pun_phpstan::types::Type,
) -> Option<pun_phpstan::types::Type> {
    use pun_phpstan::types::Type;
    let Type::Template(template) = t else {
        return None;
    };
    let [model] = template.bound.object_class_names().try_into().ok()?;
    let builder = forwards::builder_name(codebase, model.as_bytes());
    let generic =
        (codebase.get_class_like(builder.as_bytes())).is_some_and(|k| !k.template_types.is_empty());
    let arguments = if generic { vec![t.clone()] } else { vec![] };
    Some(Type::Object(
        crate::text(builder.as_bytes()),
        arguments,
        false,
    ))
}

/// BuilderOfType's and RelationOfType's `getResult()`, also while an argument holds a template (whose bound's
/// classes are the models); `None` when nothing resolves.
pub(crate) fn resolve_late(
    codebase: &mago_codex::metadata::CodebaseMetadata,
    name: &str,
    args: &[pun_phpstan::types::Type],
) -> Option<pun_phpstan::types::Type> {
    use pun_phpstan::types::{Type, union};
    if let ("builder-of", [t @ Type::Template(_)]) = (name, args) {
        return template_builder(codebase, t);
    }
    let models: Vec<TAtomic> = (args[0].object_class_names().iter())
        .map(|n| TAtomic::Object(TObject::Named(TNamedObject::new(mago_word::word(n)))))
        .collect();
    if models.is_empty() {
        return None;
    }
    let model = TUnion::from_vec(models);
    let key = args.get(1).map(|key| match key.constant_strings() {
        keys if keys.is_empty() => mago_codex::ttype::get_string(),
        keys => TUnion::from_vec(
            (keys.iter())
                .map(|s| {
                    mago_codex::ttype::get_literal_string(mago_word::word(s)).get_single_owned()
                })
                .collect(),
        ),
    });
    fn from(t: &TUnion) -> Type {
        union(
            (t.types.iter())
                .map(|a| match a {
                    TAtomic::Object(TObject::Named(n)) => Type::Object(
                        crate::text(n.name.as_bytes()),
                        n.type_parameters.iter().flatten().map(from).collect(),
                        false,
                    ),
                    _ => pun_phpstan::types::mixed(),
                })
                .collect(),
        )
    }
    let arguments: Vec<TUnion> = std::iter::once(model).chain(key).collect();
    forwards::late_builder(codebase, name.as_bytes(), &arguments).map(|t| from(&t))
}

fn type_node(
    codebase: &mago_codex::metadata::CodebaseMetadata,
    larastan: &Larastan,
    name: &str,
    args: &[pun_phpstan::types::Type],
) -> Option<pun_phpstan::types::Type> {
    use pun_phpstan::types::{Accessory, Type, is_super_type_of};
    let model = || {
        Type::Object(
            "Illuminate\\Database\\Eloquent\\Model".into(),
            vec![],
            false,
        )
    };
    match (name, args) {
        ("|", [a, b]) => {
            const BUILDER: &[u8] = b"Illuminate\\Database\\Eloquent\\Builder";
            let class = |t: &Type| match t {
                Type::Object(n, args, _) if args.is_empty() => {
                    codebase.get_class_like(n.as_bytes())
                }
                _ => None,
            };
            let is_model = |t: &Type| class(t).is_some() && is_super_type_of(&model(), t).yes();
            let is_builder = |t: &Type| {
                class(t).is_some_and(|k| {
                    k.name.as_bytes().eq_ignore_ascii_case(BUILDER)
                        || codebase.is_instance_of(k.name.as_bytes(), BUILDER)
                })
            };
            let model = [a, b].into_iter().rfind(|t| is_model(t))?;
            let builder = [a, b].into_iter().rfind(|t| is_builder(t))?;
            let Type::Object(builder_name, ..) = builder else {
                return None;
            };
            // `$this->provider->getClass($builderTypeName)->isGeneric()`: a builder without templates stays plain
            let generic = class(builder).is_some_and(|k| !k.template_types.is_empty());
            let arguments = if generic { vec![model.clone()] } else { vec![] };
            Some(Type::Object(builder_name.clone(), arguments, false))
        }
        // GenericEloquentCollectionTypeNodeResolverExtension: `Collection|Item[]` is `Collection<int, Item>`
        ("|[]", [Type::Object(collection, arguments, _), item])
            if arguments.is_empty()
                && collection == "Illuminate\\Database\\Eloquent\\Collection" =>
        {
            Some(Type::Object(
                collection.clone(),
                vec![Type::Int, item.clone()],
                false,
            ))
        }
        ("view-string", []) => Some(Type::Intersection(vec![
            Type::String,
            Type::Accessory(Accessory::ViewString),
        ])),
        // BuilderOfType::getResult() while the type holds a template (the return rules resolve it so): the builder of
        // the template's bound, generic in the template itself
        ("builder-of", [t @ Type::Template(_)])
            if RESOLVE_UNRESOLVABLE.with(std::cell::Cell::get) =>
        {
            template_builder(codebase, t)
        }
        ("builder-of", [_] | [_, _]) | ("relation-of", [_, _]) => {
            Some(late_of(codebase, name, args))
        }
        ("model-property", _)
            if !larastan
                .switches
                .contains(LarastanSwitches::CHECK_MODEL_PROPERTIES) =>
        {
            Some(Type::String)
        }
        ("model-property", [t])
            if !is_super_type_of(&model(), t).no() && !matches!(t, Type::Never(_)) =>
        {
            Some(Type::Intersection(vec![
                Type::String,
                Type::Accessory(Accessory::ModelProperty(Box::new(t.clone()))),
            ]))
        }
        ("model-property", _) => Some(Type::Error),
        ("collection-of", [key, t]) => {
            if is_super_type_of(&Type::Union(vec![Type::Int, Type::String]), key).no() {
                return None;
            }
            match type_node(codebase, larastan, name, std::slice::from_ref(t))? {
                Type::Union(collections) => Some(pun_phpstan::types::union(
                    collections.into_iter().map(|c| with_key(c, key)).collect(),
                )),
                c => Some(with_key(c, key)),
            }
        }
        ("collection-of", [t])
            if is_super_type_of(&model(), t).no() || matches!(t, Type::Never(_)) =>
        {
            None
        }
        ("collection-of", [t])
            if super::declared::has_template(t)
                && RESOLVE_UNRESOLVABLE.with(std::cell::Cell::get) =>
        {
            type_node(codebase, larastan, name, &[t.resolve_to_bounds()])
        }
        ("collection-of", [t]) if super::declared::has_template(t) => Some(Type::Object(
            "Illuminate\\Database\\Eloquent\\Collection".into(),
            vec![Type::Int, t.clone()],
            false,
        )),
        ("collection-of", [t]) => {
            let object = |a: &TAtomic| match a {
                TAtomic::Object(TObject::Named(n)) => Some(n.clone()),
                _ => None,
            };
            let collections = t
                .object_class_names()
                .iter()
                .map(|m| {
                    let n = object(&crate::laravel::collection_class(codebase, m.as_bytes()))?;
                    let args = n
                        .type_parameters
                        .iter()
                        .flatten()
                        .map(|p| match p.types.as_ref() {
                            [TAtomic::Scalar(_)] => {
                                Some(pun_phpstan::types::Type::Benevolent(vec![
                                    Type::Int,
                                    Type::String,
                                ]))
                            }
                            [a] => object(a).map(|o| {
                                Type::Object(crate::text(o.name.as_bytes()), vec![], false)
                            }),
                            _ => None,
                        })
                        .collect::<Option<Vec<_>>>()?;
                    Some(Type::Object(crate::text(n.name.as_bytes()), args, false))
                })
                .collect::<Option<Vec<_>>>()?;
            Some(pun_phpstan::types::union(collections))
        }
        _ => None,
    }
}
