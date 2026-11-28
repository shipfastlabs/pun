use super::*;
use serde_json::json;
use std::fmt::Write as _;

fn dir(files: &[(&str, &str)]) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("pun-config-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    for (n, c) in files {
        std::fs::write(d.join(n), c).unwrap();
    }
    d
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "a test helper called with temporaries"
)]
fn err(d: &Path, o: Options) -> String {
    format!("{:#}", load(d, &o).unwrap_err())
}

fn names(rules: &[Rule]) -> Vec<&str> {
    rules.iter().map(|r| r.name.as_str()).collect()
}

fn preset(p: &str) -> Options {
    Options {
        preset: Some(p.into()),
        ..Default::default()
    }
}

#[test]
fn needs_pun_toml() {
    let d = dir(&[("pint.json", r#"{"preset":"psr12"}"#)]);
    assert_eq!(
        err(&d, Options::default()),
        "No pun.toml found. Run `pun migrate`, or `pun init` for a new project."
    );
    assert_eq!(load(&d, &preset("psr12")).unwrap().preset, "psr12");
    let c = load(
        &d,
        &Options {
            no_config: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!((&*c.preset, c.sources.len()), ("laravel", 0));
    let o = Options {
        config: Some("other.toml".into()),
        ..Default::default()
    };
    assert!(err(&d, o.clone()).starts_with("can't read "));
    std::fs::write(d.join("other.toml"), "[format]\npreset = \"empty\"\n").unwrap();
    let c = load(&d, &o).unwrap();
    assert_eq!(
        (&*c.preset, c.sources),
        ("empty", vec![d.join("other.toml")])
    );
    assert_eq!(err(&d, preset("nope")), "Preset not found.");
}

fn lr(id: &str, level: Level, fix: bool, options: Value, fixer: Option<&'static str>) -> LintRule {
    LintRule {
        id: id.into(),
        level,
        fix,
        options,
        fixer,
        named: true,
    }
}

fn ids(rules: &[LintRule]) -> Vec<&str> {
    rules.iter().map(|r| r.id.as_str()).collect()
}

const LARAVEL_LOCK: &str = r#"{"packages":[{"name":"laravel/framework","version":"v11.9.2"}]}"#;

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one test walks every case of the behaviour it checks"
)]
fn pun_toml() {
    use Level::{Error, Warn};
    let d = dir(&[
        ("composer.json", r#"{"require":{"php":"^8.4"}}"#),
        (
            "pun.toml",
            r#"[php]
version = "8.5"
[files]
paths = ["src", "tests"]
exclude = ["build"]
[format]
preset = "laravel"
exclude = ["a"]
not-path = ["p"]
[format.rules]
"operators/concat-spacing" = { spacing = "one" }
"format/arrays/syntax" = false
"laravel/blade" = true
[lint.rules]
"style/no-alias-function" = "error"
"lint/style/require-final-class" = "warn"
"statements/require-strict-types" = "off"
"functions/no-unreachable-default-argument-value" = { level = "warn", fix = "off" }
"types/no-loose-comparison" = "error"
"style/require-ordered-traits" = { fix = "off" }
"style/prefer-cast-over-intval" = { level = "error" }
"imports/no-unused" = "error"
"phpdoc/no-superfluous-tags" = { allow-mixed = true }
"pest/no-only" = { level = "off", fix = "off" }
"#,
        ),
    ]);
    let c = load(&d, &Options::default()).unwrap();
    let n = names(&c.rules);
    assert!(!n.contains(&"array_syntax"));
    let concat = n.iter().position(|n| *n == "concat_space").unwrap();
    assert!(concat < n.iter().position(|n| *n == "Pint/laravel_blade").unwrap());
    assert_eq!(c.rules[concat].options, json!({"spacing": "one"}));
    for risky in [
        "no_alias_functions",
        "no_unreachable_default_argument_value",
        "ordered_traits",
        "php_unit_set_up_tear_down_visibility",
        "no_unused_imports",
        "no_useless_return",
        "no_empty_statement",
        "lambda_not_used_import",
        "no_superfluous_phpdoc_tags",
        "general_phpdoc_tag_rename",
        "phpdoc_no_access",
        "phpdoc_no_package",
        "phpdoc_no_useless_inheritdoc",
        "self_static_accessor",
        "php_unit_method_casing",
    ] {
        assert!(!n.contains(&risky), "{risky}");
    }
    assert!(c.blade);
    assert_eq!(
        c.lint.rules,
        [
            lr(
                "lint/functions/no-unreachable-default-argument-value",
                Warn,
                false,
                Value::Null,
                Some("no_unreachable_default_argument_value")
            ),
            lr(
                "lint/imports/no-unused",
                Error,
                true,
                Value::Null,
                Some("no_unused_imports")
            ),
            lr(
                "lint/phpdoc/no-superfluous-tags",
                Error,
                true,
                json!({"allow_mixed": true}),
                Some("no_superfluous_phpdoc_tags")
            ),
            lr(
                "lint/style/no-alias-function",
                Error,
                true,
                Value::Null,
                Some("no_alias_functions")
            ),
            lr(
                "lint/style/prefer-cast-over-intval",
                Error,
                true,
                Value::Null,
                Some("modernize_types_casting")
            ),
            lr(
                "lint/style/require-final-class",
                Warn,
                true,
                Value::Null,
                Some("final_class")
            ),
            lr(
                "lint/style/require-ordered-traits",
                Error,
                false,
                Value::Null,
                Some("ordered_traits")
            ),
            lr(
                "lint/types/no-loose-comparison",
                Error,
                true,
                Value::Null,
                Some("strict_comparison")
            ),
        ]
    );
    assert_eq!(
        c.files,
        Files {
            paths: vec!["src".into(), "tests".into()],
            exclude: vec!["build".into()]
        }
    );
    assert_eq!(
        (c.finder.in_dirs, c.finder.exclude, c.finder.not_path),
        (
            vec!["src".into(), "tests".into()],
            vec!["build".into(), "a".into()],
            vec!["p".into()]
        )
    );
    assert_eq!(c.lint.exclude, ["build"]);
    assert_eq!(c.php.to_string(), "8.5");
    assert_eq!(c.sources, [d.join("composer.json"), d.join("pun.toml")]);
    let d = dir(&[(
        "pun.toml",
        "[format]\npreset = \"empty\"\n[format.rules]\n\"imports/ordered\" = { sort = \"alphabetical\", order = [\"class\", \"const\"] }\n\"laravel/blade\" = true\n",
    )]);
    let c = load(&d, &Options::default()).unwrap();
    assert_eq!(names(&c.rules), ["ordered_imports", "Pint/laravel_blade"]);
    assert_eq!(
        c.rules[0].options,
        json!({"sort_algorithm": "alpha", "imports_order": ["class", "const"]})
    );
    assert!(c.blade);
    assert_eq!(c.php.to_string(), "8.3");
    assert!(c.lint.rules.is_empty() && c.files == Files::default());
}

#[test]
fn indent_and_line_ending() {
    let c = from_toml(
        &dir(&[]),
        "[format]\nnot-name = [\"*.inc\"]\n\n[format.rules]\n\"whitespace/indentation-type\" = \"tabs\"\n\"whitespace/line-ending\" = { ending = \"crlf\" }\n",
    )
    .unwrap();
    let options = |n: &str| &c.rules.iter().find(|r| r.name == n).unwrap().options;
    assert_eq!(options("indentation_type"), &json!({"indent": "\t"}));
    assert_eq!(options("line_ending"), &json!({"line_ending": "\r\n"}));
    assert_eq!(c.finder.not_name, ["*.inc"]);
    let e = from_toml(
        &dir(&[]),
        "[format.rules]\n\"whitespace/indentation-type\" = \"2\"\n",
    )
    .unwrap_err();
    assert!(
        format!("{e:#}").contains("expected one of \"spaces\", \"tabs\""),
        "{e:#}"
    );
}

#[test]
fn php_version() {
    let c = |files: &[(&str, &str)]| {
        load(&dir(files), &Options::default())
            .unwrap()
            .php
            .to_string()
    };
    let toml = ("pun.toml", "");
    assert_eq!(
        c(&[toml, ("composer.json", r#"{"require":{"php":"^8.5"}}"#)]),
        "8.5"
    );
    assert_eq!(
        c(&[toml, ("composer.json", r#"{"require":{"php":">=8.4.3"}}"#)]),
        "8.4"
    );
    assert_eq!(c(&[toml, ("composer.json", "{}")]), "8.3");
    assert_eq!(
        c(&[toml, ("composer.json", r#"{"require":{"php":"^8.3"}}"#)]),
        "8.3"
    );
    assert_eq!(
        c(&[
            ("pun.toml", "[php]\nversion = \"8.6\"\n"),
            ("composer.json", r#"{"require":{"php":"^8.4"}}"#)
        ]),
        "8.6"
    );
}

#[test]
fn new_expression_parentheses_needs_php_8_4() {
    let has = |version: &str, options: &str| {
        let toml = format!(
            "[php]\nversion = \"{version}\"\n[format]\npreset = \"empty\"\n[format.rules]\n\"operators/new-expression-parentheses\" = {options}\n"
        );
        let c = load(&dir(&[("pun.toml", &toml)]), &Options::default()).unwrap();
        names(&c.rules).contains(&"new_expression_parentheses")
    };
    assert!(has("8.4", "true"));
    assert!(!has("8.3", "true"));
    assert!(!has("8.3", "{ use-parentheses = false }"));
    assert!(has("8.3", "{ use-parentheses = true }"));
}

#[test]
fn analyse_php_version() {
    let id = |files: &[(&str, &str)]| {
        load(&dir(files), &Options::default())
            .unwrap()
            .analyse_php_version
            .id()
    };
    let toml = ("pun.toml", "");
    assert_eq!(
        id(&[toml, ("composer.json", r#"{"require":{"php":"^8.4"}}"#)]),
        80500
    );
    assert_eq!(
        id(&[
            toml,
            (
                "composer.json",
                r#"{"require":{"php":"^8.4"},"config":{"platform":{"php":"8.3.12"}}}"#
            )
        ]),
        80312
    );
    assert_eq!(
        id(&[
            toml,
            ("composer.json", r#"{"config":{"platform":{"php":"9.1"}}}"#)
        ]),
        80699
    );
    assert_eq!(
        id(&[
            ("pun.toml", "[php]\nversion = \"8.4\"\n"),
            (
                "composer.json",
                r#"{"config":{"platform":{"php":"8.3.12"}}}"#
            )
        ]),
        80400
    );
}

#[test]
fn top_level_preset() {
    let load_with = |files: &[(&str, &str)]| load(&dir(files), &Options::default()).unwrap();
    let strict = load_with(&[("pun.toml", "preset = \"strict\"\n")]);
    assert_eq!(strict.lint.preset, Some(LintPreset::Strict));
    assert_eq!(strict.analyse.preset.to_string(), "strict");
    assert_eq!(strict.preset, FormatPreset::Psr12);
    // The Laravel extension makes the format preset `laravel`.
    let laravel = "preset = \"essential\"\n[extensions]\nlaravel = \"on\"\n";
    let lara = load_with(&[("pun.toml", laravel), ("composer.lock", LARAVEL_LOCK)]);
    assert_eq!(lara.preset, FormatPreset::Laravel);
    assert_eq!(lara.lint.preset, Some(LintPreset::Essential));
    // Each command's own key wins.
    let own = "preset = \"strict\"\n[format]\npreset = \"laravel\"\n[lint]\npreset = \"essential\"\n[analyse]\npreset = \"max\"\n";
    let own_keys = load_with(&[("pun.toml", own)]);
    assert_eq!(own_keys.preset, FormatPreset::Laravel);
    assert_eq!(own_keys.lint.preset, Some(LintPreset::Essential));
    assert_eq!(own_keys.analyse.preset.to_string(), "max");
    let err = load(
        &dir(&[("pun.toml", "preset = \"max\"\n")]),
        &Options::default(),
    );
    let err = format!("{:#}", err.err().unwrap());
    assert!(
        err.contains("preset = \"max\": expected `essential`, `recommended` or `strict`"),
        "{err}"
    );
}

#[test]
fn lint_presets() {
    let lint = |files: &[(&str, &str)]| load(&dir(files), &Options::default()).unwrap().lint;
    let has = |l: &Lint, id: &str| ids(&l.rules).contains(&id);
    let essential = lint(&[("pun.toml", "[lint]\npreset = \"essential\"\n")]);
    assert_eq!(essential.preset, Some(LintPreset::Essential));
    assert!(has(&essential, "lint/dead-code/no-unused-assignment"));
    assert!(!has(&essential, "lint/style/prefer-early-return"));
    // Each preset includes the ones below it.
    let recommended = lint(&[("pun.toml", "[lint]\npreset = \"recommended\"\n")]);
    assert!(ids(&essential.rules).iter().all(|id| has(&recommended, id)));
    assert!(has(&recommended, "lint/style/prefer-early-return"));
    assert!(has(&recommended, "lint/php-8.3/prefer-json-validate"));
    assert!(!has(&recommended, "lint/types/add-return-type-from-cast"));
    let strict = lint(&[("pun.toml", "[lint]\npreset = \"strict\"\n")]);
    assert!(ids(&recommended.rules).iter().all(|id| has(&strict, id)));
    assert!(has(&strict, "lint/types/add-return-type-from-cast"));
    // Rules a preset holds still wait for their PHP version and extension.
    assert!(!has(&strict, "lint/php-8.4/prefer-array-find"));
    assert!(!has(&strict, "lint/laravel/add-relation-generics"));
    let laravel = "[extensions]\nlaravel = \"on\"\n[lint]\npreset = \"strict\"\n";
    let l = lint(&[("pun.toml", laravel), ("composer.lock", LARAVEL_LOCK)]);
    assert!(has(&l, "lint/laravel/add-relation-generics"));
    assert!(!has(&l, "lint/laravel/no-controller-middleware"));
    // No preset and no sets: nothing runs, even with an extension on.
    let l = lint(&[
        ("pun.toml", "[extensions]\nlaravel = \"on\"\n"),
        ("composer.lock", LARAVEL_LOCK),
    ]);
    assert!(l.rules.is_empty());
}

#[test]
fn rector_sets() {
    let lint = |toml: &str| load(&dir(&[("pun.toml", toml)]), &Options::default()).map(|c| c.lint);
    let has = |l: &Lint, id: &str| ids(&l.rules).contains(&id);
    // Rector 2.8.4's dead-code set: 72 rules, 71 of them lint rules in Pun (ReplaceBlockToItsStmts is
    // format/control-flow/no-unneeded-braces), merged into 65 Pun rules.
    let l = lint("[lint]\nsets = [\"rector/dead-code\"]\n").unwrap();
    assert_eq!(l.rules.len(), 65);
    assert!(!has(&l, "lint/style/prefer-early-return"));
    // Of no-useless-ternary's and prefer-simple-condition's Rector rules, Rector 2.8.5's dead-code set runs only
    // RemoveUselessTernaryRector and SimplifyBoolIdenticalTrueRector (crates/pun-cli/src/rector_config.php's output,
    // as in tests/rector.rs), so each runs those checks only.
    let options = |l: &Lint, id: &str| l.rules.iter().find(|r| r.id == id).unwrap().options.clone();
    assert_eq!(
        options(&l, "lint/dead-code/no-useless-ternary"),
        json!({"checks": ["useless"]})
    );
    assert_eq!(
        options(&l, "lint/complexity/prefer-simple-condition"),
        json!({"checks": ["identical-true"]})
    );
    // Through a preset as well, the rule runs whole.
    let l = lint("[lint]\npreset = \"essential\"\nsets = [\"rector/dead-code\"]\n").unwrap();
    assert_eq!(
        options(&l, "lint/dead-code/no-useless-ternary"),
        Value::Null
    );
    // `rector/php-sets` is `withPhpSets()`: every PHP set up to `[php] version`.
    let php = |v: &str| {
        lint(&format!(
            "[php]\nversion = \"{v}\"\n[lint]\nsets = [\"rector/php-sets\"]\n"
        ))
        .unwrap()
    };
    let l = php("8.4");
    assert!(has(&l, "lint/php-7.4/prefer-arrow-function"));
    assert!(has(&l, "lint/php-8.4/prefer-array-find"));
    assert!(!has(&l, "lint/php-8.5/no-null-array-key"));
    assert!(!has(&php("8.3"), "lint/php-8.4/prefer-array-find"));
    assert!(has(&php("8.5"), "lint/php-8.5/no-null-array-key"));
    // The sets before 2026-11-24 name their Rector set.
    let err = |toml: &str| lint(toml).unwrap_err().to_string();
    assert!(
        err("[lint]\nsets = [\"types\"]\n").ends_with(
            "[lint] sets: `types` is now `rector/type-declaration`, Rector's set; for Pun's own picks use [lint] preset = \"essential\" | \"recommended\" | \"strict\""
        )
    );
    assert!(
        err("[lint]\nsets = [\"naming\"]\n")
            .contains("`naming` is gone: Rector's set has no rules")
    );
    assert!(err("[lint]\npreset = \"max\"\n").ends_with(
        "[lint] preset: `max` is not a lint preset; use \"essential\" | \"recommended\" | \"strict\""
    ));
}

#[test]
fn lint_groups() {
    let lint = |toml: &str| {
        load(&dir(&[("pun.toml", toml)]), &Options::default())
            .unwrap()
            .lint
    };
    let level = |l: &Lint, id: &str| l.rules.iter().find(|r| r.id == id).map(|r| r.level);
    // rector/privatization: three classes/ rules and style/prefer-property-over-local-getter.
    let set = "[lint]\nsets = [\"rector/privatization\"]\n";
    let l = lint(&format!("{set}[lint.groups]\nstyle = \"warn\"\n"));
    // A group's level turns on every rule of the group: the 91 style rules, plus the set's three others.
    assert_eq!(l.rules.len(), 91 + 3);
    assert!(
        l.rules
            .iter()
            .all(|r| r.level == Level::Warn || r.id.starts_with("lint/classes/no-protected-"))
    );
    assert_eq!(
        level(&l, "lint/classes/no-protected-method-in-final"),
        Some(Level::Error)
    );
    let l = lint(&format!(
        "{set}[lint.groups]\nstyle = \"off\"\n[lint.rules]\n\"style/prefer-early-return\" = \"warn\"\n"
    ));
    assert_eq!(
        ids(&l.rules),
        [
            "lint/classes/no-protected-constant-in-final",
            "lint/classes/no-protected-method-in-final",
            "lint/classes/no-protected-property-in-final",
            "lint/style/prefer-early-return"
        ]
    );
    assert_eq!(
        level(&l, "lint/style/prefer-early-return"),
        Some(Level::Warn)
    );
    let l = lint(&format!(
        "{set}[lint.rules]\n\"classes/no-protected-method-in-final\" = \"off\"\n"
    ));
    assert_eq!(l.rules.len(), 3);
    let l =
        lint("[lint.rules]\n\"style/prefer-early-return\" = { level = \"warn\", max-depth = 2 }\n");
    assert_eq!(
        l.rules,
        [LintRule {
            id: "lint/style/prefer-early-return".into(),
            level: Level::Warn,
            fix: true,
            options: json!({"max-depth": 2}),
            fixer: None,
            named: true,
        }]
    );
    let l = lint("[lint.groups]\nlaravel-12 = \"error\"\n");
    assert_eq!(ids(&l.rules), ["lint/laravel-12/prefer-scope-attribute"]);
    assert!(l.rules.iter().all(|r| !r.named && r.level == Level::Error));
}

#[test]
fn upgrade_gates() {
    let gates = |lock: Option<&str>| {
        let d = dir(&lock
            .map(|l| ("composer.lock", l))
            .into_iter()
            .collect::<Vec<_>>());
        composer::detect(&d).unwrap()
    };
    let check = |composer: &composer::Composer, group: &str| {
        let g = schema::Gates {
            php: crate::PhpVersion::FLOOR,
            composer,
            switches: [("laravel", crate::extensions::Switch::On)].into(),
        };
        g.allows(group, &g.ecosystems(|_| false))
    };
    let l11 = gates(Some(LARAVEL_LOCK));
    assert!(check(&l11, "laravel-11") && check(&l11, "laravel-10") && check(&l11, "laravel-5.7"));
    assert!(!check(&l11, "laravel-12"));
    assert!(check(&l11, "laravel") && !check(&l11, "pest"));
    assert!(check(&l11, "php-8.3") && !check(&l11, "php-8.4") && check(&l11, "style"));
    let branch = gates(Some(
        r#"{"packages":[{"name":"laravel/framework","version":"dev-main"}]}"#,
    ));
    assert!(check(&branch, "laravel") && !check(&branch, "laravel-5.7"));
    assert!(!check(&gates(None), "laravel-5.7"));
}

#[test]
fn lint_overrides() {
    let c = load(
        &dir(&[(
            "pun.toml",
            "[[lint.overrides]]\nfiles = [\"tests/**\"]\nrules = { \"pest/no-only\" = \"off\", \"style/require-final-class\" = { level = \"warn\", fix = \"off\" } }\n\n[[lint.overrides]]\nfiles = [\"app/Legacy.php\", \"database/**\"]\n",
        )]),
        &Options::default(),
    )
    .unwrap();
    assert_eq!(
        c.lint.overrides,
        [
            Override {
                files: vec!["tests/**".into()],
                rules: vec![
                    LintSetting {
                        id: "lint/pest/no-only".into(),
                        level: None,
                        fix: true,
                        options: Value::Null
                    },
                    LintSetting {
                        id: "lint/style/require-final-class".into(),
                        level: Some(Level::Warn),
                        fix: false,
                        options: Value::Null
                    },
                ]
            },
            Override {
                files: vec!["app/Legacy.php".into(), "database/**".into()],
                rules: vec![]
            },
        ]
    );
    assert_eq!(c.lint.rules, [] as [schema::LintRule; 0]);
}

#[test]
fn analyse() {
    let an = |files: &[(&str, &str)]| load(&dir(files), &Options::default()).unwrap().analyse;
    let has = |a: &Analyse, id: &str| a.rules.iter().any(|r| r.id == id);
    let a = an(&[(
        "pun.toml",
        "[files]\npaths = [\"app\"]\nexclude = [\"storage\"]\n",
    )]);
    assert_eq!(
        a,
        Analyse {
            preset: crate::AnalysePreset::Recommended,
            paths: vec!["app".into()],
            exclude: vec!["storage".into()],
            baseline: None,
            stubs: vec![],
            scan: vec![],
            bootstrap: vec![],
            ignore: vec![],
            switches: vec![],
            ecosystems: vec![],
            larastan: false,
            rules: a.rules.clone(),
            bleeding_edge: false,
            report_unmatched: true,
        }
    );
    assert_eq!(a.rules.len(), 151 + 106 + 75 - 11);
    assert!(has(&a, "analyse/undefined/method") && !has(&a, "analyse/types/missing-return-type"));
    let preset = |p: &str, lock: bool| {
        let on = if lock {
            "[extensions]\nlaravel = \"on\"\n"
        } else {
            ""
        };
        let toml = format!("{on}[analyse]\npreset = \"{p}\"\n");
        let mut files = vec![("pun.toml", toml.as_str())];
        if lock {
            files.push(("composer.lock", LARAVEL_LOCK));
        }
        let a = an(&files);
        (a.rules.len(), a.switches)
    };
    assert_eq!(preset("basic", false), (151, vec![]));
    assert_eq!(preset("essential", true), (151 + 106, vec![]));
    assert_eq!(preset("recommended", true), (151 + 106 + 75, vec![]));
    assert_eq!(
        preset("strict", true),
        (151 + 106 + 75 + 35, vec!["union-types", "nullables"])
    );
    assert_eq!(
        preset("max", false),
        (
            370 - 15,
            vec![
                "union-types",
                "nullables",
                "explicit-mixed",
                "implicit-mixed"
            ]
        )
    );
    let a = an(&[(
        "pun.toml",
        r#"[files]
paths = ["app"]
exclude = ["storage"]
[analyse]
preset = "basic"
paths = ["app", "routes"]
exclude = ["tests/**"]
baseline = "pun-baseline.toml"
[analyse.rules]
"types/missing-return-type" = "on"
"analyse/undefined/method" = { check-function-name-case = true }
"undefined/function" = "off"
"types/argument-not-numeric" = "on"
laravel = "on"
"laravel/no-model-make" = "off"
"#,
    )]);
    assert_eq!(
        (a.paths, a.exclude, a.baseline),
        (
            vec!["app".into(), "routes".into()],
            vec!["storage".into(), "tests/**".into()],
            Some("pun-baseline.toml".into())
        )
    );
    assert_eq!(a.rules.len(), 151 + 2 + 14 - 1);
    let rule = |id: &str| {
        a.rules
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.options.clone())
    };
    assert_eq!(
        rule("analyse/undefined/method"),
        Some(json!({"check-function-name-case": true}))
    );
    assert_eq!(rule("analyse/types/missing-return-type"), Some(Value::Null));
    assert_eq!(rule("analyse/undefined/function"), None);
    assert_eq!(rule("analyse/laravel/no-model-make"), None);
    let a = an(&[
        ("pun.toml", "[analyse.rules]\nlaravel = \"off\"\n"),
        ("composer.lock", LARAVEL_LOCK),
    ]);
    assert_eq!(
        (a.rules.len(), a.ecosystems.len()),
        (151 + 106 + 75 - 11, 0)
    );
}
/// `phpstan/level_<n>` runs what `phpstan analyse --level <n>` runs: PHPStan core's rules of that level and below
/// (no opt-in, strict-rules or deprecation-rules ones), the level's type switches and level-tied parameters, and the
/// PHPStan checks that are lint rules in Pun.
#[test]
fn phpstan_level_presets() {
    let cfg = |body: &str| {
        let toml = format!("[format]\npreset = \"empty\"\n\n{body}");
        load(&dir(&[("pun.toml", toml.as_str())]), &Options::default()).unwrap()
    };
    let at = |p: &str| cfg(&format!("[analyse]\npreset = \"{p}\"\n"));
    let ids = |c: &Config| -> std::collections::BTreeSet<String> {
        c.analyse.rules.iter().map(|r| r.id.clone()).collect()
    };
    let classes = crate::mapping::section(&["upstream", "phpstan-classes"]).table();
    let package_of = |id: &str| {
        (classes.values())
            .find(|e| e.get("pun").and_then(toml::Value::as_str) == Some(id))
            .and_then(|e| e.get("package")?.as_str())
    };
    let mut below = std::collections::BTreeSet::new();
    for level in 0..=10u8 {
        let c = at(&format!("phpstan/level_{level}"));
        assert_eq!(c.analyse.preset, AnalysePreset::PhpstanLevel(level));
        assert_eq!(c.analyse.level(), level);
        let want: Vec<&str> = [
            ("union-types", 7),
            ("nullables", 8),
            ("explicit-mixed", 9),
            ("implicit-mixed", 10),
        ]
        .into_iter()
        .filter(|(_, l)| level >= *l)
        .map(|(s, _)| s)
        .collect();
        assert_eq!(c.analyse.switches, want, "level {level}");
        let on = ids(&c);
        assert!(
            below.is_subset(&on),
            "level {level} drops rules of a lower level"
        );
        for id in &on {
            assert_eq!(package_of(id), Some("phpstan"), "{id} at level {level}");
        }
        below = on;
    }
    assert_eq!(ids(&at("phpstan/level_max")), below);
    assert_eq!(at("phpstan/level_max").analyse.level(), 10);
    let l0 = ids(&at("phpstan/level_0"));
    let l6 = ids(&at("phpstan/level_6"));
    assert!(
        l0.contains("analyse/undefined/method")
            && !l0.contains("analyse/types/missing-return-type")
    );
    assert!(l6.contains("analyse/types/missing-return-type"));
    // PHPStan's opt-ins stay off, though Pun's own presets turn some on.
    for id in [
        "analyse/classes/uninitialized-property",
        "analyse/types/throws-not-covariant",
        "analyse/types/missing-throws",
    ] {
        assert!(!below.contains(id), "{id}");
    }
    assert!(ids(&at("max")).contains("analyse/types/missing-throws"));
    // Bleeding edge adds its toggled rules, as PHPStan's bleedingEdge.neon does.
    let be = ids(&cfg(
        "[analyse]\npreset = \"phpstan/level_5\"\nbleeding-edge = true\n",
    ));
    assert!(be.contains("analyse/types/printf-argument"));
    assert!(!ids(&at("phpstan/level_5")).contains("analyse/types/printf-argument"));
    // Explicit switches and rules still win.
    let c = cfg(
        "[analyse]\npreset = \"phpstan/level_8\"\nnullables = false\n\n[analyse.rules]\n\"undefined/method\" = \"off\"\n",
    );
    assert_eq!(c.analyse.switches, ["union-types"]);
    assert!(!ids(&c).contains("analyse/undefined/method"));

    // PHPStan checks that are lint rules in Pun come with the preset.
    let lint = |c: &Config| -> Vec<String> { c.lint.rules.iter().map(|r| r.id.clone()).collect() };
    let l3 = lint(&at("phpstan/level_3"));
    let l4 = lint(&at("phpstan/level_4"));
    assert!(l3.contains(&"lint/dead-code/no-duplicate-array-key".to_string()));
    assert!(!l3.contains(&"lint/dead-code/no-unreachable-code".to_string()));
    assert!(
        l4.contains(&"lint/dead-code/no-unreachable-code".to_string()),
        "{l4:?}"
    );
    assert!(lint(&at("recommended")).is_empty());
    let c = cfg(
        "[lint.rules]\n\"dead-code/no-unreachable-code\" = \"off\"\n\n[analyse]\npreset = \"phpstan/level_4\"\n",
    );
    assert!(!lint(&c).contains(&"lint/dead-code/no-unreachable-code".to_string()));
}

#[test]
fn extensions() {
    let load_ = |files: &[(&str, &str)]| load(&dir(files), &Options::default());
    let types = "[lint]\npreset = \"strict\"\n";
    let with = |switch: &str| format!("{types}[extensions]\nlaravel = \"{switch}\"\n");
    let c = load_(&[("pun.toml", &with("off")), ("composer.lock", LARAVEL_LOCK)]).unwrap();
    let without = c.lint.rules.len();
    assert_eq!(c.lint.ecosystems.len(), 0);
    assert!(
        !c.lint
            .rules
            .iter()
            .any(|r| r.id.starts_with("lint/laravel/"))
    );
    assert!(c.extensions.is_empty() && c.analyse.ecosystems.is_empty() && !c.analyse.larastan);
    assert_eq!(c.show()["extensions"]["laravel"], "off");
    let c = load_(&[("pun.toml", &with("on"))]).unwrap();
    // On, the extension adds the preset's Laravel rules.
    assert_eq!(c.extensions, [&crate::extensions::LARAVEL]);
    let laravel = c
        .lint
        .rules
        .iter()
        .filter(|r| r.id.starts_with("lint/laravel/"))
        .count();
    assert!(laravel > 0);
    assert_eq!(c.lint.rules.len(), without + laravel);
    assert!(c.analyse.larastan && c.analyse.ecosystems == [&crate::extensions::LARAVEL]);
    let shown = c.show();
    assert_eq!(
        shown["extensions"],
        json!({
            "laravel": "on",
            "pest": "off",
            "phpunit": "off",
            "carbon": "off",
            "mockery": "off",
            "disallowed-calls": "off",
            "type-coverage": "off"
        })
    );
    // Left out of [extensions], an extension is off even with its package locked.
    let c = load_(&[("pun.toml", types), ("composer.lock", LARAVEL_LOCK)]).unwrap();
    assert!(c.extensions.is_empty() && !c.analyse.larastan);
    let pest = "[extensions]\npest = \"off\"\n[lint.groups]\npest = \"error\"\n";
    let c = load_(&[("pun.toml", pest)]).unwrap();
    assert!(c.lint.rules.iter().any(|r| r.id.starts_with("lint/pest/")));
    let e = |toml: &str| format!("{:#}", load_(&[("pun.toml", toml)]).unwrap_err());
    assert!(
        e("[extensions]\nsymfony = \"on\"\n").ends_with(
            "pun.toml: [extensions] `symfony` is not an extension: expected `laravel`, `pest`, `phpunit`, `carbon`, `mockery`, `disallowed-calls`, `type-coverage`"
        )
    );
    assert!(
        e("[extensions]\nlaravel = true\n")
            .ends_with("pun.toml: [extensions] `laravel` = true: expected \"on\" or \"off\"")
    );
    // `carbon` adds what Carbon's extension.neon adds: its bootstrap file. Off, or left out, it adds nothing.
    let bootstrap = |toml: &str| {
        let root = dir(&[("pun.toml", toml)]);
        let analyse = load(&root, &Options::default()).unwrap().analyse;
        let prefix = format!("{}/", root.display());
        let parameters = analyse.parameters(&root);
        let files = (parameters.get_str("bootstrapFiles"))
            .and_then(pun_phpstan::Value::as_array)
            .unwrap()
            .values()
            .filter_map(|v| Some(v.as_str()?.strip_prefix(&prefix)?.to_string()))
            .collect::<Vec<_>>();
        (analyse.ecosystems == [&crate::extensions::CARBON], files)
    };
    assert_eq!(
        bootstrap("[extensions]\ncarbon = \"on\"\n"),
        (
            true,
            vec!["vendor/nesbot/carbon/lazy/Carbon/UnprotectedDatePeriod.php".to_string()]
        )
    );
    assert_eq!(
        bootstrap("[extensions]\ncarbon = \"off\"\n"),
        (false, vec![])
    );
    assert_eq!(bootstrap(types), (false, vec![]));
    // `mockery` sets what phpstan-mockery's extension.neon sets; off, the parameter is absent.
    let mockery = |toml: &str| {
        let root = dir(&[("pun.toml", toml)]);
        let parameters = load(&root, &Options::default())
            .unwrap()
            .analyse
            .parameters(&root);
        (parameters
            .get_str("mockery")
            .and_then(pun_phpstan::Value::as_array))
        .and_then(|m| m.get_str("convertUnionToIntersectionType").cloned())
    };
    assert_eq!(
        mockery("[extensions]\nmockery = \"on\"\n"),
        Some(pun_phpstan::Value::Bool(true))
    );
    assert_eq!(mockery("[extensions]\nmockery = \"off\"\n"), None);
    assert!(e("[extensions]\nlaravel = \"auto\"\n").contains(
        "[extensions] `laravel` = \"auto\" was removed: an extension is on only when pun.toml says so"
    ));
}

#[test]
fn extension_groups() {
    let owned: Vec<&str> = (crate::extensions::ALL.iter())
        .flat_map(|e| e.groups.iter().copied())
        .collect();
    let unique: std::collections::BTreeSet<_> = owned.iter().collect();
    assert_eq!(unique.len(), owned.len(), "{owned:?}");
    assert!(owned.iter().all(|g| groups().iter().any(|x| x == g)));
}

#[test]
fn composer_gate() {
    let d = dir(&[("composer.json", r#"{"require":{"php":"^8.2"}}"#)]);
    assert!(err(&d, preset("laravel")).contains("PHP < 8.3"));
    let d = dir(&[("composer.json", r#"{"require":{"php":"^8.3"}}"#)]);
    assert!(load(&d, &preset("laravel")).is_ok());
    let ignored = Options {
        ignore_php_constraint: true,
        ..preset("laravel")
    };
    assert!(load(&d, &ignored).is_ok());
    assert!(load(&dir(&[("composer.json", "{}")]), &preset("laravel")).is_ok());
    let d = dir(&[("composer.json", "{")]);
    assert!(err(&d, preset("laravel")).starts_with("invalid "));
    let d = dir(&[("composer.json", r#"{"require":{"php":">="}}"#)]);
    assert!(err(&d, preset("laravel")).starts_with("invalid constraint"));
}

#[test]
fn mapping_lookups() {
    assert_eq!(
        rule_of("no_alias_functions"),
        Some("lint/style/no-alias-function")
    );
    assert_eq!(rule_of("no_unused_imports"), Some("lint/imports/no-unused"));
    assert_eq!(rule_of("Pint/laravel_blade"), Some("format/laravel/blade"));
    assert_eq!(rule_of("Pun/no_only"), Some("lint/pest/no-only"));
    assert_eq!(rule_of("braces"), None);
    assert_eq!(
        fixer_of("format", "format/imports/ordered"),
        Some("ordered_imports")
    );
    assert_eq!(
        fixer_of("format", "imports/ordered"),
        Some("ordered_imports")
    );
    assert_eq!(fixer_of("lint", "imports/ordered"), None);
    assert_eq!(fixer_of("lint", "format/imports/ordered"), None);
    assert_eq!(fixer_of("format", "ordered-imports"), None);
    assert_eq!(fixer_of("format", "format.ordered-imports"), None);
}

#[test]
fn rule_metadata() {
    let m = rule("lint/dead-code/no-self-assignment").unwrap();
    assert_eq!(
        (m.fix.name(), m.summary),
        (
            "safe",
            Some(
                "Detects and removes self-assignments where a variable or property is assigned to itself."
            )
        )
    );
    assert_eq!(
        rule("lint/types/require-strict-param").unwrap().fix.name(),
        "mixed"
    );
    let m = rule("format/arrays/syntax").unwrap();
    assert_eq!(
        (m.fix.name(), m.summary),
        (
            "safe",
            Some("PHP arrays should be declared using the configured syntax.")
        )
    );
    assert_eq!(
        rule("lint/imports/no-unused").unwrap().summary,
        Some("Unused `use` statements must be removed.")
    );
    assert_eq!(rule("analyse/types/argument").unwrap().fix.name(), "none");
    assert_eq!(
        rule("lint/style/no-array-push").unwrap().fix.name(),
        "unsafe"
    );
    assert!(rule("dead-code/no-self-assignment").is_none());
    assert!(rule("lint.no-self-assignment").is_none());
    assert!(rule("format/imports/no-unused").is_none());
}

#[test]
fn show() {
    let d = dir(&[(
        "pun.toml",
        "[files]\npaths = [\"app\"]\nexclude = [\"build\"]\n[format]\npreset = \"empty\"\nexclude = [\"gen\"]\n[format.rules]\n\"operators/concat-spacing\" = { spacing = \"one\" }\n\"strings/single-quote\" = true\n[lint.rules]\n\"style/require-final-class\" = \"error\"\n\"style/no-alias-function\" = { level = \"warn\", fix = \"off\", sets = [\"@internal\"] }\n\"types/no-loose-comparison\" = { fix = \"off\" }\n[[lint.overrides]]\nfiles = [\"tests/**\"]\nrules = { \"style/require-final-class\" = \"off\", \"types/no-loose-comparison\" = \"warn\" }\n[analyse]\npreset = \"basic\"\n[analyse.rules]\n\"undefined/method\" = { check-function-name-case = true }\n",
    )]);
    let v = load(&d, &Options::default()).unwrap().show();
    assert_eq!(
        v["lint"],
        json!({
            "sets": [],
            "exclude": ["build"],
            "ecosystems": [],
            "rules": {
                "style/no-alias-function": {"level": "warn", "fix": "off", "sets": ["@internal"]},
                "style/require-final-class": "error",
                "types/no-loose-comparison": {"fix": "off"},
            },
            "overrides": [{
                "files": ["tests/**"],
                "rules": {"style/require-final-class": "off", "types/no-loose-comparison": "warn"},
            }],
        })
    );
    assert_eq!(
        v["format"],
        json!({
            "preset": "empty", "exclude": ["build", "gen"], "not-path": [], "not-name": [],
            "rules": {"operators/concat-spacing": "one", "strings/single-quote": true},
        })
    );
    assert_eq!(
        (&v["files"], &v["php"]),
        (
            &json!({"paths": ["app"], "exclude": ["build"]}),
            &json!({"version": "8.3"}),
        )
    );
    assert_eq!(
        v["analyse"]["rules"]["undefined/method"],
        json!({"check-function-name-case": true})
    );
    assert_eq!(v["analyse"]["rules"]["undefined/function"], "on");
    assert_eq!(v["analyse"]["preset"], "basic");
    assert!(v["analyse"].get("baseline").is_none());
    assert!(v["lint"].get("baseline").is_none());
    let d = dir(&[("pun.toml", "[lint]\nbaseline = \"lint.toml\"\n")]);
    let c = load(&d, &Options::default()).unwrap();
    assert_eq!(c.lint.baseline.as_deref(), Some("lint.toml"));
    assert_eq!(c.show()["lint"]["baseline"], "lint.toml");
    let d = dir(&[(
        "pun.toml",
        "[analyse]\nbaseline = \"b.toml\"\nstubs = [\"stubs/A.stub\"]\nscan = [\"stubs\", \"helpers.php\"]\nbootstrap = [\"tests/bootstrap.php\"]\n[[analyse.ignore]]\nrule = \"undefined/method\"\nmessage = \"#^Call#\"\ncount = 2\n",
    )]);
    let a = &load(&d, &Options::default()).unwrap().show()["analyse"];
    assert_eq!(
        (
            &a["baseline"],
            &a["stubs"],
            &a["scan"],
            &a["bootstrap"],
            &a["ignore"]
        ),
        (
            &json!("b.toml"),
            &json!(["stubs/A.stub"]),
            &json!(["stubs", "helpers.php"]),
            &json!(["tests/bootstrap.php"]),
            &json!([{"rule": "analyse/undefined/method", "message": "#^Call#", "count": 2}])
        )
    );
}

#[test]
fn presets() {
    for p in crate::FormatPreset::ALL {
        assert_eq!(load(&dir(&[]), &preset(&p)).unwrap().preset, p);
    }
}

fn explained(name: &str, id: &str) -> String {
    let out = explain(name).unwrap();
    let (head, docs) = out.rsplit_once("docs      ").unwrap();
    assert_eq!(
        docs,
        format!("https://punphp.com/docs/rules/{}\n", id.replace('.', "-"))
    );
    head.to_string()
}

#[test]
fn explain_rules() {
    assert_eq!(
        explained("format/arrays/syntax", "format/arrays/syntax"),
        "format/arrays/syntax
PHP arrays should be declared using the configured syntax.

group     arrays: Array literals, list() destructuring and array access
presets   laravel
fix       safe
upstream  array_syntax (php-cs-fixer)
          LongArrayToShortArray (rector)
          array-style (mago-linter)
"
    );
    assert_eq!(
        explained("arrays/syntax", "format/arrays/syntax"),
        explained("format/arrays/syntax", "format/arrays/syntax")
    );
    assert_eq!(
        explained(
            "lint/style/no-alias-function",
            "lint/style/no-alias-function"
        ),
        "lint/style/no-alias-function
Master functions shall be used instead of aliases.

group     style: One consistent way to write the same thing
preset    strict
fix       unsafe
upstream  no_alias_functions (php-cs-fixer)
          no-alias-function (mago-linter)
"
    );
    assert_eq!(
        explained("analyse/undefined/method", "analyse/undefined/method"),
        "analyse/undefined/method

group     undefined: References to something declared nowhere
preset    basic
phpstan   phpstan/level_0 and up
fix       none
upstream  CallMethodsRule (phpstan-classes)
          method.nameNotString (phpstan)
          method.notFound (phpstan)
          checkThisOnly (phpstan-parameters)
          reportMagicMethods (phpstan-parameters)
          NonExistentMethod (mago-analyzer)
          PossiblyNonExistentMethod (mago-analyzer)
"
    );
    let out = explained("analyse/undefined/trait", "analyse/undefined/trait");
    assert!(
        out.contains("\n          NonExistentClassLike (mago-analyzer, also)\n"),
        "{out}"
    );
    assert_eq!(
        explained(
            "lint/comments/no-unused-ignore",
            "lint/comments/no-unused-ignore"
        ),
        "lint/comments/no-unused-ignore
A @pun-ignore comment that suppressed nothing. Fix removes it.

group     comments: Comments other than PHPDoc, including ignore comments
preset    essential
intent    suspicious
fix       safe
upstream  none (Pun's own rule)
"
    );
    let out = explained(
        "analyse/types/missing-return-type",
        "analyse/types/missing-return-type",
    );
    assert!(
        out.contains(
            "preset    strict\nphpstan   phpstan/level_6 and up\nfix       none\nfixed by  lint/types/add-void-return-type\n"
        ),
        "{out}"
    );
    let out = explained(
        "format/arrays/no-space-in-empty",
        "format/arrays/no-space-in-empty",
    );
    assert!(
        out.contains("\npresets   none (opt-in by name or group)\n"),
        "{out}"
    );
    // Rector's RemoveUselessTernaryRector is in its dead-code set, SimplifyTautologyTernaryRector in code-quality.
    let out = explained(
        "lint/dead-code/no-useless-ternary",
        "lint/dead-code/no-useless-ternary",
    );
    assert!(
        out.contains("\nsets      rector/code-quality, rector/dead-code\n"),
        "{out}"
    );
}

#[test]
fn explain_names() {
    let ok = |n: &str| explain(n).unwrap();
    assert_eq!(
        ok("method.notFound"),
        format!(
            "`method.notFound` is `analyse/undefined/method` in Pun.\n\n{}",
            ok("analyse/undefined/method")
        )
    );
    assert!(ok("lint.no-alias-functions").starts_with(
        "`lint.no-alias-functions` is `lint/style/no-alias-function` in Pun.\n\nlint/style/no-alias-function\n"
    ));
    let err = |n: &str| format!("{:#}", explain(n).unwrap_err());
    assert_eq!(
        err("checkUnionTypes"),
        "`checkUnionTypes` (phpstan-parameters) is `[analyse] union-types = true` in pun.toml, not a rule"
    );
    assert_eq!(
        err("curly_braces_position"),
        "`curly_braces_position` (php-cs-fixer) has no Pun rule: deprecated upstream; successor: braces_position"
    );
    assert_eq!(
        err("OverrideFinalPropertyHook"),
        "`OverrideFinalPropertyHook` (mago-analyzer) has no Pun rule: no PHPStan-parity equivalent; Mago analyzer is not a migrate input"
    );
    assert_eq!(
        err("nope"),
        "`nope` is not a Pun rule or an upstream rule name"
    );
}

#[test]
fn files_paths_stay_in_the_project() {
    let d = dir(&[]);
    let root = d.join("project");
    std::fs::create_dir_all(root.join("app")).unwrap();
    std::fs::create_dir_all(d.join("outside")).unwrap();
    std::os::unix::fs::symlink(d.join("outside"), root.join("link")).unwrap();
    let outside = d.join("outside").display().to_string();
    let escaping = ["../outside".to_string(), outside, "link".to_string()];
    let at = root.join("pun.toml").display().to_string();
    for path in escaping {
        std::fs::write(
            root.join("pun.toml"),
            format!("[files]\npaths = [{path:?}]\n"),
        )
        .unwrap();
        assert_eq!(
            err(&root, Options::default()),
            format!("{at}: [files] paths: \"{path}\" is outside the project")
        );
    }
    std::fs::write(
        root.join("pun.toml"),
        "[analyse]\nbaseline = \"../outside/b.toml\"\n",
    )
    .unwrap();
    assert_eq!(
        err(&root, Options::default()),
        format!("{at}: [analyse] baseline: \"../outside/b.toml\" is outside the project")
    );
    std::fs::write(
        root.join("pun.toml"),
        "[lint]\nbaseline = \"../outside/lint.toml\"\n",
    )
    .unwrap();
    assert_eq!(
        err(&root, Options::default()),
        format!("{at}: [lint] baseline: \"../outside/lint.toml\" is outside the project")
    );
    std::fs::write(
        root.join("pun.toml"),
        "[files]\npaths = [\"app\", \"later\"]\n",
    )
    .unwrap();
    let c = load(&root, &Options::default()).unwrap();
    assert_eq!(c.files.paths, ["app", "later"]);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one test walks every case of the behaviour it checks"
)]
fn pun_toml_errors() {
    let d = dir(&[]);
    let e = |toml: &str| {
        std::fs::write(d.join("pun.toml"), toml).unwrap();
        let p = d.join("pun.toml").display().to_string();
        err(&d, Options::default()).replace(&p, "pun.toml")
    };
    for (toml, msg) in [
        (
            "[workspace]\nmembers = []\n\n[require-dev]\n\"pestphp/pest\" = \"^4.0\"\n",
            "[require-dev] belongs in composer.json: Pun reads the dependencies from composer.json and composer.lock, and pun.toml holds only Pun's own settings",
        ),
        (
            "[lint.rules]\n\"style/require-final-class\" = true\n",
            "`style/require-final-class` = true: lint rules take a level; use \"error\"",
        ),
        (
            "[lint.rules]\n\"style/require-final-class\" = false\n",
            "`style/require-final-class` = false: lint rules take a level; use \"off\"",
        ),
        (
            "[analyse]\nlevel = 5\n",
            "[analyse] level: PHPStan's levels are presets in Pun; use preset = \"basic\" | \"essential\" | \"recommended\" | \"strict\" | \"max\" or \"phpstan/level_<n>\": level 5 is preset = \"phpstan/level_5\" (or Pun's \"recommended\")",
        ),
        (
            "[analyse]\nlevel = \"max\"\n",
            "[analyse] level: PHPStan's levels are presets in Pun; use preset = \"basic\" | \"essential\" | \"recommended\" | \"strict\" | \"max\" or \"phpstan/level_<n>\"",
        ),
        (
            "[analyse]\npreset = 8\n",
            "[analyse] preset = 8: PHPStan level 8 is preset = \"phpstan/level_8\" (or Pun's \"strict\")",
        ),
        (
            "[analyse.rules]\n\"undefined/method\" = \"warn\"\n",
            "[analyse.rules] `undefined/method` = \"warn\": analyse rules are \"on\" or \"off\" (or a table of options); there are no levels",
        ),
        (
            "[format]\npreset = \"pint\"\n",
            "[format] preset = \"pint\": expected `laravel`, `psr12` or `empty`",
        ),
        (
            "[php]\nversion = \"8\"\n",
            "[php] version = \"8\": expected a PHP version as major.minor, such as \"8.4\"",
        ),
        (
            "[php]\nversion = \"eight.four\"\n",
            "[php] version = \"eight.four\": expected a PHP version as major.minor, such as \"8.4\"",
        ),
        (
            "[lint]\nsets = [\"recommended\"]\n",
            "[lint] sets: `recommended` is not a lint set; the sets are Rector's, such as `rector/dead-code`, `rector/php-sets` and `rector-laravel/laravel-code-quality` (`pun explain` lists a rule's sets)",
        ),
        (
            "[lint.groups]\nnope = \"error\"\n",
            "[lint.groups]: `nope` is not a group (see [meta.groups] in crates/pun-config/rules/mapping.toml)",
        ),
        (
            "[lint.groups]\nstyle = true\n",
            "[lint.groups] `style` = true: expected \"error\", \"warn\" or \"off\"",
        ),
        (
            "[lint.rules]\n\"style/require-final-class\" = \"loud\"\n",
            "`style/require-final-class` = \"loud\": expected \"error\", \"warn\" or \"off\"",
        ),
        (
            "[lint.rules]\n\"style/require-final-class\" = { level = 2 }\n",
            "`style/require-final-class`: level = 2: expected \"error\", \"warn\" or \"off\"",
        ),
        (
            "[lint.rules]\n\"style/require-final-class\" = { fix = \"yes\" }\n",
            "`style/require-final-class`: fix = \"yes\": the only value is \"off\"",
        ),
        (
            "[lint.rules]\n\"style/require-final-class\" = 1\n",
            "`style/require-final-class` = 1: expected \"error\", \"warn\", \"off\" or a table",
        ),
        (
            "[analyse]\npreset = \"level-5\"\n",
            "[analyse] preset = \"level-5\": expected `basic`, `essential`, `recommended`, `strict`, `max`, or `phpstan/level_0` to `phpstan/level_10` or `phpstan/level_max`",
        ),
        (
            "[analyse.rules]\nlaravel = { magic = true }\n",
            "[analyse.rules] `laravel` is a group: it takes \"on\" or \"off\"",
        ),
        (
            "[analyse.rules]\n\"undefined/import\" = { magic = true }\n",
            "[analyse.rules] `undefined/import` has no option `magic`; its options are `check-function-name-case`, `check-internal-class-case-sensitivity`",
        ),
        (
            "[analyse.rules]\n\"types/yield-outside-function\" = { maybe = true }\n",
            "[analyse.rules] `types/yield-outside-function` has no options: it takes \"on\" or \"off\"",
        ),
        (
            "[analyse.rules]\n\"undefined/import\" = { check-function-name-case = \"yes\" }\n",
            "[analyse.rules] `undefined/import`: check-function-name-case = \"yes\": expected true or false",
        ),
        (
            "[analyse.rules]\n\"laravel/no-env-outside-config\" = { config-directories = \"config\" }\n",
            "[analyse.rules] `laravel/no-env-outside-config`: config-directories = \"config\": expected a list of strings",
        ),
        (
            "[analyse.rules]\n\"type-coverage/return\" = { min = \"high\" }\n",
            "[analyse.rules] `type-coverage/return`: min = \"high\": expected a number",
        ),
        (
            "[analyse.rules]\n\"disallowed/function\" = { calls = [\"var_dump()\"] }\n",
            "[analyse.rules] `disallowed/function`: calls = [\"var_dump()\"]: expected a list of tables",
        ),
        (
            "[format.rules]\n\"strings/single-quote\" = \"yes\"\n",
            "`strings/single-quote` must be true, false or a table of options",
        ),
        (
            "[format.rules]\nnope = true\n",
            "`nope` is not a format rule",
        ),
        (
            "[lint.rules]\nnope = \"error\"\n",
            "`nope` is not a lint rule",
        ),
        (
            "[analyse.rules]\nnope = \"on\"\n",
            "`nope` is not an analyse rule",
        ),
        (
            "[[analyse.ignore]]\nrule = \"nope\"\n",
            "`nope` is not an analyse rule",
        ),
        (
            "[[analyse.ignore]]\nmessage = \"#^A#\"\nraw-message = \"A\"\n",
            "[[analyse.ignore]] #1: message and raw-message: give one",
        ),
        (
            "[[analyse.ignore]]\nidentifier = \"x.y\"\n\n[[analyse.ignore]]\npaths = [\"app\"]\n",
            "[[analyse.ignore]] #2: name a rule, an identifier or a message",
        ),
        (
            "[format.rules]\nordered = true\n",
            "`ordered` is not a format rule",
        ),
    ] {
        let got = e(toml);
        assert_eq!(got, format!("pun.toml: {msg}"), "{toml}");
    }
    assert!(e("[format]\nexlude = [\"typo\"]\n").contains("unknown field `exlude`"));
    assert!(e("[lint]\nset = []\n").contains("unknown field `set`"));
    assert!(e("preset = \"basic\"\n").contains("expected `essential`, `recommended` or `strict`"));
    assert!(e("[format]\npaths = [\"app\"]\n").contains("unknown field `paths`"));
    assert!(e("[[lint.overrides]]\nrules = {}\n").contains("missing field `files`"));
    for (table, name, id) in [
        ("format", "format.array-syntax", "format/arrays/syntax"),
        ("format", "array-syntax", "format/arrays/syntax"),
        ("lint", "laravel-blade", "format/laravel/blade"),
        ("format", "no-unused-imports", "lint/imports/no-unused"),
        ("format", "imports/no-unused", "lint/imports/no-unused"),
        (
            "format",
            "self-static-accessor",
            "lint/classes/prefer-self-in-final",
        ),
        (
            "lint",
            "lint.no-alias-function",
            "lint/style/no-alias-function",
        ),
        ("lint", "final-class", "lint/style/require-final-class"),
        ("format", "final-class", "lint/style/require-final-class"),
        (
            "lint",
            "simplify-mirror-assign",
            "lint/dead-code/no-self-assignment",
        ),
        (
            "lint",
            "no-redundant-pest-uses",
            "lint/pest/no-redundant-uses",
        ),
        (
            "lint",
            "analyse.undefined-method",
            "analyse/undefined/method",
        ),
        ("analyse", "undefined-method", "analyse/undefined/method"),
        ("format", "array_syntax", "format/arrays/syntax"),
        ("format", "Pint/laravel_blade", "format/laravel/blade"),
        (
            "lint",
            "StrictInArray",
            "lint/functions/require-strict-in-array",
        ),
        ("lint", "method.notFound", "analyse/undefined/method"),
        ("analyse", "method.notFound", "analyse/undefined/method"),
        ("lint", "imports/ordered", "format/imports/ordered"),
        (
            "format",
            "style/require-final-class",
            "lint/style/require-final-class",
        ),
        ("lint", "format/imports/ordered", "format/imports/ordered"),
    ] {
        let value = if table == "format" { "true" } else { "\"off\"" };
        let got = e(&format!("[{table}.rules]\n\"{name}\" = {value}\n"));
        assert_eq!(got, format!("pun.toml: `{name}` is `{id}` in pun.toml"));
    }
    assert_eq!(
        e("[[lint.overrides]]\nfiles = [\"tests/**\"]\nrules = { final_class = \"off\" }\n"),
        "pun.toml: `final_class` is `lint/style/require-final-class` in pun.toml"
    );
    // A rule merged into another, by its full ID or its `<group>/<name>`.
    for name in [
        "lint/arrays/prefer-array-append",
        "arrays/prefer-array-append",
    ] {
        assert_eq!(
            e(&format!("[lint.rules]\n\"{name}\" = \"error\"\n")),
            format!(
                "pun.toml: `{name}` was merged into `lint/style/no-array-push`: use `lint/style/no-array-push` in pun.toml"
            )
        );
    }
    // Its upstream rule now maps to the rule it was merged into.
    assert_eq!(
        e("[lint.rules]\nChangeArrayPushToArrayAssign = \"error\"\n"),
        "pun.toml: `ChangeArrayPushToArrayAssign` is `lint/style/no-array-push` in pun.toml"
    );
}

#[test]
fn rule_sets_like_php_cs_fixer() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("../tests/rule_sets.json")).unwrap();
    let resolved = |rules: Vec<Rule>| -> Vec<(String, Value)> {
        rules.into_iter().map(|r| (r.name, r.options)).collect()
    };
    let expected = |case: &Value| -> Vec<(String, Value)> {
        let m = case["expected"].as_object().unwrap();
        m.iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    if *v == Value::Bool(true) {
                        Value::Null
                    } else {
                        v.clone()
                    },
                )
            })
            .collect()
    };
    let psr12 = load(&dir(&[]), &preset("psr12")).unwrap();
    let mut want = expected(&cases[0]);
    want.retain(|(r, _)| r != "ordered_class_elements" && r != "no_unused_imports");
    assert_eq!(resolved(psr12.rules), want);
    for case in &cases {
        let rules = case["rules"].as_object().unwrap().clone();
        assert_eq!(resolved(rule_set(rules).unwrap()), expected(case));
    }
    let rules = json!({"@auto": true}).as_object().unwrap().clone();
    assert_eq!(
        resolved(rule_set(rules).unwrap()),
        [("@auto".to_string(), Value::Null)]
    );
    let rules = json!({"@PSR12": {}}).as_object().unwrap().clone();
    assert_eq!(
        rule_set(rules).unwrap_err().to_string(),
        "Nested rule set \"@PSR12\" configuration must be a boolean."
    );
}

mod migrate {
    use super::*;
    use crate::migrate::{from_pint, from_rules};

    fn pint(json: &str) -> crate::migrate::Migration {
        from_pint(
            &dir(&[("pint.json", json)]).join("pint.json"),
            Path::new("/project"),
            None,
            &|_| true,
        )
        .unwrap()
    }

    #[test]
    fn writes_a_delta_and_moves_risky_rules_to_lint() {
        let m = pint(
            r#"{"exclude":["a"],"notPath":"y","rules":{"concat_space":{"spacing":"one"},"array_syntax":false,"single_quote":true,"final_class":true,"Pint/laravel_blade":true}}"#,
        );
        assert_eq!(m.unsupported, [] as [std::string::String; 0]);
        assert_eq!(
            m.toml,
            r#"[files]
exclude = ["a"]

[format]
preset = "laravel"
not-path = ["y"]

[format.rules]
"operators/concat-spacing" = "one"
"laravel/blade" = true
"arrays/syntax" = false

[lint.rules]
"refactor/rename-phpdoc-tag" = "error"
"functions/no-unused-closure-use" = "error"
"style/no-alias-function" = "error"
"statements/no-empty" = "error"
"phpdoc/no-superfluous-tags" = { allow-mixed = true, allow-unused-params = true }
"functions/no-unreachable-default-argument-value" = "error"
"imports/no-unused" = "error"
"control-flow/no-useless-return" = "error"
"style/require-ordered-traits" = "error"
"phpunit/require-method-casing" = { case = "snake-case" }
"phpunit/require-protected-setup" = "error"
"phpdoc/no-access" = "error"
"phpdoc/no-package" = "error"
"phpdoc/no-useless-inheritdoc" = "error"
"classes/prefer-self-in-final" = "error"
"style/require-final-class" = "error"

[[lint.overrides]]
files = ["*y*"]

[lint.overrides.rules]
"refactor/rename-phpdoc-tag" = "off"
"functions/no-unused-closure-use" = "off"
"style/no-alias-function" = "off"
"statements/no-empty" = "off"
"phpdoc/no-superfluous-tags" = "off"
"functions/no-unreachable-default-argument-value" = "off"
"imports/no-unused" = "off"
"control-flow/no-useless-return" = "off"
"style/require-ordered-traits" = "off"
"phpunit/require-method-casing" = "off"
"phpunit/require-protected-setup" = "off"
"phpdoc/no-access" = "off"
"phpdoc/no-package" = "off"
"phpdoc/no-useless-inheritdoc" = "off"
"classes/prefer-self-in-final" = "off"
"style/require-final-class" = "off"
"#
        );
        assert_eq!(
            pint(r#"{"rules":{"single_quote":true}}"#).toml,
            pint("{}").toml
        );
    }

    /// Pint runs `final_class` and every other rule as a fixer on the files its finder keeps (issue #41), so the
    /// files `notPath` and `notName` skip are skipped by the lint rules the config brings too; `exclude` is
    /// `[files] exclude`, which lint already honors.
    #[test]
    fn skips_lint_rules_where_pint_skips_files() {
        let m = pint(
            r##"{"preset":"empty","notPath":["tests/Support/TestCase.php","a[1]*.php","#^legacy#"],"notName":["*Controller.php","_ide_helper.php","Foo*.php","/^x/"],"exclude":["legacy"],"rules":{"final_class":true,"no_alias_functions":false}}"##,
        );
        assert_eq!(
            m.toml,
            r##"[files]
exclude = ["legacy"]

[format]
preset = "empty"
not-path = ["tests/Support/TestCase.php", "a[1]*.php", "#^legacy#"]
not-name = ["*Controller.php", "_ide_helper.php", "Foo*.php", "/^x/"]

[lint.rules]
"style/require-final-class" = "error"

[[lint.overrides]]
files = ["*tests/Support/TestCase.php*", '*a\[1\]\*.php*', "**/*Controller.php", "**/_ide_helper.php"]

[lint.overrides.rules]
"style/require-final-class" = "off"
"##
        );
        assert_eq!(
            m.unsupported,
            [
                "notPath \"#^legacy#\": a regex, which no [[lint.overrides]] glob holds, so the lint rules still check the files it skips",
                "notName \"Foo*.php\": a [[lint.overrides]] glob's wildcards would also match directory names, so the lint rules still check the files it skips",
                "notName \"/^x/\": a regex, which no [[lint.overrides]] glob holds, so the lint rules still check the files it skips",
            ]
        );
        let c = from_toml(&dir(&[]), &m.toml).unwrap();
        assert_eq!(
            c.lint.overrides[0].files,
            [
                "*tests/Support/TestCase.php*",
                "*a\\[1\\]\\*.php*",
                "**/*Controller.php",
                "**/_ide_helper.php"
            ]
        );
        assert_eq!(
            c.lint.overrides[0].rules[0].id,
            "lint/style/require-final-class"
        );
        assert_eq!(c.lint.overrides[0].rules[0].level, None);

        // Symfony walks each `in` directory, `.` the project too, and matches `notPath` against the path below it; an
        // absolute directory in the project (here `/project`) is matched relative to it.
        let m = pint(
            r#"{"preset":"empty","in":["./app/","src",".","/project/lib/","/elsewhere"],"notPath":"Legacy","rules":{"final_class":true}}"#,
        );
        assert!(
            m.toml.contains(
                "files = [\"app/*Legacy*\", \"src/*Legacy*\", \"*Legacy*\", \"lib/*Legacy*\", \"/elsewhere/*Legacy*\"]\n\n[lint.overrides.rules]\n\"style/require-final-class\" = \"off\"\n"
            ),
            "{}",
            m.toml
        );
        assert_eq!(m.unsupported, [] as [String; 0]);

        // A file under a nested `in` directory stays when one walk keeps it: Pint keeps `app/Legacy/A.php` from the
        // `app/Legacy` walk (`A.php`), while the `app` walk's glob would match it; so for `Legacy/Old` and, across `.`
        // and `app`, `pp/x`. `Models` can only occur below `app/Legacy` where both walks see it: its globs are exact.
        let m = pint(
            r#"{"preset":"empty","in":["app","app/Legacy",".","lib"],"notPath":["Legacy","Models","pp/x","Legacy/Old"],"rules":{"final_class":true}}"#,
        );
        assert!(
            m.toml.contains(
                "files = [\"app/*Models*\", \"app/Legacy/*Models*\", \"*Models*\", \"lib/*Models*\"]\n"
            ),
            "{}",
            m.toml
        );
        assert_eq!(
            m.unsupported,
            [
                "notPath \"Legacy\": the `in` directories \"app\" and \"app/Legacy\" nest, and a file Pint keeps from one would match the other's glob, so the lint rules still check the files it skips",
                "notPath \"pp/x\": the `in` directories \".\" and \"app\" nest, and a file Pint keeps from one would match the other's glob, so the lint rules still check the files it skips",
                "notPath \"Legacy/Old\": the `in` directories \"app\" and \"app/Legacy\" nest, and a file Pint keeps from one would match the other's glob, so the lint rules still check the files it skips",
            ]
        );

        // Without a lint rule on there is nothing to skip, and nothing to report.
        let m = pint(r#"{"preset":"empty","notPath":["x","/y/"],"rules":{"final_class":false}}"#);
        assert!(!m.toml.contains("[[lint.overrides]]"), "{}", m.toml);
        assert_eq!(m.unsupported, [] as [String; 0]);
    }

    #[test]
    fn round_trips() {
        for (preset, json) in [
            (
                "psr12",
                r#"{"preset":"psr12","in":"app","rules":{"concat_space":{"spacing":"one"},"braces_position":false,"no_alias_functions":{"sets":["@all"]},"@PSR2":true}}"#,
            ),
            (
                "laravel",
                r#"{"rules":{"ordered_imports":{"sort_algorithm":"length"},"no_unused_imports":false,"strict_comparison":true}}"#,
            ),
            (
                "empty",
                r#"{"preset":"empty","rules":{"@PSR12":true,"@PSR2":false,"single_quote":true}}"#,
            ),
        ] {
            let m = pint(json);
            let c = from_toml(&dir(&[]), &m.toml).unwrap();
            assert_eq!(c.preset, preset);
            let mut want = preset_rules(preset).unwrap();
            want.extend(
                serde_json::from_str::<Value>(json).unwrap()["rules"]
                    .as_object()
                    .unwrap()
                    .clone(),
            );
            let want = rule_set(want).unwrap();
            let lint =
                (c.lint.rules.iter()).map(|l| (l.fixer.unwrap().to_string(), l.options.clone()));
            let mut got: Vec<(String, Value)> = (c.rules.iter())
                .map(|r| (r.name.clone(), r.options.clone()))
                .chain(lint)
                .collect();
            let mut want: Vec<(String, Value)> =
                want.into_iter().map(|r| (r.name, r.options)).collect();
            got.sort_by(|a, b| a.0.cmp(&b.0));
            want.sort_by(|a, b| a.0.cmp(&b.0));
            assert_eq!(got, want, "{json}");
            assert!((c.lint.rules.iter()).all(|l| l.fix && l.level == Level::Error && l.named));
        }
    }

    #[test]
    fn visibility_required_becomes_modifier_keywords() {
        let mk = |json: &str| {
            let c = from_toml(&dir(&[]), &pint(json).toml).unwrap();
            assert!(!names(&c.rules).contains(&"visibility_required"));
            c.rules
                .into_iter()
                .find(|r| r.name == "modifier_keywords")
                .map(|r| r.options)
        };
        assert_eq!(
            mk(r#"{"rules":{"visibility_required":true}}"#),
            Some(Value::Null)
        );
        assert_eq!(
            mk(r#"{"rules":{"visibility_required":{"elements":["const","method"]}}}"#),
            Some(json!({"elements": ["method", "property", "const"]}))
        );
        assert_eq!(
            mk(r#"{"preset":"empty","rules":{"visibility_required":{"elements":["method"]}}}"#),
            Some(json!({"elements": ["method"]}))
        );
        assert_eq!(
            mk(
                r#"{"rules":{"modifier_keywords":true,"visibility_required":{"elements":["method"]}}}"#
            ),
            Some(Value::Null)
        );
    }

    #[test]
    fn lists_unsupported_rules() {
        let m = pint(
            r#"{"preset":"empty","rules":{"braces":true,"Custom/fixer":true,"header_comment":{"header":"h","validator":null},"no_whitespace_before_comma_in_array":{"after_heredoc":[null]},"@auto":true}}"#,
        );
        assert_eq!(m.unsupported.len(), 4);
        assert!(
            m.unsupported[0].starts_with(
                "braces: deprecated upstream; successor: single_space_around_construct"
            )
        );
        let mut rest = m.unsupported[1..].to_vec();
        rest.sort();
        assert_eq!(
            rest,
            [
                "@auto: not a PHP-CS-Fixer or Pint rule Pun knows (a custom fixer or rule set)",
                "Custom/fixer: not a PHP-CS-Fixer or Pint rule Pun knows (a custom fixer or rule set)",
                "no_whitespace_before_comma_in_array: an option is null, which TOML can't hold",
            ]
        );
        let e = from_pint(
            &dir(&[(
                "pint.json",
                r#"{"preset":"empty","rules":{"final_internal_class":{"include":null}}}"#,
            )])
            .join("pint.json"),
            Path::new("/project"),
            None,
            &|_| true,
        )
        .err()
        .unwrap();
        assert_eq!(
            e.to_string(),
            "[final_internal_class] Invalid configuration: The option \"include\" with value null is expected to be of type \"string[]\", but is of type \"null\"."
        );
        assert_eq!(
            m.toml,
            "[format]\npreset = \"empty\"\n\n[format.rules]\n\"comments/header\" = { header = \"h\" }\n"
        );
    }

    #[test]
    fn expands_per() {
        let m = pint(r#"{"preset":"per"}"#);
        assert_eq!(m.unsupported, [] as [std::string::String; 0]);
        let c = from_toml(&dir(&[]), &m.toml).unwrap();
        assert_eq!(c.preset, "empty");
        let rules = names(&c.rules);
        for r in [
            "types_spaces",
            "attribute_block_no_spaces",
            "concat_space",
            "lowercase_cast",
        ] {
            assert!(rules.contains(&r), "{r}");
        }
        let lint: Vec<_> = c.lint.rules.iter().map(|l| l.fixer.unwrap()).collect();
        assert_eq!(lint, ["ordered_class_elements", "no_unused_imports"]);
    }

    #[test]
    fn skips_rules_pun_cant_run() {
        let runs = |n: &str| n != "single_quote" && n != "final_class";
        let d = dir(&[("pint.json", r#"{"rules":{"final_class":true}}"#)]);
        let m = from_pint(&d.join("pint.json"), Path::new("/project"), None, &runs).unwrap();
        assert_eq!(
            m.unsupported,
            [
                "single_quote: not supported by Pun yet",
                "final_class: not supported by Pun yet"
            ]
        );
        assert!(
            m.toml
                .contains("\n[format.rules]\n\"strings/single-quote\" = false\n"),
            "{}",
            m.toml
        );
        assert!(!m.toml.contains("require-final-class"));
    }

    /// The phpunit extension brings `php_unit_method_casing`; Pint with it `false` leaves `test_foo_bar` alone, so
    /// the migrated pun.toml turns it off. `final_class` belongs to no extension, so nothing brings it back.
    #[test]
    fn keeps_extension_rules_the_config_turns_off() {
        let m = pint(r#"{"rules":{"php_unit_method_casing":false,"final_class":false}}"#);
        assert!(
            m.toml
                .contains("\n\"phpunit/require-method-casing\" = \"off\"\n"),
            "{}",
            m.toml
        );
        assert!(!m.toml.contains("require-final-class"), "{}", m.toml);
        let on = format!("[extensions]\nphpunit = \"on\"\n\n{}", m.toml);
        let c = from_toml(&dir(&[]), &on).unwrap();
        let fixers: Vec<_> = c.lint.rules.iter().filter_map(|l| l.fixer).collect();
        assert!(!fixers.contains(&"php_unit_method_casing"), "{fixers:?}");
        let c = from_toml(
            &dir(&[]),
            &format!("[extensions]\nphpunit = \"on\"\n\n{}", pint("{}").toml),
        )
        .unwrap();
        assert!(
            c.lint
                .rules
                .iter()
                .any(|l| l.fixer == Some("php_unit_method_casing"))
        );
    }

    /// nikic/php-parser's `@PSR12` with `curly_braces_position` (deprecated, a proxy of `braces_position`) skipped the
    /// proxy, so Pun moved every function brace to the next line, which PHP-CS-Fixer leaves alone. A proxy runs its
    /// successor with its own configuration or the fixed one its `createProxyFixers()` sets (expected values read off
    /// PHP-CS-Fixer 3.95.25's proxies). Beside the successor both run in name order (`Utils::sortFixers`), so the later
    /// name's options win; a run within the successor's defaults adds nothing; any other second run is skipped.
    #[test]
    fn deprecated_proxies_become_their_successors() {
        let migrate = |rules: serde_json::Value| {
            from_rules(
                rules.as_object().unwrap().clone(),
                &serde_json::Map::new(),
                &|_| true,
            )
            .unwrap()
        };
        let has = |m: &crate::migrate::Migration, lines: &[&str]| {
            for line in lines {
                assert!(
                    m.toml.contains(&format!("\n{line}\n")),
                    "{line}\n{}",
                    m.toml
                );
            }
        };
        let m = migrate(json!({
            "@PSR12": true,
            "curly_braces_position": {"functions_opening_brace": "same_line"},
            "new_with_braces": {"anonymous_class": false, "named_class": false},
            "no_trailing_comma_in_singleline_array": true,
            "single_blank_line_before_namespace": true,
            "compact_nullable_typehint": true,
            "no_blank_lines_before_namespace": true,
        }));
        assert_eq!(
            m.unsupported,
            [
                "no_blank_lines_before_namespace: deprecated upstream; successor: blank_lines_before_namespace"
            ]
        );
        has(
            &m,
            &[
                "\"statements/braces-position\" = { allow-single-line-anonymous-functions = false, allow-single-line-empty-anonymous-classes = true, functions-opening-brace = \"same-line\" }",
                "\"operators/new-parentheses\" = { anonymous-class = true, named-class = false }",
                "\"arrays/no-trailing-comma-single-line\" = [\"array\"]",
                "\"imports/blank-lines-before-namespace\" = true",
                "\"types/compact-nullable\" = true",
            ],
        );
        let m = migrate(json!({
            "no_trailing_comma_in_singleline": true,
            "no_trailing_comma_in_singleline_array": true,
            "no_blank_lines_before_namespace": true,
            "no_unneeded_curly_braces": true,
            "no_unneeded_braces": {"namespaces": true},
        }));
        assert_eq!(
            m.unsupported,
            ["no_unneeded_curly_braces: deprecated upstream; successor: no_unneeded_braces"]
        );
        has(
            &m,
            &[
                "\"arrays/no-trailing-comma-single-line\" = true",
                "\"imports/blank-lines-before-namespace\" = { min-line-breaks = 0, max-line-breaks = 1 }",
            ],
        );
    }

    #[test]
    fn php_cs_fixer_rules() {
        let rules = json!({"@PSR12": true, "strict_param": true, "concat_space": {"spacing": "one"}, "indentation_type": {"indent": "\t"}, "line_ending": {"line_ending": "\r\n"}});
        let finder = json!({"in": ["src", "/elsewhere"], "exclude": [], "notName": ["*.inc"], "notPath": "x/y.php", "unsupported": ["depth()", "setIndent(\"  \")"]});
        let m = from_rules(
            rules.as_object().unwrap().clone(),
            finder.as_object().unwrap(),
            &|_| true,
        )
        .unwrap();
        assert_eq!(
            m.unsupported,
            [
                "Finder depth(): no pun.toml key for it",
                "Config setIndent(\"  \"): format indents with four spaces or tabs"
            ]
        );
        assert!(
            m.toml.starts_with("[files]\npaths = [\"src\", \"/elsewhere\"]\n\n[format]\npreset = \"empty\"\nnot-path = [\"x/y.php\"]\nnot-name = [\"*.inc\"]\n\n[format.rules]\n"),
            "{}",
            m.toml
        );
        assert!(
            m.toml
                .contains("\"whitespace/indentation-type\" = \"tabs\"\n")
                && m.toml.contains("\"whitespace/line-ending\" = \"crlf\"\n"),
            "{}",
            m.toml
        );
        let c = from_toml(&dir(&[]), &m.toml).unwrap();
        assert_eq!(c.preset, "empty");
        assert_eq!(c.finder.not_name, ["*.inc"]);
        let options = |n: &str| &c.rules.iter().find(|r| r.name == n).unwrap().options;
        assert_eq!(options("indentation_type"), &json!({"indent": "\t"}));
        assert_eq!(options("line_ending"), &json!({"line_ending": "\r\n"}));
        assert!(
            names(&c.rules).contains(&"braces_position")
                && names(&c.rules).contains(&"concat_space")
        );
        let fixers: Vec<_> = c.lint.rules.iter().map(|l| l.fixer.unwrap()).collect();
        assert_eq!(fixers, ["ordered_class_elements", "strict_param"]);
        // PHP-CS-Fixer's Finder skips `notPath` (below each `in` directory) and `notName` for its risky rules too.
        assert_eq!(
            c.lint.overrides[0].files,
            ["src/*x/y.php*", "/elsewhere/*x/y.php*", "**/*.inc"]
        );
        let off: Vec<_> = (c.lint.overrides[0].rules.iter())
            .map(|r| (r.id.as_str(), r.level))
            .collect();
        assert_eq!(
            off,
            [
                ("lint/classes/require-member-order", None),
                ("lint/types/require-strict-param", None)
            ]
        );
    }

    #[test]
    fn pint_errors() {
        let e = |files: &[(&str, &str)]| {
            from_pint(
                &dir(files).join("pint.json"),
                Path::new("/project"),
                None,
                &|_| true,
            )
            .err()
            .unwrap()
            .to_string()
        };
        assert_eq!(
            e(&[("pint.json", r#"{"preset":"nope"}"#)]),
            "Preset not found."
        );
        assert_eq!(
            e(&[("pint.json", r#"{"preset":"symfony"}"#)]),
            "the symfony preset was removed: Pun supports the laravel, psr12, per and empty presets"
        );
        assert_eq!(pint("[1]").toml, pint("{}").toml);
        assert_eq!(
            from_pint(
                &dir(&[]).join("pint.json"),
                Path::new("/project"),
                Some("psr12"),
                &|_| true
            )
            .unwrap()
            .toml,
            "[format]\npreset = \"psr12\"\n\n[lint.rules]\n\"classes/require-member-order\" = { order = [\"use_trait\"] }\n\"imports/no-unused\" = \"error\"\n"
        );
        assert_eq!(
            e(&[
                ("a.json", r#"{"extend":"b.json"}"#),
                ("pint.json", r#"{"extend":"a.json"}"#)
            ]),
            "Pint configuration cannot extend from more than 1 file."
        );
        assert!(e(&[("pint.json", "1")]).ends_with("pint.json] is not valid JSON."));
        let d = dir(&[]);
        std::fs::create_dir(d.join("pint.json")).unwrap();
        let err = from_pint(&d.join("pint.json"), Path::new("/project"), None, &|_| true)
            .err()
            .unwrap();
        assert_eq!(
            format!("{err:#}"),
            format!(
                "can't read {}: Is a directory (os error 21)",
                d.join("pint.json").display()
            )
        );
        assert!(e(&[("pint.json", "{")]).ends_with("pint.json] is not valid JSON."));
        assert_eq!(
            e(&[("pint.json", r#"{"rules":{"final_class":null}}"#)]),
            "[final_class] Rule must be enabled (true), disabled (false) or configured (non-empty, assoc array). Other values are not allowed. To disable the rule, use \"FALSE\" instead of \"NULL\"."
        );
        assert_eq!(
            e(&[("pint.json", r#"{"rules":{"@PSR12":1}}"#)]),
            "[@PSR12] Set must be enabled (true) or disabled (false). Other values are not allowed."
        );
        for (base, given) in [
            ("{bad", "null"),
            ("\"x\"", "string"),
            ("1", "int"),
            ("1.5", "float"),
            ("true", "true"),
            ("false", "false"),
        ] {
            assert_eq!(
                e(&[
                    ("pint.json", r#"{"extend":"base.json"}"#),
                    ("base.json", base)
                ]),
                format!(
                    "array_replace_recursive(): Argument #1 ($array) must be of type array, {given} given"
                )
            );
        }
        assert_eq!(
            e(&[("pint.json", r#"{"extend":"base.json"}"#)]),
            "Path must not be empty"
        );
    }

    #[test]
    fn extend() {
        let d = dir(&[
            (
                "base.json",
                r#"{"preset":"empty","exclude":["a","b"],"notName":["n"],"rules":{"single_quote":true}}"#,
            ),
            (
                "custom.json",
                r#"{"extend":"base.json","exclude":["x"],"notName":["n","m"],"in":"src","rules":{"concat_space":{"spacing":"one"}}}"#,
            ),
        ]);
        let m = from_pint(&d.join("custom.json"), Path::new("/project"), None, &|_| {
            true
        })
        .unwrap();
        assert_eq!(
            m.toml,
            "[files]\npaths = [\"src\"]\nexclude = [\"x\", \"b\"]\n\n[format]\npreset = \"empty\"\nnot-name = [\"n\", \"m\"]\n\n[format.rules]\n\"strings/single-quote\" = true\n\"operators/concat-spacing\" = \"one\"\n"
        );
        assert!(m.unsupported.is_empty());
    }
}

mod options;
mod phpstan;
mod rector;

#[test]
fn natural_language_join_matches_upstream() {
    use crate::options::natural_language_join;
    assert_eq!(natural_language_join(["a"]), r#""a""#);
    assert_eq!(natural_language_join(["a", "b"]), r#""a" and "b""#);
    assert_eq!(
        natural_language_join(["a", "b", "c"]),
        r#""a", "b" and "c""#
    );
}

#[test]
fn php_version_id() {
    let d = dir(&[("pun.toml", "[php]\nversion = \"8.5\"\n")]);
    let c = load(&d, &Options::default()).unwrap();
    assert_eq!(c.php_version().id(), 80500);
    assert_eq!(c.show()["php"]["version"], "8.5");
    assert_eq!(crate::PhpVersion::FLOOR.id(), 80300);
    assert_eq!(crate::FormatPreset::Psr12.to_string(), "psr12");
}

#[test]
fn analyse_parameters() {
    fn show(v: &V) -> String {
        match v {
            V::Array(a) => format!("[{}]", a.values().map(show).collect::<Vec<_>>().join(", ")),
            V::Int(i) => i.to_string(),
            V::Bool(b) => b.to_string(),
            v => v.as_str().unwrap().to_string(),
        }
    }
    use pun_phpstan::Value as V;
    let groups: std::collections::BTreeSet<&str> = (rules().iter())
        .filter_map(|(id, _)| id.strip_prefix("analyse/")?.split('/').next())
        .collect();
    let mut toml = String::from(
        "[analyse]\nstubs = [\"stubs/a.stub\"]\nscan = [\"stubs\", \"helpers.php\"]\nbootstrap = [\"tests/bootstrap.php\"]\n\n[[analyse.ignore]]\nrule = \"undefined/constant\"\npaths = [\"app/A.php\"]\ncount = 2\n\n[[analyse.ignore]]\nidentifier = \"x.y\"\nmessage = \"#m#\"\n",
    );
    toml.push_str("\n[[analyse.ignore]]\nraw-message = \"r\"\npaths = [\"*/B.php\", \"app/../C.php\"]\nreport-unmatched = false\n\n[analyse.rules]\n");
    for g in &groups {
        let _ = writeln!(toml, "{g} = \"off\"");
    }
    toml.push_str("\"undefined/constant\" = \"on\"\n\"types/by-ref-foreach-on-expression\" = \"on\"\n\"undefined/property\" = \"on\"\n\"undefined/function\" = { check-function-name-case = true }\n\"laravel/missing-translation\" = { translation-directories = [\"lang\"] }\n\"deprecated/constant\" = \"on\"\n");
    let d = dir(&[("pun.toml", &toml)]);
    std::fs::create_dir(d.join("stubs")).unwrap();
    let a = load(&d, &Options::default()).unwrap().analyse;
    assert_eq!(a.level(), 1);
    let p = a.parameters(&d);
    let get = |k: &str| p.get_str(k).cloned();
    assert_eq!(get("level"), Some(V::Int(1)));
    assert_eq!(get("checkClassCaseSensitivity"), Some(V::Bool(false)));
    let root = d.display().to_string();
    let list =
        |items: &[&str]| V::Array(pun_phpstan::Array::list(items.iter().map(|s| V::str(*s))));
    assert_eq!(
        get("stubFiles"),
        Some(list(&[&format!("{root}/stubs/a.stub")]))
    );
    // a directory is a scanned directory, anything else a scanned file (which need not exist yet)
    assert_eq!(
        get("scanDirectories"),
        Some(list(&[&format!("{root}/stubs")]))
    );
    assert_eq!(
        get("scanFiles"),
        Some(list(&[&format!("{root}/helpers.php")]))
    );
    assert_eq!(
        get("bootstrapFiles"),
        Some(list(&[&format!("{root}/tests/bootstrap.php")]))
    );
    assert_eq!(get("translationDirectories"), Some(list(&["lang"])));
    assert_eq!(get("checkFunctionNameCase"), Some(V::Bool(true)));
    assert_eq!(get("deprecationRulesInstalled"), Some(V::Bool(true)));
    let toggles = get("featureToggles").unwrap();
    let toggle = |k: &str| toggles.as_array().unwrap().get_str(k).cloned();
    assert_eq!(toggle("assignToByRefForeachExpr"), Some(V::Bool(true)));
    assert_eq!(toggle("unusedLabel"), Some(V::Bool(false)));
    assert_eq!(get("noModelMake"), None);
    let ignores: Vec<String> = (get("ignoreErrors").unwrap().as_array().unwrap().values())
        .map(|e| {
            let e = e.as_array().unwrap();
            let keys: Vec<String> = e
                .iter()
                .map(|(k, v)| format!("{}={}", k.as_string(), show(v)))
                .collect();
            keys.join(" ")
        })
        .collect();
    assert_eq!(
        ignores,
        [
            format!("rule=analyse/undefined/constant path={root}/app/A.php count=2"),
            "message=#m# identifier=x.y".to_string(),
            format!("rawMessage=r paths=[*/B.php, {root}/C.php] reportUnmatched=false"),
        ]
    );

    let level = |preset: &str| {
        let d = dir(&[("pun.toml", &format!("[analyse]\npreset = \"{preset}\"\n"))]);
        load(&d, &Options::default()).unwrap().analyse.level()
    };
    assert_eq!((level("strict"), level("max")), (8, 10));
    let d = dir(&[("pun.toml", "[analyse]\npreset = \"essential\"\n")]);
    let p = load(&d, &Options::default())
        .unwrap()
        .analyse
        .parameters(&d);
    assert_eq!(p.get_str("checkClassCaseSensitivity"), Some(&V::Bool(true)));
    assert_eq!(
        p.get_str("deprecationRulesInstalled"),
        Some(&V::Bool(false))
    );
}

#[test]
fn disallowed_entries_keep_their_numbers() {
    use pun_phpstan::Value as V;
    let toml = "[extensions]\ndisallowed-calls = \"on\"\n\n[analyse.rules.\"disallowed/function\"]\ncalls = [{ function = \"f()\", allow-params-anywhere = { 1 = 2, 2 = 1.5 } }]\n";
    let d = dir(&[("pun.toml", toml)]);
    let on = load(&d, &Options::default())
        .unwrap()
        .analyse
        .parameters(&d);
    let entry = (on.get_str("disallowedFunctionCalls").and_then(V::as_array))
        .and_then(|l| l.values().next())
        .and_then(V::as_array)
        .unwrap();
    let params = (entry.get_str("allowParamsAnywhere").and_then(V::as_array)).unwrap();
    assert_eq!(
        (params.get_str("1"), params.get_str("2")),
        (Some(&V::Int(2)), Some(&V::Float(1.5)))
    );
}

#[test]
fn variable_parameters() {
    use pun_phpstan::Value as V;
    let d = dir(&[]);
    let mut base = String::from("[analyse.rules]\n");
    let groups: std::collections::BTreeSet<&str> = (rules().iter())
        .filter_map(|(id, _)| id.strip_prefix("analyse/")?.split('/').next())
        .collect();
    for group in groups {
        let _ = writeln!(base, "{group} = \"off\"");
    }
    for consumer in ["undefined/variable", "undefined/compact-variable"] {
        for level in [0, 1] {
            for explicit in [None, Some(false), Some(true)] {
                let option = match explicit {
                    None => "\"on\"".to_string(),
                    Some(value) => format!("{{ maybe = {value} }}"),
                };
                let constant = if level == 1 { "on" } else { "off" };
                let text = format!(
                    "{base}\"{consumer}\" = {option}\n\"undefined/constant\" = \"{constant}\"\n"
                );
                let a = from_toml(&d, &text).unwrap().analyse;
                assert_eq!(a.level(), level, "{text}");
                let p = a.parameters(&d);
                assert_eq!(
                    p.get_str("cliArgumentsVariablesRegistered"),
                    Some(&V::Bool(true)),
                    "{text}"
                );
                assert_eq!(
                    p.get_str("checkMaybeUndefinedVariables"),
                    Some(&V::Bool(explicit.unwrap_or(level >= 1))),
                    "{text}"
                );
            }
        }
    }
}

#[test]
fn strict_rules_parameters_follow_its_rules_neon() {
    use pun_phpstan::Value as V;
    let neon = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../pun-analyse/tests/fixtures/coverage/strict-rules/conf/rules.neon"
    ))
    .unwrap();
    let neon = pun_phpstan::neon::decode(&neon).unwrap();
    let upstream = neon.as_array().unwrap().get_str("parameters").unwrap();
    let mut base = String::from("[analyse]\nbleeding-edge = true\n\n[analyse.rules]\n");
    let groups: std::collections::BTreeSet<&str> = (rules().iter())
        .filter_map(|(id, _)| id.strip_prefix("analyse/")?.split('/').next())
        .collect();
    for group in groups {
        let _ = writeln!(base, "{group} = \"off\"");
    }
    let d = dir(&[]);
    let off = from_toml(&d, &base).unwrap().analyse;
    let strict = from_toml(
        &d,
        &format!("{base}\"operators/non-boolean-and-operand\" = \"on\"\n"),
    )
    .unwrap()
    .analyse;
    let cast = |a: &crate::schema::Analyse, lint: bool| {
        let p = a.parameters_with_lint(&d, |r| lint && r == "lint/dead-code/no-useless-cast");
        let toggles = p.get_str("strictRules").and_then(V::as_array);
        toggles.and_then(|t| t.get_str("uselessCast")).cloned()
    };
    // Every parameter rules.neon sets, with `%featureToggles.bleedingEdge%` on
    let p = strict.parameters(&d);
    for (key, value) in upstream.as_array().unwrap().iter() {
        let key = key.as_string();
        if key == "strictRules" {
            continue;
        }
        let expected = match value {
            V::Str(s) if s.starts_with('%') => V::Bool(true),
            v => v.clone(),
        };
        assert_eq!(p.get_str(&key), Some(&expected), "{key}");
    }
    // Without a strict-rules check on, PHPStan's defaults stay
    let p = off.parameters(&d);
    assert_eq!(p.get_str("strictRulesInstalled"), Some(&V::Bool(false)));
    assert_eq!(p.get_str("reportMaybesInMethodSignatures"), None);
    // UselessCastRule's lint rule turns its gate on only once strict-rules is in
    assert_eq!(cast(&strict, true), Some(V::Bool(true)));
    assert_eq!(cast(&strict, false), Some(V::Bool(false)));
    assert_eq!(cast(&off, true), Some(V::Bool(false)));
}

#[test]
fn larastan_follows_the_locked_laravel() {
    use crate::composer::Version;
    let v = |major, minor, patch| {
        Some(Version {
            major,
            minor,
            patch,
        })
    };
    let supports = crate::phpstan_parameters::larastan_supports;
    let yes = [
        v(11, 44, 2),
        v(11, 45, 0),
        v(12, 4, 1),
        v(12, 5, 0),
        v(13, 0, 0),
        None,
    ];
    let no = [
        v(10, 48, 0),
        v(11, 44, 1),
        v(11, 9, 2),
        v(12, 4, 0),
        v(12, 3, 9),
        v(14, 0, 0),
    ];
    assert!(yes.iter().all(|v| supports(*v)) && !no.iter().any(|v| supports(*v)));
    let lock = |version: &str| {
        let lock =
            format!(r#"{{"packages":[{{"name":"laravel/framework","version":"{version}"}}]}}"#);
        let toml = "[extensions]\nlaravel = \"on\"\n[analyse]\n";
        let d = dir(&[("pun.toml", toml), ("composer.lock", &lock)]);
        let a = load(&d, &Options::default()).unwrap().analyse;
        (a.larastan, a.parameters(&d).get_str("noModelMake").cloned())
    };
    assert_eq!(lock("v11.9.2"), (false, None));
    assert_eq!(
        lock("v12.10.0"),
        (true, Some(pun_phpstan::Value::Bool(true)))
    );
    let d = dir(&[("pun.toml", "[analyse]\n")]);
    assert!(!load(&d, &Options::default()).unwrap().analyse.larastan);
}

/// Every cache (results, `pun x`, Blade's scripts) lives in this one directory: `PUN_CACHE_DIR` wins over the
/// platform's, an empty value counts as unset, and macOS ignores `XDG_CACHE_HOME`.
#[test]
fn cache_dir_follows_pun_cache_dir_then_the_platform() {
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            (vars.iter())
                .find(|(n, _)| *n == k)
                .map(|(_, v)| std::ffi::OsString::from(v))
        }
    };
    let all = env(&[
        ("PUN_CACHE_DIR", "/p"),
        ("HOME", "/h"),
        ("XDG_CACHE_HOME", "/x"),
    ]);
    assert_eq!(cache_dir_in(all, false), Some("/p".into()));
    assert_eq!(cache_dir_in(all, true), Some("/p".into()));
    let platform = env(&[
        ("PUN_CACHE_DIR", ""),
        ("HOME", "/h"),
        ("XDG_CACHE_HOME", "/x"),
    ]);
    assert_eq!(cache_dir_in(platform, false), Some("/x/pun".into()));
    assert_eq!(
        cache_dir_in(platform, true),
        Some("/h/Library/Caches/pun".into())
    );
    let home = env(&[("HOME", "/h")]);
    assert_eq!(cache_dir_in(home, false), Some("/h/.cache/pun".into()));
    assert_eq!(cache_dir_in(env(&[("XDG_CACHE_HOME", "/x")]), false), None);
}

#[test]
fn native_blade_engine() {
    let toml = |blade: &str| {
        let text =
            format!("[format]\nblade = \"{blade}\"\n[format.rules]\n\"laravel/blade\" = true\n");
        dir(&[
            ("composer.json", r#"{"require":{"php":"^8.4"}}"#),
            ("pun.toml", &text),
        ])
    };
    let options = |c: &Config| {
        (c.rules.iter())
            .find(|r| r.name == "Pint/laravel_blade")
            .unwrap()
            .options
            .clone()
    };
    let c = load(&toml("native"), &Options::default()).unwrap();
    assert!(c.blade);
    assert_eq!(options(&c), json!({"engine": "native"}));
    let c = load(&toml("prettier"), &Options::default()).unwrap();
    assert_eq!(options(&c), Value::Null);
    let e = load(&toml("node"), &Options::default())
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("[format] blade = \"node\": expected `prettier` or `native`"),
        "{e}"
    );
}

#[test]
fn rule_docs_url_has_no_dots() {
    assert_eq!(
        crate::rule_docs_url("lint/laravel-5.8/prefer-deferrable-provider"),
        "https://punphp.com/docs/rules/lint/laravel-5-8/prefer-deferrable-provider"
    );
    assert_eq!(
        crate::rule_docs_url("format/arrays/syntax"),
        "https://punphp.com/docs/rules/format/arrays/syntax"
    );
}
