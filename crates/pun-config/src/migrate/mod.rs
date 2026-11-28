pub mod phpstan;
pub(crate) mod rector;

pub use rector::{Configured, Reconciled, Rector, RectorConfig, compose, from_rector};

use crate::{Rule, fixer_entries, preset_rules, rule_of, rule_set};
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use std::fmt::Write;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct Parts {
    pub php: String,
    pub files: String,
    pub extensions: String,
    pub format: String,
    pub format_rules: String,
    pub lint: String,
    pub lint_rules: Vec<(String, String)>,
    /// The files Pint's or PHP-CS-Fixer's finder skips (`notPath`, `notName`), as `[[lint.overrides]]` globs, and
    /// the lint rules from that config turned off there: a rule another config also asks for leaves this list.
    pub lint_skip_files: Vec<String>,
    pub lint_skip_rules: Vec<String>,
    pub overrides: String,
    pub analyse: String,
    pub analyse_rules: Vec<(String, String)>,
    pub analyse_ignores: String,
}

pub(super) fn require_lint(parts: &mut Parts, id: &str, value: &str) -> bool {
    let ported = rector::ported(id);
    let name = &id[5..];
    parts.lint_rules.retain(|(k, _)| !(ported && k == name));
    (parts.lint_rules).extend(ported.then(|| (name.to_string(), value.to_string())));
    ported
}

impl Parts {
    pub fn render(&self) -> String {
        let pairs = |rules: &[(String, String)]| -> String {
            rules.iter().fold(String::new(), |mut out, (k, v)| {
                let _ = writeln!(out, "\"{k}\" = {v}");
                out
            })
        };
        let sections = [
            ("[php]\n", self.php.clone()),
            ("[files]\n", self.files.clone()),
            ("[extensions]\n", self.extensions.clone()),
            ("[format]\n", self.format.clone()),
            ("[format.rules]\n", self.format_rules.clone()),
            ("[lint]\n", self.lint.clone()),
            ("[lint.rules]\n", pairs(&self.lint_rules)),
            ("", self.lint_skip()),
            ("", self.overrides.clone()),
            ("[analyse]\n", self.analyse.clone()),
            ("[analyse.rules]\n", pairs(&self.analyse_rules)),
            ("", self.analyse_ignores.clone()),
        ];
        let shown: Vec<String> = (sections.into_iter())
            .filter(|(_, body)| !body.is_empty())
            .map(|(head, body)| format!("{head}{body}"))
            .collect();
        shown.join("\n")
    }

    fn lint_skip(&self) -> String {
        if self.lint_skip_files.is_empty() || self.lint_skip_rules.is_empty() {
            return String::new();
        }
        let files = toml::Value::try_from(&self.lint_skip_files).unwrap();
        (self.lint_skip_rules.iter()).fold(
            format!("[[lint.overrides]]\nfiles = {files}\n\n[lint.overrides.rules]\n"),
            |mut out, id| {
                writeln!(out, "\"{id}\" = \"off\"").unwrap();
                out
            },
        )
    }
}

pub struct Migration {
    pub toml: String,
    pub parts: Parts,
    pub unsupported: Vec<String>,
    pub preset: &'static str,
    pub format_rules: usize,
    pub lint_rules: usize,
}

pub type Runs<'a> = &'a dyn Fn(&str) -> bool;

/// `[extensions]`'s body: each extension `composer` shows the project uses, turned on.
pub fn extensions(composer: &crate::composer::Composer) -> String {
    (crate::extensions::detected(composer).into_iter()).fold(String::new(), |mut out, e| {
        let _ = writeln!(out, "{} = \"on\"", e.id);
        out
    })
}

/// `root` is the project: the directory `pun migrate` runs in, which Pint's relative `in` directories start from.
pub fn from_pint(path: &Path, root: &Path, preset: Option<&str>, runs: Runs) -> Result<Migration> {
    let pint = read_pint_json(path)?;
    let preset = preset
        .or_else(|| pint.get("preset").and_then(Value::as_str))
        .unwrap_or("laravel");
    if preset == "symfony" {
        bail!(
            "the symfony preset was removed: Pun supports the laravel, psr12, per and empty presets"
        );
    }
    let mut rules = preset_rules(preset).context("Preset not found.")?;
    if let Some(r) = pint.get("rules").and_then(Value::as_object) {
        rules.extend(r.clone());
    }
    for (name, v) in &rules {
        if !matches!(v, Value::Bool(_) | Value::Array(_) | Value::Object(_)) {
            let (what, kind) = if name.starts_with('@') {
                ("Set must be enabled (true) or disabled (false).", "set")
            } else {
                (
                    "Rule must be enabled (true), disabled (false) or configured (non-empty, assoc array).",
                    "rule",
                )
            };
            let null = if v.is_null() {
                format!(" To disable the {kind}, use \"FALSE\" instead of \"NULL\".")
            } else {
                String::new()
            };
            bail!("[{name}] {what} Other values are not allowed.{null}");
        }
    }
    let keys = finder(&pint, Some(root));
    let off = disabled(pint.get("rules").and_then(Value::as_object));
    write(preset, rule_set(rules)?, &off, keys, vec![], runs)
}

