use super::*;
use crate::migrate::{Configured, Parts, Rector, RectorConfig, compose, from_rector};
use std::collections::{BTreeMap, BTreeSet};

const FOREACH: &str = "Rector\\DeadCode\\Rector\\Foreach_\\RemoveUnusedForeachKeyRector";
const AND_TRUE: &str = "Rector\\DeadCode\\Rector\\BooleanAnd\\RemoveAndTrueRector";
const MIRROR: &str = "Rector\\DeadCode\\Rector\\Expression\\SimplifyMirrorAssignRector";
const PROMOTION: &str =
    "Rector\\Php80\\Rector\\Class_\\ClassPropertyAssignToConstructorPromotionRector";
const RENAME: &str = "Rector\\Renaming\\Rector\\FuncCall\\RenameFunctionRector";
const NOT_EQUAL: &str = "Rector\\CodeQuality\\Rector\\NotEqual\\CommonNotEqualRector";

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(std::string::ToString::to_string).collect()
}

#[test]
fn reads_sets_rules_and_skips() {
    let root = dir(&[]);
    let at = |p: &str| format!("{}/{p}", root.display());
    let config = RectorConfig {
        rules: strings(&[
            FOREACH,
            "App\\Rector\\MyRector",
            PROMOTION,
            RENAME,
            MIRROR,
            AND_TRUE,
            FOREACH,
            NOT_EQUAL,
        ]),
        configured: serde_json::from_value(json!({
            RENAME: {"configuration": {"split": "explode"}, "constants": []},
        }))
        .unwrap(),
        skip_rules: serde_json::from_value(
            json!({ MIRROR: null, AND_TRUE: [at("tests"), at("app/Legacy.php")], NOT_EQUAL: [at("tests")] }),
        )
        .unwrap(),
        skip_paths: vec![at("legacy"), "*/Fixtures/*".into()],
        paths: vec![at("app"), at("tests/")],
        sets: vec![
            at("vendor/rector/rector/src/Set/ValueObject/../../../config/set/dead-code.php"),
            at("vendor/pestphp/pest-plugin-rector/src/Set/../../config/sets/coding-style.php"),
            at("vendor/rector/rector/src/Set/ValueObject/../../../config/set/type-declaration.php"),
            at("vendor/rector/rector/src/Set/ValueObject/../../../config/set/php84.php"),
        ],
        php_sets: true,
        settings: vec![],
    };
    let r = from_rector(&root, &config);
    assert_eq!(
        r.sets,
        [
            "rector/dead-code",
            "pest/coding-style",
            "rector/type-declaration",
            "rector/php84",
            "rector/php-sets"
        ]
    );
    // On PHP 8.4 (composer.json), `rector/php-sets` holds php83 and php84 already; php85, named on its own, stays.
    let root84 = dir(&[("composer.json", "{\"require\":{\"php\":\"^8.4\"}}")]);
    let level = |v: &str| {
        format!(
            "{}/vendor/rector/rector/src/Set/ValueObject/../../../config/set/{v}.php",
            root84.display()
        )
    };
    let config84 = RectorConfig {
        sets: vec![level("php83"), level("php84"), level("php85")],
        php_sets: true,
        ..RectorConfig::default()
    };
    assert_eq!(
        from_rector(&root84, &config84).sets,
        ["rector/php85", "rector/php-sets"]
    );
    let want: Vec<(&str, &str)> = r
        .want
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    assert_eq!(
        want,
        [
            (
                "lint/dead-code/no-unused-foreach-key",
                "RemoveUnusedForeachKeyRector"
            ),
            ("lint/refactor/rename-function", "RenameFunctionRector"),
            ("lint/dead-code/no-and-true", "RemoveAndTrueRector"),
            (
                "format/operators/standardize-not-equals",
                "CommonNotEqualRector"
            ),
        ]
    );
    let unsupported: Vec<(&str, &str)> = (r.unsupported.iter())
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    assert_eq!(
        unsupported,
        [
            (
                "MyRector",
                "not a Rector rule Pun knows (a custom rule or another package's)"
            ),
            (
                "ClassPropertyAssignToConstructorPromotionRector",
                "only in php52..php81 upgrade sets (below php82)"
            ),
        ]
    );
    assert_eq!(
        r.overrides,
        [(
            strings(&["tests", "app/Legacy.php"]),
            "lint/dead-code/no-and-true".to_string()
        )]
    );
    assert_eq!(r.exclude, ["legacy", "Fixtures/*", "*/Fixtures/*"]);
    assert_eq!(r.paths, ["app", "tests"]);
    let whole = RectorConfig {
        paths: vec![root.display().to_string(), "/elsewhere/src/".into()],
        ..RectorConfig::default()
    };
    assert_eq!(from_rector(&root, &whole).paths, [".", "/elsewhere/src"]);
}

