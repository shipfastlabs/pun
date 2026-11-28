pub mod composer;
mod constraint;
mod explain;
pub mod extensions;
mod identifiers;
pub mod manifest;
mod mapping;
pub mod migrate;
pub mod options;
mod phpstan_parameters;
mod schema;

pub use explain::{RULE_DOCS_URL, explain, rule_docs_url};
pub use identifiers::{identifiers_of_rules, rule_of_identifier};
pub use schema::{
    Analyse, AnalysePreset, AnalyseRule, Files, FixSafety, FormatPreset, Ignore, Level, Lint,
    LintPreset, LintRule, LintSetting, Override, PHP_SETS, PhpVersion, RectorSets, groups,
    lint_sets, phpstan_level_rules, rector_sets,
};

use anyhow::{Context, Result, bail};
use composer::Composer;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Pun's cache directory: `PUN_CACHE_DIR`, else the platform's user cache directory (`~/Library/Caches/pun` on
/// macOS, `$XDG_CACHE_HOME/pun` or `~/.cache/pun` elsewhere). Nothing when neither `PUN_CACHE_DIR` nor `HOME` is set.
pub fn cache_dir() -> Option<PathBuf> {
    cache_dir_in(|k| std::env::var_os(k), cfg!(target_os = "macos"))
}

fn cache_dir_in(var: impl Fn(&str) -> Option<std::ffi::OsString>, macos: bool) -> Option<PathBuf> {
    let env = |k: &str| var(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = env("PUN_CACHE_DIR") {
        return Some(dir);
    }
    let home = env("HOME")?;
    if macos {
        return Some(home.join("Library/Caches/pun"));
    }
    Some(
        env("XDG_CACHE_HOME")
            .unwrap_or_else(|| home.join(".cache"))
            .join("pun"),
    )
}

pub const PRESETS: [(&str, &str); 4] = [
    ("laravel", include_str!("../presets/laravel.json")),
    ("per", include_str!("../presets/per.json")),
    ("psr12", include_str!("../presets/psr12.json")),
    ("empty", include_str!("../presets/empty.json")),
];

/// PHP-CS-Fixer's rule sets (`RuleSets::getSetDefinition($name)->getRules()` of the pinned version, exported by
/// scripts/export-rule-sets.php). The `@auto` sets read the project, so they stay unexpanded and are reported as not
/// ported.
const SETS: &str = include_str!("../presets/sets.json");

pub const NO_CONFIG: &str =
    "No pun.toml found. Run `pun migrate`, or `pun init` for a new project.";

#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub name: String,
    pub options: Value,
}

#[derive(Debug, Clone, Default)]
pub struct Finder {
    pub in_dirs: Vec<String>,
    pub exclude: Vec<String>,
    pub not_path: Vec<String>,
    pub not_name: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub root: PathBuf,
    pub preset: FormatPreset,
    pub php: PhpVersion,
    pub analyse_php_version: PhpVersion,
    pub rules: Vec<Rule>,
    pub finder: Finder,
    pub blade: bool,
    pub files: Files,
    pub lint: Lint,
    pub analyse: Analyse,
    /// The extensions that are on, in `extensions::ALL`'s order.
    pub extensions: Vec<&'static extensions::ExtensionMeta>,
    /// `[extensions]` as written; an extension it leaves out is off.
    pub extension_switches: std::collections::BTreeMap<&'static str, extensions::Switch>,
    pub sources: Vec<PathBuf>,
}

#[derive(Debug, Default, Clone)]
pub struct Options {
    pub config: Option<PathBuf>,
    pub no_config: bool,
    pub preset: Option<String>,
    /// Load a project whose composer.json allows PHP < 8.4 (the harness's escape hatch for older corpora).
    pub ignore_php_constraint: bool,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PunToml {
    /// The top-level preset: `[lint] preset` and `[analyse] preset` default to it, and `[format] preset` to
    /// `laravel` with the Laravel extension on, else `psr12`.
    preset: Option<String>,
    #[serde(default)]
    php: PunPhp,
    #[serde(default)]
    files: schema::FilesToml,
    #[serde(default)]
    format: PunFormat,
    #[serde(default)]
    lint: schema::LintToml,
    #[serde(default)]
    analyse: schema::AnalyseToml,
    #[serde(default)]
    extensions: toml::Table,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PunPhp {
    version: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct PunFormat {
    preset: Option<String>,
    /// `"prettier"` (the default) or `"native"`, the opt-in Rust Blade formatter.
    blade: Option<String>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    not_path: Vec<String>,
    #[serde(default)]
    not_name: Vec<String>,
    #[serde(default)]
    rules: toml::Table,
}

pub fn fixer_entries() -> &'static toml::Table {
    static T: LazyLock<toml::Table> = LazyLock::new(|| {
        let mut out = toml::Table::new();
        for tool in ["php-cs-fixer", "pint", "pun"] {
            out.extend(mapping::section(&["upstream", tool]).table().clone());
        }
        out
    });
    &T
}

/// Whether a Finder pattern (`notPath`, `notName`, `path`) is a delimited regex rather than a string or glob: Symfony's
/// `MultiplePcreFilterIterator::isRegex`.
pub fn is_regex(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 3 {
        return false;
    }
    let body = &b[..s.trim_end_matches(|c| "imsxuADUn".contains(c)).len().max(3)];
    let (start, end) = (body[0], body[body.len() - 1]);
    if start == end {
        return !(start.is_ascii_alphanumeric() || b"*? \\".contains(&start));
    }
    matches!(
        (start, end),
        (b'{', b'}') | (b'(', b')') | (b'[', b']') | (b'<', b'>')
    )
}

pub fn rule_of(fixer: &str) -> Option<&'static str> {
    fixer_entries().get(fixer)?.get("pun")?.as_str()
}

pub fn fixer_of(command: &str, rule: &str) -> Option<&'static str> {
    let id = match rule.strip_prefix(command).and_then(|r| r.strip_prefix('/')) {
        Some(_) => rule.to_string(),
        None => format!("{command}/{rule}"),
    };
    fixer_entries()
        .iter()
        .find(|(_, e)| e.get("pun").and_then(toml::Value::as_str) == Some(&id))
        .map(|(k, _)| k.as_str())
}

pub fn load(root: &Path, opts: &Options) -> Result<Config> {
    let composer = composer::detect(root)?;
    check_composer(&composer, opts.ignore_php_constraint)?;
    let mut sources = composer.sources.clone();
    let path = root.join(opts.config.as_deref().unwrap_or(Path::new("pun.toml")));
    let text = if opts.no_config || (opts.config.is_none() && !path.exists()) {
        if !opts.no_config && opts.preset.is_none() {
            bail!(NO_CONFIG);
        }
        String::new()
    } else {
        let t = std::fs::read_to_string(&path)
            .with_context(|| format!("can't read {}", path.display()))?;
        if !sources.contains(&path) {
            sources.push(path.clone());
        }
        t
    };
    parse(root, &path, &text, opts, &composer, sources)
}

pub fn from_toml(root: &Path, text: &str) -> Result<Config> {
    parse(
        root,
        &root.join("pun.toml"),
        text,
        &Options::default(),
        &Composer::default(),
        vec![],
    )
}

fn inside(root: &Path, path: &str) -> bool {
    let (Ok(real), Ok(root)) = (root.join(path).canonicalize(), root.canonicalize()) else {
        return true;
    };
    real.starts_with(root)
}

/// Checks `[format] preset` and `blade`, and says whether `blade = "native"` picks the Rust Blade formatter.
fn blade_and_preset(at: &impl std::fmt::Display, f: &PunFormat) -> Result<bool> {
    if let Some(p) = f.preset.as_deref()
        && FormatPreset::parse(p).is_none()
    {
        bail!("{at}: [format] preset = \"{p}\": expected `laravel`, `psr12` or `empty`")
    }
    Ok(match f.blade.as_deref() {
        None | Some("prettier") => false,
        Some("native") => true,
        Some(b) => bail!("{at}: [format] blade = \"{b}\": expected `prettier` or `native`"),
    })
}

/// The format preset: `--preset`, else `[format] preset`, else the top-level preset's pick (`laravel` with the
/// Laravel extension on, else `psr12`), else `laravel`.
fn format_preset(own: Option<&str>, top: bool, laravel: bool) -> Result<FormatPreset> {
    Ok(match own {
        Some(p) => FormatPreset::parse(p).context("Preset not found.")?,
        None if top && !laravel => FormatPreset::Psr12,
        None => FormatPreset::Laravel,
    })
}

/// The top-level `preset`: fills in `[lint] preset` and `[analyse] preset` where pun.toml leaves them out.
fn top_preset(
    at: &impl std::fmt::Display,
    top: Option<&str>,
    lint: &mut schema::LintToml,
    analyse: &mut schema::AnalyseToml,
) -> Result<Option<LintPreset>> {
    let Some(p) = top else { return Ok(None) };
    let p = LintPreset::parse(p).with_context(|| {
        format!("{at}: preset = \"{p}\": expected `essential`, `recommended` or `strict`")
    })?;
    lint.preset.get_or_insert_with(|| p.name().to_string());
    (analyse.preset).get_or_insert_with(|| toml::Value::from(p.name()));
    Ok(Some(p))
}

/// `[lint] baseline` and `[analyse] baseline`, and their directories, must stay inside the project.
fn baseline_inside(
    root: &Path,
    at: &impl std::fmt::Display,
    section: &str,
    baseline: Option<&str>,
) -> Result<()> {
    let escapes = |b: &&str| {
        let dir = Path::new(b).parent().unwrap_or(Path::new(""));
        !inside(root, b) || !inside(root, &dir.to_string_lossy())
    };
    if let Some(b) = baseline.filter(escapes) {
        bail!("{at}: [{section}] baseline: \"{b}\" is outside the project");
    }
    Ok(())
}

fn parse(
    root: &Path,
    path: &Path,
    text: &str,
    opts: &Options,
    composer: &Composer,
    sources: Vec<PathBuf>,
) -> Result<Config> {
    let mut table: toml::Table =
        toml::from_str(text).with_context(|| format!("invalid {}", path.display()))?;
    if let Some(s) = manifest::SECTIONS.iter().find(|s| table.contains_key(**s)) {
        bail!(
            "{}: [{s}] belongs in composer.json: Pun reads the dependencies from composer.json and composer.lock, and pun.toml holds only Pun's own settings",
            path.display()
        );
    }
    table.remove("workspace");
    let toml: PunToml = table
        .try_into()
        .with_context(|| format!("invalid {}", path.display()))?;
    let PunToml {
        preset: top,
        php,
        files,
        format: f,
        mut lint,
        mut analyse,
        extensions,
    } = toml;
    let at = path.display();
    let top = top_preset(&at, top.as_deref(), &mut lint, &mut analyse)?;
    let native_blade = blade_and_preset(&at, &f)?;
    let explicit = php.version.is_some();
    let php = match php.version {
        Some(v) => v.parse::<PhpVersion>().map_err(|_| {
            anyhow::anyhow!(
                "{at}: [php] version = \"{v}\": expected a PHP version as major.minor, such as \"8.4\""
            )
        })?,
        None => composer.php_version().unwrap_or(PhpVersion::FLOOR),
    };
    let analyse_php_version = if explicit {
        php
    } else {
        composer
            .platform_php_version()
            .unwrap_or(PhpVersion::NEWEST)
    };
    let files = Files::from(files);
    for p in &files.paths {
        if !inside(root, p) {
            bail!("{at}: [files] paths: \"{p}\" is outside the project");
        }
    }
    let gates = schema::Gates {
        php,
        composer,
        switches: extensions::switches(&at.to_string(), extensions)?,
    };
    let phpstan_lint = analyse.phpstan_lint(path)?;
    let lint = schema::lint(path, lint, &files, &gates, &phpstan_lint)?;
    let analyse = schema::analyse(path, analyse, &files, &gates)?;
    baseline_inside(root, &at, "lint", lint.baseline.as_deref())?;
    baseline_inside(root, &at, "analyse", analyse.baseline.as_deref())?;
    let laravel = gates.switches.get(extensions::LARAVEL.id) == Some(&extensions::Switch::On);
    let preset = format_preset(
        opts.preset.as_deref().or(f.preset.as_deref()),
        top.is_some(),
        laravel,
    )?;
    let mut rules = resolve_set(preset_rules(&preset).unwrap())?;
    for (name, v) in f.rules {
        let id = schema::rule_id(path, "format", &name)?;
        let fixer = fixer_of("format", &id).unwrap();
        let v = options::to_fixer(&name, &id, v).map_err(|e| anyhow::anyhow!("{at}: {e}"))?;
        rules.insert(fixer.into(), v);
    }
    rules.retain(|name, _| !rule_of(name).is_some_and(|id| id.starts_with("lint/")));
    let mut rules = enabled(rules);
    rules.retain(|r| php_allows(r, php));
    let blade = rules.iter().any(|r| r.name == "Pint/laravel_blade");
    if native_blade {
        for r in rules.iter_mut().filter(|r| r.name == "Pint/laravel_blade") {
            r.options = serde_json::json!({ "engine": "native" });
        }
    }

    Ok(Config {
        root: root.to_path_buf(),
        preset,
        php,
        analyse_php_version,
        rules,
        finder: Finder {
            in_dirs: files.paths.clone(),
            exclude: [&files.exclude[..], &f.exclude[..]].concat(),
            not_path: f.not_path,
            not_name: f.not_name,
        },
        blade,
        files,
        lint,
        analyse,
        extensions: gates.extensions(),
        extension_switches: gates.switches,
        sources,
    })
}

/// The rule a removed rule was merged into: `name` is an ID in a rule's `replaces` (`lint/arrays/prefer-array-append`),
/// with or without its `<command>/`.
pub fn merged_into(name: &str) -> Option<&'static str> {
    rules().iter().find_map(|(id, e)| {
        let replaces = e.get("replaces")?.as_array()?;
        replaces
            .iter()
            .filter_map(toml::Value::as_str)
            .any(|old| old == name || old.split_once('/').is_some_and(|(_, rest)| rest == name))
            .then_some(id.as_str())
    })
}

