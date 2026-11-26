use super::Parts;
use super::rector::resolve;
use crate::phpstan_parameters::SWITCH_LEVELS;
use crate::{AnalysePreset, mapping, rules};
use anyhow::Result;
use pun_phpstan::config::{Options, ProjectFiles, resolve_with};
use pun_phpstan::di::Files;
use pun_phpstan::{Array, Value};
use std::collections::BTreeSet;
use std::fmt::Write;
use std::path::Path;

#[derive(Debug, Default)]
pub struct Phpstan {
    pub level: u8,
    /// `level: max` (and `level` is 10).
    pub max: bool,
    pub no_rules: bool,
    pub paths: Vec<String>,
    pub exclude: Vec<String>,
    pub stubs: Vec<String>,
    /// `scanFiles` and `scanDirectories`, as `[analyse] scan`.
    pub scan: Vec<String>,
    pub bootstrap: Vec<String>,
    pub ignores: Vec<String>,
    pub baselines: Vec<String>,
    pub unsupported: Vec<(String, String)>,
    pub extensions: BTreeSet<String>,
    pub bleeding_edge: bool,
    /// `phpVersion` (its `min` in the range form) as `[php] version`'s `major.minor`.
    pub php_version: Option<String>,
    pub parameters: Array,
    pub changed: Vec<(String, Value)>,
    /// phpat's settings, when the config includes phpat or tags a service `phpat.test`.
    pub phpat: Option<crate::extensions::Phpat>,
}

const BUILT_IN: [(&str, Option<&str>); 11] = [
    ("larastan/larastan", Some("larastan")),
    ("phpstan/phpstan-strict-rules", Some("phpstan-strict-rules")),
    (
        "phpstan/phpstan-deprecation-rules",
        Some("phpstan-deprecation-rules"),
    ),
    ("pestphp/pest-plugin-phpstan", None),
    ("phpstan/phpstan-phpunit", None),
    ("phpstan/extension-installer", None),
    ("nesbot/carbon", None),
    ("phpstan/phpstan-mockery", None),
    (
        "spaze/phpstan-disallowed-calls",
        Some("phpstan-disallowed-calls"),
    ),
    ("tomasvotruba/type-coverage", Some("type-coverage")),
    ("phpat/phpat", Some("phpat")),
];

const HANDLED: [&str; 20] = [
    "level",
    "phpat",
    "stubFiles",
    "paths",
    "excludePaths",
    "ignoreErrors",
    "reportUnmatchedIgnoredErrors",
    "tmpDir",
    "parallel",
    "memoryLimitFile",
    "editorUrl",
    "editorUrlTitle",
    "errorFormat",
    "tips",
    "fileExtensions",
    "bootstrapFiles",
    "scanFiles",
    "scanDirectories",
    "resultCachePath",
    "resultCacheChecksProjectExtensionFilesDependencies",
];

struct WithoutBaselines {
    files: ProjectFiles,
    seen: std::cell::RefCell<BTreeSet<String>>,
    virtual_file: std::cell::RefCell<Option<(String, String)>>,
}

fn is_baseline(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.to_ascii_lowercase().contains("baseline")
}

impl Files for WithoutBaselines {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        if let Some((p, text)) = &*self.virtual_file.borrow()
            && p == path
        {
            return Some(text.clone().into_bytes());
        }
        if is_baseline(path) && self.files.is_file(path) {
            self.seen.borrow_mut().insert(path.to_string());
            return Some(vec![]);
        }
        self.files.read(path)
    }

    fn is_file(&self, path: &str) -> bool {
        (self.virtual_file.borrow().as_ref()).is_some_and(|(p, _)| p == path)
            || self.files.is_file(path)
    }
}

fn relative(root: &Path, path: &str) -> String {
    let root = root.display().to_string();
    match path.strip_prefix(&root) {
        Some("") => ".".into(),
        Some(r) if r.starts_with('/') => r[1..].trim_end_matches('/').to_string(),
        _ => path.to_string(),
    }
}