/// A rule merged with a PHP-CS-Fixer fixer keeps to its Rector rules' checks when rector.php brings it in: Rector
/// 2.8.4's `SimplifyUselessVariableRector` skips a commented assignment and `StringClassNameToClassConstantRector` a
/// global name (`'Throwable'`), and `StrlenZeroToIdenticalEmptyStringRector` changes only `=== 0` and `> 0`, where
/// `return_assignment`, `class_keyword` and `string_length_to_empty` change all of them. `TernaryToNullCoalescingRector`
/// rewrites `$a !== null ? $a : $b`, which `ternary_to_null_coalescing` leaves.
#[test]
fn rector_rules_merged_with_a_fixer_keep_to_rector() {
    let config = RectorConfig {
        rules: strings(&[
            "Rector\\CodeQuality\\Rector\\FunctionLike\\SimplifyUselessVariableRector",
            "Rector\\Php55\\Rector\\String_\\StringClassNameToClassConstantRector",
            "Rector\\CodeQuality\\Rector\\Identical\\StrlenZeroToIdenticalEmptyStringRector",
            "Rector\\Php70\\Rector\\Ternary\\TernaryToNullCoalescingRector",
            FOREACH,
        ]),
        ..RectorConfig::default()
    };
    let r = from_rector(&dir(&[]), &config);
    let options: Vec<(&str, String)> = (r.options.iter())
        .map(|(id, t)| (id.as_str(), t.to_string().trim().to_string()))
        .collect();
    assert_eq!(
        options,
        [
            (
                "lint/control-flow/no-return-assignment",
                "skip-commented = true".to_string()
            ),
            (
                "lint/operators/prefer-coalesce",
                "null-checks = true".to_string()
            ),
            (
                "lint/style/prefer-class-constant",
                "namespaced-only = true\nchecks = [\"string-class-name\"]".to_string()
            ),
            (
                "lint/style/prefer-empty-string-compare",
                "identical-or-greater-only = true".to_string()
            ),
        ]
    );
}

/// A rule merged from several Rector rules keeps to the ones rector.php runs. The input is what
/// crates/pun-cli/src/rector_config.php printed, on Rector 2.8.5, for `withPhpSets(php84: true)` with three of the four
/// `ForeachToArray` rules in `withSkip()`; Rector then rewrites only the `array_find()` loop of a file with all four.
#[test]
fn merged_rector_rules_keep_to_the_ones_rector_runs() {
    let class = |name: &str| format!("Rector\\Php84\\Rector\\Foreach_\\ForeachToArray{name}Rector");
    let rules: Vec<String> = ["Find", "FindKey", "All", "Any"].map(class).into();
    let options = |skipped: &[&str]| {
        let config = RectorConfig {
            rules: rules.clone(),
            skip_rules: skipped.iter().map(|s| (class(s), Value::Null)).collect(),
            php_sets: true,
            ..RectorConfig::default()
        };
        let r = from_rector(&dir(&[]), &config);
        (r.options.get("lint/php-8.4/prefer-array-find")).map(|t| t.to_string().trim().to_string())
    };
    assert_eq!(
        options(&["All", "Any", "FindKey"]).as_deref(),
        Some("functions = [\"array-find\"]")
    );
    assert_eq!(
        options(&["Any", "Find"]).as_deref(),
        Some("functions = [\"array-all\", \"array-find-key\"]")
    );
    assert_eq!(options(&[]), None);
    // `withPreparedSets(deadCode: true)`: of no-useless-ternary's and prefer-simple-condition's three Rector rules
    // each, Rector 2.8.5 resolves only RemoveUselessTernaryRector and SimplifyBoolIdenticalTrueRector; the others are
    // in its code-quality set.
    let config = RectorConfig {
        rules: strings(&[
            "Rector\\DeadCode\\Rector\\Ternary\\RemoveUselessTernaryRector",
            "Rector\\CodeQuality\\Rector\\Identical\\SimplifyBoolIdenticalTrueRector",
        ]),
        sets: vec![
            "/p/vendor/rector/rector/src/Set/ValueObject/../../../config/set/dead-code.php".into(),
        ],
        ..RectorConfig::default()
    };
    let r = from_rector(&dir(&[]), &config);
    assert_eq!(r.sets, ["rector/dead-code"]);
    assert_eq!(
        r.options["lint/dead-code/no-useless-ternary"]
            .to_string()
            .trim(),
        "checks = [\"useless\"]"
    );
    assert_eq!(
        r.options["lint/complexity/prefer-simple-condition"]
            .to_string()
            .trim(),
        "checks = [\"identical-true\"]"
    );
}