fn renamed(name: &str) -> Option<&'static str> {
    let bare = ["format.", "lint.", "analyse."]
        .iter()
        .find_map(|p| name.strip_prefix(p))
        .unwrap_or(name);
    let norm = |s: &str| -> String {
        s.chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    let key = norm(bare);
    if let Some((_, id)) = upstream().iter().find(|(k, _)| norm(k) == key) {
        return Some(id);
    }
    let mut hits = rules().iter().map(|(id, _)| id.as_str()).filter(|id| {
        let mut parts = id.splitn(3, '/').skip(1);
        let (group, rule) = (parts.next().unwrap(), parts.next().unwrap());
        [
            bare.to_string(),
            bare.replacen(&format!("{group}-"), "", 1),
            bare.replacen(&format!("-{group}"), "", 1),
        ]
        .contains(&rule.to_string())
            || format!("{group}-{rule}") == bare
    });
    hits.next().filter(|_| hits.next().is_none())
}

fn upstream() -> &'static Vec<(String, &'static str)> {
    static T: LazyLock<Vec<(String, &'static str)>> = LazyLock::new(|| {
        let mut out = vec![];
        for (_, s) in mapping::under("upstream") {
            for (k, e) in s.table() {
                if let Some(id) = e.get("pun").and_then(toml::Value::as_str) {
                    let id = rules()
                        .iter()
                        .find(|(r, _)| r == id)
                        .map(|(r, _)| r.as_str());
                    out.extend(id.map(|id| (k.clone(), id)));
                }
            }
        }
        out
    });
    &T
}

