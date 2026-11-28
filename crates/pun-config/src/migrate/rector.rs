use super::Parts;
use crate::composer::Composer;
use crate::{Config, mapping, rules};
use anyhow::Result;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write;
use std::path::Path;

#[derive(Debug, Default, Deserialize)]
pub struct RectorConfig {
    pub rules: Vec<String>,
    #[serde(default)]
    pub configured: BTreeMap<String, Configured>,
    #[serde(default)]
    pub skip_rules: Map<String, Value>,
    #[serde(default)]
    pub skip_paths: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub sets: Vec<String>,
    #[serde(default)]
    pub php_sets: bool,
    /// The Rector settings (`Rector\Configuration\Option` names) rector.php changes from Rector's defaults.
    #[serde(default)]
    pub settings: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Configured {
    pub configuration: Value,
    #[serde(default)]
    pub constants: Vec<String>,
}

fn kebab(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_uppercase() && !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        out.push(if c == '_' {
            '-'
        } else {
            c.to_ascii_lowercase()
        });
    }
    out
}

fn toml_value(v: &Value) -> Result<toml::Value, String> {
    Ok(match v {
        Value::Null => return Err("its configuration holds a null, which pun.toml can't".into()),
        Value::Bool(b) => toml::Value::Boolean(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => toml::Value::Integer(i),
            None => toml::Value::Float(n.as_f64().unwrap()),
        },
        Value::String(s) => toml::Value::String(s.clone()),
        Value::Array(a) => toml::Value::Array(a.iter().map(toml_value).collect::<Result<_, _>>()?),
        Value::Object(o) => {
            if let Some(t) = o.get("@unsupported").and_then(Value::as_str) {
                return Err(format!(
                    "its configuration holds a {t}, which pun.toml can't"
                ));
            }
            let mut table = toml::Table::new();
            match o.get("@props").and_then(Value::as_object) {
                Some(props) => {
                    for (k, v) in props.iter().filter(|(_, v)| !v.is_null()) {
                        table.insert(kebab(k), toml_value(v)?);
                    }
                }
                None => {
                    for (k, v) in o {
                        table.insert(k.clone(), toml_value(v)?);
                    }
                }
            }
            toml::Value::Table(table)
        }
    })
}

fn rule_options(c: &Configured) -> Result<toml::Table, String> {
    let named = matches!(&c.configuration, Value::Object(o)
        if !o.is_empty() && o.keys().all(|k| c.constants.contains(k)));
    let table: toml::Table = match toml_value(&c.configuration)? {
        toml::Value::Table(t) => (t.into_iter())
            .map(|(k, v)| (if named { kebab(&k) } else { k }, v))
            .collect(),
        v => toml::Table::from_iter([("items".to_string(), v)]),
    };
    match ["level", "fix"]
        .into_iter()
        .find(|k| table.contains_key(*k))
    {
        Some(k) => Err(format!(
            "its configuration has a `{k}` key, which pun.toml keeps for the rule's level"
        )),
        None => Ok(table),
    }
}

#[derive(Debug, Default)]
pub struct Rector {
    pub sets: Vec<String>,
    pub want: Vec<(String, String)>,
    pub overrides: Vec<(Vec<String>, String)>,
    pub exclude: Vec<String>,
    pub paths: Vec<String>,
    pub unsupported: Vec<(String, String)>,
    pub options: BTreeMap<String, toml::Table>,
}

/// A Rector set file rector.php names, as its Pun set (`rector/dead-code`, `rector-laravel/laravel-code-quality`), when
/// Pun has it.
fn set_of(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?.strip_suffix(".php")?;
    let pun = if path.contains("rector-phpunit") {
        format!("rector-phpunit/{name}")
    } else if path.contains("rector-laravel") {
        format!("rector-laravel/{name}")
    } else if path.contains("pest-plugin-rector") {
        format!("pest/{}", name.strip_prefix("pest-").unwrap_or(name))
    } else if path.contains("/rector/rector/") {
        format!("rector/{name}")
    } else {
        return None;
    };
    crate::schema::rector_sets()
        .contains_key(&pun)
        .then_some(pun)
}