/// The rules merged from several Rector rules, each with its Rector rules (under `Rector\`, or a full name after
/// `@`) and the `checks` that keep it to all of them but the first.
const MERGED: [(&str, &str, &str); 13] = [
    (
        "lint/complexity/no-negated-group",
        "CodeQuality\\Rector\\BooleanNot\\NegatedAndsToPositiveOrs CodeQuality\\Rector\\BooleanNot\\SimplifyDeMorganBinary",
        "[\"de-morgan\"]",
    ),
    (
        "lint/complexity/no-redundant-null-check",
        "CodeQuality\\Rector\\If_\\SimplifyIfNotNullReturn CodeQuality\\Rector\\If_\\SimplifyIfNullableReturn",
        "[\"nullable\"]",
    ),
    (
        "lint/complexity/prefer-empty-array-compare",
        "CodeQuality\\Rector\\Empty_\\SimplifyEmptyCheckOnEmptyArray CodeQuality\\Rector\\BooleanAnd\\SimplifyEmptyArrayCheck",
        "[\"is-array\"]",
    ),
    (
        "lint/complexity/prefer-in-array",
        "CodeQuality\\Rector\\Identical\\SimplifyArraySearch CodeQuality\\Rector\\Foreach_\\ForeachToInArray CodeQuality\\Rector\\BooleanOr\\RepeatedOrEqualToInArray",
        "[\"foreach\", \"repeated-or\"]",
    ),
    (
        "lint/complexity/prefer-null-coalesce",
        "CodeQuality\\Rector\\Ternary\\ArrayKeyExistsTernaryThenValueToCoalescing CodeQuality\\Rector\\If_\\ConsecutiveNullCompareReturnsToNullCoalesceQueue CodeQuality\\Rector\\Foreach_\\SimplifyForeachToCoalescing",
        "[\"consecutive-returns\", \"foreach\"]",
    ),
    (
        "lint/dead-code/no-always-true-instanceof",
        "DeadCode\\Rector\\StmtsAwareInterface\\RemoveDeadInstanceOfAssert DeadCode\\Rector\\If_\\RemoveDeadInstanceOf",
        "[\"if\"]",
    ),
    (
        "lint/dead-code/no-empty-block",
        "DeadCode\\Rector\\For_\\RemoveDeadIfForeachFor DeadCode\\Rector\\For_\\RemoveDeadLoop",
        "[\"loop\"]",
    ),
    (
        "lint/dead-code/no-redundant-default-argument",
        "DeadCode\\Rector\\MethodCall\\RemoveNullArgOnNullDefaultParam DeadCode\\Rector\\MethodCall\\RemoveNullNamedArgOnNullDefaultParam DeadCode\\Rector\\ClassMethod\\RemoveArgumentFromDefaultParentCall",
        "[\"null-named\", \"parent-call\"]",
    ),
    (
        "lint/dead-code/no-unused-assignment",
        "DeadCode\\Rector\\Assign\\RemoveDoubleAssign DeadCode\\Rector\\Assign\\RemoveUnusedVariableAssign",
        "[\"unused\"]",
    ),
    (
        "lint/dead-code/no-unused-private-method-param",
        "DeadCode\\Rector\\ClassMethod\\RemoveTestsOverriddenPrivateMethodParameter DeadCode\\Rector\\ClassMethod\\RemoveUnusedPrivateMethodParameter",
        "[\"unused\"]",
    ),
    (
        "lint/phpunit/prefer-will-return-callback",
        "PHPUnit\\CodeQuality\\Rector\\MethodCall\\MergeWithCallableAndWillReturn PHPUnit\\CodeQuality\\Rector\\Class_\\VoidMethodWithCallbackToWillReturnCallback",
        "[\"void-method\"]",
    ),
    (
        "lint/style/no-sleep-wakeup",
        "Php85\\Rector\\Class_\\SleepToSerialize Php85\\Rector\\Class_\\WakeupToUnserialize",
        "[\"wakeup\"]",
    ),
    (
        "lint/pest/prefer-to-throw",
        "@Pest\\Rector\\Rules\\ConvertExpectExceptionToThrow @Pest\\Rector\\Rules\\UseToThrow",
        "[\"try-catch\"]",
    ),
];