pub fn from_rules(
    rules: Map<String, Value>,
    finder_of: &Map<String, Value>,
    runs: Runs,
) -> Result<Migration> {
    let keys = finder(finder_of, None);
    let unsupported = (finder_of.get("unsupported").and_then(Value::as_array))
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|s| {
            if s.starts_with("setIndent(") {
                format!("Config {s}: format indents with four spaces or tabs")
            } else {
                format!("Finder {s}: no pun.toml key for it")
            }
        })
        .collect();
    let off = disabled(Some(&rules));
    write("empty", rule_set(rules)?, &off, keys, unsupported, runs)
}

/// The rules a config turns off itself (`"rule": false`), not through a set or preset.
fn disabled(rules: Option<&Map<String, Value>>) -> Vec<String> {
    (rules.into_iter().flatten())
        .filter(|(name, v)| !name.starts_with('@') && **v == Value::Bool(false))
        .map(|(name, _)| name.clone())
        .collect()
}

/// A Pint or PHP-CS-Fixer finder's keys in pun.toml.
#[derive(Default)]
struct FinderKeys {
    /// `[files]`'s lines.
    paths: String,
    /// `[format]`'s lines.
    format: String,
    /// Pint and PHP-CS-Fixer run every rule, lint ones too, on the files their finder keeps, so the files `notPath`
    /// and `notName` skip get a `[[lint.overrides]]` with these globs that turns the config's lint rules off.
    lint_skip: Vec<String>,
    /// The `notPath` and `notName` patterns no glob matches the same files as.
    lint_unsupported: Vec<String>,
}

/// `[files]` and `[format]`'s finder keys, and the lint globs for `notPath` and `notName`; with `root`, an `in`
/// directory under it becomes relative, as `pun lint` matches the globs against the path below the project.
fn finder(config: &Map<String, Value>, root: Option<&Path>) -> FinderKeys {
    let mut keys = FinderKeys::default();
    let list = |json: &str| -> Vec<&str> {
        match config.get(json) {
            Some(Value::String(s)) => vec![s],
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        }
    };
    for (json, key) in [
        ("in", "paths"),
        ("exclude", "exclude"),
        ("notPath", "not-path"),
        ("notName", "not-name"),
    ] {
        let list = list(json);
        if list.is_empty() {
            continue;
        }
        let out = if matches!(json, "in" | "exclude") {
            &mut keys.paths
        } else {
            &mut keys.format
        };
        writeln!(out, "{key} = {}", toml::Value::try_from(list).unwrap()).unwrap();
    }
    // Symfony's Finder matches a `notPath` string anywhere in the path relative to the directory searched
    // (`PathFilterIterator`), and a `notName` glob against the file name, where `*` stops at `/` (`Glob::toRegex`).
    // A `[[lint.overrides]]` glob is matched against the path relative to the project, and its `*` crosses `/`.
    // Each `in` directory is its own walk, and a file stays when one walk keeps it: `""` is the project.
    let mut dirs: Vec<String> = (list("in").into_iter())
        .map(|d| root.map_or_else(|| d.to_string(), |r| rector::relative(r, d)))
        .map(|d| {
            let d = d.trim_start_matches("./").trim_end_matches('/');
            if d == "." {
                String::new()
            } else {
                d.to_string()
            }
        })
        .collect();
    if dirs.is_empty() {
        dirs.push(String::new());
    }
    dirs.dedup();
    'paths: for p in list("notPath") {
        if crate::is_regex(p) {
            keys.lint_unsupported.push(format!(
                "notPath \"{p}\": a regex, which no [[lint.overrides]] glob holds, so the lint rules still check the files it skips"
            ));
            continue;
        }
        for (outer, inner) in nested(&dirs) {
            if spans(
                p,
                &format!("{}/", inner[outer.len()..].trim_start_matches('/')),
            ) {
                let shown = |d: &str| {
                    if d.is_empty() {
                        ".".to_string()
                    } else {
                        d.to_string()
                    }
                };
                keys.lint_unsupported.push(format!(
                    "notPath \"{p}\": the `in` directories \"{}\" and \"{}\" nest, and a file Pint keeps from one would match the other's glob, so the lint rules still check the files it skips",
                    shown(outer),
                    shown(inner)
                ));
                continue 'paths;
            }
        }
        let p = escape_glob(p);
        (keys.lint_skip).extend(dirs.iter().map(|d| match d.as_str() {
            "" => format!("*{p}*"),
            d => format!("{}/*{p}*", escape_glob(d)),
        }));
    }
    for n in list("notName") {
        let why = if crate::is_regex(n) {
            "a regex, which no [[lint.overrides]] glob holds"
        } else if n.strip_prefix('*').unwrap_or(n).contains(GLOB) || n.contains('/') {
            "a [[lint.overrides]] glob's wildcards would also match directory names"
        } else {
            keys.lint_skip.push(format!("**/{n}"));
            continue;
        };
        keys.lint_unsupported.push(format!(
            "notName \"{n}\": {why}, so the lint rules still check the files it skips"
        ));
    }
    keys
}

