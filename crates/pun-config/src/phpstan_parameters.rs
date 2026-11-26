use crate::composer::Version;
use crate::{Analyse, mapping};
use pun_phpstan::php::normalize_path;
use pun_phpstan::{Array, Key, Value};
use std::collections::HashSet;
use std::path::Path;

/// The parameters phpstan-strict-rules' rules.neon sets (2.1.1), applied when one of its checks is on. The last
/// one is `%featureToggles.bleedingEdge%` there.
const STRICT_RULES_PARAMETERS: [(&str, bool); 12] = [
    ("polluteScopeWithLoopInitialAssignments", false),
    ("polluteScopeWithAlwaysIterableForeach", false),
    ("polluteScopeWithBlock", false),
    ("checkDynamicProperties", true),
    ("checkExplicitMixedMissingReturn", true),
    ("checkFunctionNameCase", true),
    ("checkInternalClassCaseSensitivity", true),
    ("reportMaybesInMethodSignatures", true),
    ("reportStaticMethodSignatures", true),
    ("reportMaybesInPropertyPhpDocTypes", true),
    ("reportWrongPhpDocTypeInVarTag", true),
    ("checkStrictPrintfPlaceholderTypes", true),
];

/// The analyse rule phpat's rules report under.
pub(crate) const PHPAT_RULE: &str = "analyse/phpat/architecture-violation";

/// The bootstrap file nesbot/carbon's `extension.neon` names, from the project root.
const CARBON_BOOTSTRAP: &str = "vendor/nesbot/carbon/lazy/Carbon/UnprotectedDatePeriod.php";

/// The PHPStan level at which each `[analyse]` type switch is on.
pub(crate) const SWITCH_LEVELS: [(&str, u8); 4] = [
    ("union-types", 7),
    ("nullables", 8),
    ("explicit-mixed", 9),
    ("implicit-mixed", 10),
];

pub(crate) fn larastan_supports(laravel: Option<Version>) -> bool {
    laravel.is_none_or(|v| match v.major {
        11 => v.minor >= 44 && (v.minor > 44 || v.patch >= 2),
        12 => v.minor > 4 || (v.minor == 4 && v.patch >= 1),
        13 => true,
        _ => false,
    })
}

impl Analyse {
    fn on(&self) -> HashSet<&str> {
        self.rules.iter().map(|r| r.id.as_str()).collect()
    }

    pub fn level(&self) -> u8 {
        let on = self.on();
        let classes = mapping::section(&["upstream", "phpstan-classes"]).table();
        let rules = (classes.values())
            .filter(|e| e.get("package").and_then(toml::Value::as_str) == Some("phpstan"))
            .filter(|e| {
                (e.get("pun").and_then(toml::Value::as_str)).is_some_and(|p| on.contains(p))
            })
            .filter_map(|e| e.get("level")?.as_integer());
        let switches = (SWITCH_LEVELS.iter())
            .filter(|(s, _)| self.switches.contains(s))
            .map(|(_, l)| i64::from(*l));
        u8::try_from(rules.chain(switches).max().unwrap_or(0)).expect("PHPStan levels are 0 to 10")
    }

    /// `stubFiles`, `scanFiles`, `scanDirectories` and `bootstrapFiles`, as absolute paths: a `scan` entry that is a
    /// directory is a scanned directory, any other a scanned file.
    fn file_parameters(&self, root: &Path, p: &mut Array) {
        let (dirs, files): (Vec<_>, Vec<_>) = (self.scan.iter())
            .map(|s| absolute(root, s))
            .partition(|s| Path::new(s).is_dir());
        let absolute = |paths: &[String]| paths.iter().map(|s| absolute(root, s)).collect();
        // Carbon's extension.neon adds its bootstrap file, which declares `Carbon\DatePeriodBase`
        let carbon = (self.ecosystems.contains(&&crate::extensions::CARBON))
            .then(|| CARBON_BOOTSTRAP.to_string());
        let bootstrap = absolute(&[&self.bootstrap[..], carbon.as_slice()].concat());
        for (name, paths) in [
            ("stubFiles", absolute(&self.stubs)),
            ("scanFiles", files),
            ("scanDirectories", dirs),
            ("bootstrapFiles", bootstrap),
        ] {
            let paths = paths.into_iter().map(Value::str);
            p.set(Key::from(name), Value::Array(Array::list(paths)));
        }
    }