/// Every other rule merged from several Rector rules keeps to the ones rector.php runs, as its `checks`: rector.php has
/// the rule's Rector rules in `withRules()` and the first in `withSkip()`, which crates/pun-cli/src/rector_config.php
/// prints as a skip with no paths. The tests of pun-lint (`tests/merged_checks.rs`) hold what Rector 2.8.4 writes then.
#[test]
fn every_merged_rector_rule_keeps_to_the_ones_rector_runs() {
    for (id, classes, checks) in MERGED {
        let class = |c: &str| match c.strip_prefix('@') {
            Some(c) => format!("{c}Rector"),
            None => format!("Rector\\{c}Rector"),
        };
        let rules: Vec<String> = classes.split(' ').map(class).collect();
        let config = RectorConfig {
            skip_rules: [(rules[0].clone(), Value::Null)].into_iter().collect(),
            rules,
            ..RectorConfig::default()
        };
        let r = from_rector(&dir(&[]), &config);
        assert_eq!(
            r.options[id].to_string().trim(),
            format!("checks = {checks}"),
            "{id}"
        );
        let all = RectorConfig {
            rules: classes.split(' ').map(class).collect(),
            ..RectorConfig::default()
        };
        assert!(
            !from_rector(&dir(&[]), &all).options.contains_key(id),
            "{id}"
        );
    }
}

/// `withPhpSets()` brings in `StringClassNameToClassConstantRector` (Rector's php55 set) but not
/// `UseClassKeywordForClassNameResolutionRector` (its coding-style set only): crates/pun-cli/src/rector_config.php
/// prints the first and not the second for a rector.php with only `withPhpSets()` on Rector 2.8.4, which then rewrites
/// `'App\User'` and leaves `'App\User::method'` alone. The coding-style set and `withRules()` with both run both.
#[test]
fn prefer_class_constant_keeps_to_the_rector_rules_that_run() {
    const STRING: &str = "Rector\\Php55\\Rector\\String_\\StringClassNameToClassConstantRector";
    const KEYWORD: &str =
        "Rector\\CodingStyle\\Rector\\String_\\UseClassKeywordForClassNameResolutionRector";
    let options = |rules: &[&str], php_sets: bool| {
        let config = RectorConfig {
            rules: strings(rules),
            php_sets,
            ..RectorConfig::default()
        };
        let r = from_rector(&dir(&[]), &config);
        r.options["lint/style/prefer-class-constant"].to_string()
    };
    assert_eq!(
        options(&[STRING], true),
        "namespaced-only = true\nchecks = [\"string-class-name\"]\n"
    );
    assert_eq!(
        options(&[KEYWORD], false),
        "namespaced-only = true\nchecks = [\"class-name-resolution\"]\n"
    );
    assert_eq!(
        options(&[STRING, KEYWORD], false),
        "namespaced-only = true\n"
    );
}

