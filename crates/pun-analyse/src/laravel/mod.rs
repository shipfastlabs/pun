mod collections;
mod schema;
mod sql;
mod strings;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mago_allocator::LocalArena;
use mago_analyzer::plugin::{
    ExpressionHook, HookContext, HookResult, PluginRegistry, Provider, ProviderMeta,
};
use mago_codex::metadata::CodebaseMetadata;
use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::atomic::object::TObject;
use mago_codex::ttype::atomic::object::named::TNamedObject;
use mago_codex::ttype::expander::{StaticClassType, TypeExpansionOptions, expand_union};
use mago_codex::ttype::union::TUnion;
use mago_codex::ttype::*;
use mago_database::file::FileType;
use mago_names::ResolvedNames;
use mago_names::resolver::NameResolver;
use mago_span::HasSpan;
use mago_syntax::cst::*;
use mago_syntax::parser::parse_file;
use mago_syntax::walker::MutWalker;
use mago_word::Word;

use crate::Project;
use crate::codebase::read;
pub(crate) use collections::{
    collection_class, collection_of, proxy_result, references_collection, replace_collections,
};
use schema::{Column, SchemaAggregator};
pub(crate) use strings::{camel, snake};
use strings::{class_basename, plural_studly, singular, studly};

const MODEL: &str = "Illuminate\\Database\\Eloquent\\Model";
const ATTRIBUTE: &str = "Illuminate\\Database\\Eloquent\\Casts\\Attribute";
const TABLE_ATTRIBUTE: &str = "Illuminate\\Database\\Eloquent\\Attributes\\Table";
const CARBON_IMMUTABLE: &str = "Carbon\\CarbonImmutable";
const PROXY: &str = "Illuminate\\Support\\HigherOrderCollectionProxy";

#[derive(Clone, Copy, Debug)]
pub struct Laravel<'a> {
    pub root: &'a Path,
    pub parse_casts_method: bool,
    /// MigrationHelper's directories (`databaseMigrationsPath`, none with `disableMigrationScan`)
    pub migrations: &'a [PathBuf],
    /// SquashedMigrationHelper's directories (`squashedMigrationsPath`, none with `disableSchemaScan`)
    pub schemas: &'a [PathBuf],
}

/// `DirectoryResolver::resolve()`: each path absolute from `cwd`; an existing directory or a path without glob
/// characters is kept as it is, a glob is the directories it matches.
pub(crate) fn resolve_directories(cwd: &Path, paths: &[String]) -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = Vec::new();
    for path in paths {
        let path = pun_phpstan::php::absolutize_path(&cwd.to_string_lossy(), path);
        let matches = if Path::new(&path).is_dir() || !path.contains(['*', '?', '[']) {
            vec![path]
        } else {
            glob_directories(&path)
        };
        for directory in matches {
            let directory = pun_phpstan::php::normalize_path(&directory);
            let directory = PathBuf::from(directory.trim_end_matches(['/', '\\']));
            if !directories.contains(&directory) {
                directories.push(directory);
            }
        }
    }
    directories
}

/// `glob($pattern, GLOB_ONLYDIR)`: the directories an absolute pattern matches, sorted, with `fnmatch()` per path
/// component (a leading dot is matched only by a dot).
fn glob_directories(pattern: &str) -> Vec<String> {
    let mut found = vec![String::new()];
    for component in pattern.split('/').filter(|c| !c.is_empty()) {
        let mut next = Vec::new();
        for base in &found {
            if !component.contains(['*', '?', '[']) {
                next.push(format!("{base}/{component}"));
                continue;
            }
            let mut names: Vec<String> =
                std::fs::read_dir(if base.is_empty() { "/" } else { base })
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| !n.starts_with('.') || component.starts_with('.'))
                    .filter(|n| pun_tokens::php_compat::fnmatch(component.as_bytes(), n.as_bytes()))
                    .collect();
            names.sort();
            next.extend(names.into_iter().map(|n| format!("{base}/{n}")));
        }
        found = next;
    }
    found.retain(|d| Path::new(d).is_dir());
    found
}

/// MigrationHelper's migration files below `directories`.
pub(crate) fn migration_files(directories: &[PathBuf]) -> Vec<PathBuf> {
    files_below(directories, &|path| {
        path.to_ascii_lowercase().ends_with(".php")
    })
}

/// The files below `directories` whose path `matches`, by path; RecursiveDirectoryIterator does not enter a linked
/// directory.
fn files_below(directories: &[PathBuf], matches: &dyn Fn(&str) -> bool) -> Vec<PathBuf> {
    fn walk(dir: &Path, matches: &dyn Fn(&str) -> bool, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                walk(&path, matches, out);
            } else if matches(&path.to_string_lossy()) {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    for directory in directories {
        let mut found = Vec::new();
        walk(directory, matches, &mut found);
        found.sort();
        // `$files += iterator_to_array(...)`: a path an earlier directory gave is kept once
        found.retain(|f| !files.contains(f));
        files.extend(found);
    }
    files
}

pub(crate) fn register(
    registry: &mut PluginRegistry,
    app: Laravel,
    project: &Project,
    codebase: &CodebaseMetadata,
) {
    registry.register_expression_hook(ModelAttributes {
        models: std::sync::Arc::new(models(app, project, codebase)),
    });
}

#[derive(Default)]
struct ModelSource {
    table: Option<String>,
    primary_key: Option<String>,
    key_type: Option<String>,
    incrementing: Option<bool>,
    timestamps: Option<bool>,
    casts: Vec<(String, String)>,
    casts_method: Vec<(String, String)>,
}

#[derive(Default)]
struct Scan {
    models: HashMap<String, (String, ModelSource)>,
    date_class: Option<String>,
    parse_casts_method: bool,
}

struct Collector<'s, 'n, 'arena> {
    scan: &'s mut Scan,
    names: &'n ResolvedNames<'arena>,
    codebase: &'s CodebaseMetadata,
    parse_casts_method: bool,
}