    /// phpat's `parameters.phpat`, with the test classes its services name, when its rule is on.
    fn phpat_parameter(&self, on: &HashSet<&str>, p: &mut Array) {
        if let Some(phpat) = &self.phpat
            && on.contains(PHPAT_RULE)
        {
            let mut a = Array::new();
            let tests = phpat.tests.iter().map(|t| Value::str(t.as_str()));
            a.set(Key::from("tests"), Value::Array(Array::list(tests)));
            for (name, value) in [
                ("ignore_doc_comments", phpat.ignore_doc_comments),
                ("ignore_built_in_classes", phpat.ignore_built_in_classes),
                ("show_rule_names", phpat.show_rule_names),
            ] {
                a.set(Key::from(name), Value::Bool(value));
            }
            p.set(Key::from("phpat"), Value::Array(a));
        }
    }

    pub fn parameters(&self, root: &Path) -> Array {
        self.parameters_with_lint(root, |_| false)
    }

    /// [`Self::parameters`], with `lint_on` saying which lint rules are on: a strict-rules class a lint rule owns
    /// (rules.md O5, `UselessCastRule`) runs in analyse when strict-rules is in (one of its checks is on) and the lint
    /// rule is on.
    pub fn parameters_with_lint(&self, root: &Path, lint_on: impl Fn(&str) -> bool) -> Array {
        let on = self.on();
        let mut p = Array::new();
        let level = self.level();
        p.set(Key::from("level"), Value::Int(i64::from(level)));
        for (name, value) in [
            ("checkFunctionArgumentTypes", level >= 5),
            ("checkArgumentsPassedByReference", level >= 5),
            ("checkThisOnly", level < 2),
        ] {
            p.set(Key::from(name), Value::Bool(value));
        }
        self.phpat_parameter(&on, &mut p);
        p.set(
            Key::from("cliArgumentsVariablesRegistered"),
            Value::Bool(true),
        );
        p.set(
            Key::from("checkMaybeUndefinedVariables"),
            Value::Bool(level >= 1),
        );
        p.set(
            Key::from("checkClassCaseSensitivity"),
            Value::Bool(level >= 2),
        );
        p.set(
            Key::from("reportUnmatchedIgnoredErrors"),
            Value::Bool(self.report_unmatched),
        );
        self.file_parameters(root, &mut p);
        let ignores = self.ignore.iter().map(|i| Value::Array(ignore(root, i)));
        p.set(
            Key::from("ignoreErrors"),
            Value::Array(Array::list(ignores)),
        );
        let classes = mapping::section(&["upstream", "phpstan-classes"]).table();
        let installed = |name: &str| {
            classes.values().any(|e| {
                package(e) == Some(name)
                    && (e.get("pun").and_then(toml::Value::as_str)).is_some_and(|p| on.contains(p))
            })
        };
        let strict_rules = installed("phpstan-strict-rules");
        p.set(Key::from("strictRulesInstalled"), Value::Bool(strict_rules));
        if strict_rules {
            strict_rules_parameters(&mut p, self.bleeding_edge);
        }
        for e in classes.values() {
            let (Some(gate), Some(pun)) = (
                e.get("gated_by").and_then(toml::Value::as_str),
                e.get("pun").and_then(toml::Value::as_str),
            ) else {
                continue;
            };
            if package(e) == Some("larastan") && !self.larastan {
                continue;
            }
            for part in gate.split(" + ") {
                let (name, note) = part.split_once(" (").unwrap_or((part, ""));
                let name = match (name, note.strip_prefix("featureToggles.")) {
                    ("bleedingEdge", Some(t)) => {
                        format!("featureToggles.{}", t.trim_end_matches(')'))
                    }
                    ("bleedingEdge", None) => "featureToggles.bleedingEdge".into(),
                    _ => name.to_string(),
                };
                if get(&p, &name) != Some(&Value::Bool(true)) {
                    let lint_owned = strict_rules
                        && package(e) == Some("phpstan-strict-rules")
                        && pun.starts_with("lint/")
                        && lint_on(pun);
                    set(&mut p, &name, Value::Bool(on.contains(pun) || lint_owned));
                }
            }
        }
        if self.bleeding_edge {
            set(
                &mut p,
                "featureToggles.unresolvedTemplateArguments",
                Value::Bool(true),
            );
        }
        p.set(
            Key::from("deprecationRulesInstalled"),
            Value::Bool(installed("phpstan-deprecation-rules")),
        );
        for (name, e) in mapping::section(&["phpstan-parameters"]).table() {
            let (Some(option), Some(rules)) = (
                e.get("option").and_then(toml::Value::as_str),
                e.get("rules").and_then(toml::Value::as_array),
            ) else {
                continue;
            };
            let value = (self.rules.iter())
                .filter(|r| rules.iter().any(|x| x.as_str() == Some(r.id.as_str())))
                .find_map(|r| r.options.get(option));
            if let Some(v) = value {
                set(&mut p, name, json(v));
            }
        }
        p
    }
}