/// A configured rule's configuration becomes its options. The input is what crates/pun-cli/src/rector_config.php printed
/// for a rector.php with these four `withConfiguredRule()` calls, run on .oracle/rector-ecosystem's Rector: a data map
/// keeps its keys, a value object list goes under `items` with its properties kebab-cased, Rector's option constant
/// `exclude_methods` is kebab-cased. What TOML can't hold, or a key pun.toml keeps for the level, is named.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one test walks every case of the behaviour it checks"
)]
fn carries_rule_configurations() {
    let root = dir(&[]);
    let printed = json!({"Rector\\Renaming\\Rector\\FuncCall\\RenameFunctionRector": {"configuration": {"split": "explode", "join": "implode"}, "constants": []}, "Rector\\Removing\\Rector\\FuncCall\\RemoveFuncCallArgRector": {"configuration": [{"@props": {"function": "ldap_first_attribute", "argumentPosition": 2}}], "constants": []}, "RectorLaravel\\Rector\\PropertyFetch\\OptionalToNullsafeOperatorRector": {"configuration": {"exclude_methods": ["present"]}, "constants": ["exclude_methods"]}, "Rector\\Renaming\\Rector\\ConstFetch\\RenameConstantRector": {"configuration": {"OLD": "NEW"}, "constants": []}});
    let mut configured: BTreeMap<String, Configured> = serde_json::from_value(printed).unwrap();
    let by_hand = [
        (
            "Rector\\Renaming\\Rector\\Cast\\RenameCastRector",
            json!([{"@props": {"fromCastKind": 3, "scope": null, "strict": true, "ratio": 0.5}}]),
        ),
        (
            "Rector\\Renaming\\Rector\\Name\\RenameClassRector",
            json!({"Old": null}),
        ),
        (
            "RectorLaravel\\Rector\\FuncCall\\FuncCallToStaticCallRector",
            json!([{"@unsupported": "Closure"}]),
        ),
        (
            "Rector\\Renaming\\Rector\\ClassConstFetch\\RenameClassConstFetchRector",
            json!({"level": "x"}),
        ),
    ];
    for (class, configuration) in &by_hand {
        configured.insert(
            class.to_string(),
            Configured {
                configuration: configuration.clone(),
                constants: vec![],
            },
        );
    }
    let config = RectorConfig {
        rules: configured.keys().cloned().collect(),
        configured,
        ..RectorConfig::default()
    };
    let r = from_rector(&root, &config);
    let options: Vec<(&str, String)> = (r.options.iter())
        .map(|(id, t)| (id.as_str(), toml::Value::Table(t.clone()).to_string()))
        .collect();
    assert_eq!(
        options,
        [
            (
                "lint/laravel/prefer-nullsafe",
                "{ exclude-methods = [\"present\"] }".to_string()
            ),
            (
                "lint/refactor/remove-function-argument",
                "{ items = [{ function = \"ldap_first_attribute\", argument-position = 2 }] }"
                    .into()
            ),
            (
                "lint/refactor/rename-cast",
                "{ items = [{ from-cast-kind = 3, strict = true, ratio = 0.5 }] }".into()
            ),
            ("lint/refactor/rename-constant", "{ OLD = \"NEW\" }".into()),
            (
                "lint/refactor/rename-function",
                "{ split = \"explode\", join = \"implode\" }".into()
            ),
        ]
    );
    let unsupported: Vec<(&str, &str)> = (r.unsupported.iter())
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    assert_eq!(
        unsupported,
        [
            (
                "FuncCallToStaticCallRector",
                "configured; its configuration holds a Closure, which pun.toml can't"
            ),
            (
                "RenameClassConstFetchRector",
                "configured; its configuration has a `level` key, which pun.toml keeps for the rule's level"
            ),
            (
                "RenameClassRector",
                "configured; its configuration holds a null, which pun.toml can't"
            ),
        ]
    );

    let rector = Rector {
        sets: vec!["rector/code-quality".into()],
        want: vec![
            (
                "lint/complexity/no-and-assignment".to_string(),
                "AndAssignsToSeparateLinesRector".to_string(),
            ),
            (
                "lint/refactor/rename-function".to_string(),
                "RenameFunctionRector".to_string(),
            ),
        ],
        options: BTreeMap::from([
            (
                "lint/complexity/no-and-assignment".to_string(),
                toml::Table::from_iter([("items".to_string(), toml::Value::Integer(1))]),
            ),
            (
                "lint/refactor/rename-function".to_string(),
                r.options["lint/refactor/rename-function"].clone(),
            ),
        ]),
        ..Rector::default()
    };
    let mut parts = Parts {
        format: "preset = \"empty\"\n".into(),
        ..Parts::default()
    };
    let done = compose(&root, &mut parts, &rector).unwrap();
    let text = parts.render();
    assert!(
        text.contains("\n\"complexity/no-and-assignment\" = { items = 1 }\n"),
        "{text}"
    );
    assert!(rule_meta_ported("lint/refactor/rename-function"));
    assert!(
        text.contains(
            "\n\"refactor/rename-function\" = { split = \"explode\", join = \"implode\" }\n"
        ),
        "{text}"
    );
    assert_eq!(done.on, 2);
    let cfg = from_toml(&root, &text).unwrap();
    let rule = cfg
        .lint
        .rules
        .iter()
        .find(|r| r.id == "lint/complexity/no-and-assignment");
    assert_eq!(rule.unwrap().options, json!({"items": 1}));
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one test walks every case of the behaviour it checks"
)]
fn composes_the_difference_with_the_sets() {
    let root = dir(&[]);
    let id = |s: &str| s.to_string();
    let rector = Rector {
        sets: vec!["rector/dead-code".into()],
        want: vec![
            (
                id("lint/dead-code/no-unused-foreach-key"),
                id("RemoveUnusedForeachKeyRector"),
            ),
            (
                id("lint/complexity/no-compact"),
                id("CompactToVariablesRector"),
            ),
            (
                id("lint/complexity/no-and-assignment"),
                id("AndAssignsToSeparateLinesRector"),
            ),
            (
                id("lint/imports/no-unused"),
                id("RemoveUnusedImportsRector"),
            ),
        ],
        overrides: vec![
            (
                strings(&["tests"]),
                id("lint/dead-code/no-unused-foreach-key"),
            ),
            (strings(&["tests"]), id("lint/complexity/no-compact")),
            (strings(&["app/A.php"]), id("lint/complexity/no-compact")),
        ],
        exclude: strings(&["legacy"]),
        ..Rector::default()
    };
    let mut parts = Parts {
        format: "preset = \"empty\"\n".into(),
        lint_rules: vec![(id("imports/no-unused"), id("\"error\""))],
        ..Parts::default()
    };
    let done = compose(&root, &mut parts, &rector).unwrap();
    let text = parts.render();
    assert!(
        text.starts_with(
            "[format]\npreset = \"empty\"\n\n[lint]\nsets = [\"rector/dead-code\"]\nexclude = [\"legacy\"]\n\n[lint.rules]\n\"imports/no-unused\" = \"error\"\n"
        ),
        "{text}"
    );
    assert!(
        text.ends_with(
            "\n[[lint.overrides]]\nfiles = [\"tests\"]\nrules = { \"dead-code/no-unused-foreach-key\" = \"off\", \"complexity/no-compact\" = \"off\" }\n\n[[lint.overrides]]\nfiles = [\"app/A.php\"]\nrules = { \"complexity/no-compact\" = \"off\" }\n"
        ),
        "{text}"
    );
    let cfg = from_toml(&root, &text).unwrap();
    let on: BTreeSet<&str> = cfg.lint.rules.iter().map(|r| r.id.as_str()).collect();
    let ported = |id: &str| rule_meta_ported(id);
    let mut expect: BTreeSet<&str> = [
        "lint/dead-code/no-unused-foreach-key",
        "lint/imports/no-unused",
        "lint/complexity/no-and-assignment",
    ]
    .into();
    assert!(text.contains("\n\"complexity/no-and-assignment\" = \"error\"\n"));
    if ported("lint/complexity/no-compact") {
        expect.insert("lint/complexity/no-compact");
        assert_eq!((done.on, done.unsupported.len()), (2, 0));
    } else {
        assert_eq!(done.on, 1);
        assert_eq!(
            done.unsupported,
            [(
                id("CompactToVariablesRector"),
                id("not supported by Pun yet")
            )]
        );
    }
    assert_eq!(on, expect);
    assert_eq!(done.off, text.matches("= \"off\"\n").count());
    assert!(done.off > 0);

    let not_equal = Rector {
        want: vec![(
            id("format/operators/standardize-not-equals"),
            id("CommonNotEqualRector"),
        )],
        ..Rector::default()
    };
    for (preset, unsupported) in [
        ("laravel", vec![]),
        (
            "empty",
            vec![(
                id("CommonNotEqualRector"),
                id(
                    "the format rule `format/operators/standardize-not-equals` in Pun; turn it on in [format.rules]",
                ),
            )],
        ),
    ] {
        let mut parts = Parts {
            format: format!("preset = \"{preset}\"\n"),
            ..Parts::default()
        };
        let done = compose(&root, &mut parts, &not_equal).unwrap();
        assert_eq!((done.on, done.unsupported), (0, unsupported));
        assert!(!parts.render().contains("[lint]\n"), "{}", parts.render());
        assert!(
            !parts.render().contains("standardize"),
            "{}",
            parts.render()
        );
    }
}