/// The pairs of `in` directories where the second lies under the first.
fn nested(dirs: &[String]) -> impl Iterator<Item = (&str, &str)> {
    dirs.iter().flat_map(move |outer| {
        (dirs.iter())
            .filter(move |inner| {
                *inner != outer && (outer.is_empty() || inner.starts_with(&format!("{outer}/")))
            })
            .map(move |inner| (outer.as_str(), inner.as_str()))
    })
}

/// Whether `p` can occur in `q` followed by any path at a start inside `q`: a file below the inner directory (`q` is
/// its path below the outer one, with a trailing `/`) then matches the outer walk's `notPath` but maybe not the
/// inner walk's. Any other occurrence lies in the path below the inner directory, which both walks see.
fn spans(p: &str, q: &str) -> bool {
    (0..q.len())
        .filter(|&i| q.is_char_boundary(i))
        .any(|i| q[i..].starts_with(p) || p.starts_with(&q[i..]))
}

/// The characters a `[[lint.overrides]]` glob gives a meaning.
const GLOB: [char; 7] = ['*', '?', '[', ']', '{', '}', '\\'];

fn escape_glob(s: &str) -> String {
    s.chars().fold(String::new(), |mut out, c| {
        if GLOB.contains(&c) {
            out.push('\\');
        }
        out.push(c);
        out
    })
}

fn read_pint_json(path: &Path) -> Result<Map<String, Value>> {
    if !path.exists() {
        return Ok(Map::new());
    }
    let invalid = || {
        format!(
            "The configuration file [{}] is not valid JSON.",
            path.display()
        )
    };
    let raw = std::fs::read(path).with_context(|| format!("can't read {}", path.display()))?;
    let mut v: Value = serde_json::from_slice(&raw).with_context(invalid)?;
    if let Some(ext) = v.get("extend").and_then(Value::as_str) {
        let ext_path = path.parent().unwrap().join(ext);
        let raw =
            std::fs::read(&ext_path).map_err(|_| anyhow::anyhow!("Path must not be empty"))?;
        let base: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
        let given = match &base {
            Value::Object(_) | Value::Array(_) => None,
            Value::Null => Some("null"),
            Value::Bool(true) => Some("true"),
            Value::Bool(false) => Some("false"),
            Value::Number(n) if n.is_i64() => Some("int"),
            Value::Number(_) => Some("float"),
            Value::String(_) => Some("string"),
        };
        if let Some(t) = given {
            bail!(
                "array_replace_recursive(): Argument #1 ($array) must be of type array, {t} given"
            );
        }
        if base.get("extend").is_some() {
            bail!("Pint configuration cannot extend from more than 1 file.");
        }
        let mut merged = base;
        replace_recursive(&mut merged, v);
        v = merged;
    }
    match v {
        Value::Object(m) => Ok(m),
        Value::Array(_) => Ok(Map::new()),
        _ => bail!(invalid()),
    }
}