fn strict_rules_parameters(p: &mut Array, bleeding_edge: bool) {
    for (name, value) in STRICT_RULES_PARAMETERS {
        p.set(Key::from(name), Value::Bool(value));
    }
    if bleeding_edge {
        p.set(Key::from("reportNonIntStringArrayKey"), Value::Bool(true));
    }
}

fn package(e: &toml::Value) -> Option<&str> {
    e.get("package").and_then(toml::Value::as_str)
}

fn absolute(root: &Path, path: &str) -> String {
    if path.starts_with('*') {
        path.to_string()
    } else {
        normalize_path(&root.join(path).to_string_lossy())
    }
}

fn ignore(root: &Path, i: &crate::Ignore) -> Array {
    let mut e = Array::new();
    let mut put = |k: &str, v: Option<&String>| {
        if let Some(v) = v {
            e.set(Key::from(k), Value::str(v.as_str()));
        }
    };
    put("message", i.message.as_ref());
    put("rawMessage", i.raw_message.as_ref());
    put("identifier", i.identifier.as_ref());
    put("rule", i.rule.as_ref());
    let paths: Vec<String> = i.paths.iter().map(|p| absolute(root, p)).collect();
    match paths.as_slice() {
        [] => {}
        [one] => e.set(Key::from("path"), Value::str(one.as_str())),
        many => e.set(
            Key::from("paths"),
            Value::Array(Array::list(many.iter().map(|p| Value::str(p.as_str())))),
        ),
    }
    if let Some(c) = i.count {
        e.set(Key::from("count"), Value::Int(i64::from(c)));
    }
    if let Some(r) = i.report_unmatched {
        e.set(Key::from("reportUnmatched"), Value::Bool(r));
    }
    e
}

/// The parameters (Larastan's) that take a list of strings; every other one a rule option sets is a bool.
const LIST_PARAMETERS: [&str; 7] = [
    "configDirectories",
    "viewDirectories",
    "translationDirectories",
    "databaseMigrationsPath",
    "squashedMigrationsPath",
    "noUnnecessaryCollectionCallOnly",
    "noUnnecessaryCollectionCallExcept",
];

/// Checks the option table of analyse rule `id`: each key is the option of a PHPStan parameter the rule reads
/// (`[phpstan-parameters]`), and its value is what the parameter takes. The error follows the rule's name.
pub(crate) fn check_options(
    id: &str,
    t: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let options: Vec<(&str, &str)> = (mapping::section(&["phpstan-parameters"]).table().iter())
        .filter(|(_, e)| {
            (e.get("rules").and_then(toml::Value::as_array))
                .is_some_and(|r| r.iter().any(|x| x.as_str() == Some(id)))
        })
        .filter_map(|(name, e)| Some((e.get("option")?.as_str()?, name.as_str())))
        .collect();
    for (k, v) in t {
        let Some((_, name)) = options.iter().find(|(o, _)| o == k) else {
            return Err(match &options[..] {
                [] => " has no options: it takes \"on\" or \"off\"".to_string(),
                o => format!(
                    " has no option `{k}`; its options are {}",
                    (o.iter().map(|(o, _)| format!("`{o}`")))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        };
        let (ok, expected) = if LIST_PARAMETERS.contains(name) {
            let strings =
                (v.as_array()).is_some_and(|a| a.iter().all(serde_json::Value::is_string));
            (strings, "a list of strings")
        } else {
            (v.is_boolean(), "true or false")
        };
        if !ok {
            return Err(format!(": {k} = {v}: expected {expected}"));
        }
    }
    Ok(())
}

fn json(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::String(s) => Value::str(s.as_str()),
        serde_json::Value::Array(a) => Value::Array(Array::list(a.iter().map(json))),
        v => v.as_bool().map_or(Value::Null, Value::Bool),
    }
}

fn get<'a>(p: &'a Array, dotted: &str) -> Option<&'a Value> {
    let mut parts = dotted.split('.');
    let mut v = p.get_str(parts.next()?)?;
    for k in parts {
        v = v.as_array()?.get_str(k)?;
    }
    Some(v)
}

fn set(p: &mut Array, dotted: &str, value: Value) {
    match dotted.split_once('.') {
        None => p.set(Key::from(dotted), value),
        Some((head, rest)) => {
            let mut inner = match p.get_str(head) {
                Some(Value::Array(a)) => a.clone(),
                _ => Array::new(),
            };
            set(&mut inner, rest, value);
            p.set(Key::from(head), Value::Array(inner));
        }
    }
}