#[derive(Debug)]
pub struct RuleMeta {
    pub fix: FixSafety,
    pub summary: Option<&'static str>,
}

fn rules() -> &'static Vec<(String, toml::Value)> {
    static T: LazyLock<Vec<(String, toml::Value)>> = LazyLock::new(|| {
        ["format", "lint", "analyse"]
            .iter()
            .flat_map(|c| {
                (mapping::section(&["rules", c]).table().iter())
                    .map(move |(rule, e)| (format!("{c}/{rule}"), e.clone()))
            })
            .collect()
    });
    &T
}

pub fn rule(id: &str) -> Option<RuleMeta> {
    let (_, e) = rules().iter().find(|(r, _)| r == id)?;
    let str = |k| e.get(k).and_then(toml::Value::as_str);
    Some(RuleMeta {
        fix: FixSafety::parse(str("fix").unwrap_or("none")),
        summary: str("summary"),
    })
}

pub fn preset_rules(preset: &str) -> Option<Map<String, Value>> {
    let (_, json) = PRESETS.iter().find(|(n, _)| *n == preset)?;
    let mut rules: Map<String, Value> = serde_json::from_str(json).unwrap();
    if let Some(Value::Object(r)) = rules.remove("rules_as_configured") {
        rules = r;
    }
    Some(rules)
}

