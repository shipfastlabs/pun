use super::*;
use crate::migrate::Parts;
use crate::migrate::phpstan::{Phpstan, compose, from_phpstan};

fn project(neon: &str, extra: &[(&str, &str)]) -> PathBuf {
    let d = dir(&[]);
    for p in [
        "app/Legacy",
        "tests",
        "stubs",
        "vendor/larastan/larastan",
        "vendor/foo/bar",
        "build",
    ] {
        std::fs::create_dir_all(d.join(p)).unwrap();
    }
    std::fs::create_dir_all(d.join("vendor/phpstan/phpstan/conf")).unwrap();
    std::fs::write(
        d.join("vendor/phpstan/phpstan/conf/bleedingEdge.neon"),
        "includes:\n\t- phar://phpstan.phar/conf/bleedingEdge.neon\n",
    )
    .unwrap();
    std::fs::write(d.join("phpstan.neon"), neon).unwrap();
    std::fs::create_dir_all(d.join("app/Models")).unwrap();
    std::fs::write(d.join("app/Models/User.php"), "<?php\n").unwrap();
    for (n, c) in extra {
        std::fs::create_dir_all(d.join(n).parent().unwrap()).unwrap();
        std::fs::write(d.join(n), c).unwrap();
    }
    d
}

const NEON: &str = "includes:
    - vendor/larastan/larastan/extension.neon
    - vendor/phpstan/phpstan/conf/bleedingEdge.neon
    - phpstan-baseline.neon
    - build/custom.neon
    - vendor/phpstan/phpstan/conf/extra.neon