fn resolved(names: &ResolvedNames, e: &Expression) -> Option<String> {
    match e {
        Expression::Identifier(id) => {
            Some(String::from_utf8_lossy(names.get(&id.span())).into_owned())
        }
        _ => None,
    }
}

fn constant_string(names: &ResolvedNames, e: &Expression) -> Option<String> {
    match e {
        Expression::Literal(Literal::String(s)) => s
            .value
            .map(|v| String::from_utf8_lossy(v).replace("\\\\", "\\")),
        Expression::Binary(b) if b.operator.is_concatenation() => {
            Some(constant_string(names, b.lhs)? + &constant_string(names, b.rhs)?)
        }
        Expression::Parenthesized(p) => constant_string(names, p.expression),
        Expression::Access(Access::ClassConstant(c)) if matches!(&c.constant, ClassLikeConstantSelector::Identifier(i) if i.value.eq_ignore_ascii_case(b"class")) => {
            resolved(names, c.class)
        }
        _ => None,
    }
}

fn string_pairs(names: &ResolvedNames, e: &Expression) -> Vec<(String, String)> {
    let elements = match e {
        Expression::Array(a) => &a.elements,
        Expression::LegacyArray(a) => &a.elements,
        _ => return Vec::new(),
    };
    elements
        .iter()
        .filter_map(|el| match el {
            ArrayElement::KeyValue(kv) => Some((
                constant_string(names, kv.key)?,
                constant_string(names, kv.value)?,
            )),
            _ => None,
        })
        .collect()
}

#[derive(Default)]
struct FirstReturn<'ast, 'arena>(Option<&'ast Return<'arena>>);

impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for FirstReturn<'ast, 'arena> {
    fn walk_in_return(&mut self, r: &'ast Return<'arena>, (): &mut ()) {
        self.0.get_or_insert(r);
    }
}

impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for Collector<'_, '_, 'arena> {
    fn walk_in_class(&mut self, class: &'ast Class<'arena>, (): &mut ()) {
        let name = String::from_utf8_lossy(self.names.get(&class.name.span())).into_owned();
        if !self
            .codebase
            .is_instance_of(name.as_bytes(), MODEL.as_bytes())
        {
            return;
        }
        let mut model = ModelSource::default();
        for member in &class.members {
            match member {
                ClassLikeMember::Property(Property::Plain(p)) => {
                    for item in &p.items {
                        let PropertyItem::Concrete(item) = item else {
                            continue;
                        };
                        let string = constant_string(self.names, item.value);
                        let bool = match item.value {
                            Expression::Literal(Literal::True(_)) => Some(true),
                            Expression::Literal(Literal::False(_)) => Some(false),
                            _ => None,
                        };
                        match item.variable.name {
                            b"$table" => model.table = string,
                            b"$primaryKey" => model.primary_key = string,
                            b"$keyType" => model.key_type = string,
                            b"$incrementing" => model.incrementing = bool,
                            b"$timestamps" => model.timestamps = bool,
                            b"$casts" => model.casts = string_pairs(self.names, item.value),
                            _ => {}
                        }
                    }
                }
                ClassLikeMember::Method(m)
                    if self.parse_casts_method && m.name.value == b"casts" =>
                {
                    let mut first = FirstReturn::default();
                    first.walk_method(m, &mut ());
                    if let Some(value) = first.0.and_then(|r| r.value) {
                        model.casts_method = string_pairs(self.names, value);
                    }
                }
                _ => {}
            }
        }
        // Laravel 13's `#[Table]` (Model::initializeModelAttributes()): its name, key, key type, incrementing and
        // timestamps apply where the class itself doesn't declare the property; a parent's attribute is the parent's
        // source in the chain `model()` walks
        for attribute in class
            .attribute_lists
            .iter()
            .flat_map(|list| list.attributes.iter())
        {
            if !self
                .names
                .get(&attribute.name.span())
                .eq_ignore_ascii_case(TABLE_ATTRIBUTE.as_bytes())
            {
                continue;
            }
            let parameters = ["name", "key", "keyType", "incrementing", "timestamps"];
            for (i, argument) in attribute
                .argument_list
                .iter()
                .flat_map(|a| a.arguments.iter())
                .enumerate()
            {
                let (parameter, value) = match argument {
                    PartialArgument::Positional(a) => (parameters.get(i).copied(), a.value),
                    PartialArgument::Named(a) => (
                        parameters
                            .iter()
                            .copied()
                            .find(|p| p.as_bytes() == a.name.value),
                        a.value,
                    ),
                    _ => continue,
                };
                let bool = match value {
                    Expression::Literal(Literal::True(_)) => Some(true),
                    Expression::Literal(Literal::False(_)) => Some(false),
                    _ => None,
                };
                match parameter {
                    Some("name") if model.table.is_none() => {
                        model.table = constant_string(self.names, value);
                    }
                    // `key: false` is a model without a primary key, which doesn't increment
                    Some("key") if bool == Some(false) => {
                        model.incrementing = model.incrementing.or(Some(false));
                    }
                    Some("key") if model.primary_key.is_none() => {
                        model.primary_key = constant_string(self.names, value);
                    }
                    Some("keyType") if model.key_type.is_none() => {
                        model.key_type = constant_string(self.names, value);
                    }
                    Some("incrementing") if model.incrementing.is_none() => {
                        model.incrementing = bool;
                    }
                    Some("timestamps") if model.timestamps.is_none() => model.timestamps = bool,
                    _ => {}
                }
            }
        }
        self.scan
            .models
            .insert(name.to_ascii_lowercase(), (name, model));
    }

    fn walk_in_static_method_call(&mut self, call: &'ast StaticMethodCall<'arena>, (): &mut ()) {
        if resolved(self.names, call.class)
            .is_some_and(|c| c.eq_ignore_ascii_case("Illuminate\\Support\\Facades\\Date"))
            && matches!(&call.method, ClassLikeMemberSelector::Identifier(m) if m.value.eq_ignore_ascii_case(b"use"))
            && let Some(Argument::Positional(a)) = call.argument_list.arguments.first()
            && let Some(class) = constant_string(self.names, a.value)
        {
            self.scan.date_class = Some(class);
        }
    }
}