pub fn rule_set(rules: Map<String, Value>) -> Result<Vec<Rule>> {
    Ok(enabled(resolve_set(rules)?))
}

fn enabled(rules: Map<String, Value>) -> Vec<Rule> {
    rules
        .into_iter()
        .filter(|(_, v)| *v != Value::Bool(false))
        .map(|(name, v)| Rule {
            name,
            options: if v == Value::Bool(true) {
                Value::Null
            } else {
                v
            },
        })
        .collect()
}

fn resolve_set(rules: Map<String, Value>) -> Result<Map<String, Value>> {
    let sets: std::collections::BTreeMap<String, Map<String, Value>> =
        serde_json::from_str(SETS).unwrap();
    let mut out = Map::new();
    for (name, value) in rules {
        if name.starts_with('@') && !value.is_boolean() {
            bail!("Nested rule set \"{name}\" configuration must be a boolean.");
        }
        match resolve_subset(&sets, &name, value == Value::Bool(true)) {
            Some(set) => out.extend(set),
            None => drop(out.insert(name, value)),
        }
    }
    Ok(out)
}

fn resolve_subset(
    sets: &std::collections::BTreeMap<String, Map<String, Value>>,
    name: &str,
    enabled: bool,
) -> Option<Map<String, Value>> {
    let def = sets.get(name)?;
    let mut rules = def.clone();
    for (k, v) in def {
        if k.starts_with('@') {
            rules.shift_remove(k);
            rules.extend(resolve_subset(sets, k, enabled).unwrap());
        } else {
            rules.insert(
                k.clone(),
                if enabled {
                    v.clone()
                } else {
                    Value::Bool(false)
                },
            );
        }
    }
    Some(rules)
}