fn strings(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Str(s) | Value::OptionalPath(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.values().flat_map(|v| strings(Some(v))).collect(),
        _ => vec![],
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "long; to be split (code audit: functions over 100 lines)"
)]
pub fn from_phpstan(root: &Path, config: &Path) -> Result<Phpstan> {
    let cwd = root.display().to_string();
    let mut o = Options::for_project(&cwd);
    o.config = Some(config.display().to_string());
    let phar = std::fs::canonicalize(&o.phar)
        .map_or_else(|_| o.phar.clone(), |p| p.to_string_lossy().into_owned());
    o.phar.clone_from(&phar);
    let files = WithoutBaselines {
        files: ProjectFiles::new(&phar, &cwd),
        seen: std::cell::RefCell::default(),
        virtual_file: std::cell::RefCell::default(),
    };
    let resolved = resolve_with(&o, &files).map_err(|e| anyhow::anyhow!(e))?;
    let p = &resolved.parameters;
    let mut out = Phpstan {
        baselines: (files.seen.borrow().iter())
            .map(|b| relative(root, b))
            .collect(),
        ..Phpstan::default()
    };
    out.max = p.get_str("level").and_then(Value::to_php_string).as_deref() == Some("max");
    out.level = match p.get_str("level").and_then(Value::to_php_string).as_deref() {
        Some("max") => crate::schema::PHPSTAN_MAX_LEVEL,
        l => l.and_then(|l| l.parse().ok()).unwrap_or(0),
    };
    out.no_rules = matches!(p.get_str("customRulesetUsed"), None | Some(Value::Null));
    out.paths = strings(p.get_str("paths"))
        .iter()
        .map(|s| relative(root, s))
        .collect();
    let exclude = (p.get_str("excludePaths").and_then(Value::as_array))
        .cloned()
        .unwrap_or_default();
    out.exclude = strings(exclude.get_str("analyseAndScan"))
        .into_iter()
        .chain(strings(exclude.get_str("analyse")))
        .map(|s| relative(root, &s))
        .collect();
    let includes = includes(root, config);
    for inc in &includes {
        if is_baseline(inc) {
            continue;
        }
        let rel = relative(root, inc);
        match package(&rel) {
            Some(pkg) if pkg == "phpstan/phpstan" => {
                out.bleeding_edge |= inc.ends_with("/bleedingEdge.neon");
            }
            Some(pkg) if BUILT_IN.iter().any(|b| b.0 == pkg) => {
                out.extensions.insert(pkg);
            }
            Some(pkg) => out
                .unsupported
                .push((pkg, "a PHPStan extension Pun doesn't have built in".into())),
            None => out.unsupported.push((
                rel,
                "the project's own PHPStan config (services, rules, parameters)".into(),
            )),
        }
    }
    for pkg in installer_extensions(root) {
        if BUILT_IN.iter().any(|b| b.0 == pkg) {
            out.extensions.insert(pkg);
        } else if !out.unsupported.iter().any(|u| u.0 == pkg) {
            out.unsupported
                .push((pkg, "a PHPStan extension Pun doesn't have built in".into()));
        }
    }
    out.unsupported
        .extend(custom_classes(root, config, &includes));
    let tests = phpat_tests(root, config, &includes);
    if out.extensions.contains("phpat/phpat") || !tests.is_empty() {
        let options = p.get_str("phpat").and_then(Value::as_array);
        let flag = |k: &str| options.and_then(|o| o.get_str(k)) == Some(&Value::Bool(true));
        out.phpat = Some(crate::extensions::Phpat {
            tests,
            ignore_doc_comments: flag("ignore_doc_comments"),
            ignore_built_in_classes: flag("ignore_built_in_classes"),
            show_rule_names: flag("show_rule_names"),
        });
    }
    let packages: Vec<&String> = (includes.iter())
        .filter(|i| package(&relative(root, i)).is_some())
        .collect();
    let mut text = String::from("includes:\n");
    for inc in &packages {
        writeln!(text, "    - '{inc}'").unwrap();
    }
    writeln!(text, "parameters:\n    level: {}", out.level).unwrap();
    let path = format!("{cwd}/.pun-migrate-defaults.neon");
    *files.virtual_file.borrow_mut() = Some((path.clone(), text));
    o.config = Some(path);
    let defaults = resolve_with(&o, &files)
        .map_err(|e| anyhow::anyhow!(e))?
        .parameters;
    let gates = gate_parameters();
    let table = mapping::section(&["phpstan-parameters"]).table();
    for key in own_parameters(root, config, &includes) {
        if HANDLED.contains(&key.as_str()) {
            continue;
        }
        if key == "phpVersion" {
            php_version(p.get_str(&key), &mut out);
            continue;
        }
        let mut leaves = vec![];
        differences(&key, p.get_str(&key), defaults.get_str(&key), &mut leaves);
        for (name, value) in leaves {
            let entry = table.get(&name);
            let level = entry.is_some_and(|e| e.get("level").is_some());
            let why = match name.as_str() {
                "checkFunctionArgumentTypes" | "checkArgumentsPassedByReference" => {
                    "independent argument-check overrides have no pun.toml option yet"
                }
                "checkThisOnly" => "independent receiver eligibility has no pun.toml option yet",
                _ if gates.contains(&name) => continue,
                _ if entry.is_some() && !level => {
                    out.changed.push((name, value));
                    continue;
                }
                _ if level => "the level sets it; Pun's analyse presets decide it",
                _ => "no pun.toml setting for this parameter yet",
            };
            out.unsupported
                .push((format!("parameter {name}"), why.into()));
        }
    }
    // the project's own files: an extension's (in vendor or the phar) come back with the extension
    let own = |names: &[&str]| -> Vec<String> {
        (names.iter())
            .flat_map(|n| strings(p.get_str(n)))
            .filter(|s| !s.starts_with("phar://") && !s.contains("/vendor/"))
            .map(|s| relative(root, &s))
            .collect()
    };
    out.stubs = own(&["stubFiles"]);
    out.scan = own(&["scanFiles", "scanDirectories"]);
    out.bootstrap = own(&["bootstrapFiles"]);
    out.parameters = p.clone();
    let ignore = (p.get_str("ignoreErrors").and_then(Value::as_array))
        .cloned()
        .unwrap_or_default();
    let report_unmatched = p.get_str("reportUnmatchedIgnoredErrors") != Some(&Value::Bool(false));
    for e in ignore.values() {
        ignore_entry(root, e, report_unmatched, &mut out);
    }
    Ok(out)
}