struct Model<'a> {
    name: &'a str,
    table: String,
    key: String,
    key_type: String,
    casts: HashMap<String, String>,
    dates: Vec<&'static str>,
}

fn model<'a>(scan: &'a Scan, codebase: &CodebaseMetadata, class: &str) -> Option<Model<'a>> {
    let (name, _) = scan.models.get(&class.to_ascii_lowercase())?;
    let mut chain = Vec::new();
    let mut current = Some(name.clone());
    while let Some(c) = current {
        if let Some((_, source)) = scan.models.get(&c.to_ascii_lowercase()) {
            chain.push(source);
        }
        current = codebase
            .get_class_like(c.as_bytes())
            .and_then(|m| m.direct_parent_class)
            .map(|p| p.as_str_lossy().into_owned());
    }
    fn first<T>(chain: &[&ModelSource], f: impl Fn(&ModelSource) -> Option<T>) -> Option<T> {
        chain.iter().find_map(|s| f(s))
    }
    let pivot = codebase.class_uses_trait(
        name.as_bytes(),
        b"Illuminate\\Database\\Eloquent\\Relations\\Concerns\\AsPivot",
    );
    let table = first(&chain, |s| s.table.clone()).unwrap_or_else(|| {
        let basename = class_basename(name);
        if pivot {
            snake(&singular(basename))
        } else {
            snake(&plural_studly(basename))
        }
    });
    let key = first(&chain, |s| s.primary_key.clone()).unwrap_or_else(|| "id".into());
    let unique = ["HasUniqueStringIds", "HasUuids", "HasUlids"]
        .iter()
        .any(|t| {
            codebase.class_uses_trait(
                name.as_bytes(),
                format!("Illuminate\\Database\\Eloquent\\Concerns\\{t}").as_bytes(),
            )
        });
    let key_type = if unique {
        "string".into()
    } else {
        first(&chain, |s| s.key_type.clone()).unwrap_or_else(|| "int".into())
    };
    let mut casts = HashMap::new();
    if !unique && first(&chain, |s| s.incrementing).unwrap_or(true) {
        casts.insert(key.clone(), key_type.clone());
    }
    casts.extend(
        first(&chain, |s| (!s.casts.is_empty()).then(|| s.casts.clone())).unwrap_or_default(),
    );
    if scan.parse_casts_method {
        casts.extend(
            first(&chain, |s| {
                (!s.casts_method.is_empty()).then(|| s.casts_method.clone())
            })
            .unwrap_or_default(),
        );
    } else {
        casts.extend(declared_casts(codebase, name));
    }
    let mut dates = Vec::new();
    if first(&chain, |s| s.timestamps).unwrap_or(true) {
        dates.extend(["created_at", "updated_at"]);
    }
    if codebase.class_uses_trait(
        name.as_bytes(),
        b"Illuminate\\Database\\Eloquent\\SoftDeletes",
    ) {
        dates.push("deleted_at");
    }
    Some(Model {
        name,
        table,
        key,
        key_type,
        casts,
        dates,
    })
}

fn declared_casts(codebase: &CodebaseMetadata, model: &str) -> Vec<(String, String)> {
    use mago_codex::ttype::atomic::array::TArray;
    use mago_codex::ttype::atomic::array::key::ArrayKey;
    use mago_codex::ttype::atomic::scalar::TScalar;
    let Some(returned) = codebase
        .get_declaring_method(model.as_bytes(), b"casts")
        .and_then(|m| m.return_type_metadata.as_ref())
    else {
        return Vec::new();
    };
    let [TAtomic::Array(TArray::Keyed(k))] = returned.type_union.types.as_ref() else {
        return Vec::new();
    };
    let Some(items) = k.known_items.as_deref().filter(|_| k.parameters.is_none()) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|(key, (_, value))| {
            let ArrayKey::String(key) = key else {
                return None;
            };
            let [TAtomic::Scalar(TScalar::String(v))] = value.types.as_ref() else {
                return None;
            };
            let v = String::from_utf8_lossy(v.get_known_literal_value()?).replace("\\\\", "\\");
            Some((key.as_str_lossy().into_owned(), v))
        })
        .collect()
}