/// Whether format rule `r` can run on a project that targets `php`: dropping the parentheses around
/// `(new Foo())->bar()` writes PHP 8.4 syntax, so `operators/new-expression-parentheses` skips older targets
/// unless it adds them (`use-parentheses = true`). PHP-CS-Fixer gates it the same way on the PHP it runs on.
fn php_allows(r: &Rule, php: PhpVersion) -> bool {
    r.name != "new_expression_parentheses"
        || php >= PhpVersion::new(8, 4)
        || r.options.get("use_parentheses") == Some(&Value::Bool(true))
}

fn check_composer(composer: &Composer, ignore_php_constraint: bool) -> Result<()> {
    let Some(php) = &composer.php else {
        return Ok(());
    };
    let floor = PhpVersion::FLOOR;
    let lowest = (u64::from(floor.major()), u64::from(floor.minor()), 0);
    if php
        .lowest
        .is_some_and(|l| l < composer::Version::from(lowest))
        && !ignore_php_constraint
    {
        bail!(
            "{} requires PHP \"{}\", which allows PHP < {floor}. Pun only supports projects on PHP {floor}+; raise the constraint (for example \"^{floor}\") or keep using Pint.",
            (composer.sources.first())
                .and_then(|p| p.file_name())
                .map_or("composer.json".into(), |f| f.to_string_lossy()),
            php.constraint
        );
    }
    Ok(())
}

impl Config {
    pub fn pun_toml(&self) -> Option<&Path> {
        (self.sources.iter())
            .find(|s| s.extension().is_some_and(|e| e == "toml"))
            .map(PathBuf::as_path)
    }