/// The Pun options that keep a rule merged with a PHP-CS-Fixer fixer to what its Rector rules check, so a project
/// clean under Rector is clean under Pun. The fixer's own checks run only when it is named in pint.json.
const AS_RECTOR: [(&str, &str); 5] = [
    ("lint/control-flow/no-return-assignment", "skip-commented"),
    ("lint/operators/prefer-coalesce", "null-checks"),
    ("lint/style/prefer-class-constant", "namespaced-only"),
    (
        "lint/style/prefer-empty-string-compare",
        "identical-or-greater-only",
    ),
    ("lint/style/prefer-short-ternary", "all-contexts"),
];

/// A rule merged from several Rector rules runs only the rewrites of the ones rector.php runs (`classes`, short names):
/// the rule's Pun-only list option whose values name its Rector rules, set to those, when it leaves one out.
fn only_these(id: &str, classes: &HashSet<String>) -> Option<(String, toml::Value)> {
    let option = crate::options::rule_options(id).iter().find(|o| {
        o.pun
            && o.kind == crate::options::Kind::List
            && (o.values.iter()).any(|(_, rector)| classes.contains(&format!("{rector}Rector")))
    })?;
    let parts: Vec<toml::Value> = (option.values.iter())
        .filter(|(_, rector)| classes.contains(&format!("{rector}Rector")))
        .map(|(pun, _)| toml::Value::String(pun.clone()))
        .collect();
    (parts.len() < option.values.len()).then(|| (option.name.clone(), toml::Value::Array(parts)))
}

/// The options that keep rule `id` to what `classes` (short names of its Rector rules that run) rewrite: Rector's
/// checks for a rule merged with a PHP-CS-Fixer fixer, and only those Rector rules' rewrites for a rule merged from
/// several.
pub(crate) fn as_rector_options(id: &str, classes: &BTreeSet<String>) -> toml::Table {
    let mut out = toml::Table::new();
    if let Some((_, key)) = AS_RECTOR.iter().find(|(r, _)| *r == id) {
        out.insert((*key).to_string(), toml::Value::Boolean(true));
    }
    let classes: HashSet<String> = classes.iter().cloned().collect();
    if let Some((key, parts)) = only_these(id, &classes) {
        out.insert(key, parts);
    }
    out
}

/// What rector.php's settings become: none has a pun.toml equivalent, so each is listed as not migrated, by the
/// `RectorConfig` method that sets it.
const SETTINGS: [(&str, &str, &str); 10] = [
    ("auto_import_names", "withImportNames()", IMPORT_NAMES),
    (
        "auto_import_doc_block_names",
        "withImportNames()",
        IMPORT_NAMES,
    ),
    ("remove-unused-imports", "withImportNames()", IMPORT_NAMES),
    (
        "treat_classes_as_final",
        "withTreatClassesAsFinal()",
        "no project-wide setting in Pun; `lint/types/add-param-type-method-call` and `lint/php-8.3/add-const-type` take it as their `treat-classes-as-final` option",
    ),
    (
        "php_version_features",
        "withPhpVersion()",
        "Pun takes the PHP version from `require.php` in composer.json; set [php] version for another",
    ),
    ("file_extensions", "withFileExtensions()", NO_SETTING),
    (
        "new_line_on_fluent_call",
        "withFluentCallNewLine()",
        NO_SETTING,
    ),
    ("indent-char", "withIndent()", NO_SETTING),
    ("indent-size", "withIndent()", NO_SETTING),
    (
        "type_guarded_classes",
        "withTypeGuardedClasses()",
        NO_SETTING,
    ),
];

const IMPORT_NAMES: &str = "no project-wide setting in Pun: lint fixes write class names fully qualified, and `lint/imports/no-unused` removes unused imports";
const NO_SETTING: &str = "no pun.toml setting for it";

fn by_class() -> &'static BTreeMap<String, toml::Value> {
    static T: std::sync::LazyLock<BTreeMap<String, toml::Value>> = std::sync::LazyLock::new(|| {
        let mut out = BTreeMap::new();
        for (tool, s) in mapping::under("upstream") {
            if !(tool.starts_with("rector") || tool == "pest-plugin-rector") {
                continue;
            }
            for e in s.table().values() {
                if let Some(c) = e.get("class").and_then(toml::Value::as_str) {
                    out.insert(c.to_string(), e.clone());
                }
            }
        }
        out
    });
    &T
}