fn scan_tables(
    app: Laravel,
    project: &Project,
    codebase: &CodebaseMetadata,
) -> (Scan, HashMap<String, schema::Table>) {
    let mut scan = Scan {
        parse_casts_method: app.parse_casts_method,
        ..Scan::default()
    };
    let id =
        |p: &std::path::PathBuf| mago_database::file::FileId::new(p.to_string_lossy().as_bytes());
    let elsewhere: HashMap<_, _> = project
        .scanned
        .iter()
        .chain(project.autoloaded.iter().map(|a| &a.path))
        .map(|p| (id(p), p))
        .collect();
    let analysed: std::collections::HashSet<_> = project.analysed.iter().map(id).collect();
    let mut models: Vec<_> = codebase
        .class_likes
        .values()
        .filter(|k| !analysed.contains(&k.span.file_id))
        .filter(|k| codebase.is_instance_of(k.name.as_bytes(), MODEL.as_bytes()))
        .filter_map(|k| elsewhere.get(&k.span.file_id).copied())
        .collect();
    models.sort();
    models.dedup();
    for file in project
        .analysed
        .iter()
        .chain(models)
        .filter_map(|p| read(p, FileType::Host).ok().flatten())
    {
        let arena = LocalArena::new();
        let program = parse_file(&arena, &file);
        let names = NameResolver::new(&arena).resolve(program);
        let parse_casts_method = app.parse_casts_method;
        Collector {
            scan: &mut scan,
            names: &names,
            codebase,
            parse_casts_method,
        }
        .walk_program(program, &mut ());
    }
    let model_key = |class: &str| model(&scan, codebase, class).map(|m| (m.table, m.key));
    let mut schema = SchemaAggregator::new(codebase, &model_key);
    // SquashedMigrationHelper::initializeTables(): the schema dumps first, by path (`ksort`)
    let mut dumps = files_below(app.schemas, &|path| {
        let path = path.to_ascii_lowercase();
        path.contains(".dump") || path.contains(".sql")
    });
    dumps.sort();
    for dump in dumps {
        if let Some(tables) = std::fs::read(&dump)
            .ok()
            .and_then(|sql| sql::parse_tables(&sql))
        {
            schema.add_schema_dump(tables);
        }
    }
    // MigrationHelper::initializeTables(): the migrations on top, by file name (a stable `uasort`)
    let mut migrations = migration_files(app.migrations);
    migrations.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    for file in migrations
        .iter()
        .filter_map(|p| read(p, FileType::Host).ok().flatten())
    {
        let arena = LocalArena::new();
        let program = parse_file(&arena, &file);
        let names = NameResolver::new(&arena).resolve(program);
        schema.add_statements(program, &names);
    }
    let tables = schema.tables;
    (scan, tables)
}

pub(crate) fn database_properties(
    app: Laravel,
    project: &Project,
    codebase: &CodebaseMetadata,
) -> (
    HashMap<String, std::collections::HashSet<String>>,
    HashMap<String, std::collections::HashSet<String>>,
) {
    let (scan, tables) = scan_tables(app, project, codebase);
    let columns = tables
        .iter()
        .map(|(name, t)| (name.clone(), t.columns.keys().cloned().collect()))
        .collect();
    let properties = scan
        .models
        .keys()
        .filter_map(|key| model(&scan, codebase, key))
        .filter(|m| {
            codebase
                .get_class_like(m.name.as_bytes())
                .is_some_and(|c| !c.flags.is_abstract())
        })
        .map(|m| {
            let mut properties: std::collections::HashSet<String> = tables
                .get(&m.table)
                .into_iter()
                .flat_map(|t| t.columns.keys().cloned())
                .collect();
            properties.insert(m.key);
            (m.name.to_ascii_lowercase(), properties)
        })
        .collect();
    (properties, columns)
}

pub(crate) type Models = HashMap<String, HashMap<String, TUnion>>;

pub(crate) fn register_models(registry: &mut PluginRegistry, models: std::sync::Arc<Models>) {
    registry.register_expression_hook(ModelAttributes { models });
}

pub(crate) fn models(
    app: Laravel,
    project: &Project,
    codebase: &CodebaseMetadata,
) -> HashMap<String, HashMap<String, TUnion>> {
    let (scan, tables) = scan_tables(app, project, codebase);
    let date_class = scan
        .date_class
        .clone()
        .filter(|c| c != "Illuminate\\Support\\Carbon");
    let date = named(date_class.as_deref().unwrap_or("Carbon\\Carbon"));
    let mut out = HashMap::new();
    for m in scan
        .models
        .keys()
        .filter_map(|key| model(&scan, codebase, key))
    {
        if codebase
            .get_class_like(m.name.as_bytes())
            .is_none_or(|c| c.flags.is_abstract())
        {
            continue;
        }
        let mut properties = HashMap::new();
        let table = tables.get(&m.table);
        if table.is_none_or(|t| !t.columns.contains_key(&m.key)) {
            properties.insert(m.key.clone(), column_type(&m.key_type));
        }
        for (name, column) in table.into_iter().flat_map(|t| &t.columns) {
            if has_accessor(codebase, m.name, name) {
                continue;
            }
            properties.insert(name.clone(), readable(&m, name, column, &date, codebase));
        }
        out.insert(m.name.to_ascii_lowercase(), properties);
    }
    out
}

