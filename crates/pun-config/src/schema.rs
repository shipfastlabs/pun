use crate::composer::{Composer, Locked};
use crate::extensions::{self, ExtensionMeta};
use crate::{fixer_of, options, renamed, rules};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub use pun_tokens::PhpVersion;

/// `[lint] preset`: Pun's own lint presets, each including the ones below it (mapping.toml's `tier`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LintPreset {
    Essential,
    Recommended,
    Strict,
}

impl LintPreset {
    /// Lowest first.
    pub const ALL: [Self; 3] = [Self::Essential, Self::Recommended, Self::Strict];
    /// The preset `pun init` writes.
    pub const DEFAULT: Self = Self::Recommended;

    pub fn name(self) -> &'static str {
        match self {
            Self::Essential => "essential",
            Self::Recommended => "recommended",
            Self::Strict => "strict",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == name)
    }
}

impl std::fmt::Display for LintPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl Serialize for LintPreset {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

/// `withPhpSets()`: every `rector/phpNN` set up to `[php] version`.
pub const PHP_SETS: &str = "rector/php-sets";

/// The lint sets before 2026-11-24, each with the Rector set that replaced it (`None`: Rector's set has no rules).
const OLD_SETS: [(&str, Option<&str>); 8] = [
    ("dead-code", Some("rector/dead-code")),
    ("modernize", Some(PHP_SETS)),
    ("code-quality", Some("rector/code-quality")),
    ("types", Some("rector/type-declaration")),
    ("early-return", None),
    ("privatization", Some("rector/privatization")),
    ("naming", None),
    ("coding-style", Some("rector/coding-style")),
];

/// Every Rector set under its Pun name: the Pun lint rules its Rector rules map to, each with those Rector rules'
/// short class names (rules/rector-sets.json, written by scripts/gen-rector-sets.py).
pub type RectorSets = BTreeMap<String, BTreeMap<String, Vec<String>>>;

pub fn rector_sets() -> &'static RectorSets {
    static T: std::sync::LazyLock<RectorSets> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("../rules/rector-sets.json")).unwrap()
    });
    &T
}

/// The names `[lint] sets` takes: every Rector set, and `rector/php-sets`.
pub fn lint_sets() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = rector_sets().keys().map(String::as_str).collect();
    out.push(PHP_SETS);
    out.sort_unstable();
    out
}

/// The Rector sets `sets` name, resolved: each Pun rule they hold, with the Rector rules (short class names) behind it.
pub(crate) fn rector_members(
    sets: &[String],
    php: PhpVersion,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut add = |name: &str| {
        for (id, classes) in &rector_sets()[name] {
            out.entry(id.clone())
                .or_default()
                .extend(classes.iter().cloned());
        }
    };
    for s in sets {
        if s == PHP_SETS {
            let target = php.major() * 10 + php.minor();
            for name in rector_sets().keys() {
                let version = name
                    .strip_prefix("rector/php")
                    .and_then(|v| v.parse::<u32>().ok());
                if version.is_some_and(|v| v <= target) {
                    add(name);
                }
            }
        } else {
            add(s);
        }
    }
    out
}

fn check_sets(path: &Path, sets: &[String]) -> Result<()> {
    for s in sets {
        if s == PHP_SETS || rector_sets().contains_key(s) {
            continue;
        }
        match OLD_SETS.iter().find(|(old, _)| old == s) {
            Some((_, Some(new))) => bail!(
                "{}: [lint] sets: `{s}` is now `{new}`, Rector's set; for Pun's own picks use [lint] preset = {}",
                path.display(),
                presets_list()
            ),
            Some((_, None)) => bail!(
                "{}: [lint] sets: `{s}` is gone: Rector's set has no rules; use [lint] preset = {}",
                path.display(),
                presets_list()
            ),
            None => bail!(
                "{}: [lint] sets: `{s}` is not a lint set; the sets are Rector's, such as `rector/dead-code`, `rector/php-sets` and `rector-laravel/laravel-code-quality` (`pun explain` lists a rule's sets)",
                path.display()
            ),
        }
    }
    Ok(())
}

fn presets_list() -> String {
    let names: Vec<String> = LintPreset::ALL.iter().map(|p| format!("\"{p}\"")).collect();
    names.join(" | ")
}