parameters:
    level: 5
    paths:
        - app
        - tests/
    excludePaths:
        - app/Legacy
        - /tmp
    phpVersion: 80400
    tmpDir: build/phpstan
    checkUninitializedProperties: true
    checkMissingTypehints: true
    treatPhpDocTypesAsCertain: true
    universalObjectCratesClasses: [App\\Bag]
    checkBenevolentUnionTypes: true
    additionalConstructors: [App\\Bag::setUp]
    exceptions:
        implicitThrows: false
    stubFiles:
        - stubs/Foo.stub
    scanFiles:
        - stubs/debugbar.php
    scanDirectories:
        - app/Legacy
    bootstrapFiles:
        - tests/bootstrap.php
    ignoreErrors:
        - '#^Call to an undefined method#'
        -
            identifier: method.notFound
            path: app/Models/User.php
            count: 2
        -
            message: '#Access#'
            identifiers: [property.notFound, number.alwaysTrue]
            paths: [app/*]
        -
            rawMessage: 'Exact.'
            identifier: no.suchIdentifier
        -
            identifier: whitespace.bom
        -
            identifier: phpstanApi.method
";

const GENERATED: &str = "<?php declare(strict_types = 1);

namespace PHPStan\\ExtensionInstaller;

final class GeneratedConfig
{

\tpublic const EXTENSIONS = array (
  'foo/bar' => 
  array (
    'install_path' => '/p/vendor/foo/bar',
    'relative_install_path' => '../../../foo/bar',
    'extra' => NULL,
    'version' => '1.0.0',
  ),
  'nesbot/carbon' => 
  array (
    'install_path' => '/p/vendor/nesbot/carbon',
    'relative_install_path' => '../../../nesbot/carbon',
    'extra' => 
    array (
      'includes' => 
      array (
        0 => 'extension.neon',
      ),
    ),
    'version' => '3.15.1',
  ),
  'phpstan/phpstan-webmozart-assert' => 
  array (
    'install_path' => '/p/vendor/phpstan/phpstan-webmozart-assert',
    'relative_install_path' => '../../phpstan-webmozart-assert',
    'extra' => 
    array (
      'includes' => 
      array (
        0 => 'extension.neon',
      ),
    ),
    'version' => '2.0.0',
  ),
  'phpstan/phpstan-strict-rules' => 
  array (
    'install_path' => '/p/vendor/phpstan/phpstan-strict-rules',
    'relative_install_path' => '../../phpstan-strict-rules',
    'extra' => 
    array (
      'includes' => 
      array (
        0 => 'rules.neon',
      ),
    ),
    'version' => '2.0.7',
  ),
);

\tpublic const NOT_INSTALLED = array (
  'phpstan/phpstan-phpunit' => 
  array (
  ),
);

}
";

/// Issue #144: PHPStan reads stubFiles and scanFiles under vendor/, such as php-stubs/wordpress-stubs, so they are
/// migrated; a built-in extension's own (Larastan's) come back with the extension.
#[test]
fn migrates_vendor_stubs_but_not_a_built_in_extensions() {
    let neon = "includes:\n    - vendor/larastan/larastan/extension.neon\nparameters:\n    level: 5\n    stubFiles:\n        - vendor/php-stubs/wordpress-stubs/wordpress-stubs.php\n    scanFiles:\n        - vendor/php-stubs/acf-pro-stubs/acf-pro-stubs.php\n";
    let larastan = "parameters:\n    stubFiles:\n        - stubs/Model.stub\n    scanFiles:\n        - stubs/Helpers.php\n";
    let d = project(
        neon,
        &[("vendor/larastan/larastan/extension.neon", larastan)],
    );
    let p = from_phpstan(&d, &d.join("phpstan.neon")).unwrap();
    assert_eq!(
        p.stubs,
        ["vendor/php-stubs/wordpress-stubs/wordpress-stubs.php"]
    );
    assert_eq!(p.scan, ["vendor/php-stubs/acf-pro-stubs/acf-pro-stubs.php"]);
}

#[test]
fn reads_level_paths_ignores_and_includes() {
    let d = project(
        NEON,
        &[
            ("vendor/larastan/larastan/extension.neon", ""),
            ("vendor/foo/bar/rules.neon", ""),
            ("vendor/phpstan/phpstan/conf/extra.neon", ""),
            ("vendor/phpstan/phpstan-webmozart-assert/extension.neon", ""),
            ("vendor/nesbot/carbon/extension.neon", ""),
            ("vendor/phpstan/phpstan-strict-rules/rules.neon", ""),
            (
                "build/custom.neon",
                "includes:\n    - ../vendor/foo/bar/rules.neon\nparameters:\n    checkFunctionNameCase: true\n",
            ),
            (
                "phpstan-baseline.neon",
                "parameters:\n    ignoreErrors:\n        - '#x#'\n",
            ),
        ],
    );
    let installer = d.join("vendor/phpstan/extension-installer/src");
    std::fs::create_dir_all(&installer).unwrap();
    let generated = GENERATED.replace("'/p/", &format!("'{}/", d.display()));
    std::fs::write(installer.join("GeneratedConfig.php"), generated).unwrap();
    let p = from_phpstan(&d, &d.join("phpstan.neon")).unwrap();
    assert_eq!(p.level, 5);
    assert_eq!(p.paths, ["app", "tests"]);
    assert_eq!(p.exclude, ["app/Legacy", "/tmp"]);
    assert_eq!(p.stubs, ["stubs/Foo.stub"]);
    assert_eq!(p.scan, ["stubs/debugbar.php", "app/Legacy"]);
    assert_eq!(p.bootstrap, ["tests/bootstrap.php"]);
    assert_eq!(p.baselines, ["phpstan-baseline.neon"]);
    assert_eq!(
        p.extensions.iter().collect::<Vec<_>>(),
        [
            "larastan/larastan",
            "nesbot/carbon",
            "phpstan/phpstan-strict-rules"
        ]
    );
    assert!(p.bleeding_edge);
    assert_eq!(p.php_version.as_deref(), Some("8.4"));
    let names: Vec<&str> = p.unsupported.iter().map(|u| u.0.as_str()).collect();
    assert_eq!(
        names,
        [
            "build/custom.neon",
            "foo/bar",
            "phpstan/phpstan-webmozart-assert",
            "parameter additionalConstructors",
            "parameter checkBenevolentUnionTypes",
            "parameter checkMissingTypehints",
            "parameter universalObjectCratesClasses",
            "ignoreErrors no.suchIdentifier",
            "ignoreErrors whitespace.bom",
            "ignoreErrors phpstanApi.method",
        ]
    );
    let why = |n: &str| &p.unsupported.iter().find(|u| u.0 == n).unwrap().1;
    assert_eq!(
        why("ignoreErrors whitespace.bom"),
        "`whitespace.bom` is the format rule `format/whitespace/encoding` in Pun; ignore it there"
    );
    assert_eq!(
        p.changed,
        [
            (
                "checkFunctionNameCase".to_string(),
                pun_phpstan::Value::Bool(true)
            ),
            (
                "exceptions.implicitThrows".to_string(),
                pun_phpstan::Value::Bool(false)
            ),
        ]
    );
    assert_eq!(
        why("parameter checkMissingTypehints"),
        "the level sets it; Pun's analyse presets decide it"
    );
    assert_eq!(
        why("parameter additionalConstructors"),
        "no pun.toml setting for this parameter yet"
    );
    assert_eq!(
        why("parameter universalObjectCratesClasses"),
        "no pun.toml setting for this parameter yet"
    );
    assert_eq!(
        why("ignoreErrors phpstanApi.method"),
        "`phpstanApi.method`: PHPStan extension-API checks; Pun has no PHP extension API"
    );
    assert_eq!(
        p.ignores,
        [
            "[[analyse.ignore]]\nmessage = \"#^Call to an undefined method#\"\n",
            "[[analyse.ignore]]\nrule = \"undefined/method\"\nidentifier = \"method.notFound\"\npaths = [\"app/Models/User.php\"]\ncount = 2\n",
            "[[analyse.ignore]]\nrule = \"undefined/property\"\nidentifier = \"property.notFound\"\nmessage = \"#Access#\"\npaths = [\"app/*\"]\n",
            "[[analyse.ignore]]\nrule = \"dead-code/number-comparison-always-constant\"\nidentifier = \"number.alwaysTrue\"\nmessage = \"#Access#\"\npaths = [\"app/*\"]\n",
        ]
    );
    let mut parts = Parts::default();
    let done = compose(&d, &mut parts, &p).unwrap();
    let text = parts.render();
    for line in [
        "\"undefined/function\" = { check-function-name-case = true }",
        "\"undefined/method\" = { check-function-name-case = true }",
    ] {
        assert!(text.contains(&format!("\n{line}\n")), "{line}\n{text}");
    }
    // PHPStan's opt-in throw-type covariance is not in the level preset, so nothing turns it off.
    assert!(!text.contains("throws-not-covariant"), "{text}");
    assert_eq!(done.options, 1);
    let cfg = crate::from_toml(&d, &text).unwrap();
    assert!(cfg.analyse.bleeding_edge);
    let function = cfg
        .analyse
        .rules
        .iter()
        .find(|r| r.id == "analyse/undefined/function");
    assert_eq!(
        function.unwrap().options,
        serde_json::json!({"check-function-name-case": true})
    );
    assert!(
        parts.render().contains(
            "[[lint.overrides]]\nfiles = [\"app\", \"tests\"]\nrules = { \"complexity/no-redundant-nullsafe\" = \"error\", "
        ) && parts.render().contains(
            "[[lint.overrides]]\nfiles = [\"app/Legacy\", \"/tmp\"]\nrules = { \"complexity/no-redundant-nullsafe\" = \"off\", "
        ) && parts.render().starts_with("[php]\nversion = \"8.4\"\n\n")
            && parts.render().contains(
            "\n\n[analyse]\npreset = \"phpstan/level_5\"\npaths = [\"app\", \"tests\"]\nexclude = [\"app/Legacy\", \"/tmp\"]\nstubs = [\"stubs/Foo.stub\"]\nscan = [\"stubs/debugbar.php\", \"app/Legacy\"]\nbootstrap = [\"tests/bootstrap.php\"]\nbleeding-edge = true\n"
        ),
        "{}",
        parts.render()
    );
}

#[test]
fn turns_on_the_extensions_phpstan_loads() {
    let neon = "includes:\n    - vendor/phpstan/phpstan-mockery/extension.neon\n    - vendor/tomasvotruba/type-coverage/config/extension.neon\nparameters:\n    level: 5\n";
    let d = project(
        neon,
        &[
            ("vendor/phpstan/phpstan-mockery/extension.neon", ""),
            (
                "vendor/tomasvotruba/type-coverage/config/extension.neon",
                "",
            ),
        ],
    );
    let p = from_phpstan(&d, &d.join("phpstan.neon")).unwrap();
    assert!(p.unsupported.is_empty(), "{:?}", p.unsupported);
    // What composer.lock turned on: carbon stays on, and disallowed-calls goes off, as PHPStan doesn't load it.
    let mut parts = Parts {
        extensions: "carbon = \"on\"\ndisallowed-calls = \"on\"\n".into(),
        ..Parts::default()
    };
    compose(&d, &mut parts, &p).unwrap();
    assert_eq!(
        parts.extensions,
        "carbon = \"on\"\nmockery = \"on\"\ndisallowed-calls = \"off\"\ntype-coverage = \"on\"\n"
    );
    let cfg = crate::from_toml(&d, &parts.render()).unwrap();
    let on: Vec<&str> = cfg.extensions.iter().map(|e| e.id).collect();
    assert_eq!(on, ["carbon", "mockery", "type-coverage"]);
}

#[test]
fn migrates_mockery_union_conversion() {
    // Pun always converts, so `true` needs no pun.toml line and `false` is the one it can't do.
    let run = |value: bool| {
        let neon = format!(
            "includes:\n    - vendor/phpstan/phpstan-mockery/extension.neon\nparameters:\n    level: 5\n    mockery:\n        convertUnionToIntersectionType: {value}\n"
        );
        let d = project(
            &neon,
            // phpstan-mockery 2.0.0's extension.neon, its parameters only
            &[(
                "vendor/phpstan/phpstan-mockery/extension.neon",
                "parameters:\n    mockery:\n        convertUnionToIntersectionType: true\nparametersSchema:\n    mockery: structure([\n        convertUnionToIntersectionType: bool()\n    ])\n",
            )],
        );
        from_phpstan(&d, &d.join("phpstan.neon"))
            .unwrap()
            .unsupported
    };
    assert!(run(true).is_empty(), "{:?}", run(true));
    assert_eq!(
        run(false),
        [(
            "parameter mockery.convertUnionToIntersectionType".to_string(),
            "Pun's mockery extension always reads `MockInterface|Foo` as `MockInterface&Foo`"
                .to_string()
        )]
    );
}

#[test]
fn migrates_maybe_undefined_variables() {
    use pun_phpstan::Value as V;
    for level in [0, 1] {
        for explicit in [None, Some(false), Some(true)] {
            let option = explicit.map_or_else(String::new, |value| {
                format!("    checkMaybeUndefinedVariables: {value}\n")
            });
            let neon = format!("parameters:\n    level: {level}\n{option}");
            let d = project(&neon, &[]);
            let p = from_phpstan(&d, &d.join("phpstan.neon")).unwrap();
            let expected = explicit.unwrap_or(level >= 1);
            let changed = explicit.is_some_and(|value| value != (level >= 1));
            let changes = if changed {
                vec![(
                    "checkMaybeUndefinedVariables".to_string(),
                    V::Bool(expected),
                )]
            } else {
                vec![]
            };
            assert_eq!(p.changed, changes, "{neon}");
            assert!(p.unsupported.is_empty(), "{neon}: {:?}", p.unsupported);
            let mut parts = Parts::default();
            let done = compose(&d, &mut parts, &p).unwrap();
            assert_eq!(done.options, usize::from(changed), "{neon}");
            let text = parts.render();
            let a = crate::from_toml(&d, &text).unwrap().analyse;
            assert_eq!(a.level(), level, "{text}");
            for consumer in [
                "analyse/undefined/variable",
                "analyse/undefined/compact-variable",
            ] {
                let rule = a.rules.iter().find(|rule| rule.id == consumer).unwrap();
                let options = if changed {
                    serde_json::json!({"maybe": expected})
                } else {
                    serde_json::Value::Null
                };
                assert_eq!(rule.options, options, "{neon}: {consumer}\n{text}");
            }
            let parameters = a.parameters(&d);
            assert_eq!(
                parameters.get_str("checkMaybeUndefinedVariables"),
                Some(&V::Bool(expected)),
                "{neon}\n{text}"
            );
            assert_eq!(
                parameters.get_str("cliArgumentsVariablesRegistered"),
                Some(&V::Bool(true)),
                "{neon}\n{text}"
            );
            std::fs::remove_dir_all(d).unwrap();
        }
    }
}

#[test]
fn argument_settings_preserve_defaults_and_report_unmapped_overrides() {
    use pun_phpstan::Value as V;

    for level in [0, 5] {
        for types in [false, true] {
            for references in [false, true] {
                for this_only in [false, true] {
                    let neon = format!(
                        "parameters:\n    level: {level}\n    checkFunctionArgumentTypes: {types}\n    checkArgumentsPassedByReference: {references}\n    checkThisOnly: {this_only}\n"
                    );
                    let d = project(&neon, &[]);
                    let p = from_phpstan(&d, &d.join("phpstan.neon")).unwrap();
                    let settings = [
                        ("checkFunctionArgumentTypes", types, level >= 5),
                        ("checkArgumentsPassedByReference", references, level >= 5),
                        ("checkThisOnly", this_only, level < 2),
                    ];
                    let mut unsupported: Vec<_> = settings.iter().filter(|(_, value, default)| value != default)
                        .map(|(name, _, _)| (format!("parameter {name}"), if *name == "checkThisOnly" {
                            "independent receiver eligibility has no pun.toml option yet".to_string()
                        } else {
                            "independent argument-check overrides have no pun.toml option yet".to_string()
                        })).collect();
                    unsupported.sort();
                    assert_eq!(p.unsupported, unsupported, "{neon}");
                    assert!(p.changed.is_empty(), "{neon}: {:?}", p.changed);
                    let mut parts = Parts::default();
                    compose(&d, &mut parts, &p).unwrap();
                    let text = parts.render();
                    let a = crate::from_toml(&d, &text).unwrap().analyse;
                    assert_eq!(a.level(), level, "{neon}\n{text}");
                    let parameters = a.parameters(&d);
                    for (name, _, default) in settings {
                        assert_eq!(
                            parameters.get_str(name),
                            Some(&V::Bool(default)),
                            "{neon}\n{text}"
                        );
                    }
                    std::fs::remove_dir_all(d).unwrap();
                }
            }
        }
    }
}

#[test]
fn reads_max_and_rejects_bad_levels() {
    let d = project("parameters:\n    level: max\n", &[]);
    assert_eq!(from_phpstan(&d, &d.join("phpstan.neon")).unwrap().level, 10);
    let d = project("parameters:\n    level: 11\n", &[]);
    let e = from_phpstan(&d, &d.join("phpstan.neon"))
        .unwrap_err()
        .to_string();
    assert!(
        e.starts_with("Level config file phar://")
            && e.ends_with("/conf/config.level11.neon was not found."),
        "{e}"
    );
}

/// PHPStan 2.3.6 on these entries, none matching, reports A, B, D and E as unmatched, and with
/// `reportUnmatchedIgnoredErrors: false` only B: an entry's own `reportUnmatched` wins, a bare pattern and each
/// expanded `messages` item take the global.
#[test]
fn carries_report_unmatched() {
    let entries = "    ignoreErrors:\n        - '#^A$#'\n        - message: '#^B$#'\n          reportUnmatched: true\n        - message: '#^C$#'\n          reportUnmatched: false\n        - messages: ['#^D$#', '#^E$#']\n";
    let ignores = |global: &str| {
        let neon = format!("parameters:\n    level: 0\n{global}{entries}");
        let d = project(&neon, &[]);
        from_phpstan(&d, &d.join("phpstan.neon")).unwrap().ignores
    };
    let off = "report-unmatched = false\n";
    assert_eq!(
        ignores(""),
        [
            "[[analyse.ignore]]\nmessage = \"#^A$#\"\n".to_string(),
            "[[analyse.ignore]]\nmessage = \"#^B$#\"\n".to_string(),
            format!("[[analyse.ignore]]\nmessage = \"#^C$#\"\n{off}"),
            "[[analyse.ignore]]\nmessage = \"#^D$#\"\n".to_string(),
            "[[analyse.ignore]]\nmessage = \"#^E$#\"\n".to_string(),
        ]
    );
    assert_eq!(
        ignores("    reportUnmatchedIgnoredErrors: false\n"),
        [
            format!("[[analyse.ignore]]\nmessage = \"#^A$#\"\n{off}"),
            "[[analyse.ignore]]\nmessage = \"#^B$#\"\n".to_string(),
            format!("[[analyse.ignore]]\nmessage = \"#^C$#\"\n{off}"),
            format!("[[analyse.ignore]]\nmessage = \"#^D$#\"\n{off}"),
            format!("[[analyse.ignore]]\nmessage = \"#^E$#\"\n{off}"),
        ]
    );
}

/// PHPStan 2.3.5 with `reportUnmatchedIgnoredErrors: false` reports no `ignore.unmatchedLine` for a
/// `// @phpstan-ignore-line` on a line without an error; with the default it does.
#[test]
fn migrates_report_unmatched_ignored_errors() {
    for (global, want) in [
        ("", true),
        ("    reportUnmatchedIgnoredErrors: false\n", false),
    ] {
        let d = project(&format!("parameters:\n    level: 0\n{global}"), &[]);
        let p = from_phpstan(&d, &d.join("phpstan.neon")).unwrap();
        let mut parts = Parts::default();
        compose(&d, &mut parts, &p).unwrap();
        let text = parts.render();
        let a = crate::from_toml(&d, &text).unwrap().analyse;
        assert_eq!(a.report_unmatched, want, "{text}");
        assert_eq!(
            a.parameters(&d).get_str("reportUnmatchedIgnoredErrors"),
            Some(&pun_phpstan::Value::Bool(want))
        );
        std::fs::remove_dir_all(d).unwrap();
    }
}

#[test]
fn takes_phar_bleeding_edge() {
    let d = project(
        "includes:\n    - phar://phpstan.phar/conf/bleedingEdge.neon\nparameters:\n    level: 0\n",
        &[],
    );
    let p = from_phpstan(&d, &d.join("phpstan.neon")).unwrap();
    assert!(p.bleeding_edge);
    assert!(p.unsupported.is_empty(), "{:?}", p.unsupported);
}

/// Issue #25: PHPStan 2.3.5 reads `phar://vendor/phpstan/phpstan/phpstan.phar/conf/bleedingEdge.neon` from the project
/// root, where PHP opens the relative archive path; a path that names no phar there is "missing or is not readable",
/// as PHPStan reports it.
#[test]
fn takes_bleeding_edge_from_a_relative_phar_path() {
    let neon = |archive: &str| {
        format!(
            "includes:\n    - phar://{archive}/conf/bleedingEdge.neon\nparameters:\n    level: 0\n"
        )
    };
    let d = project(&neon("vendor/phpstan/phpstan/phpstan.phar"), &[]);
    let p = from_phpstan(&d, &d.join("phpstan.neon")).unwrap();
    assert!(p.bleeding_edge);
    assert!(p.unsupported.is_empty(), "{:?}", p.unsupported);
    std::fs::write(d.join("phpstan.neon"), neon("vendor/phpstan.phar")).unwrap();
    let e = from_phpstan(&d, &d.join("phpstan.neon")).unwrap_err();
    assert_eq!(
        e.to_string(),
        "File 'phar://vendor/phpstan.phar/conf/bleedingEdge.neon' is missing or is not readable."
    );
    std::fs::remove_dir_all(d).unwrap();
}

/// PHPStan's config.level7.neon to config.level10.neon turn on checkUnionTypes, checkNullables, checkExplicitMixed
/// and checkImplicitMixed: a migrated level keeps the switches of its own level, not those of its preset's top level.
#[test]
fn migrates_the_type_switches_of_the_level() {
    for level in 0..=10u8 {
        let d = project(&format!("parameters:\n    level: {level}\n"), &[]);
        let p = from_phpstan(&d, &d.join("phpstan.neon")).unwrap();
        let mut parts = Parts::default();
        compose(&d, &mut parts, &p).unwrap();
        let text = parts.render();
        let a = crate::from_toml(&d, &text).unwrap().analyse;
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
        assert_eq!(a.switches, want, "{text}");
        assert_eq!(a.level(), level, "{text}");
        std::fs::remove_dir_all(d).unwrap();
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one test walks every case of the behaviour it checks"
)]
fn composes_the_level_extensions_and_parameters() {
    let at_in =
        |level: u8, ext: &[&str], strict: &[(&str, bool)], files: &[(&str, &str)], format: &str| {
            let d = project(&format!("parameters:\n    level: {level}\n"), files);
            let mut p: Phpstan = from_phpstan(&d, &d.join("phpstan.neon")).unwrap();
            p.extensions = ext.iter().map(std::string::ToString::to_string).collect();
            let mut rules = pun_phpstan::Array::new();
            for (k, v) in strict {
                rules.set(pun_phpstan::Key::from(*k), pun_phpstan::Value::Bool(*v));
            }
            p.parameters.set(
                pun_phpstan::Key::from("strictRules"),
                pun_phpstan::Value::Array(rules),
            );
            p.ignores =
                vec!["[[analyse.ignore]]\nrule = \"undefined/method\"\nmessage = \"#x#\"\n".into()];
            // As `pun migrate` does: [extensions] from composer.lock first, then [analyse] from phpstan.neon.
            let mut parts = Parts {
                format: format.into(),
                extensions: crate::migrate::extensions(&crate::composer::detect(&d).unwrap()),
                ..Parts::default()
            };
            let done = compose(&d, &mut parts, &p).unwrap();
            (done, parts, d)
        };
    let at = |level: u8, ext: &[&str], strict: &[(&str, bool)]| {
        at_in(level, ext, strict, &[("composer.json", "{}")], "")
    };
    let lines = |parts: &Parts, v: &str| -> Vec<String> {
        (parts.analyse_rules.iter())
            .filter(|(_, x)| x == v)
            .map(|(k, _)| k.clone())
            .collect()
    };
    // A plain level is its PHPStan preset, exactly: nothing to turn on or off.
    let (done, parts, d) = at(5, &[], &[]);
    assert_eq!(done.preset, AnalysePreset::PhpstanLevel(5));
    assert_eq!((done.on, done.off), (0, 0), "{:?}", parts.analyse_rules);
    let cfg = crate::from_toml(&d, &parts.render()).unwrap();
    assert_eq!(cfg.analyse.ignore.len(), 1);
    assert_eq!(
        cfg.analyse.ignore[0].rule.as_deref(),
        Some("analyse/undefined/method")
    );
    let has = |id: &str| cfg.analyse.rules.iter().any(|r| r.id == id);
    // deprecation rules are a separate package; uninitialized properties and bleeding-edge printf checks opt-in
    for id in [
        "analyse/deprecated/class",
        "analyse/classes/uninitialized-property",
        "analyse/types/printf-argument",
    ] {
        assert!(!has(id), "{id}");
    }
    assert!(has("analyse/types/random-int-range") && has("analyse/deprecated/to-string"));

    let (done, parts, _) = at(4, &[], &[]);
    assert_eq!(done.preset, AnalysePreset::PhpstanLevel(4));
    assert_eq!((done.on, done.off), (0, 0), "{:?}", parts.analyse_rules);

    // With deprecation rules installed, migrate turns them on beside the preset.
    let (done, parts, _) = at(5, &["phpstan/phpstan-deprecation-rules"], &[]);
    assert!(lines(&parts, "\"on\"").contains(&"deprecated/class".to_string()));
    assert_eq!(done.off, 0);

    let strict = "phpstan/phpstan-strict-rules";
    let params = [
        ("booleansInConditions", true),
        ("disallowedEmpty", true),
        ("uselessCast", false),
    ];
    let (done, parts, _) = at(0, &[strict], &params);
    assert_eq!(done.preset, AnalysePreset::PhpstanLevel(0));
    let on = lines(&parts, "\"on\"");
    assert!(
        on.contains(&"operators/non-boolean-and-operand".to_string()),
        "{on:?}"
    );
    assert!(!(parts.lint_rules.iter()).any(|(k, _)| k == "dead-code/no-useless-cast"));
    assert!(
        !on.contains(&"control-flow/non-boolean-while".to_string()),
        "needs bleedingEdge too"
    );
    assert_eq!(done.lint, 2);
    // strict-rules' check is written; phpstan/level_0 turns on the duplicate-key check itself.
    assert_eq!(
        parts.lint_rules,
        [("style/no-empty".into(), "\"error\"".into())]
    );
    let (done, parts, _) = at_in(5, &[], &[], &[("composer.lock", super::LARAVEL_LOCK)], "");
    assert!((parts.analyse_rules.iter()).any(|(k, v)| k == "laravel" && v == "\"off\""));
    assert!(
        !parts
            .analyse_rules
            .iter()
            .any(|(k, _)| k.starts_with("laravel/"))
    );
    assert_eq!(done.off, lines(&parts, "\"off\"").len());

    let backtick = [("disallowedBacktick", true)];
    let format_rule = (
        "format/functions/backtick-to-shell-exec".to_string(),
        "a PHPStan check that is this format rule in Pun; turn it on in [format.rules]".to_string(),
    );
    let (done, ..) = at_in(0, &[strict], &backtick, &[], "preset = \"empty\"\n");
    let format = |id: &str| (id.to_string(), format_rule.1.clone());
    assert_eq!(
        done.unsupported,
        [
            format("format/casts/short-scalar"),
            format_rule.clone(),
            format("format/whitespace/encoding")
        ]
    );
}

#[test]
fn identifiers_and_groups() {
    use crate::rule_of_identifier;
    assert_eq!(
        rule_of_identifier("function.notFound"),
        Some("analyse/undefined/function")
    );
    assert_eq!(
        rule_of_identifier("greater.alwaysTrue"),
        Some("analyse/dead-code/number-comparison-always-constant")
    );
    assert_eq!(
        rule_of_identifier("larastan.configCollection"),
        Some("analyse/laravel/config-collection")
    );
    assert_eq!(
        rule_of_identifier("whitespace.bom"),
        Some("format/whitespace/encoding")
    );
    assert_eq!(rule_of_identifier("no.such.identifier"), None);
    assert_eq!(rule_of_identifier(".alwaysTrue"), None);
    assert!(crate::groups().iter().any(|g| g == "dead-code"));
    assert!(!crate::groups().iter().any(|g| g == "dead"));
}

#[test]
fn no_level_runs_no_rules() {
    let d = project(
        "parameters:\n    paths: [app]\n",
        &[("composer.json", "{}")],
    );
    let p = from_phpstan(&d, &d.join("phpstan.neon")).unwrap();
    assert!(p.no_rules);
    let mut parts = Parts::default();
    let done = compose(&d, &mut parts, &p).unwrap();
    assert!(done.off > 10 && done.on == 0, "{done:?}");
    std::fs::write(d.join("pun.toml"), parts.render()).unwrap();
    let cfg = load(&d, &Options::default()).unwrap();
    assert_eq!(cfg.analyse.rules, [] as [schema::AnalyseRule; 0]);
    let d = project("parameters:\n    level: 0\n", &[]);
    assert!(!from_phpstan(&d, &d.join("phpstan.neon")).unwrap().no_rules);
}

/// PHPStan reports `array.duplicateKey` (a level 0 check that is a lint rule in Pun) only in its `paths`, minus
/// `excludePaths`: migrate turns the rule on there alone, in overrides, so `pun lint` does not report it in tests/ that
/// PHPStan never reads (laravel-framework's phpstan.neon analyses src/ only).
#[test]
fn lint_rules_keep_to_the_phpstan_paths() {
    let d = project(
        "parameters:\n    level: 0\n    paths:\n        - src\n    excludePaths:\n        - src/Legacy\n",
        &[("src/a.php", "<?php\n"), ("src/Legacy/b.php", "<?php\n")],
    );
    let p = from_phpstan(&d, &d.join("phpstan.neon")).unwrap();
    let mut parts = Parts {
        format: "preset = \"empty\"\n".into(),
        ..Parts::default()
    };
    let done = compose(&d, &mut parts, &p).unwrap();
    assert_eq!(done.lint, 1);
    // phpstan/level_0 turns it on everywhere; migrate turns it off there and on in PHPStan's paths.
    assert_eq!(
        parts.lint_rules,
        [("dead-code/no-duplicate-array-key".into(), "\"off\"".into())]
    );
    assert_eq!(
        parts.overrides,
        "[[lint.overrides]]\nfiles = [\"src\"]\nrules = { \"dead-code/no-duplicate-array-key\" = \"error\" }\n\n[[lint.overrides]]\nfiles = [\"src/Legacy\"]\nrules = { \"dead-code/no-duplicate-array-key\" = \"off\" }\n"
    );
    let cfg = crate::from_toml(&d, &parts.render()).unwrap();
    assert!(cfg.lint.rules.is_empty());
    assert_eq!(cfg.lint.overrides.len(), 2);

    // A rule another config turns on everywhere stays on everywhere.
    let mut parts = Parts {
        format: "preset = \"empty\"\n".into(),
        lint_rules: vec![(
            "dead-code/no-duplicate-array-key".into(),
            "\"error\"".into(),
        )],
        ..Parts::default()
    };
    let done = compose(&d, &mut parts, &p).unwrap();
    assert_eq!(
        (done.lint, parts.overrides.as_str(), parts.lint_rules.len()),
        (1, "", 1)
    );
}

/// What the project's own configs register with PHPStan is named, once each: a `rules:` entry and a service tagged
/// `phpstan.rules.rule` as a rule, a service with another `phpstan.*` tag as an extension, an untagged service not at
/// all. `phpVersion`'s range becomes `[php] version` from its `min` (80200 is PHP 8.2), its `max` is named.
#[test]
fn names_custom_rules_and_takes_the_php_version_range() {
    let neon = "includes:
    - build/rules.neon
parameters:
    level: 0
    phpVersion:
        min: 80200
        max: 80400
rules:
    - Support\\Lint\\NoSeederDataCreationInMigration
services:
    -
        class: App\\PHPStan\\TaggedRule
        tags: [phpstan.rules.rule]
    - { class: App\\PHPStan\\ReturnType, tags: [phpstan.broker.dynamicMethodReturnTypeExtension] }
    - { factory: App\\PHPStan\\Made(1), tags: [phpstan.rules.rule] }
    - App\\PHPStan\\Helper
";
    let included = "rules:\n    - App\\PHPStan\\TaggedRule\n    - App\\PHPStan\\Included\n";
    let d = project(neon, &[("build/rules.neon", included)]);
    let p = from_phpstan(&d, &d.join("phpstan.neon")).unwrap();
    let rule = "a custom PHPStan rule; Pun runs no PHP rules";
    let want = [
        (
            "build/rules.neon",
            "the project's own PHPStan config (services, rules, parameters)",
        ),
        ("rule Support\\Lint\\NoSeederDataCreationInMigration", rule),
        ("rule App\\PHPStan\\TaggedRule", rule),
        (
            "extension App\\PHPStan\\ReturnType",
            "a custom PHPStan extension (phpstan.broker.dynamicMethodReturnTypeExtension); Pun runs no PHP extensions",
        ),
        ("rule App\\PHPStan\\Made", rule),
        ("rule App\\PHPStan\\Included", rule),
        (
            "parameter phpVersion.max (8.4)",
            "[php] version is one version; it takes phpVersion.min",
        ),
    ];
    let got: Vec<(&str, &str)> = (p.unsupported.iter())
        .map(|(n, w)| (n.as_str(), w.as_str()))
        .collect();
    assert_eq!(got, want);
    assert_eq!(p.php_version.as_deref(), Some("8.2"));
    let mut parts = Parts::default();
    compose(&d, &mut parts, &p).unwrap();
    let cfg = crate::from_toml(&d, &parts.render()).unwrap();
    assert_eq!(cfg.php.to_string(), "8.2");
    assert_eq!(cfg.analyse_php_version.to_string(), "8.2");
    std::fs::remove_dir_all(d).unwrap();
    // `null` is PHPStan's default (the running PHP): nothing to carry, nothing to report.
    let d = project("parameters:\n    level: 0\n    phpVersion: null\n", &[]);
    let p = from_phpstan(&d, &d.join("phpstan.neon")).unwrap();
    assert_eq!((p.php_version, p.unsupported), (None, vec![]));
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn carries_disallowed_calls_lists_into_rule_options() {
    use pun_phpstan::Value as V;
    let ext = "vendor/spaze/phpstan-disallowed-calls";
    let neon = format!(
        "includes:
    - {ext}/extension.neon
    - {ext}/disallowed-dangerous-calls.neon
parameters:
    level: 0
    filesRootDir: %currentWorkingDirectory%/app
    disallowedFunctionCalls:
        -
            function: 'var_dump()'
            allowIn: ['Legacy/*', '*/tests/*']
            errorIdentifier: no.dump
        -
            function: 'Foo\\bar()'
            definedIn: 'lib/*.php'
            allowParamsAnywhere:
                2: 1
    ignoreErrors:
        - identifier: no.dump
          path: app/Models/User.php
"
    );
    let d = project(
        &neon,
        &[
            (
                &format!("{ext}/extension.neon"),
                include_str!("../../../pun-analyse/tests/fixtures/disallowed/extension.neon"),
            ),
            (
                &format!("{ext}/disallowed-dangerous-calls.neon"),
                include_str!(
                    "../../../pun-analyse/tests/fixtures/disallowed/config/bundled-dangerous.neon"
                ),
            ),
        ],
    );
    let p = from_phpstan(&d, &d.join("phpstan.neon")).unwrap();
    assert!(p.extensions.contains("spaze/phpstan-disallowed-calls"));
    // filesRootDir is folded into the paths, so it is no unsupported parameter
    assert!(p.unsupported.is_empty(), "{:?}", p.unsupported);
    let mut parts = Parts::default();
    compose(&d, &mut parts, &p).unwrap();
    let text = parts.render();
    assert!(text.contains("disallowed-calls = \"on\""), "{text}");
    let cfg = crate::from_toml(&d, &text).unwrap();
    let rule = (cfg.analyse.rules.iter())
        .find(|r| r.id == "analyse/disallowed/function")
        .unwrap();
    let calls = rule.options["calls"].as_array().unwrap();
    // the dangerous config's 20 entries, then the project's own
    assert_eq!(calls.len(), 22, "{text}");
    assert_eq!(calls[0]["function"], "apache_setenv()");
    assert_eq!(
        calls[20],
        serde_json::json!({
            "function": "var_dump()",
            "allow-in": ["app/Legacy/*", "*/tests/*"],
            "error-identifier": "no.dump",
        })
    );
    assert_eq!(
        calls[21],
        serde_json::json!({
            "function": "Foo\\bar()",
            "defined-in": "app/lib/*.php",
            "allow-params-anywhere": {"2": 1},
        })
    );
    let ignore = &cfg.analyse.ignore[0];
    assert_eq!(ignore.rule.as_deref(), Some("analyse/disallowed/function"));
    assert_eq!(ignore.identifier.as_deref(), Some("no.dump"));
    // back to PHPStan's camelCase keys for the rules
    let parameters = cfg.analyse.parameters(&d);
    let calls = parameters
        .get_str("disallowedFunctionCalls")
        .and_then(V::as_array)
        .unwrap();
    let own = calls.values().nth(20).and_then(V::as_array).unwrap();
    assert_eq!(own.get_str("errorIdentifier"), Some(&V::str("no.dump")));
    let params = (calls.values().last().and_then(V::as_array))
        .and_then(|e| e.get_str("allowParamsAnywhere"))
        .and_then(V::as_array)
        .unwrap();
    assert_eq!(params.get_str("2"), Some(&V::Int(1)));
    assert_eq!(
        own.get_str("allowIn")
            .and_then(V::as_array)
            .map(pun_phpstan::Array::len),
        Some(2)
    );
    std::fs::remove_dir_all(d).unwrap();
}

/// An include Pun's PHP reader can't run is named with the reason and skipped once, not also as the project's own
/// config; a PHP baseline is emptied on purpose and only gets the baseline note.
#[test]
fn names_and_skips_a_php_include_it_cant_read() {
    let d = project(
        "includes:\n    - bad.php\n    - phpstan-baseline.php\nparameters:\n    level: 5\n",
        &[
            (
                "bad.php",
                "<?php\n\nreturn ['parameters' => ['level' => dirname(__DIR__)]];\n",
            ),
            (
                "phpstan-baseline.php",
                "<?php\n\nreturn ['parameters' => ['ignoreErrors' => []]];\n",
            ),
        ],
    );
    let p = from_phpstan(&d, &d.join("phpstan.neon")).unwrap();
    assert_eq!(p.level, 5);
    assert_eq!(p.baselines, ["phpstan-baseline.php"]);
    assert_eq!(
        p.unsupported,
        [(
            "bad.php".to_string(),
            "a PHP config Pun can't read (unexpected \"dirname(__DIR__)]];\\n\" on line 3); its parameters are not migrated".to_string()
        )]
    );
}

/// A parameter only the skipped PHP include defines can't resolve: the error names the skipped file.
#[test]
fn a_missing_parameter_names_the_skipped_php_include() {
    let d = project(
        "includes:\n    - dirs.php\nparametersSchema:\n    supportDir: string()\nparameters:\n    level: 5\n    scanDirectories:\n        - %supportDir%\n",
        &[(
            "dirs.php",
            "<?php\n\nreturn ['parameters' => ['supportDir' => dirname(__DIR__).'/support']];\n",
        )],
    );
    let e = from_phpstan(&d, &d.join("phpstan.neon"))
        .unwrap_err()
        .to_string();
    assert_eq!(
        e,
        "Missing parameter 'supportDir'. Pun skipped dirs.php (unexpected \"dirname(__DIR__).'/s\" on line 3), a PHP config it can't read."
    );
}

/// A package include whose path holds a quote is written to the defaults config as NEON, not cut at the quote.
#[test]
fn reads_a_package_include_with_a_quote_in_its_path() {
    let d = project(
        "includes:\n    - \"vendor/acme/team's/extension.neon\"\nparameters:\n    level: 5\n",
        &[(
            "vendor/acme/team's/extension.neon",
            "parameters:\n    level: 5\n",
        )],
    );
    assert_eq!(from_phpstan(&d, &d.join("phpstan.neon")).unwrap().level, 5);
}