fn named(class: &str) -> TUnion {
    get_named_object(Word::new(class.as_bytes()), None)
}

fn column_type(ty: &str) -> TUnion {
    match ty {
        "int" => get_int(),
        "non-negative-int" => get_int_range(Some(0), None),
        "string" => get_string(),
        "bool" => get_bool(),
        "float" => get_float(),
        _ => get_mixed(),
    }
}

fn readable(
    m: &Model,
    name: &str,
    column: &Column,
    date: &TUnion,
    codebase: &CodebaseMetadata,
) -> TUnion {
    let ty = if m.dates.contains(&name) {
        date.clone()
    } else if let Some(cast) = m.casts.get(name) {
        cast_type(cast, column_type(&column.readable_type), date, codebase)
    } else if matches!(column.readable_type.as_str(), "enum" | "set") {
        match &column.options {
            Some(options) if !options.is_empty() => TUnion::from_vec(
                options
                    .iter()
                    .flat_map(|o| {
                        get_literal_string(Word::new(o.as_bytes()))
                            .types
                            .into_owned()
                    })
                    .collect(),
            ),
            _ => get_string(),
        }
    } else {
        column_type(&column.readable_type)
    };
    if column.nullable {
        ty.as_nullable()
    } else {
        ty
    }
}

fn cast_type(cast: &str, original: TUnion, date: &TUnion, codebase: &CodebaseMetadata) -> TUnion {
    let cast = cast.split(':').find(|p| *p != "encrypted").unwrap_or(cast);
    match cast {
        "int" | "integer" | "timestamp" => {
            return if original.is_int() {
                original
            } else {
                get_int()
            };
        }
        "real" | "float" | "double" => return get_float(),
        "decimal" => return get_numeric_string(),
        "string" => return get_string(),
        "bool" | "boolean" => return get_bool(),
        "object" => return named("stdClass"),
        "array" | "json" => return get_keyed_array(get_arraykey(), get_mixed()),
        "collection" => return named("Illuminate\\Support\\Collection"),
        "date" | "datetime" => return date.clone(),
        "immutable_date" | "immutable_datetime" => return named(CARBON_IMMUTABLE),
        "Illuminate\\Database\\Eloquent\\Casts\\AsArrayObject"
        | "Illuminate\\Database\\Eloquent\\Casts\\AsEncryptedArrayObject" => {
            return named("Illuminate\\Database\\Eloquent\\Casts\\ArrayObject");
        }
        "Illuminate\\Database\\Eloquent\\Casts\\AsCollection"
        | "Illuminate\\Database\\Eloquent\\Casts\\AsEncryptedCollection" => {
            let mut key = get_arraykey();
            key.set_ignore_nullable_issues(true);
            key.set_ignore_falsable_issues(true);
            let collection = TUnion::from_atomic(TAtomic::Object(TObject::Named(
                TNamedObject::new_with_type_parameters(
                    Word::new(b"Illuminate\\Support\\Collection"),
                    Some(vec![key, get_mixed()]),
                ),
            )));
            let mut ty = collection.as_nullable();
            ty.set_ignore_nullable_issues(true);
            ty.set_ignore_falsable_issues(true);
            return ty;
        }
        "Illuminate\\Database\\Eloquent\\Casts\\AsStringable" => {
            return named("Illuminate\\Support\\Stringable");
        }
        _ => {}
    }
    let Some(class) = codebase.get_class_like(cast.as_bytes()) else {
        return original;
    };
    if class.kind.is_enum() {
        return named(class.original_name.as_str_lossy().as_ref());
    }
    let mut cast = class.name.as_str_lossy().into_owned();
    if codebase.is_instance_of(
        cast.as_bytes(),
        b"Illuminate\\Contracts\\Database\\Eloquent\\Castable",
    ) && let Some(TAtomic::Object(TObject::Named(n))) = codebase
        .get_declaring_method(cast.as_bytes(), b"castUsing")
        .and_then(|m| m.return_type_metadata.as_ref())
        .and_then(|t| t.type_union.types.first())
    {
        cast = n.name.as_str_lossy().into_owned();
    }
    if codebase.is_instance_of(
        cast.as_bytes(),
        b"Illuminate\\Contracts\\Database\\Eloquent\\CastsAttributes",
    ) {
        return codebase
            .get_declaring_method(cast.as_bytes(), b"get")
            .and_then(|m| m.return_type_metadata.as_ref())
            .map_or_else(get_mixed, |t| t.type_union.clone());
    }
    if codebase.is_instance_of(
        cast.as_bytes(),
        b"Illuminate\\Contracts\\Database\\Eloquent\\CastsInboundAttributes",
    ) {
        return original;
    }
    get_mixed()
}