fn includes(root: &Path, config: &Path) -> Vec<String> {
    let mut out = vec![];
    let mut todo = vec![config.display().to_string()];
    while let Some(file) = todo.pop() {
        let dir = Path::new(&file).parent().unwrap_or(root).to_path_buf();
        for inc in strings(neon(&file).get_str("includes")) {
            let path = if inc.starts_with("phar://") {
                inc
            } else {
                pun_phpstan::php::normalize_path(&dir.join(inc).display().to_string())
            };
            out.push(path.clone());
            if package(&relative(root, &path)).is_none() {
                todo.push(path);
            }
        }
    }
    out
}

fn neon(file: &str) -> Array {
    (std::fs::read(file).ok())
        .and_then(|b| pun_phpstan::neon::decode_bytes(&b).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

fn package(rel: &str) -> Option<String> {
    if (rel.strip_prefix("phar://")).is_some_and(|p| {
        p.starts_with("phpstan.phar/") || p.contains("vendor/phpstan/phpstan/phpstan.phar/")
    }) {
        return Some("phpstan/phpstan".into());
    }
    let rest = rel.strip_prefix("vendor/")?;
    let mut parts = rest.splitn(3, '/');
    Some(format!("{}/{}", parts.next()?, parts.next()?))
}

fn installer_extensions(root: &Path) -> Vec<String> {
    let path = root.join("vendor/phpstan/extension-installer/src/GeneratedConfig.php");
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    let after = text.split_once("EXTENSIONS").map_or("", |x| x.1);
    let mut out = vec![];
    for line in after.lines().skip(1) {
        let t = line.trim();
        if t.starts_with("const ") || t.starts_with("public const") {
            break;
        }
        if let Some(rest) = t.strip_prefix('\'')
            && let Some((name, after)) = rest.split_once('\'')
            && after.trim_start().starts_with("=>")
            && name.matches('/').count() == 1
            && !name.contains(' ')
        {
            out.push(name.to_string());
        }
    }
    out
}

/// `phpVersion` (`80400`, or `{min: 80200, max: 80400}`) into `out.php_version`, as `[php] version` holds it:
/// `major.minor`, one version. The range's `max` has no pun.toml key and is reported. `null` (PHP's own version) is
/// the default.
fn php_version(v: Option<&Value>, out: &mut Phpstan) {
    let version = |v: Option<&Value>| match v {
        Some(Value::Int(id)) => u32::try_from(*id).ok().map(crate::PhpVersion::from_id),
        _ => None,
    };
    let (min, max) = match v {
        Some(Value::Array(r)) => (version(r.get_str("min")), version(r.get_str("max"))),
        v => (version(v), None),
    };
    out.php_version = min.map(|m| m.to_string());
    if let Some(max) = max.filter(|max| Some(max.to_string()) != out.php_version) {
        out.unsupported.push((
            format!("parameter phpVersion.max ({max})"),
            "[php] version is one version; it takes phpVersion.min".into(),
        ));
    }
}

/// The classes the project's own configs register with PHPStan, which Pun can't run: each `rules:` entry, and each
/// service with a `phpstan.*` tag (a rule tagged `phpstan.rules.rule`, a collector, a type extension), as (name, why).
fn custom_classes(root: &Path, config: &Path, includes: &[String]) -> Vec<(String, String)> {
    fn class(v: &Value) -> Option<String> {
        match v {
            Value::Entity(e) => class(&e.value),
            Value::Array(a) => class(a.get_str("class").or(a.get_str("factory"))?),
            v => v.as_str().map(|s| s.trim_start_matches('\\').to_string()),
        }
    }
    let own = std::iter::once(config.display().to_string()).chain(
        (includes.iter())
            .filter(|i| package(&relative(root, i)).is_none() && !is_baseline(i))
            .cloned(),
    );
    let mut out: Vec<(String, String)> = vec![];
    let mut add = |name: String, why: String| {
        if !out.iter().any(|o| o.0 == name) {
            out.push((name, why));
        }
    };
    let rule = "a custom PHPStan rule; Pun runs no PHP rules";
    for file in own {
        let neon = neon(&file);
        let rules = neon.get_str("rules").and_then(Value::as_array);
        for c in rules.into_iter().flat_map(Array::values).filter_map(class) {
            add(format!("rule {c}"), rule.into());
        }
        let services = neon.get_str("services").and_then(Value::as_array);
        for s in services.into_iter().flat_map(Array::values) {
            let tags = (s
                .as_array()
                .and_then(|a| a.get_str("tags"))
                .and_then(Value::as_array))
            .map_or_default(|t| {
                (t.iter())
                    .map(|(k, v)| v.as_str().map_or_else(|| k.as_string(), str::to_string))
                    .collect::<Vec<_>>()
            });
            let Some(tag) = tags.iter().find(|t| t.starts_with("phpstan.")) else {
                continue;
            };
            let Some(c) = class(s) else { continue };
            if tag == "phpstan.rules.rule" {
                add(format!("rule {c}"), rule.into());
            } else {
                let why = format!("a custom PHPStan extension ({tag}); Pun runs no PHP extensions");
                add(format!("extension {c}"), why);
            }
        }
    }
    out
}

/// The classes of the services the project's own config files tag `phpat.test`: phpat's test classes.
fn phpat_tests(root: &Path, config: &Path, includes: &[String]) -> Vec<String> {
    let own = std::iter::once(config.display().to_string()).chain(
        (includes.iter())
            .filter(|i| package(&relative(root, i)).is_none() && !is_baseline(i))
            .cloned(),
    );
    let mut out: Vec<String> = Vec::new();
    for file in own {
        let neon = neon(&file);
        let services = neon.get_str("services").and_then(Value::as_array);
        for s in services.into_iter().flat_map(Array::values) {
            let Some(a) = s.as_array() else { continue };
            let tagged = (a.get_str("tags").and_then(Value::as_array))
                .is_some_and(|t| t.values().any(|v| v.as_str() == Some("phpat.test")));
            let class = a.get_str("class").and_then(Value::as_str);
            if let (true, Some(c)) = (tagged, class) {
                let c = c.trim_start_matches('\\').to_string();
                if !out.contains(&c) {
                    out.push(c);
                }
            }
        }
    }
    out
}

fn own_parameters(root: &Path, config: &Path, includes: &[String]) -> BTreeSet<String> {
    let own = std::iter::once(config.display().to_string()).chain(
        (includes.iter())
            .filter(|i| package(&relative(root, i)).is_none() && !is_baseline(i))
            .cloned(),
    );
    own.flat_map(|file| {
        (neon(&file).get_str("parameters").and_then(Value::as_array)).map_or_default(|p| {
            p.keys()
                .map(pun_phpstan::Key::as_string)
                .collect::<Vec<_>>()
        })
    })
    .collect()
}

fn gate_parameters() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for e in mapping::section(&["upstream", "phpstan-classes"])
        .table()
        .values()
    {
        for part in (e.get("gated_by").and_then(toml::Value::as_str))
            .into_iter()
            .flat_map(|g| g.split(" + "))
        {
            let (name, note) = part.split_once(" (").unwrap_or((part, ""));
            out.insert(name.to_string());
            out.extend(
                note.strip_prefix("featureToggles.")
                    .map(|t| format!("featureToggles.{}", t.trim_end_matches(')'))),
            );
        }
    }
    out
}

fn differences(name: &str, a: Option<&Value>, b: Option<&Value>, out: &mut Vec<(String, Value)>) {
    let table = |v: Option<&Value>| {
        v.and_then(Value::as_array)
            .filter(|t| !t.is_list())
            .cloned()
    };
    match (table(a), table(b)) {
        (Some(x), Some(y)) => {
            let keys: BTreeSet<String> = x
                .keys()
                .chain(y.keys())
                .map(pun_phpstan::Key::as_string)
                .collect();
            for k in keys {
                differences(&format!("{name}.{k}"), x.get_str(&k), y.get_str(&k), out);
            }
        }
        _ if a != b => out.push((name.to_string(), a.cloned().unwrap_or_default())),
        _ => {}
    }
}

/// One `ignoreErrors` entry as `[[analyse.ignore]]` tables. `report_unmatched` is `reportUnmatchedIgnoredErrors`,
/// which the entry's own `reportUnmatched` overrides; pun.toml has no global, so a `false` goes on each entry.
fn ignore_entry(root: &Path, e: &Value, report_unmatched: bool, out: &mut Phpstan) {
    let empty = Array::new();
    let a = e.as_array().unwrap_or(&empty);
    let mut messages: Vec<(&str, String)> = vec![];
    messages.extend(e.as_str().map(|m| ("message", m.to_string())));
    for (key, kind) in [
        ("message", "message"),
        ("messages", "message"),
        ("rawMessage", "raw-message"),
        ("rawMessages", "raw-message"),
    ] {
        messages.extend(strings(a.get_str(key)).into_iter().map(|m| (kind, m)));
    }
    let identifiers: Vec<String> = strings(a.get_str("identifier"))
        .into_iter()
        .chain(strings(a.get_str("identifiers")))
        .collect();
    let paths: Vec<String> = strings(a.get_str("path"))
        .into_iter()
        .chain(strings(a.get_str("paths")))
        .map(|p| relative(root, &p))
        .collect();
    let count = a.get_str("count").and_then(Value::to_php_string);
    let report_unmatched = match a.get_str("reportUnmatched") {
        Some(Value::Bool(b)) => *b,
        _ => report_unmatched,
    };
    let messages = if messages.is_empty() {
        vec![None]
    } else {
        messages.into_iter().map(Some).collect()
    };
    let identifiers: Vec<Option<String>> = if identifiers.is_empty() {
        vec![None]
    } else {
        identifiers.into_iter().map(Some).collect()
    };
    let q = |s: &str| toml::Value::String(s.to_string()).to_string();
    for id in &identifiers {
        let mut rule = None;
        if let Some(id) = id {
            let entry = crate::identifiers::entry(id);
            let pun = entry
                .and_then(|e| e.get("pun"))
                .and_then(toml::Value::as_str);
            match pun {
                Some(p) if p.starts_with("analyse/") => {
                    let narrow = entry.unwrap().get("variant").is_some();
                    rule = Some((p[8..].to_string(), narrow.then(|| id.clone())));
                }
                Some(p) => {
                    out.unsupported.push((
                        format!("ignoreErrors {id}"),
                        format!(
                            "`{id}` is the {} rule `{p}` in Pun; ignore it there",
                            p.split('/').next().unwrap()
                        ),
                    ));
                    continue;
                }
                None => {
                    let why = (entry.and_then(|e| e.get("unsupported").or(e.get("drop"))))
                        .and_then(toml::Value::as_str)
                        .map_or(
                            format!("`{id}` is not a PHPStan identifier Pun knows"),
                            |w| format!("`{id}`: {w}"),
                        );
                    out.unsupported.push((format!("ignoreErrors {id}"), why));
                    continue;
                }
            }
        }
        for m in &messages {
            let mut t = String::from("[[analyse.ignore]]\n");
            if let Some((r, identifier)) = &rule {
                writeln!(t, "rule = {}", q(r)).unwrap();
                if let Some(i) = identifier {
                    writeln!(t, "identifier = {}", q(i)).unwrap();
                }
            }
            if let Some((kind, text)) = m {
                writeln!(t, "{kind} = {}", q(text)).unwrap();
            }
            if !paths.is_empty() {
                let list = toml::Value::try_from(&paths).unwrap();
                writeln!(t, "paths = {list}").unwrap();
            }
            if let Some(c) = &count {
                writeln!(t, "count = {c}").unwrap();
            }
            if !report_unmatched {
                t.push_str("report-unmatched = false\n");
            }
            out.ignores.push(t);
        }
    }
}

fn gate(condition: &str, parameters: &Array) -> bool {
    condition.split(" + ").all(|part| {
        let (name, note) = match part.split_once(" (") {
            Some((n, rest)) => (n, rest.trim_end_matches(')')),
            None => (part, ""),
        };
        let path = match (name, note.strip_prefix("featureToggles.")) {
            ("bleedingEdge", Some(toggle)) => format!("featureToggles.{toggle}"),
            ("bleedingEdge", None) => "featureToggles.bleedingEdge".into(),
            _ => name.to_string(),
        };
        let mut v = Some(&Value::Array(parameters.clone()));
        for k in path.split('.') {
            v = v.and_then(Value::as_array).and_then(|a| a.get_str(k));
        }
        match v {
            Some(v) => !matches!(v, Value::Null | Value::Bool(false) | Value::Int(0)),
            None => note.starts_with("default true"),
        }
    })
}

fn wanted(phpstan: &Phpstan) -> (BTreeSet<String>, BTreeSet<String>) {
    let (mut run, mut mapped) = (BTreeSet::new(), BTreeSet::new());
    for e in mapping::section(&["upstream", "phpstan-classes"])
        .table()
        .values()
    {
        let Some(id) = e.get("pun").and_then(toml::Value::as_str) else {
            continue;
        };
        mapped.insert(id.to_string());
        let level = e.get("level").and_then(toml::Value::as_integer);
        let package = e
            .get("package")
            .and_then(toml::Value::as_str)
            .unwrap_or("phpstan");
        let included = package == "phpstan"
            || (BUILT_IN.iter()).any(|b| b.1 == Some(package) && phpstan.extensions.contains(b.0));
        let gated = (e.get("gated_by").and_then(toml::Value::as_str))
            .is_none_or(|g| gate(g, &phpstan.parameters));
        if included && gated && level.is_none_or(|l| l <= i64::from(phpstan.level)) {
            run.insert(id.to_string());
        }
    }
    (run, mapped)
}

#[derive(Debug, PartialEq)]
pub struct Analysed {
    pub preset: AnalysePreset,
    pub on: usize,
    pub off: usize,
    pub lint: usize,
    pub options: usize,
    pub unsupported: Vec<(String, String)>,
}

/// The `[analyse]` keys of the migrated config before its rules: the preset, the type switches of the level, the
/// paths, the stubs, scanned and bootstrap files, bleeding edge and `reportUnmatchedIgnoredErrors`.
fn analyse_keys(analyse: &mut String, phpstan: &Phpstan, preset: AnalysePreset) {
    writeln!(analyse, "preset = \"{preset}\"").unwrap();
    analyse.push_str(&switch_keys(phpstan.level, preset));
    let list = |v: &[String]| toml::Value::try_from(v).unwrap().to_string();
    if !phpstan.paths.is_empty() {
        writeln!(analyse, "paths = {}", list(&phpstan.paths)).unwrap();
    }
    if !phpstan.exclude.is_empty() {
        writeln!(analyse, "exclude = {}", list(&phpstan.exclude)).unwrap();
    }
    for (key, paths) in [
        ("stubs", &phpstan.stubs),
        ("scan", &phpstan.scan),
        ("bootstrap", &phpstan.bootstrap),
    ] {
        if !paths.is_empty() {
            writeln!(analyse, "{key} = {}", list(paths)).unwrap();
        }
    }
    if phpstan.bleeding_edge {
        writeln!(analyse, "bleeding-edge = true").unwrap();
    }
    if (phpstan.parameters).get_str("reportUnmatchedIgnoredErrors") == Some(&Value::Bool(false)) {
        writeln!(analyse, "report-unmatched = false").unwrap();
    }
}

/// The type switches `preset` gets wrong for PHPStan level `level`, as `[analyse]` lines. A PHPStan preset has its
/// level's switches; one of Pun's presets follows its highest level, so a level below that turns the higher ones off.
pub fn switch_keys(level: u8, preset: AnalysePreset) -> String {
    let mut out = String::new();
    if preset.phpstan_level().is_some() {
        return out;
    }
    for (on_in, switch) in crate::schema::SWITCHES {
        let on = (SWITCH_LEVELS.iter()).any(|(s, l)| *s == switch && level >= *l);
        if on != preset.includes(on_in) {
            writeln!(out, "{switch} = {on}").unwrap();
        }
    }
    out
}

/// `[analyse]`'s first lines for Pun's own preset closest to PHPStan level `level`, in place of the PHPStan preset
/// `parts` has (`pun migrate --offer-preset`).
pub fn with_pun_preset(analyse: &str, level: u8) -> String {
    let preset = AnalysePreset::of_level(usize::from(level));
    let mut out = String::new();
    for line in analyse.lines() {
        if line.starts_with("preset = ") {
            writeln!(out, "preset = \"{preset}\"").unwrap();
            out.push_str(&switch_keys(level, preset));
        } else {
            writeln!(out, "{line}").unwrap();
        }
    }
    out
}

/// `[extensions]`'s body (`id = "on"` lines) with each extension whose package phpstan.neon loads turned on. The
/// rule packages it doesn't load are off even when composer.lock locks them, so Pun reports no errors PHPStan never
/// did (PLAN.md §2).
fn loaded_extensions(body: &str, phpstan: &Phpstan) -> String {
    let rule_packages = [
        &crate::extensions::DISALLOWED_CALLS,
        &crate::extensions::TYPE_COVERAGE,
    ];
    let mut out = String::new();
    for e in crate::extensions::ALL {
        let locked = body.lines().any(|l| l == format!("{} = \"on\"", e.id));
        let loaded = (e.packages.iter()).any(|p| phpstan.extensions.contains(*p));
        let value = match (loaded, locked) {
            (false, true) if rule_packages.contains(&e) => "off",
            (true, _) | (false, true) => "on",
            (false, false) => continue,
        };
        writeln!(out, "{} = \"{value}\"", e.id).unwrap();
    }
    out
}

/// `[extensions] phpat` as a table of its settings.
fn phpat_line(phpat: &crate::extensions::Phpat) -> String {
    let tests: Vec<String> = (phpat.tests.iter())
        .map(|t| serde_json::Value::String(t.clone()).to_string())
        .collect();
    let mut fields = vec![format!("tests = [{}]", tests.join(", "))];
    for (key, on) in [
        ("ignore-doc-comments", phpat.ignore_doc_comments),
        ("ignore-built-in-classes", phpat.ignore_built_in_classes),
        ("show-rule-names", phpat.show_rule_names),
    ] {
        if on {
            fields.push(format!("{key} = true"));
        }
    }
    format!("phpat = {{ {} }}\n", fields.join(", "))
}

pub fn compose(root: &Path, parts: &mut Parts, phpstan: &Phpstan) -> Result<Analysed> {
    parts.extensions = loaded_extensions(&parts.extensions, phpstan);
    let preset = AnalysePreset::phpstan_of(phpstan.level, phpstan.max);
    if let Some(phpat) = &phpstan.phpat {
        let mut lines = (parts.extensions.lines())
            .filter(|l| !l.starts_with("phpat ="))
            .fold(String::new(), |mut out, l| {
                out.push_str(l);
                out.push('\n');
                out
            });
        lines.push_str(&phpat_line(phpat));
        parts.extensions = lines;
    }
    if let Some(v) = &phpstan.php_version {
        parts.php = format!("version = \"{v}\"\n");
    }
    analyse_keys(&mut parts.analyse, phpstan, preset);
    let cfg = resolve(root, &parts.render())?;
    let on: BTreeSet<&str> = cfg.analyse.rules.iter().map(|r| r.id.as_str()).collect();
    let (run, mapped) = wanted(phpstan);
    let mut done = Analysed {
        preset,
        on: 0,
        off: 0,
        lint: 0,
        options: 0,
        unsupported: vec![],
    };
    if phpstan.no_rules {
        let analyse: BTreeSet<&str> = (rules().iter())
            .filter_map(|(id, _)| id.strip_prefix("analyse/")?.split('/').next())
            .collect();
        for group in analyse {
            parts.analyse_rules.push((group.into(), "\"off\"".into()));
            done.off += 1;
        }
        parts.analyse_ignores = phpstan.ignores.join("\n");
        return Ok(done);
    }
    let larastan = phpstan.extensions.contains("larastan/larastan");
    let laravel = cfg
        .analyse
        .ecosystems
        .contains(&&crate::extensions::LARAVEL);
    if !larastan && laravel {
        parts
            .analyse_rules
            .push(("laravel".into(), "\"off\"".into()));
        done.off += 1;
    }
    for (id, _) in rules().iter().filter(|(id, _)| id.starts_with("analyse/")) {
        if !mapped.contains(id) || (id.starts_with("analyse/laravel/") && !(larastan && laravel)) {
            continue;
        }
        match (on.contains(id.as_str()), run.contains(id)) {
            (true, false) => {
                parts
                    .analyse_rules
                    .push((id[8..].to_string(), "\"off\"".into()));
                done.off += 1;
            }
            (false, true) => {
                parts
                    .analyse_rules
                    .push((id[8..].to_string(), "\"on\"".into()));
                done.on += 1;
            }
            _ => {}
        }
    }
    let formats = |id: &str| {
        cfg.rules
            .iter()
            .any(|r| crate::rule_of(&r.name) == Some(id))
    };
    // The lint rules on without the PHPStan preset (other configs, sets): the preset's own lint rules are scoped
    // below like any other PHPStan check.
    let mut plain = parts.clone();
    plain.analyse = with_pun_preset(&parts.analyse, phpstan.level);
    let base = resolve(root, &plain.render())?;
    let lint_on: BTreeSet<&str> = base.lint.rules.iter().map(|r| r.id.as_str()).collect();
    let mut scoped = vec![];
    for id in run
        .iter()
        .filter(|id| !id.starts_with("analyse/") && !formats(id))
    {
        let (lint, on) = (id.starts_with("lint/"), lint_on.contains(id.as_str()));
        let turned_on = lint && (on || super::rector::ported(id));
        scoped.extend((turned_on && !on).then_some(id.as_str()));
        done.lint += usize::from(turned_on);
        let why = if lint {
            "a PHPStan check that is this lint rule in Pun, not supported yet"
        } else {
            "a PHPStan check that is this format rule in Pun; turn it on in [format.rules]"
        };
        done.unsupported
            .extend((!turned_on).then(|| (id.clone(), why.to_string())));
    }
    scope_to_paths(parts, phpstan, &scoped, preset);
    options(root, parts, phpstan, &mut done)?;
    parts.analyse_ignores = phpstan.ignores.join("\n");
    Ok(done)
}

/// Turns on `scoped`, the lint rules PHPStan runs that nothing else turns on. PHPStan checks only its `paths`, so they
/// run there alone: `[[lint.overrides]]` on in `paths` and off in `excludePaths` (a rule on already runs on every lint
/// file and keeps its options).
///
/// The PHPStan preset turns them on everywhere already: with PHPStan's paths, they are turned off in `[lint.rules]`
/// first, so they run in the overrides alone.
fn scope_to_paths(parts: &mut Parts, phpstan: &Phpstan, scoped: &[&str], preset: AnalysePreset) {
    let by_preset = (preset.phpstan_level())
        .map_or_default(|l| crate::schema::phpstan_level_rules(l, phpstan.bleeding_edge));
    if phpstan.paths.is_empty() || phpstan.paths.iter().any(|p| p == ".") {
        for id in scoped.iter().filter(|id| !by_preset.contains(**id)) {
            super::require_lint(parts, id, "\"error\"");
        }
        return;
    }
    if scoped.is_empty() {
        return;
    }
    for id in scoped.iter().filter(|id| by_preset.contains(**id)) {
        super::require_lint(parts, id, "\"off\"");
    }
    let list = |v: &[String]| toml::Value::try_from(v).unwrap().to_string();
    let rules = |level: &str| {
        (scoped
            .iter()
            .map(|id| format!("\"{}\" = \"{level}\"", &id[5..])))
        .collect::<Vec<_>>()
        .join(", ")
    };
    let mut add = |files: &[String], level: &str| {
        let gap = if parts.overrides.is_empty() { "" } else { "\n" };
        let (files, rules) = (list(files), rules(level));
        write!(
            parts.overrides,
            "{gap}[[lint.overrides]]\nfiles = {files}\nrules = {{ {rules} }}\n"
        )
        .unwrap();
    };
    add(&phpstan.paths, "error");
    if !phpstan.exclude.is_empty() {
        add(&phpstan.exclude, "off");
    }
}

fn options(root: &Path, parts: &mut Parts, phpstan: &Phpstan, done: &mut Analysed) -> Result<()> {
    let cfg = resolve(root, &parts.render())?;
    let running: BTreeSet<&str> = cfg.analyse.rules.iter().map(|r| &r.id[8..]).collect();
    let table = mapping::section(&["phpstan-parameters"]).table();
    let mut by_rule: Vec<(String, toml::Table)> = vec![];
    for (name, value) in &phpstan.changed {
        let value = toml_value(value);
        let entry = &table[name.as_str()];
        let option = entry["option"].as_str().unwrap();
        let rules = entry["rules"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(toml::Value::as_str);
        let mut applied = false;
        for rule in rules.map(|r| &r[8..]).filter(|r| running.contains(r)) {
            let at = (by_rule.iter().position(|(r, _)| r == rule)).unwrap_or_else(|| {
                by_rule.push((rule.to_string(), toml::Table::new()));
                by_rule.len() - 1
            });
            by_rule[at].1.insert(option.to_string(), value.clone());
            applied = true;
        }
        done.options += usize::from(applied);
    }
    for (rule, options) in by_rule {
        let line = toml::Value::Table(options).to_string();
        parts.analyse_rules.retain(|(k, _)| *k != rule);
        parts.analyse_rules.push((rule, line));
    }
    Ok(())
}

fn toml_value(v: &Value) -> toml::Value {
    match v {
        Value::Bool(b) => toml::Value::Boolean(*b),
        Value::Array(a) => toml::Value::Array(a.values().map(toml_value).collect()),
        v => toml::Value::String(v.as_str().unwrap_or_default().to_string()),
    }
}