fn replace_recursive(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(slot) => replace_recursive(slot, v),
                    None => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (Value::Array(b), Value::Array(o)) => {
            for (i, v) in o.into_iter().enumerate() {
                match b.get_mut(i) {
                    Some(slot) => replace_recursive(slot, v),
                    None => b.push(v),
                }
            }
        }
        (b, o) => *b = o,
    }
}

fn write(
    preset: &str,
    mut rules: Vec<Rule>,
    off: &[String],
    keys: FinderKeys,
    mut unsupported: Vec<String>,
    runs: Runs,
) -> Result<Migration> {
    let preset = crate::FormatPreset::parse(preset).map_or("empty", crate::FormatPreset::name);
    resolve_proxies(&mut rules);
    merge_visibility_required(&mut rules);
    let base = rule_set(preset_rules(preset).unwrap())?;
    let mut parts = Parts {
        files: keys.paths,
        format: format!("preset = \"{preset}\"\n{}", keys.format),
        ..Parts::default()
    };
    let mut format_rules = 0;
    for r in &rules {
        let id = rule_of(&r.name);
        let lint = id.is_some_and(|id| id.starts_with("lint/"));
        let Some(id) = id.filter(|_| runs(&r.name)) else {
            let entry = fixer_entries().get(&r.name);
            let why = entry
                .and_then(|e| e.get("drop").or(e.get("unsupported")))
                .and_then(toml::Value::as_str)
                .unwrap_or(match id {
                    Some(_) => "not supported by Pun yet",
                    None => {
                        "not a PHP-CS-Fixer or Pint rule Pun knows (a custom fixer or rule set)"
                    }
                });
            unsupported.push(format!("{}: {why}", r.name));
            continue;
        };
        if !lint
            && base
                .iter()
                .any(|b| b.name == r.name && b.options == r.options)
        {
            continue;
        }
        let name = &id[id.find('/').unwrap() + 1..];
        let v = match crate::options::to_pun(&r.name, id, &r.options)? {
            Value::Bool(true) if lint => Value::from("error"),
            v => v,
        };
        let Some(v) = toml_value(&v) else {
            unsupported.push(format!(
                "{}: an option is null, which TOML can't hold",
                r.name
            ));
            continue;
        };
        if lint {
            parts.lint_rules.push((name.to_string(), v));
        } else {
            writeln!(parts.format_rules, "\"{name}\" = {v}").unwrap();
            format_rules += 1;
        }
    }
    // An extension brings every rule of its group (`phpunit/require-method-casing` for `php_unit_method_casing`), so
    // one the config turns off stays off, as in Pint.
    for name in off.iter().filter(|n| runs(n)) {
        let Some(id) = rule_of(name).and_then(|id| id.strip_prefix("lint/")) else {
            continue;
        };
        let owned = (id.split('/').next()).is_some_and(|g| crate::extensions::owner(g).is_some());
        if owned && !parts.lint_rules.iter().any(|(k, _)| k == id) {
            parts.lint_rules.push((id.to_string(), "\"off\"".into()));
        }
    }
    skip_lint(
        &mut parts,
        keys.lint_skip,
        keys.lint_unsupported,
        &mut unsupported,
    );
    for b in &base {
        let off = !rules.iter().any(|r| r.name == b.name) || !runs(&b.name);
        if let Some(name) = rule_of(&b.name).and_then(|id| id.strip_prefix("format/"))
            && off
        {
            writeln!(parts.format_rules, "\"{name}\" = false").unwrap();
            format_rules += 1;
        }
    }
    Ok(Migration {
        toml: parts.render(),
        lint_rules: parts.lint_rules.len(),
        parts,
        unsupported,
        preset,
        format_rules,
    })
}