pub(crate) fn has_accessor(codebase: &CodebaseMetadata, class: &str, property: &str) -> bool {
    let Some(method) = codebase.get_declaring_method(class.as_bytes(), camel(property).as_bytes())
    else {
        let getter = format!("get{}Attribute", studly(property));
        return codebase
            .get_declaring_method(class.as_bytes(), getter.as_bytes())
            .is_some();
    };
    let visibility = method.method_metadata.as_ref().map(|m| m.visibility);
    if visibility.is_none_or(|v| v.is_public() || v.is_private()) {
        return false;
    }
    method
        .return_type_metadata
        .as_ref()
        .is_some_and(|t| t.type_union.types.iter().all(|a| is_named(a, ATTRIBUTE)))
}

pub(crate) fn accessor_type(
    codebase: &CodebaseMetadata,
    class: &str,
    property: &str,
) -> Option<TUnion> {
    let Some(method) = codebase.get_declaring_method(class.as_bytes(), camel(property).as_bytes())
    else {
        let getter = format!("get{}Attribute", studly(property));
        let method = codebase.get_declaring_method(class.as_bytes(), getter.as_bytes())?;
        return Some(
            method
                .return_type_metadata
                .as_ref()
                .map_or_else(get_mixed, |t| t.type_union.clone()),
        );
    };
    let visibility = method.method_metadata.as_ref().map(|m| m.visibility);
    if visibility.is_none_or(|v| v.is_public() || v.is_private()) {
        return None;
    }
    let returned = &method.return_type_metadata.as_ref()?.type_union;
    match returned.types.as_ref() {
        [TAtomic::Object(TObject::Named(n))]
            if is_named(&TAtomic::Object(TObject::Named(n.clone())), ATTRIBUTE) =>
        {
            n.type_parameters
                .as_deref()
                .and_then(|p| p.first())
                .cloned()
        }
        _ => None,
    }
}

/// The model a class that neither declares `property` nor is a model names in its `@mixin` tags.
fn mixin_model(
    codebase: &CodebaseMetadata,
    object: &TNamedObject,
    property: &str,
) -> Option<TNamedObject> {
    let class = object.name.as_bytes();
    let k = codebase.get_class_like(class)?;
    if codebase.is_instance_of(class, MODEL.as_bytes())
        || codebase
            .get_declaring_property(class, property.as_bytes())
            .is_some()
        || k.magic_property_ids
            .contains_key(&Word::new(property.as_bytes()))
    {
        return None;
    }
    k.mixins
        .iter()
        .flat_map(|m| m.type_union.types.iter())
        .find_map(|m| match m {
            TAtomic::Object(TObject::Named(m))
                if codebase.is_instance_of(m.name.as_bytes(), MODEL.as_bytes()) =>
            {
                Some(m.clone())
            }
            _ => None,
        })
}

fn relation_count(codebase: &CodebaseMetadata, class: &[u8], property: &str) -> bool {
    let Some(relation) = property.strip_suffix("_count") else {
        return false;
    };
    codebase.is_instance_of(class, MODEL.as_bytes())
        && [camel(relation), relation.to_string()].iter().any(|m| {
            codebase
                .get_declaring_method(class, m.as_bytes())
                .and_then(|f| f.return_type_metadata.as_ref())
                .is_some_and(|t| collections::is_relation(codebase, &t.type_union))
        })
}

fn is_named(atomic: &TAtomic, class: &str) -> bool {
    matches!(atomic, TAtomic::Object(TObject::Named(n)) if n.name.as_bytes().eq_ignore_ascii_case(class.as_bytes()))
}

struct ModelAttributes {
    models: std::sync::Arc<Models>,
}

impl Provider for ModelAttributes {
    fn meta() -> &'static ProviderMeta {
        static META: ProviderMeta = ProviderMeta::new(
            "pun-laravel-model-attributes",
            "Laravel model attributes",
            "Model columns from migrations",
        );
        &META
    }
}

impl ModelAttributes {
    fn property(
        &self,
        codebase: &CodebaseMetadata,
        object: &TNamedObject,
        name: &str,
    ) -> Option<TUnion> {
        let class = object.name.as_bytes();
        let dollar = format!("${name}");
        if let Some(ty) = codebase.get_property_type(class, dollar.as_bytes()) {
            let mut ty = ty.clone();
            let options = TypeExpansionOptions {
                self_class: Some(codebase.get_class_like(class)?.name),
                static_class_type: StaticClassType::Object(TObject::Named(object.clone())),
                ..Default::default()
            };
            expand_union(codebase, &mut ty, &options);
            return Some(ty);
        }
        if codebase.is_instance_of(class, MODEL.as_bytes())
            && let Some(ty) = accessor_type(codebase, &object.name.as_str_lossy(), name)
        {
            return Some(ty);
        }
        if let Some(ty) = self
            .models
            .get(&object.name.as_str_lossy().to_ascii_lowercase())
            .and_then(|m| m.get(name))
        {
            return Some(ty.clone());
        }
        if relation_count(codebase, class, name) {
            return Some(get_int_range(Some(0), None));
        }
        if codebase.is_instance_of(class, MODEL.as_bytes()) {
            return collections::relation_property(codebase, object, name);
        }
        None
    }