/// Issue #15: `RemoveDeadInstanceOfRector` and `RemoveDeadInstanceOfAssertRector` are both
/// `lint/dead-code/no-always-true-instanceof`; skipped for the same paths, the override names it once, or pun.toml has
/// a duplicate key and doesn't parse.
#[test]
fn rules_merged_into_one_are_skipped_once_per_override() {
    let root = dir(&[]);
    let tests = format!("{}/tests/*", root.display());
    let config = RectorConfig {
        rules: strings(&[
            "Rector\\DeadCode\\Rector\\If_\\RemoveDeadInstanceOfRector",
            "Rector\\DeadCode\\Rector\\StmtsAwareInterface\\RemoveDeadInstanceOfAssertRector",
        ]),
        skip_rules: serde_json::from_value(json!({
            "Rector\\DeadCode\\Rector\\If_\\RemoveDeadInstanceOfRector": [tests],
            "Rector\\DeadCode\\Rector\\StmtsAwareInterface\\RemoveDeadInstanceOfAssertRector": [tests],
        }))
        .unwrap(),
        ..RectorConfig::default()
    };
    let rector = from_rector(&root, &config);
    let mut parts = Parts {
        format: "preset = \"empty\"\n".into(),
        ..Parts::default()
    };
    compose(&root, &mut parts, &rector).unwrap();
    assert_eq!(
        parts.overrides,
        "[[lint.overrides]]\nfiles = [\"tests/*\"]\nrules = { \"dead-code/no-always-true-instanceof\" = \"off\" }\n"
    );
    let cfg = from_toml(&root, &parts.render()).unwrap();
    assert_eq!(cfg.lint.overrides.len(), 1);
}