/// PHP-CS-Fixer's deprecated `visibility_required` runs `modifier_keywords`: its elements join the successor's.
fn merge_visibility_required(rules: &mut Vec<Rule>) {
    if let Some(i) = rules.iter().position(|r| r.name == "visibility_required") {
        let vr = rules.remove(i).options;
        match rules.iter_mut().find(|r| r.name == "modifier_keywords") {
            None => rules.push(Rule {
                name: "modifier_keywords".into(),
                options: vr,
            }),
            Some(mk) if vr.is_null() || mk.options.is_null() => mk.options = Value::Null,
            Some(mk) => {
                let mut all = mk.options["elements"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                for e in vr["elements"].as_array().into_iter().flatten() {
                    if !all.contains(e) {
                        all.push(e.clone());
                    }
                }
                mk.options = serde_json::json!({ "elements": all });
            }
        }
    }
}

/// The `[[lint.overrides]]` that turns the config's lint rules off where its finder skips files, and the patterns no
/// glob holds; neither when no lint rule is on.
fn skip_lint(
    parts: &mut Parts,
    globs: Vec<String>,
    notes: Vec<String>,
    unsupported: &mut Vec<String>,
) {
    parts.lint_skip_rules = (parts.lint_rules.iter())
        .filter(|(_, v)| v != "\"off\"")
        .map(|(k, _)| k.clone())
        .collect();
    if !parts.lint_skip_rules.is_empty() {
        parts.lint_skip_files = globs;
        unsupported.extend(notes);
    }
}

/// PHP-CS-Fixer's deprecated proxy fixers run only their successor: with the proxy's own configuration, or with
/// the fixed one the proxy gives it (its `createProxyFixers()`). Beside the successor (from a set such as @PSR12)
/// both run, in name order at their equal priority (`Utils::sortFixers`): the later name's options win. A proxy
/// run within the successor's default run adds nothing; any other second run is one a pun.toml rule can't hold,
/// so the proxy stays and is named as skipped.
fn resolve_proxies(rules: &mut Vec<Rule>) {
    for (old, new, fixed, within_defaults) in PROXIES {
        let Some(i) = rules.iter().position(|r| r.name == *old) else {
            continue;
        };
        if let Some(j) = rules.iter().position(|r| r.name == *new) {
            match rules[i].options.clone() {
                Value::Object(own) if fixed.is_none() => {
                    let mut merged = rules[j].options.as_object().cloned().unwrap_or_default();
                    if old > new {
                        merged.extend(own);
                    } else {
                        let mut first = own;
                        first.extend(merged);
                        merged = first;
                    }
                    rules[j].options = Value::Object(merged);
                    rules.remove(i);
                }
                Value::Null if *within_defaults && rules[j].options.is_null() => {
                    rules.remove(i);
                }
                _ => {}
            }
            continue;
        }
        let proxy = rules.remove(i);
        let options = fixed.map_or(proxy.options, |f| serde_json::from_str(f).unwrap());
        rules.insert(
            i,
            Rule {
                name: (*new).into(),
                options,
            },
        );
    }
}

/// PHP-CS-Fixer's deprecated fixers that only run a successor: (name, successor, the successor's fixed configuration
/// when the proxy sets one instead of passing its own through, whether that run is within the successor's default
/// one). The successors' defaults: `no_trailing_comma_in_singleline` all four elements, `type_declaration_spaces`
/// function and property, `blank_lines_before_namespace` 2 and 2.
const PROXIES: &[(&str, &str, Option<&str>, bool)] = &[
    (
        "compact_nullable_typehint",
        "compact_nullable_type_declaration",
        None,
        true,
    ),
    ("curly_braces_position", "braces_position", None, true),
    (
        "function_typehint_space",
        "type_declaration_spaces",
        Some(r#"{"elements":["function"]}"#),
        true,
    ),
    (
        "native_function_type_declaration_casing",
        "native_type_declaration_casing",
        None,
        true,
    ),
    ("new_with_braces", "new_with_parentheses", None, true),
    (
        "no_blank_lines_before_namespace",
        "blank_lines_before_namespace",
        Some(r#"{"min_line_breaks":0,"max_line_breaks":1}"#),
        false,
    ),
    (
        "no_spaces_inside_parenthesis",
        "spaces_inside_parentheses",
        None,
        true,
    ),
    (
        "no_trailing_comma_in_list_call",
        "no_trailing_comma_in_singleline",
        Some(r#"{"elements":["array_destructuring"]}"#),
        true,
    ),
    (
        "no_trailing_comma_in_singleline_array",
        "no_trailing_comma_in_singleline",
        Some(r#"{"elements":["array"]}"#),
        true,
    ),
    (
        "no_trailing_comma_in_singleline_function_call",
        "no_trailing_comma_in_singleline",
        Some(r#"{"elements":["arguments","array_destructuring"]}"#),
        true,
    ),
    ("no_unneeded_curly_braces", "no_unneeded_braces", None, true),
    (
        "single_blank_line_before_namespace",
        "blank_lines_before_namespace",
        Some(r#"{"min_line_breaks":2,"max_line_breaks":2}"#),
        true,
    ),
];

fn toml_value(v: &Value) -> Option<String> {
    serde_json::from_value::<toml::Value>(v.clone())
        .ok()
        .map(|t| t.to_string())
}