fn short(class: &str) -> String {
    class.rsplit('\\').next().unwrap().to_string()
}

pub(super) fn relative(root: &Path, path: &str) -> String {
    let p = Path::new(path);
    match p.strip_prefix(root) {
        Ok(r) if r.as_os_str().is_empty() => ".".into(),
        Ok(r) => r.display().to_string(),
        Err(_) => path.trim_end_matches('/').to_string(),
    }
}

/// A skip path as pun.toml globs. Rector's `FileInfoMatcher` runs `fnmatch()` on the absolute path, where the `*` of a
/// leading `*/` also covers the project root, so `*/tests/*` skips the top-level `tests/` too; Pun matches the path
/// relative to the root, which needs `tests/*` beside it.
fn skip_globs(root: &Path, path: &str) -> Vec<String> {
    let glob = relative(root, path);
    match glob.strip_prefix("*/") {
        Some(rest) if !rest.is_empty() => vec![rest.to_string(), glob],
        _ => vec![glob],
    }
}

pub fn from_rector(root: &Path, r: &RectorConfig) -> Rector {
    let mut out = Rector::default();
    for set in &r.sets {
        if let Some(pun) = set_of(set)
            && !out.sets.contains(&pun)
        {
            out.sets.push(pun);
        }
    }
    if r.php_sets {
        // `rector/php-sets` holds every PHP level set up to the PHP version pun.toml resolves from composer.json, so
        // the level sets Rector loaded for withPhpSets() (`rector/php52` to `rector/php84`) would only repeat it. A
        // level above that version, named on its own in withSets(), stays.
        let php = crate::composer::detect(root)
            .ok()
            .and_then(|c| c.php_version())
            .unwrap_or(crate::PhpVersion::FLOOR);
        let target = php.major() * 10 + php.minor();
        let covered = |s: &String| {
            (s.strip_prefix("rector/php"))
                .and_then(|v| v.parse::<u32>().ok())
                .is_some_and(|v| v <= target)
        };
        out.sets.retain(|s| !covered(s));
        out.sets.push(crate::schema::PHP_SETS.to_string());
    }
    let mut seen = HashSet::new();
    let mut runs: BTreeMap<String, HashSet<String>> = BTreeMap::new();
    for class in &r.rules {
        let skip = r.skip_rules.get(class);
        if skip.is_some_and(Value::is_null) {
            continue;
        }
        let entry = by_class().get(class);
        let pun = entry
            .and_then(|e| e.get("pun"))
            .and_then(toml::Value::as_str);
        let why = match (entry, pun) {
            (None, _) => Some("not a Rector rule Pun knows (a custom rule or another package's)"),
            (Some(e), None) => e
                .get("unsupported")
                .or(e.get("drop"))
                .and_then(toml::Value::as_str),
            _ => None,
        };
        if let Some(why) = why {
            out.unsupported.push((short(class), why.to_string()));
            continue;
        }
        let id = pun.unwrap().to_string();
        match r.configured.get(class).map(rule_options) {
            Some(Err(why)) => {
                out.unsupported
                    .push((short(class), format!("configured; {why}")));
                continue;
            }
            Some(Ok(options)) => {
                out.options.insert(id.clone(), options);
            }
            None => {}
        }
        if let Some((_, key)) = AS_RECTOR.iter().find(|(r, _)| *r == id) {
            (out.options.entry(id.clone()).or_default())
                .insert((*key).to_string(), toml::Value::Boolean(true));
        }
        if let Some(paths) = skip.and_then(Value::as_array)
            && id.starts_with("lint/")
        {
            let paths = (paths.iter().filter_map(Value::as_str))
                .flat_map(|p| skip_globs(root, p))
                .collect();
            out.overrides.push((paths, id.clone()));
        }
        (runs.entry(id.clone()).or_default()).insert(short(class));
        if seen.insert(id.clone()) {
            out.want.push((id, short(class)));
        }
    }
    for (id, classes) in &runs {
        if let Some((key, parts)) = only_these(id, classes) {
            (out.options.entry(id.clone()).or_default()).insert(key, parts);
        }
    }
    for (option, method, why) in SETTINGS {
        let item = (method.to_string(), why.to_string());
        if r.settings.iter().any(|s| s == option) && !out.unsupported.contains(&item) {
            out.unsupported.push(item);
        }
    }
    out.exclude = (r.skip_paths.iter())
        .flat_map(|p| skip_globs(root, p))
        .collect();
    out.paths = r.paths.iter().map(|p| relative(root, p)).collect();
    out
}