    pub fn php_version(&self) -> PhpVersion {
        self.php
    }

    pub fn show(&self) -> Value {
        let short = |id: &str| id.split_once('/').unwrap().1.to_string();
        let format: Map<String, Value> = (self.rules.iter())
            .map(|r| {
                let full = rule_of(&r.name).unwrap_or(&r.name);
                let v = options::to_pun(&r.name, full, &r.options).unwrap();
                (rule_of(&r.name).map_or(r.name.clone(), short), v)
            })
            .collect();
        let lint: Map<String, Value> = (self.lint.rules.iter())
            .map(|r| (short(&r.id), r.value()))
            .collect();
        let overrides: Vec<ShownOverride> = (self.lint.overrides.iter())
            .map(|o| ShownOverride {
                files: &o.files,
                rules: o.rules.iter().map(|r| (short(&r.id), r.value())).collect(),
            })
            .collect();
        let analyse: Map<String, Value> = (self.analyse.rules.iter())
            .map(|r| {
                let v = match &r.options {
                    Value::Null => Value::from("on"),
                    o => o.clone(),
                };
                (short(&r.id), v)
            })
            .collect();
        let a = &self.analyse;
        let f = &self.finder;
        let shown = Shown {
            php: ShownPhp {
                version: self.php.to_string(),
            },
            files: &self.files,
            extensions: (extensions::ALL.iter())
                .map(|e| {
                    let on = self.extensions.contains(e);
                    (e.id.to_string(), Value::from(if on { "on" } else { "off" }))
                })
                .collect(),
            format: ShownFormat {
                preset: &self.preset,
                exclude: &f.exclude,
                not_path: &f.not_path,
                not_name: &f.not_name,
                rules: format,
            },
            lint: ShownLint {
                preset: self.lint.preset,
                sets: &self.lint.sets,
                exclude: &self.lint.exclude,
                ecosystems: &self.lint.ecosystems,
                rules: lint,
                overrides,
                baseline: self.lint.baseline.as_deref(),
            },
            analyse: ShownAnalyse {
                preset: &a.preset,
                paths: &a.paths,
                exclude: &a.exclude,
                switches: &a.switches,
                ecosystems: &a.ecosystems,
                rules: analyse,
                baseline: a.baseline.as_deref(),
                stubs: &a.stubs,
                scan: &a.scan,
                bootstrap: &a.bootstrap,
                ignore: &a.ignore,
            },
        };
        serde_json::to_value(shown).unwrap()
    }
}

/// `pun config`: the resolved configuration in pun.toml's shape.
#[derive(Serialize)]
struct Shown<'a> {
    php: ShownPhp,
    files: &'a Files,
    extensions: Map<String, Value>,
    format: ShownFormat<'a>,
    lint: ShownLint<'a>,
    analyse: ShownAnalyse<'a>,
}

#[derive(Serialize)]
struct ShownPhp {
    version: String,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ShownFormat<'a> {
    preset: &'a FormatPreset,
    exclude: &'a [String],
    not_path: &'a [String],
    not_name: &'a [String],
    rules: Map<String, Value>,
}

#[derive(Serialize)]
struct ShownLint<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    preset: Option<LintPreset>,
    sets: &'a [String],
    exclude: &'a [String],
    ecosystems: &'a [&'static extensions::ExtensionMeta],
    rules: Map<String, Value>,
    overrides: Vec<ShownOverride<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    baseline: Option<&'a str>,
}

#[derive(Serialize)]
struct ShownOverride<'a> {
    files: &'a [String],
    rules: Map<String, Value>,
}

#[derive(Serialize)]
struct ShownAnalyse<'a> {
    preset: &'a AnalysePreset,
    paths: &'a [String],
    exclude: &'a [String],
    switches: &'a [&'static str],
    ecosystems: &'a [&'static extensions::ExtensionMeta],
    rules: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    baseline: Option<&'a str>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    stubs: &'a [String],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    scan: &'a [String],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    bootstrap: &'a [String],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    ignore: &'a [Ignore],
}

#[cfg(test)]
mod tests;