    fn only(&self, context: &HookContext<'_, '_>, call: &MethodCall<'_>) -> Option<TUnion> {
        use mago_codex::ttype::atomic::array::TArray;
        use mago_codex::ttype::atomic::array::key::ArrayKey;
        use mago_codex::ttype::atomic::array::keyed::TKeyedArray;
        use mago_codex::ttype::atomic::scalar::TScalar;
        let ClassLikeMemberSelector::Identifier(method) = &call.method else {
            return None;
        };
        if !method.value.eq_ignore_ascii_case(b"only") || call.argument_list.arguments.is_empty() {
            return None;
        }
        let codebase = context.codebase();
        let model = context.get_expression_type(call.object)?;
        let [TAtomic::Object(TObject::Named(object))] = model.types.as_ref() else {
            return None;
        };
        if !codebase.is_instance_of(object.name.as_bytes(), MODEL.as_bytes()) {
            return None;
        }
        let strings = |t: &TUnion| -> Vec<Vec<u8>> {
            let found: Vec<Vec<u8>> = t
                .types
                .iter()
                .map_while(|a| match a {
                    TAtomic::Scalar(TScalar::String(s)) => {
                        s.get_known_literal_value().map(<[u8]>::to_vec)
                    }
                    _ => None,
                })
                .collect();
            if found.len() == t.types.len() {
                found
            } else {
                Vec::new()
            }
        };
        let mut keys: Vec<Vec<u8>> = Vec::new();
        for argument in &call.argument_list.arguments {
            let value = match argument {
                Argument::Positional(a) => a.value,
                Argument::Named(a) => a.value,
            };
            let ty = context.get_expression_type(value)?;
            let mut candidates = vec![strings(ty)];
            for a in ty.types.iter() {
                let item = match a {
                    TAtomic::Array(TArray::List(l)) => TUnion::from_vec(
                        l.known_elements
                            .iter()
                            .flat_map(|k| k.values())
                            .flat_map(|(_, v)| v.types.iter().cloned())
                            .chain(
                                (!l.element_type.is_never())
                                    .then(|| l.element_type.types.iter().cloned())
                                    .into_iter()
                                    .flatten(),
                            )
                            .collect(),
                    ),
                    TAtomic::Array(TArray::Keyed(k)) => TUnion::from_vec(
                        k.known_items
                            .iter()
                            .flat_map(|k| k.values())
                            .flat_map(|(_, v)| v.types.iter().cloned())
                            .chain(
                                k.parameters
                                    .iter()
                                    .flat_map(|(_, v)| v.types.iter().cloned()),
                            )
                            .collect(),
                    ),
                    _ => continue,
                };
                candidates.push(strings(&item));
            }
            let Some(found) = candidates.into_iter().find(|c| !c.is_empty()) else {
                return Some(TUnion::from_atomic(TAtomic::Array(TArray::Keyed(
                    TKeyedArray::new_with_parameters(
                        std::sync::Arc::new(get_string()),
                        std::sync::Arc::new(get_mixed()),
                    ),
                ))));
            };
            keys.extend(found);
        }
        let items = keys
            .into_iter()
            .map(|k| {
                let name = String::from_utf8_lossy(&k).into_owned();
                let ty = self
                    .property(codebase, object, &name)
                    .unwrap_or_else(get_null);
                (ArrayKey::String(Word::new(&k)), (false, ty))
            })
            .collect();
        Some(TUnion::from_atomic(TAtomic::Array(TArray::Keyed(
            TKeyedArray::new()
                .with_known_items(items)
                .with_non_empty(true),
        ))))
    }

    fn proxy_property(
        &self,
        codebase: &CodebaseMetadata,
        proxy: &TNamedObject,
        name: &str,
    ) -> Option<TUnion> {
        use mago_codex::ttype::atomic::scalar::TScalar;
        let [method, value, collection] = proxy.type_parameters.as_deref()? else {
            return None;
        };
        let [TAtomic::Scalar(TScalar::String(method))] = method.types.as_ref() else {
            return None;
        };
        let method = String::from_utf8_lossy(method.get_known_literal_value()?).into_owned();
        let [TAtomic::Object(TObject::Named(object))] = value.types.as_ref() else {
            return None;
        };
        let returned = self.property(codebase, object, name)?;
        collections::proxy_result(codebase, &method, value, || Some(returned), collection)
    }
}

/// The analyzer's expression ID of `$a->b->c`, a variable followed by named property fetches.
fn fetch_id(e: &Expression<'_>) -> Option<String> {
    match e {
        Expression::Parenthesized(p) => fetch_id(p.expression),
        Expression::Variable(Variable::Direct(v)) => {
            Some(String::from_utf8_lossy(v.name).into_owned())
        }
        Expression::Access(Access::Property(p)) => {
            let ClassLikeMemberSelector::Identifier(id) = &p.property else {
                return None;
            };
            Some(format!(
                "{}->{}",
                fetch_id(p.object)?,
                String::from_utf8_lossy(id.value)
            ))
        }
        _ => None,
    }
}