fn lint_preset(path: &Path, v: Option<&str>) -> Result<Option<LintPreset>> {
    v.map(|name| {
        LintPreset::parse(name).ok_or_else(|| {
            anyhow::anyhow!(
                "{}: [lint] preset: `{name}` is not a lint preset; use {}",
                path.display(),
                presets_list()
            )
        })
    })
    .transpose()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatPreset {
    Laravel,
    Psr12,
    Empty,
}

impl FormatPreset {
    pub const ALL: [Self; 3] = [Self::Laravel, Self::Psr12, Self::Empty];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Laravel => "laravel",
            Self::Psr12 => "psr12",
            Self::Empty => "empty",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name() == s)
    }
}

impl std::ops::Deref for FormatPreset {
    type Target = str;

    fn deref(&self) -> &str {
        self.name()
    }
}

impl PartialEq<&str> for FormatPreset {
    fn eq(&self, other: &&str) -> bool {
        self.name() == *other
    }
}

impl std::fmt::Display for FormatPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl Serialize for FormatPreset {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixSafety {
    Safe,
    Unsafe,
    Mixed,
    None,
}

impl FixSafety {
    pub(crate) fn parse(s: &str) -> Self {
        match s {
            "safe" => Self::Safe,
            "unsafe" => Self::Unsafe,
            "mixed" => Self::Mixed,
            _ => Self::None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Unsafe => "unsafe",
            Self::Mixed => "mixed",
            Self::None => "none",
        }
    }
}

/// `[analyse] preset`: one of Pun's five presets, or one of PHPStan's levels by its upstream name
/// (`phpstan/level_0` to `phpstan/level_10`, and `phpstan/level_max`), which runs exactly the rules and type checks
/// that `phpstan analyse --level N` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysePreset {
    Basic,
    Essential,
    Recommended,
    Strict,
    Max,
    /// PHPStan's level 0 to 10.
    PhpstanLevel(u8),
    /// PHPStan's `max`: level 10 today, under its own name.
    PhpstanMax,
}

const PHPSTAN_LEVEL_NAMES: [&str; 11] = [
    "phpstan/level_0",
    "phpstan/level_1",
    "phpstan/level_2",
    "phpstan/level_3",
    "phpstan/level_4",
    "phpstan/level_5",
    "phpstan/level_6",
    "phpstan/level_7",
    "phpstan/level_8",
    "phpstan/level_9",
    "phpstan/level_10",
];

/// PHPStan's highest level, the one `max` stands for.
pub(crate) const PHPSTAN_MAX_LEVEL: u8 = 10;

impl AnalysePreset {
    /// Pun's own presets, lowest first.
    pub const ALL: [Self; 5] = [
        Self::Basic,
        Self::Essential,
        Self::Recommended,
        Self::Strict,
        Self::Max,
    ];

    /// PHPStan's level presets, lowest first, then `phpstan/level_max`.
    pub fn phpstan() -> impl Iterator<Item = Self> {
        (0..=PHPSTAN_MAX_LEVEL)
            .map(Self::PhpstanLevel)
            .chain([Self::PhpstanMax])
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Basic => "basic",
            Self::Essential => "essential",
            Self::Recommended => "recommended",
            Self::Strict => "strict",
            Self::Max => "max",
            Self::PhpstanLevel(l) => PHPSTAN_LEVEL_NAMES[usize::from(l)],
            Self::PhpstanMax => "phpstan/level_max",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .chain(Self::phpstan())
            .find(|p| p.name() == s)
    }

    /// Pun's preset that holds PHPStan level `level`.
    pub fn of_level(level: usize) -> Self {
        Self::ALL[[0, 0, 1, 1, 2, 2, 3, 3, 3, 4, 4][level]]
    }

    /// The PHPStan preset of `level` (`phpstan/level_max` when `max`).
    pub fn phpstan_of(level: u8, max: bool) -> Self {
        if max {
            Self::PhpstanMax
        } else {
            Self::PhpstanLevel(level)
        }
    }

    /// The PHPStan level of a PHPStan preset; `None` for Pun's own.
    pub fn phpstan_level(self) -> Option<u8> {
        match self {
            Self::PhpstanLevel(l) => Some(l),
            Self::PhpstanMax => Some(PHPSTAN_MAX_LEVEL),
            _ => None,
        }
    }