/// Issue #176: Rector fnmatch()es `*/tests/*` against the absolute path, so it skips the top-level `tests/` as well as
/// `packages/billing/tests/` (Rector 2.9.2 changes neither file); Pun's override matches the root-relative path.
#[test]
fn a_leading_star_slash_skip_also_covers_the_top_level_directory() {
    let root = dir(&[]);
    let config = RectorConfig {
        rules: strings(&[
            "Rector\\TypeDeclaration\\Rector\\ArrowFunction\\AddArrowFunctionReturnTypeRector",
        ]),
        skip_rules: serde_json::from_value(json!({
            "Rector\\TypeDeclaration\\Rector\\ArrowFunction\\AddArrowFunctionReturnTypeRector": ["*/tests/*", "**/stubs/*", "*"],
        }))
        .unwrap(),
        ..RectorConfig::default()
    };
    assert_eq!(
        from_rector(&root, &config).overrides,
        [(
            strings(&["tests/*", "*/tests/*", "**/stubs/*", "*"]),
            "lint/types/add-arrow-function-return-type".to_string()
        )]
    );
}

fn rule_meta_ported(id: &str) -> bool {
    (rules().iter().find(|(r, _)| r == id))
        .and_then(|(_, e)| e.get("ported"))
        .and_then(toml::Value::as_bool)
        == Some(true)
}