impl ExpressionHook for ModelAttributes {
    fn after_expression(
        &self,
        expr: &Expression<'_>,
        context: &mut HookContext<'_, '_>,
    ) -> HookResult<()> {
        if let Expression::Call(Call::Method(call)) = expr {
            if let Some(ty) = self.only(context, call) {
                context.set_expression_type(expr, ty);
            }
            return Ok(());
        }
        let (object, property, nullsafe) = match expr {
            Expression::Access(Access::Property(p)) => (p.object, &p.property, false),
            Expression::Access(Access::NullSafeProperty(p)) => (p.object, &p.property, true),
            _ => return Ok(()),
        };
        let ClassLikeMemberSelector::Identifier(property) = property else {
            return Ok(());
        };
        let Some(object_type) = context.get_expression_type(object) else {
            return Ok(());
        };
        // MutatingScope::getInstancePropertyReflection() reads `->` on the receiver's members that have the
        // property, so a nullable receiver's `null` adds nothing; only a nullsafe fetch's `null` stays
        let object_type = if nullsafe || object_type.has_nullsafe_null() || object_type.is_null() {
            object_type.clone()
        } else {
            object_type.remove_null()
        };
        let name = String::from_utf8_lossy(property.value);
        let dollar = format!("${name}");
        let mut types = Vec::new();
        let (mut short_circuits, mut properties, mut non_nullable) = (false, 0, 0);
        for atomic in object_type.types.iter() {
            let ty = match atomic {
                TAtomic::Null if nullsafe => {
                    short_circuits = true;
                    types.push(get_null());
                    continue;
                }
                TAtomic::Object(TObject::Named(n))
                    if n.name.as_bytes().eq_ignore_ascii_case(PROXY.as_bytes()) =>
                {
                    match self.proxy_property(context.codebase(), n, &name) {
                        Some(ty) => ty,
                        None => return Ok(()),
                    }
                }
                TAtomic::Object(TObject::Named(n)) => {
                    // MixinPropertiesClassReflectionExtension: a class without the property reads it on its
                    // `@mixin` model (a JsonResource's `@mixin Post`), relations and counts included
                    let mixin = mixin_model(context.codebase(), n, &dollar);
                    let n = mixin.as_ref().unwrap_or(n);
                    let class = n.name.as_bytes();
                    let declared = context
                        .codebase()
                        .get_declaring_property(class, dollar.as_bytes())
                        .is_some()
                        || context.codebase().get_class_like(class).is_some_and(|c| {
                            c.magic_property_ids
                                .contains_key(&Word::new(dollar.as_bytes()))
                        });
                    let column = self
                        .models
                        .get(&n.name.as_str_lossy().to_ascii_lowercase())
                        .and_then(|m| m.get(name.as_ref()));
                    let accessor = || {
                        context
                            .codebase()
                            .is_instance_of(class, MODEL.as_bytes())
                            .then(|| {
                                accessor_type(context.codebase(), &n.name.as_str_lossy(), &name)
                            })
                            .flatten()
                    };
                    match column {
                        _ if !declared && let Some(ty) = accessor() => ty,
                        Some(ty) if !declared => ty.clone(),
                        None if !declared && relation_count(context.codebase(), class, &name) => {
                            get_int_range(Some(0), None)
                        }
                        None if !declared
                            && context.codebase().is_instance_of(class, MODEL.as_bytes()) =>
                        {
                            match collections::relation_property(context.codebase(), n, &name) {
                                Some(ty) => ty,
                                None if n.intersection_types.is_none()
                                    && context.get_expression_type(expr).is_some_and(|t| t.is_mixed())
                                    && !context.block_context().flags.inside_assignment()
                                    && context.codebase().get_class_like(class).is_some_and(|k| {
                                        k.mixins.iter().flat_map(|m| m.type_union.types.iter()).all(|m| {
                                            matches!(m, TAtomic::Object(TObject::Named(m)) if {
                                                let c = context.codebase();
                                                c.get_declaring_property(m.name.as_bytes(), dollar.as_bytes()).is_none()
                                                    && c.get_class_like(m.name.as_bytes()).is_none_or(|k| {
                                                        !k.magic_property_ids.contains_key(&Word::new(dollar.as_bytes()))
                                                    })
                                            })
                                        })
                                    }) =>
                                {
                                    get_error()
                                }
                                None => return Ok(()),
                            }
                        }
                        _ => return Ok(()),
                    }
                }
                _ => return Ok(()),
            };
            properties += 1;
            non_nullable += usize::from(!ty.is_nullable());
            types.push(ty);
        }
        let mut ty = match types.len() {
            0 => return Ok(()),
            1 => types.pop().unwrap_or_else(get_mixed),
            _ => TUnion::from_vec(
                types
                    .into_iter()
                    .flat_map(|t| t.types.into_owned())
                    .collect(),
            ),
        };
        // The `null` of a nullsafe fetch on a null receiver short-circuits the rest of the chain, as
        // analyze_property_access() marks it, so `$a?->relation->method()` stays nullable
        // (NullsafeShortCircuitingHelper)
        if short_circuits && properties > 0 && non_nullable == properties {
            ty.set_nullsafe_null(true);
        }
        // The analyzer remembers a fetch under its expression ID, first as its own `mixed`: this type replaces it,
        // so a later `instanceof` or `!== null` narrows it, and a remembered type that is not `mixed` (this one,
        // narrowed or assigned) is what the fetch reads, as for a declared property
        if !nullsafe && let Some(id) = fetch_id(expr) {
            let id = Word::new(id.as_bytes());
            match context.block_context().locals.get(&id) {
                Some(remembered) if !remembered.is_mixed() => return Ok(()),
                Some(_) => {
                    let remembered = std::rc::Rc::new(ty.clone());
                    context.block_context_mut().locals.insert(id, remembered);
                }
                None => {}
            }
        }
        context.set_expression_type(expr, ty);
        Ok(())
    }
}