    /// Whether this Pun preset includes Pun preset `other` (each includes the ones below it). Always false for the
    /// PHPStan presets, which choose rules by PHPStan level instead.
    pub(crate) fn includes(self, other: Self) -> bool {
        let rank = |p: Self| Self::ALL.iter().position(|a| *a == p);
        matches!((rank(other), rank(self)), (Some(a), Some(b)) if a <= b)
    }
}

impl Serialize for AnalysePreset {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

impl std::fmt::Display for AnalysePreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The type switches in `[analyse]`, each with the preset that turns it on.
pub(crate) const SWITCHES: [(AnalysePreset, &str); 4] = [
    (AnalysePreset::Strict, "union-types"),
    (AnalysePreset::Strict, "nullables"),
    (AnalysePreset::Max, "explicit-mixed"),
    (AnalysePreset::Max, "implicit-mixed"),
];

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Files {
    pub paths: Vec<String>,
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LintRule {
    pub id: String,
    pub level: Level,
    pub fix: bool,
    pub options: Value,
    #[serde(skip)]
    pub fixer: Option<&'static str>,
    #[serde(skip)]
    pub named: bool,
}

impl LintRule {
    pub fn value(&self) -> Value {
        lint_value(&self.id, Some(self.level), self.fix, &self.options)
    }
}

impl LintSetting {
    pub fn value(&self) -> Value {
        lint_value(&self.id, self.level, self.fix, &self.options)
    }
}

fn lint_value(id: &str, level: Option<Level>, fix: bool, options: &Value) -> Value {
    let level = Value::from(match level {
        None => "off",
        Some(Level::Warn) => "warn",
        Some(Level::Error) => "error",
    });
    if fix && options.is_null() {
        return level;
    }
    let mut t = serde_json::Map::new();
    if level != "error" {
        t.insert("level".into(), level);
    }
    if !fix {
        t.insert("fix".into(), "off".into());
    }
    let options = match (fixer_of("lint", id), options) {
        (Some(f), Value::Object(o)) => {
            // Pun's own options (`origin = "pun"`) are not the fixer's, so they skip its translation.
            let own = |k: &String| {
                options::rule_options(id)
                    .iter()
                    .find(|r| r.pun && r.upstream == *k)
            };
            let (pun, upstream): (Map<String, Value>, Map<String, Value>) =
                o.clone().into_iter().partition(|(k, _)| own(k).is_some());
            let mut out = Map::new();
            if !upstream.is_empty() {
                let translated = options::to_pun(f, id, &Value::Object(upstream)).unwrap();
                out.extend(translated.as_object().cloned().unwrap_or_default());
            }
            out.extend(
                pun.into_iter()
                    .map(|(k, v)| (own(&k).unwrap().name.clone(), v)),
            );
            Value::Object(out)
        }
        _ => options.clone(),
    };
    if let Value::Object(o) = options {
        t.extend(o);
    }
    Value::Object(t)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LintSetting {
    pub id: String,
    pub level: Option<Level>,
    pub fix: bool,
    pub options: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Override {
    pub files: Vec<String>,
    pub rules: Vec<LintSetting>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Lint {
    pub preset: Option<LintPreset>,
    pub sets: Vec<String>,
    pub exclude: Vec<String>,
    pub ecosystems: Vec<&'static ExtensionMeta>,
    pub rules: Vec<LintRule>,
    pub overrides: Vec<Override>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnalyseRule {
    pub id: String,
    pub options: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Analyse {
    pub preset: AnalysePreset,
    pub paths: Vec<String>,
    pub exclude: Vec<String>,
    pub baseline: Option<String>,
    pub stubs: Vec<String>,
    /// Files and directories read for their symbols but not analysed: PHPStan's `scanFiles` and `scanDirectories`.
    pub scan: Vec<String>,
    /// PHPStan's `bootstrapFiles`. Pun does not run them: it reads their symbols, as `scan` does, and Larastan
    /// their macros.
    pub bootstrap: Vec<String>,
    pub ignore: Vec<Ignore>,
    pub switches: Vec<&'static str>,
    pub ecosystems: Vec<&'static ExtensionMeta>,
    pub larastan: bool,
    /// phpat's settings, when the `phpat` extension is on.
    pub phpat: Option<extensions::Phpat>,
    /// `php-rules` (prototype): neon files whose PHPStan rule classes a PHP worker runs.
    pub php_rules: Vec<String>,
    /// `php-rules-workers`: how many PHP workers run the `php-rules`; `None` is the number of cores, at most 8.
    pub php_rules_workers: Option<u16>,
    /// `php-rules-paths`: the paths the `php-rules` run on, relative to the project; empty is all of `paths`.
    pub php_rules_paths: Vec<String>,
    pub rules: Vec<AnalyseRule>,
    pub bleeding_edge: bool,
    /// PHPStan's `reportUnmatchedIgnoredErrors`: `false` reports no unmatched ignore comment and makes
    /// `report-unmatched = false` the default of each `[[analyse.ignore]]` entry.
    pub report_unmatched: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Ignore {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_message: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// PHPStan's `reportUnmatched`: `false` doesn't report the entry when it matches nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_unmatched: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct FilesToml {
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct LintToml {
    pub(crate) preset: Option<String>,
    #[serde(default)]
    sets: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    groups: toml::Table,
    #[serde(default)]
    rules: toml::Table,
    #[serde(default)]
    overrides: Vec<OverrideToml>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OverrideToml {
    files: Vec<String>,
    #[serde(default)]
    rules: toml::Table,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct AnalyseToml {
    pub(crate) preset: Option<toml::Value>,
    paths: Option<Vec<String>>,
    #[serde(default)]
    exclude: Vec<String>,
    baseline: Option<String>,
    #[serde(default)]
    stubs: Vec<String>,
    #[serde(default)]
    scan: Vec<String>,
    #[serde(default)]
    bootstrap: Vec<String>,
    #[serde(default, rename = "php-rules")]
    php_rules: Vec<String>,
    #[serde(rename = "php-rules-workers")]
    php_rules_workers: Option<std::num::NonZeroU16>,
    #[serde(default, rename = "php-rules-paths")]
    php_rules_paths: Vec<String>,
    #[serde(default)]
    ignore: Vec<Ignore>,
    #[serde(default)]
    rules: toml::Table,
    #[serde(default, rename = "bleeding-edge")]
    bleeding_edge: bool,
    #[serde(rename = "union-types")]
    union_types: Option<bool>,
    nullables: Option<bool>,
    #[serde(rename = "explicit-mixed")]
    explicit_mixed: Option<bool>,
    #[serde(rename = "implicit-mixed")]
    implicit_mixed: Option<bool>,
    #[serde(rename = "report-unmatched")]
    report_unmatched: Option<bool>,
    level: Option<toml::Value>,
}

impl AnalyseToml {
    /// The lint rules this `[analyse]` preset turns on: a PHPStan preset's checks that are lint rules in Pun.
    pub(crate) fn phpstan_lint(&self, path: &Path) -> Result<BTreeSet<String>> {
        let preset = analyse_preset(path, self.preset.as_ref())?;
        Ok((preset.phpstan_level())
            .map_or_default(|l| phpstan_level_rules(l, self.bleeding_edge))
            .into_iter()
            .filter(|id| id.starts_with("lint/"))
            .collect())
    }
}

impl Files {
    pub(crate) fn from(t: FilesToml) -> Files {
        Files {
            paths: t.paths,
            exclude: t.exclude,
        }
    }
}

pub(crate) struct Gates<'a> {
    pub php: PhpVersion,
    pub composer: &'a Composer,
    pub switches: BTreeMap<&'static str, extensions::Switch>,
    /// `[extensions] phpat`'s settings, when it is a table.
    pub phpat: Option<extensions::Phpat>,
}

impl Gates<'_> {
    fn locked(&self, e: &ExtensionMeta) -> Option<&Locked> {
        self.composer.locked(e.packages.first()?)
    }

    /// The extensions `[extensions]` turns on.
    pub(crate) fn extensions(&self) -> Vec<&'static ExtensionMeta> {
        (extensions::ALL.into_iter())
            .filter(|e| self.switches.get(e.id) == Some(&extensions::Switch::On))
            .collect()
    }

    pub(crate) fn ecosystems(&self, off: impl Fn(&str) -> bool) -> Vec<&'static ExtensionMeta> {
        let mut on = self.extensions();
        on.retain(|e| !off(e.id));
        on
    }

    pub(crate) fn allows(&self, group: &str, on: &[&ExtensionMeta]) -> bool {
        if let Some(v) = group.strip_prefix("php-") {
            return v.parse().is_ok_and(|v: PhpVersion| self.php >= v);
        }
        match extensions::owner(group) {
            Some((e, None)) => on.contains(&e),
            Some((e, Some(v))) => {
                let locked = self.locked(e).and_then(|l| l.semver);
                on.contains(&e)
                    && version(v)
                        .zip(locked)
                        .is_some_and(|(v, l)| (l.major, l.minor) >= v)
            }
            None => true,
        }
    }
}

pub(crate) fn version(v: &str) -> Option<(u64, u64)> {
    let (major, minor) = v.split_once('.').unwrap_or((v, "0"));
    Some((major.parse().ok()?, minor.parse().ok()?))
}

pub fn groups() -> &'static Vec<String> {
    static T: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| {
        (crate::mapping::section(&["meta", "groups"]).table().keys())
            .cloned()
            .collect()
    });
    &T
}

fn group_of(id: &str) -> &str {
    id.split('/').nth(1).unwrap()
}

pub(crate) fn rule_id(path: &Path, command: &str, name: &str) -> Result<String> {
    let id = match name.strip_prefix(command).and_then(|r| r.strip_prefix('/')) {
        Some(_) => name.to_string(),
        None => format!("{command}/{name}"),
    };
    if fixer_of(command, name).is_some() || rules().iter().any(|(r, _)| *r == id) {
        return Ok(id);
    }
    let other = if command == "format" {
        "lint"
    } else {
        "format"
    };
    if let Some(id) = crate::merged_into(name) {
        bail!(
            "{}: `{name}` was merged into `{id}`: use `{id}` in pun.toml",
            path.display()
        );
    }
    let found = (fixer_of(other, name).and_then(crate::rule_of)).or_else(|| renamed(name));
    if let Some(id) = found {
        bail!("{}: `{name}` is `{id}` in pun.toml", path.display())
    }
    let a = if command == "analyse" { "an" } else { "a" };
    bail!("{}: `{name}` is not {a} {command} rule", path.display())
}

fn level(path: &Path, key: &str, s: &str) -> Result<Option<Level>> {
    Ok(match s {
        "off" => None,
        "warn" => Some(Level::Warn),
        "error" => Some(Level::Error),
        _ => bail!(
            "{}: `{key}` = \"{s}\": expected \"error\", \"warn\" or \"off\"",
            path.display()
        ),
    })
}

fn setting(path: &Path, name: &str, v: toml::Value) -> Result<LintSetting> {
    let id = rule_id(path, "lint", name)?;
    let shown = |v: &toml::Value| v.to_string();
    let (level, fix, opts) = match v {
        toml::Value::String(s) => (level(path, name, &s)?, true, toml::Table::new()),
        toml::Value::Boolean(b) => bail!(
            "{}: `{name}` = {b}: lint rules take a level; use \"{}\"",
            path.display(),
            if b { "error" } else { "off" }
        ),
        toml::Value::Table(mut t) => {
            let level = match t.remove("level") {
                None => Some(Level::Error),
                Some(toml::Value::String(s)) => level(path, name, &s)?,
                Some(v) => bail!(
                    "{}: `{name}`: level = {}: expected \"error\", \"warn\" or \"off\"",
                    path.display(),
                    shown(&v)
                ),
            };
            let fix = match t.remove("fix") {
                None => true,
                Some(f) => match f.as_str() {
                    Some("off") => false,
                    _ => bail!(
                        "{}: `{name}`: fix = {}: the only value is \"off\"",
                        path.display(),
                        shown(&f)
                    ),
                },
            };
            (level, fix, t)
        }
        v => bail!(
            "{}: `{name}` = {}: expected \"error\", \"warn\", \"off\" or a table",
            path.display(),
            shown(&v)
        ),
    };
    let options = if opts.is_empty() {
        Value::Null
    } else if fixer_of("lint", &id).is_some() {
        options::to_fixer(name, &id, toml::Value::Table(opts))
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?
    } else {
        serde_json::to_value(opts).unwrap()
    };
    Ok(LintSetting {
        id,
        level,
        fix,
        options,
    })
}

/// `phpstan`: the lint rules a PHPStan preset in `[analyse]` turns on (PHPStan checks that are lint rules in Pun),
/// at `error`, below `[lint.groups]` and `[lint.rules]`.
pub(crate) fn lint(
    path: &Path,
    t: LintToml,
    files: &Files,
    gates: &Gates,
    phpstan: &BTreeSet<String>,
) -> Result<Lint> {
    check_sets(path, &t.sets)?;
    let preset = lint_preset(path, t.preset.as_deref())?;
    let mut groups = BTreeMap::new();
    for (g, v) in t.groups {
        if !groups_contains(&g) {
            bail!(
                "{}: [lint.groups]: `{g}` is not a group (see [meta.groups] in crates/pun-config/rules/mapping.toml)",
                path.display()
            );
        }
        let toml::Value::String(s) = v else {
            bail!(
                "{}: [lint.groups] `{g}` = {v}: expected \"error\", \"warn\" or \"off\"",
                path.display()
            );
        };
        groups.insert(g.clone(), level(path, &g, &s)?);
    }
    let named = (t.rules.into_iter())
        .map(|(name, v)| setting(path, &name, v))
        .collect::<Result<Vec<_>>>()?;
    let overrides = (t.overrides.into_iter())
        .map(|o| {
            Ok(Override {
                files: o.files,
                rules: (o.rules.into_iter())
                    .map(|(name, v)| setting(path, &name, v))
                    .collect::<Result<_>>()?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let ecosystems = gates.ecosystems(|e| groups.get(e) == Some(&None));
    let rule = |id: String, level, fix, options, named| LintRule {
        fixer: fixer_of("lint", &id),
        id,
        level,
        fix,
        options,
        named,
    };
    let members = rector_members(&t.sets, gates.php);
    let mut on: BTreeMap<String, LintRule> = BTreeMap::new();
    if preset.is_some() || !members.is_empty() || !groups.is_empty() {
        let ported =
            |e: &toml::Value| e.get("ported").and_then(toml::Value::as_bool) != Some(false);
        for (id, e) in rules()
            .iter()
            .filter(|(id, e)| id.starts_with("lint/") && ported(e))
        {
            let group = group_of(id);
            let tier = (e.get("preset").and_then(toml::Value::as_str)).and_then(LintPreset::parse);
            let in_tier = matches!((tier, preset), (Some(t), Some(p)) if t <= p);
            let classes = members.get(id.as_str());
            let level = match groups.get(group) {
                Some(l) => *l,
                None => ((in_tier || classes.is_some()) && gates.allows(group, &ecosystems))
                    .then_some(Level::Error),
            };
            let Some(level) = level else { continue };
            // A rule that only a Rector set brings runs as that set's Rector rules do.
            let options = match classes {
                Some(classes) if !in_tier => as_rector(path, id, classes)?,
                _ => Value::Null,
            };
            on.insert(id.clone(), rule(id.clone(), level, true, options, false));
        }
    }
    for id in phpstan_lint_on(phpstan, &groups, gates, &ecosystems) {
        on.entry(id.clone())
            .or_insert_with(|| rule(id, Level::Error, true, Value::Null, false));
    }
    for s in named {
        match s.level {
            Some(level) => drop(on.insert(s.id.clone(), rule(s.id, level, s.fix, s.options, true))),
            None => drop(on.remove(&s.id)),
        }
    }
    Ok(Lint {
        preset,
        sets: t.sets,
        exclude: [&files.exclude[..], &t.exclude[..]].concat(),
        ecosystems,
        rules: on.into_values().collect(),
        overrides,
    })
}

/// The PHPStan preset's lint rules that run: ported, in no group `[lint.groups]` sets, allowed by their gates.
fn phpstan_lint_on(
    phpstan: &BTreeSet<String>,
    groups: &BTreeMap<String, Option<Level>>,
    gates: &Gates,
    ecosystems: &[&'static ExtensionMeta],
) -> Vec<String> {
    let ported = |e: &toml::Value| e.get("ported").and_then(toml::Value::as_bool) != Some(false);
    (rules().iter())
        .filter(|(id, e)| phpstan.contains(id) && ported(e))
        .map(|(id, _)| id)
        .filter(|id| {
            let group = group_of(id);
            !groups.contains_key(group) && gates.allows(group, ecosystems)
        })
        .cloned()
        .collect()
}

/// The options that keep `id` to what `classes` (its Rector rules in the sets) rewrite, as `pun migrate` writes them.
fn as_rector(path: &Path, id: &str, classes: &BTreeSet<String>) -> Result<Value> {
    let options = crate::migrate::rector::as_rector_options(id, classes);
    if options.is_empty() {
        return Ok(Value::Null);
    }
    Ok(setting(path, &id[5..], toml::Value::Table(options))?.options)
}

fn groups_contains(g: &str) -> bool {
    groups().iter().any(|x| x == g)
}

fn list(items: &[&str]) -> String {
    let quoted: Vec<String> = items.iter().map(|s| format!("`{s}`")).collect();
    quoted.join(", ")
}

#[expect(
    clippy::too_many_lines,
    reason = "long; to be split (code audit: functions over 100 lines)"
)]
pub(crate) fn analyse(
    path: &Path,
    t: AnalyseToml,
    files: &Files,
    gates: &Gates,
) -> Result<Analyse> {
    if let Some(level) = t.level {
        let hint = level
            .as_integer()
            .and_then(|l| usize::try_from(l).ok())
            .filter(|l| *l <= 10)
            .map_or_default(|l| {
                format!(
                    ": level {l} is preset = \"phpstan/level_{l}\" (or Pun's \"{}\")",
                    AnalysePreset::of_level(l)
                )
            });
        let presets: Vec<String> = (AnalysePreset::ALL.iter())
            .map(|p| format!("\"{p}\""))
            .collect();
        bail!(
            "{}: [analyse] level: PHPStan's levels are presets in Pun; use preset = {} or \"phpstan/level_<n>\"{hint}",
            path.display(),
            presets.join(" | ")
        );
    }
    let preset = analyse_preset(path, t.preset.as_ref())?;

    let mut groups = BTreeMap::new();
    let mut named = vec![];
    for (name, v) in t.rules {
        let on = match v {
            toml::Value::String(s) if s == "on" => Some(Value::Null),
            toml::Value::String(s) if s == "off" => None,
            toml::Value::Table(t) => Some(serde_json::to_value(t).unwrap()),
            v => bail!(
                "{}: [analyse.rules] `{name}` = {v}: analyse rules are \"on\" or \"off\" (or a table of options); there are no levels",
                path.display()
            ),
        };
        if groups_contains(&name) {
            if !matches!(on, None | Some(Value::Null)) {
                bail!(
                    "{}: [analyse.rules] `{name}` is a group: it takes \"on\" or \"off\"",
                    path.display()
                );
            }
            groups.insert(name, on.is_some());
        } else {
            let id = rule_id(path, "analyse", &name)?;
            if let Some(Value::Object(o)) = &on {
                crate::phpstan_parameters::check_options(&id, o).map_err(|e| {
                    anyhow::anyhow!("{}: [analyse.rules] `{name}`{e}", path.display())
                })?;
            }
            named.push((id, on));
        }
    }
    let ecosystems = gates.ecosystems(|e| groups.get(e) == Some(&false));
    let level_rules = (preset.phpstan_level()).map(|l| phpstan_level_rules(l, t.bleeding_edge));
    let mut on: BTreeMap<String, Value> = BTreeMap::new();
    for (id, e) in rules().iter().filter(|(id, _)| id.starts_with("analyse/")) {
        let group = group_of(id);
        let enabled = if let Some(g) = groups.get(group) {
            *g
        } else {
            let in_preset = match &level_rules {
                Some(r) => r.contains(id),
                None => (e.get("preset").and_then(toml::Value::as_str))
                    .and_then(AnalysePreset::parse)
                    .is_some_and(|p| preset.includes(p)),
            };
            in_preset && gates.allows(group, &ecosystems)
        };
        if enabled {
            on.insert(id.clone(), Value::Null);
        }
    }
    for (id, v) in named {
        match v {
            Some(v) => drop(on.insert(id, v)),
            None => drop(on.remove(&id)),
        }
    }
    let mut ignore = t.ignore;
    for (n, i) in ignore.iter_mut().enumerate() {
        let at = || format!("{}: [[analyse.ignore]] #{}", path.display(), n + 1);
        if let Some(r) = &i.rule {
            i.rule = Some(rule_id(path, "analyse", r)?);
        }
        if i.message.is_some() && i.raw_message.is_some() {
            bail!("{}: message and raw-message: give one", at());
        }
        if i.rule.is_none()
            && i.identifier.is_none()
            && i.message.is_none()
            && i.raw_message.is_none()
        {
            bail!("{}: name a rule, an identifier or a message", at());
        }
    }
    Ok(Analyse {
        switches: (SWITCHES.iter())
            .zip([
                t.union_types,
                t.nullables,
                t.explicit_mixed,
                t.implicit_mixed,
            ])
            .filter(|((p, s), on)| {
                on.unwrap_or_else(|| match preset.phpstan_level() {
                    Some(level) => (crate::phpstan_parameters::SWITCH_LEVELS.iter())
                        .any(|(w, l)| w == s && level >= *l),
                    None => preset.includes(*p),
                })
            })
            .map(|((_, s), _)| *s)
            .collect(),
        preset,
        paths: t.paths.unwrap_or_else(|| files.paths.clone()),
        exclude: [&files.exclude[..], &t.exclude[..]].concat(),
        baseline: t.baseline,
        stubs: t.stubs,
        scan: t.scan,
        bootstrap: t.bootstrap,
        php_rules: t.php_rules,
        php_rules_workers: t.php_rules_workers.map(std::num::NonZeroU16::get),
        php_rules_paths: t.php_rules_paths,
        ignore,
        larastan: ecosystems.contains(&&extensions::LARAVEL)
            && crate::phpstan_parameters::larastan_supports(
                gates.locked(&extensions::LARAVEL).and_then(|l| l.semver),
            ),
        phpat: (ecosystems.contains(&&extensions::PHPAT))
            .then(|| gates.phpat.clone().unwrap_or_default()),
        ecosystems,
        rules: (on.into_iter())
            .map(|(id, options)| AnalyseRule { id, options })
            .collect(),
        bleeding_edge: t.bleeding_edge,
        report_unmatched: t.report_unmatched.unwrap_or(true),
    })
}

/// `[analyse] preset`, or `recommended` without one. A number names the presets that hold that PHPStan level.
pub(crate) fn analyse_preset(path: &Path, v: Option<&toml::Value>) -> Result<AnalysePreset> {
    Ok(match v {
        None => AnalysePreset::Recommended,
        Some(v) if let Some(p) = v.as_str().and_then(AnalysePreset::parse) => p,
        Some(toml::Value::Integer(l)) if let Ok(level @ 0..=10) = usize::try_from(*l) => bail!(
            "{}: [analyse] preset = {l}: PHPStan level {l} is preset = \"phpstan/level_{l}\" (or Pun's \"{}\")",
            path.display(),
            AnalysePreset::of_level(level)
        ),
        Some(v) => bail!(
            "{}: [analyse] preset = {v}: expected {}, or `phpstan/level_0` to `phpstan/level_10` or `phpstan/level_max`",
            path.display(),
            list(&AnalysePreset::ALL.map(AnalysePreset::name))
        ),
    })
}

/// The rules (analyse and lint IDs) `phpstan analyse --level <level>` runs with its default parameters, and with
/// Larastan's defaults (its rules only run where the `laravel` extension is on): every PHPStan core rule class of
/// that level or below, without the opt-in ones (a gate with no `default true`), the bleeding-edge ones only with
/// `bleeding_edge`. phpstan-strict-rules and phpstan-deprecation-rules are separate packages, not levels: none of
/// theirs.
pub fn phpstan_level_rules(level: u8, bleeding_edge: bool) -> BTreeSet<String> {
    let classes = crate::mapping::section(&["upstream", "phpstan-classes"]).table();
    let mut out = BTreeSet::new();
    for e in classes.values() {
        let Some(id) = e.get("pun").and_then(toml::Value::as_str) else {
            continue;
        };
        let package = e.get("package").and_then(toml::Value::as_str);
        let in_level = match package {
            Some("phpstan") => (e.get("level").and_then(toml::Value::as_integer))
                .is_none_or(|l| l <= i64::from(level)),
            Some("larastan" | "phpat") => true,
            _ => false,
        };
        let gated = (e.get("gated_by").and_then(toml::Value::as_str))
            .is_none_or(|g| default_on(g, bleeding_edge));
        if in_level && gated {
            out.insert(id.to_string());
        }
    }
    out
}

/// Whether a `gated_by` condition holds with PHPStan's default parameters: a bleeding-edge toggle with
/// `bleeding_edge`, a parameter whose note says `default true`.
fn default_on(condition: &str, bleeding_edge: bool) -> bool {
    condition.split(" + ").all(|part| {
        let (name, note) = part.split_once(" (").unwrap_or((part, ""));
        if name == "bleedingEdge" {
            bleeding_edge
        } else {
            note.starts_with("default true")
        }
    })
}