#[derive(Debug, Default, PartialEq)]
pub struct Reconciled {
    pub on: usize,
    pub off: usize,
    pub waiting: usize,
    pub unsupported: Vec<(String, String)>,
}

pub fn compose(root: &Path, parts: &mut Parts, rector: &Rector) -> Result<Reconciled> {
    let quoted = |v: &[String]| toml::Value::try_from(v).unwrap().to_string();
    if !rector.sets.is_empty() {
        writeln!(parts.lint, "sets = {}", quoted(&rector.sets)).unwrap();
    }
    if !rector.exclude.is_empty() {
        writeln!(parts.lint, "exclude = {}", quoted(&rector.exclude)).unwrap();
    }
    let mut out = Reconciled::default();
    let cfg = resolve(root, &parts.render())?;
    let on: Vec<&str> = cfg.lint.rules.iter().map(|r| r.id.as_str()).collect();
    let given: HashSet<String> = (parts.lint_rules.iter())
        .map(|(k, _)| format!("lint/{k}"))
        .collect();
    let want: HashSet<&str> = rector.want.iter().map(|w| w.0.as_str()).collect();
    for (id, class) in &rector.want {
        if !id.starts_with("lint/") {
            let runs = (cfg.rules.iter()).any(|r| crate::rule_of(&r.name) == Some(id.as_str()));
            let why = format!("the format rule `{id}` in Pun; turn it on in [format.rules]");
            out.unsupported
                .extend((!runs).then(|| (class.clone(), why)));
            continue;
        }
        if given.contains(id) {
            // Rector runs the rule on the files Pint's finder skips too.
            parts.lint_skip_rules.retain(|r| *r != id[5..]);
            continue;
        }
        let options = rector.options.get(id);
        if on.contains(&id.as_str()) && options.is_none() {
            out.waiting += usize::from(!ported(id));
        } else {
            let value = options.map_or("\"error\"".into(), |t| {
                toml::Value::Table(t.clone()).to_string()
            });
            let turned_on = super::require_lint(parts, id, &value);
            out.on += usize::from(turned_on);
            let why = "not supported by Pun yet".to_string();
            out.unsupported
                .extend((!turned_on).then(|| (class.clone(), why)));
        }
    }
    for id in on {
        if !want.contains(id) && !given.contains(id) {
            parts
                .lint_rules
                .push((id[5..].to_string(), "\"off\"".into()));
            out.off += 1;
        }
    }
    // Rector rules merged into one Pun rule can be skipped for the same paths: the rule is named once per override.
    let mut by_paths: Vec<(&Vec<String>, Vec<&str>)> = vec![];
    for (paths, id) in &rector.overrides {
        match by_paths.iter_mut().find(|(p, _)| *p == paths) {
            Some((_, ids)) if ids.contains(&&id[5..]) => {}
            Some((_, ids)) => ids.push(&id[5..]),
            None => by_paths.push((paths, vec![&id[5..]])),
        }
    }
    for (paths, ids) in by_paths {
        let rules: Vec<String> = ids.iter().map(|id| format!("\"{id}\" = \"off\"")).collect();
        write!(
            parts.overrides,
            "{}[[lint.overrides]]\nfiles = {}\nrules = {{ {} }}\n",
            if parts.overrides.is_empty() { "" } else { "\n" },
            quoted(paths),
            rules.join(", ")
        )
        .unwrap();
    }
    Ok(out)
}

pub(super) fn ported(id: &str) -> bool {
    (rules().iter().find(|(r, _)| r == id))
        .and_then(|(_, e)| e.get("ported"))
        .and_then(toml::Value::as_bool)
        == Some(true)
}

pub(super) fn resolve(root: &Path, text: &str) -> Result<Config> {
    let composer: Composer = crate::composer::detect(root)?;
    crate::parse(
        root,
        &root.join("pun.toml"),
        text,
        &crate::Options::default(),
        &composer,
        vec![],
    )
}
