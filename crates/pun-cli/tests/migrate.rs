//! `pun migrate` end to end. Expectations are hand-derived from pint.json and context/plan/rules.md §4, or come from the real
//! Pint (`php .oracle/pint/builds/pint`) on the unmigrated project.
mod common;

use common::pty::{pun, pun_with};
use common::read;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn pty(dir: &Path, keys: &str) -> String {
    let (out, code) = common::pty::pty(dir, "migrate", keys, &[("NO_COLOR", "1")]);
    assert_eq!(code, Some(0), "{out}");
    common::strip_ansi(&out)
}

fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    common::project("migrate", name, files)
}

fn counts(out: &str) -> String {
    let words: Vec<String> = out.split(' ').map(String::from).collect();
    (words.iter().enumerate())
        .map(|(i, w)| {
            let next = words.get(i + 1).map(|n| n.trim_end_matches([',', '\n']));
            let number = !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit());
            match next {
                Some("on" | "off") if number => "N".to_string(),
                _ => w.clone(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

const PINT: &str = r#"{"preset":"empty","rules":{"array_syntax":true,"final_class":true}}"#;
const TOML: &str = "# Written by `pun migrate` from pint.json.\n[format]\npreset = \"empty\"\n\n[format.rules]\n\"arrays/syntax\" = true\n\n[lint.rules]\n\"style/require-final-class\" = \"error\"\n";
const MOVED: &str = " ok pint.json  -> [format] preset empty, 1 rule changed, 1 rule -> [lint]\n";
const PR: &str = "   Want them? Send a PR to https://github.com/shipfastlabs/pun\n\n";
const IGNORED: &str = "<?php\n\n// @php-cs-fixer-ignore array_syntax,final_class\n$a = array(1);\n$s = '// @php-cs-fixer-ignore array_syntax';\n";
const UNKNOWN: &str = "\n ! Not migrated\n   Ignore comments left as is: no Pun rule for it\n     app/Unknown.php  `@php-cs-fixer-ignore Acme/rule`\n";
const REWRITTEN: &str = "<?php\n\n// @pun-ignore-file format/arrays/syntax,lint/style/require-final-class\n$a = array(1);\n$s = '// @php-cs-fixer-ignore array_syntax';\n";

#[test]
fn writes_pun_toml_and_rewrites_ignores() {
    let d = project(
        "rewrite",
        &[
            ("pint.json", PINT),
            ("app/Ignored.php", IGNORED),
            (
                "app/Unknown.php",
                "<?php\n// @php-cs-fixer-ignore Acme/rule\n",
            ),
            ("app/Plain.php", "<?php\n$a = array(1);\n"),
        ],
    );
    let (out, err, code) = pun(&d, &["migrate", "--dry-run"]);
    assert_eq!(code, Some(0));
    assert_eq!(
        out,
        format!(
            "--- pun.toml\n+++ pun.toml\n@@ -1,0 +1,9 @@\n{}--- app/Ignored.php\n+++ app/Ignored.php\n@@ -1,5 +1,5 @@\n <?php\n \n-// @php-cs-fixer-ignore array_syntax,final_class\n+// @pun-ignore-file format/arrays/syntax,lint/style/require-final-class\n $a = array(1);\n $s = '// @php-cs-fixer-ignore array_syntax';\n ok pint.json        -> [format] preset empty, 1 rule changed, 1 rule -> [lint]\n ok ignore comments  -> @pun-ignore-file in 1 file\n{UNKNOWN}{PR} - Dry run: would write pun.toml\n",
            TOML.lines().fold(String::new(), |mut out, l| {
                let _ = writeln!(out, "+{l}");
                out
            })
        )
    );
    assert_eq!(err, "");
    assert!(!d.join("pun.toml").exists());
    assert_eq!(read(&d, "app/Ignored.php"), IGNORED);

    let (out, err, code) = pun(&d, &["migrate"]);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        (
            &*format!(
                " ok pint.json        -> [format] preset empty, 1 rule changed, 1 rule -> [lint]\n ok ignore comments  -> @pun-ignore-file in 1 file\n{UNKNOWN}{PR} ok Wrote pun.toml\n"
            ),
            "",
            Some(0)
        )
    );
    assert_eq!(read(&d, "pun.toml"), TOML);
    assert_eq!(read(&d, "app/Ignored.php"), REWRITTEN);
    pun(&d, &["check", "--fix", "app/Ignored.php", "app/Plain.php"]);
    assert_eq!(read(&d, "app/Ignored.php"), REWRITTEN);
    assert_eq!(read(&d, "app/Plain.php"), "<?php\n$a = [1];\n");

    let (out, _, code) = pun(&d, &["migrate"]);
    assert_eq!(
        (out.as_str(), code),
        (
            &*format!("{MOVED}{UNKNOWN}{PR} ok pun.toml is already up to date\n"),
            Some(0)
        )
    );
    let (out, _, _) = pun(&d, &["migrate", "--dry-run"]);
    assert_eq!(
        out,
        format!("{MOVED}{UNKNOWN}{PR} - Dry run: pun.toml is already up to date\n")
    );
    std::fs::write(d.join("pun.toml"), "# mine\n").unwrap();
    let (_, err, code) = pun(&d, &["migrate"]);
    assert_eq!(
        (err.as_str(), code),
        (
            "x pun.toml exists and differs from what migrate writes; move it away first (`pun migrate --dry-run` shows the difference)\n",
            Some(2)
        )
    );
    assert_eq!(read(&d, "pun.toml"), "# mine\n");
}

/// Migrate edits only the project's own files: not one a symlink leads to outside it, nor one an absolute phpstan.neon
/// path names. It doesn't read (for the `--dry-run` diff) or write through a pun.toml symlink that leads out.
#[test]
fn stays_inside_the_project() {
    let outside = common::scratch("migrate", "outside");
    std::fs::create_dir_all(outside.join("src")).unwrap();
    std::fs::write(outside.join("Linked.php"), IGNORED).unwrap();
    std::fs::write(outside.join("src/Analysed.php"), IGNORED).unwrap();
    std::fs::write(outside.join("secret.toml"), "token = \"hunter2\"\n").unwrap();
    let src = outside.join("src");
    let neon = format!(
        "parameters:\n    level: 0\n    paths: [app, {}]\n",
        src.display()
    );
    let d = project(
        "outside",
        &[
            ("pint.json", PINT),
            ("phpstan.neon", &neon),
            ("app/Ignored.php", IGNORED),
        ],
    );
    std::os::unix::fs::symlink(outside.join("Linked.php"), d.join("app/Linked.php")).unwrap();
    let (out, err, code) = pun(&d, &["migrate", "--yes"]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert_eq!(read(&d, "app/Ignored.php"), REWRITTEN);
    assert_eq!(read(&outside, "Linked.php"), IGNORED);
    assert_eq!(read(&outside, "src/Analysed.php"), IGNORED);

    let message = "x pun.toml is a symlink that leads outside the project; migrate doesn't read or write through it. Remove it, then run `pun migrate` again.\n";
    for target in ["secret.toml", "made.toml"] {
        std::fs::remove_file(d.join("pun.toml")).unwrap();
        std::os::unix::fs::symlink(outside.join(target), d.join("pun.toml")).unwrap();
        for args in [
            &["migrate", "--dry-run", "--yes"][..],
            &["migrate", "--yes"],
        ] {
            let (out, err, code) = pun(&d, args);
            assert_eq!((out.as_str(), err.as_str(), code), ("", message, Some(2)));
        }
    }
    assert_eq!(read(&outside, "secret.toml"), "token = \"hunter2\"\n");
    assert!(!outside.join("made.toml").exists());

    // A pun.toml that isn't UTF-8 still differs: it is kept, not overwritten.
    std::fs::remove_file(d.join("pun.toml")).unwrap();
    std::fs::write(d.join("pun.toml"), b"\xff\xfe").unwrap();
    let (_, err, code) = pun(&d, &["migrate", "--yes"]);
    assert_eq!(
        (err.as_str(), code),
        (
            "x pun.toml exists and differs from what migrate writes; move it away first (`pun migrate --dry-run` shows the difference)\n",
            Some(2)
        )
    );
    assert_eq!(std::fs::read(d.join("pun.toml")).unwrap(), b"\xff\xfe");
}

#[test]
fn skips_unsupported_rules() {
    let d = project(
        "unsupported",
        &[(
            "pint.json",
            r#"{"preset":"empty","rules":{"braces":true,"array_syntax":true}}"#,
        )],
    );
    let (out, err, code) = pun_with(&d, &["migrate"], &[("LANG", "en_US.UTF-8")]);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        (
            " ✔ pint.json  → [format] preset empty, 1 rule changed\n\n ⚠ Not migrated\n   deprecated upstream; successor: single_space_around_construct, control_structure_braces, control_structure_continuation_position, declare_parentheses, no_multiple_statements_per_line, braces_position, statement_indentation, no_extra_blank_lines\n     braces\n   Want them? Send a PR to https://github.com/shipfastlabs/pun\n\n ✔ Wrote pun.toml\n",
            "",
            Some(0)
        )
    );
    assert_eq!(
        read(&d, "pun.toml"),
        "# Written by `pun migrate` from pint.json.\n[format]\npreset = \"empty\"\n\n[format.rules]\n\"arrays/syntax\" = true\n"
    );
    let d = project("broken", &[("pint.json", "{")]);
    let (_, err, code) = pun(&d, &["migrate"]);
    assert_eq!(
        (err, code),
        (
            format!(
                "x The configuration file [{}] is not valid JSON.: EOF while parsing an object at line 1 column 1\n",
                d.join("pint.json").display()
            ),
            Some(2)
        )
    );
}

#[test]
fn config_and_preset_flags() {
    let d = project(
        "flags",
        &[(
            "other.json",
            r#"{"preset":"empty","rules":{"single_quote":true}}"#,
        )],
    );
    pun(
        &d,
        &[
            "migrate",
            "--dry-run",
            "--config=other.json",
            "--preset=psr12",
        ],
    );
    let (out, _, _) = pun(
        &d,
        &[
            "migrate",
            "--dry-run",
            "--config=other.json",
            "--preset=psr12",
        ],
    );
    assert!(
        out.contains(
            "+# Written by `pun migrate` from other.json.\n+[format]\n+preset = \"psr12\"\n"
        ),
        "{out}"
    );
    assert!(out.contains("+\"strings/single-quote\" = true\n"), "{out}");
    let (out, _, code) = pun(&d, &["migrate", "--dry-run", "--preset=per"]);
    assert_eq!(code, Some(0));
    assert!(!out.contains("Want them?"), "{out}");
    let s = project("symfony", &[("pint.json", r#"{"preset":"symfony"}"#)]);
    for (dir, args) in [
        (&s, &["migrate", "--dry-run"][..]),
        (
            &d,
            &[
                "migrate",
                "--dry-run",
                "--config=other.json",
                "--preset=symfony",
            ],
        ),
    ] {
        assert_eq!(
            pun(dir, args),
            (
                String::new(),
                "x the symfony preset was removed: Pun supports the laravel, psr12, per and empty presets\n".into(),
                Some(2)
            ),
            "{args:?}"
        );
    }
    let (out, _, _) = pun(&d, &["migrate"]);
    assert_eq!(
        out,
        "No pint.json, .php-cs-fixer.php, phpstan.neon, rector.php or mago.toml found.\n ok Wrote starter pun.toml (preset laravel)\n"
    );
    assert_eq!(
        read(&d, "pun.toml"),
        "# Written by `pun migrate`: no pint.json, .php-cs-fixer.php, phpstan.neon, rector.php or mago.toml found; starter config.\n[format]\npreset = \"laravel\"\n\n[lint.rules]\n\"refactor/rename-phpdoc-tag\" = \"error\"\n\"functions/no-unused-closure-use\" = \"error\"\n\"style/no-alias-function\" = \"error\"\n\"statements/no-empty\" = \"error\"\n\"phpdoc/no-superfluous-tags\" = { allow-mixed = true, allow-unused-params = true }\n\"functions/no-unreachable-default-argument-value\" = \"error\"\n\"imports/no-unused\" = \"error\"\n\"control-flow/no-useless-return\" = \"error\"\n\"style/require-ordered-traits\" = \"error\"\n\"phpunit/require-method-casing\" = { case = \"snake-case\" }\n\"phpunit/require-protected-setup\" = \"error\"\n\"phpdoc/no-access\" = \"error\"\n\"phpdoc/no-package\" = \"error\"\n\"phpdoc/no-useless-inheritdoc\" = \"error\"\n\"classes/prefer-self-in-final\" = \"error\"\n"
    );
}

const CHECKLIST: &[(&str, &str)] = &[
    ("pint.json", PINT),
    ("phpstan.neon", "parameters:\n    level: 5\n"),
    ("rector.php", "<?php\n"),
];

#[test]
fn lists_every_detected_config() {
    let files = [
        ("pint.json", PINT),
        (".php-cs-fixer.dist.php", "<?php\nreturn 1;\n"),
        ("phpstan.dist.neon", "parameters:\n    level: 5\n"),
        ("rector.php", "<?php\n"),
        ("mago.toml", "[linter]\n"),
    ];
    let d = project("all", &files);
    let (out, err, code) = pun(&d, &["migrate"]);
    assert_eq!(
        (counts(&out).as_str(), err.as_str(), code),
        (
            " ok pint.json               -> [format] preset empty, 1 rule changed, 1 rule -> [lint]
 - .php-cs-fixer.dist.php  not migrated: pint.json gives [format], as in Pint
 ok phpstan.dist.neon       -> [analyse] preset phpstan/level_5, 8 rules -> [lint]
 ! rector.php              not read: run `composer install` so Pun can read it with Rector; left untouched

 ! Not migrated
   a PHPStan check that is this format rule in Pun; turn it on in [format.rules]
     format/casts/short-scalar
     format/functions/backtick-to-shell-exec
     format/whitespace/encoding
   Mago config: not supported by Pun yet; left untouched
     mago.toml
   Want them? Send a PR to https://github.com/shipfastlabs/pun

 ok Wrote pun.toml
",
            "",
            Some(0)
        )
    );
    let toml = read(&d, "pun.toml");
    assert!(
        toml.starts_with(TOML)
            // phpstan/level_5 turns on the PHPStan checks that are lint rules itself
            && toml[TOML.len()..].starts_with("\n[analyse]\npreset = \"phpstan/level_5\"\n"),
        "{toml}"
    );
    for (f, c) in files {
        assert_eq!(read(&d, f), c);
    }
    std::fs::remove_file(d.join("pun.toml")).unwrap();
    let (out, _, code) = pun(&d, &["migrate", "--from", "phpstan,mago"]);
    assert_eq!(
        (counts(&out).as_str(), code),
        (
            " - pint.json               not selected
 - .php-cs-fixer.dist.php  not selected
 ok phpstan.dist.neon       -> [analyse] preset phpstan/level_5, 8 rules -> [lint]
 - rector.php              not selected

 ! Not migrated
   a PHPStan check that is this format rule in Pun; turn it on in [format.rules]
     format/functions/backtick-to-shell-exec
   Mago config: not supported by Pun yet; left untouched
     mago.toml
   Want them? Send a PR to https://github.com/shipfastlabs/pun

 ok Wrote pun.toml
",
            Some(0)
        )
    );
    assert!(read(&d, "pun.toml").starts_with(
        "# Written by `pun migrate` without [format]: pint.json, .php-cs-fixer.dist.php not selected.\n"
    ));
    std::fs::remove_file(d.join("pun.toml")).unwrap();
    let (out, _, code) = pun(&d, &["migrate", "--from", "mago"]);
    assert_eq!(
        (out.as_str(), code),
        (
            " - pint.json               not selected
 - .php-cs-fixer.dist.php  not selected
 - phpstan.dist.neon       not selected
 - rector.php              not selected

 ! Not migrated
   Mago config: not supported by Pun yet; left untouched
     mago.toml
   Want them? Send a PR to https://github.com/shipfastlabs/pun

 - Nothing to migrate; pun.toml not written
",
            Some(0)
        )
    );
    assert!(!d.join("pun.toml").exists());
    let d = project("from-missing", &[("pint.json", PINT)]);
    let (out, err, code) = pun(&d, &["migrate", "--from=pint,php-cs-fixer"]);
    assert_eq!(
        (out.as_str(), err.as_str(), code),
        (
            "",
            "x --from php-cs-fixer: no .php-cs-fixer.php or .php-cs-fixer.dist.php found\n",
            Some(2)
        )
    );
    let (_, err, _) = pun(&d, &["migrate", "--from=pint", "--config=nope.json"]);
    assert_eq!(err, "x --from pint: no nope.json found\n");
}

#[test]
fn errors_name_the_file() {
    let d = project("unreadable", &[]);
    std::fs::create_dir(d.join("pint.json")).unwrap();
    let (_, err, code) = pun(&d, &["migrate", "--yes"]);
    assert_eq!(
        (err.as_str(), code),
        (
            format!(
                "x can't read {}: Is a directory (os error 21)\n",
                d.join("pint.json").display()
            )
            .as_str(),
            Some(2)
        )
    );
    let d = project("unwritable", &[("pint.json", PINT)]);
    std::fs::create_dir(d.join("pun.toml")).unwrap();
    let (_, err, code) = pun(&d, &["migrate", "--yes"]);
    assert_eq!(
        (err.as_str(), code),
        (
            format!(
                "x can't write {}: Is a directory (os error 21)\n",
                d.join("pun.toml").display()
            )
            .as_str(),
            Some(2)
        )
    );
    let d = project(
        "php-fatal",
        &[
            ("vendor/friendsofphp/php-cs-fixer/.keep", ""),
            (
                "vendor/autoload.php",
                "<?php\nfunction f() {}\nfunction f() {}\n",
            ),
            (".php-cs-fixer.php", "<?php\nreturn 1;\n"),
        ],
    );
    let (_, err, code) = pun(&d, &["migrate", "--yes"]);
    let autoload = d.join("vendor/autoload.php");
    let autoload = autoload.display();
    assert_eq!(
        (err.as_str(), code),
        (
            format!("x can't read .php-cs-fixer.php with PHP-CS-Fixer: Fatal error: Cannot redeclare function f() (previously declared in {autoload}:2) in {autoload} on line 3\n").as_str(),
            Some(2)
        )
    );
    // A config (here the autoloader it loads) that exits before the helper prints its result.
    std::fs::write(d.join("vendor/autoload.php"), "<?php\nexit(0);\n").unwrap();
    let (_, err, code) = pun(&d, &["migrate", "--yes"]);
    assert_eq!(
        (err.as_str(), code),
        (
            "x can't read .php-cs-fixer.php with PHP-CS-Fixer: the config ended the PHP process before Pun could read it\n",
            Some(2)
        )
    );
    // Reading a PHP config needs PHP; without it on PATH the error names the config.
    let empty = d.join("no-php");
    std::fs::create_dir(&empty).unwrap();
    let (_, err, code) = pun_with(
        &d,
        &["migrate", "--yes"],
        &[("PATH", empty.to_str().unwrap())],
    );
    assert_eq!(
        (err.as_str(), code),
        (
            "x can't read .php-cs-fixer.php with PHP-CS-Fixer: it needs `php` on PATH: No such file or directory (os error 2)\n",
            Some(2)
        )
    );
}

#[test]
fn starter_beside_phpstan() {
    let d = project(
        "phpstan-only",
        &[("phpstan.neon.dist", "parameters:\n    level: 5\n")],
    );
    let (out, _, code) = pun(&d, &["migrate", "--yes"]);
    assert_eq!(
        (counts(&out).as_str(), code),
        (
            " ok phpstan.neon.dist  -> [analyse] preset phpstan/level_5, 8 rules -> [lint]\n\n ! Not migrated\n   a PHPStan check that is this format rule in Pun; turn it on in [format.rules]\n     format/functions/backtick-to-shell-exec\n   Want them? Send a PR to https://github.com/shipfastlabs/pun\n\n ok Wrote starter pun.toml (preset laravel)\n",
            Some(0)
        )
    );
    assert!(read(&d, "pun.toml").starts_with(
        "# Written by `pun migrate`: no pint.json or .php-cs-fixer.php found; starter config.\n[format]\npreset = \"laravel\"\n"
    ));
}

/// A phpstan.neon without a level runs no rules (PHPStan: "No rules detected"): every analyse group is turned
/// off and no check moves to [lint], so `pun analyse` says the same.
#[test]
fn phpstan_without_a_level_runs_no_rules() {
    let d = project(
        "phpstan-no-level",
        &[
            ("phpstan.neon", "parameters:\n    paths: [app]\n"),
            ("app/A.php", "<?php\n\nundefined_fn();\n"),
        ],
    );
    let (out, _, code) = pun(&d, &["migrate", "--yes"]);
    assert_eq!(
        (counts(&out).lines().next(), code),
        (
            Some(" ok phpstan.neon  -> [analyse] preset phpstan/level_0, N on, N off"),
            Some(0)
        )
    );
    let (out, err, code) = pun(&d, &["analyse"]);
    assert_eq!((out.as_str(), code), ("", Some(2)));
    assert!(err.starts_with("x No rules detected\n"), "{err}");
}

/// phpat is built in: its include is not reported, and the classes the config tags `phpat.test` and its
/// `parameters.phpat` carry to `[extensions] phpat`, where `pun analyse` runs them.
#[test]
fn phpstan_phpat_tests_carry_over() {
    let neon = "includes:\n    - vendor/phpat/phpat/extension.neon\nparameters:\n    level: 0\n    paths: [src]\n    phpat:\n        show_rule_names: true\nservices:\n    -\n        class: Arch\\LayerTest\n        tags:\n            - phpat.test\n";
    let d = project(
        "phpstan-phpat",
        &[
            ("phpstan.neon", neon),
            // phpat 0.12.5's extension.neon, cut to its parameters and one rule
            (
                "vendor/phpat/phpat/extension.neon",
                "parameters:\n\tphpat:\n\t\tignore_doc_comments: false\n\t\tignore_built_in_classes: false\n\t\tshow_rule_names: false\n\nservices:\n\t-\n\t\tclass: PHPat\\Rule\\Assertion\\Declaration\\IsFinal\\IsFinalRule\n\t\ttags:\n\t\t\t- phpstan.rules.rule\n\nparametersSchema:\n\tphpat: structure([\n\t\tignore_doc_comments: bool(),\n\t\tignore_built_in_classes: bool(),\n\t\tshow_rule_names: bool(),\n\t])\n",
            ),
            (
                "composer.lock",
                r#"{"packages":[],"packages-dev":[{"name":"phpat/phpat","version":"0.12.5"}]}"#,
            ),
            (
                "composer.json",
                r#"{"autoload-dev":{"psr-4":{"Arch\\":"arch/"}}}"#,
            ),
            (
                "arch/LayerTest.php",
                "<?php\n\nnamespace Arch;\n\nuse PHPat\\Selector\\Selector;\nuse PHPat\\Test\\PHPat;\n\nfinal class LayerTest\n{\n    public function testFinal()\n    {\n        return PHPat::rule()->classes(Selector::all())->shouldBeFinal();\n    }\n}\n",
            ),
            ("src/A.php", "<?php\n\nnamespace App;\n\nclass A\n{\n}\n"),
        ],
    );
    let (out, _, code) = pun(&d, &["migrate", "--yes"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(!out.contains("phpat"), "{out}");
    let toml = read(&d, "pun.toml");
    assert!(
        toml.contains(
            "[extensions]\nphpat = { tests = [\"Arch\\\\LayerTest\"], show-rule-names = true }\n"
        ),
        "{toml}"
    );
    let (out, _, code) = pun(&d, &["analyse", "--error-format=raw"]);
    assert_eq!(
        (out.as_str(), code),
        (
            format!(
                "{}:5:testFinal: App\\A should be final\n",
                d.join("src/A.php").canonicalize().unwrap().display()
            )
            .as_str(),
            Some(1)
        )
    );
}

#[test]
fn phpstan_without_vendor_is_left() {
    for (name, prefix) in [("no-vendor", ""), ("no-vendor-dot", "./")] {
        let neon = format!(
            "includes:\n    - {prefix}vendor/larastan/larastan/extension.neon\nparameters:\n    level: 5\n"
        );
        let d = project(name, &[("pint.json", PINT), ("phpstan.neon", &neon)]);
        let (out, err, code) = pun(&d, &["migrate", "--yes"]);
        assert_eq!(
            (out.as_str(), err.as_str(), code),
            (
                " ok pint.json     -> [format] preset empty, 1 rule changed, 1 rule -> [lint]
 ! phpstan.neon  not read: run `composer install` so Pun can read its vendor includes; left untouched
 ok Wrote pun.toml
",
                "",
                Some(0)
            )
        );
        assert_eq!(read(&d, "pun.toml"), TOML);
    }
    let d = project(
        "no-include",
        &[
            ("pint.json", PINT),
            ("phpstan.neon", "includes:\n    - nope.neon\n"),
        ],
    );
    let (_, err, code) = pun(&d, &["migrate", "--yes"]);
    let missing = format!(
        "x can't read phpstan.neon as a PHPStan config: File '{}' is missing or is not readable.\n",
        d.join("nope.neon").display()
    );
    assert_eq!((err, code), (missing, Some(2)));
}

#[test]
fn agent_mode() {
    let unknown = (
        "app/Unknown.php",
        "<?php\n// @php-cs-fixer-ignore Acme/rule\n",
    );
    let d = project("agent", &[CHECKLIST, &[unknown]].concat());
    let agent = |args: &[&str]| {
        let (out, _, code) = pun(&d, &[args, &["--agent"]].concat());
        assert_eq!(code, Some(0));
        out
    };
    let out = agent(&["migrate", "--dry-run"]);
    let (head, rest) = out.split_once('\n').unwrap();
    assert!(
        head.starts_with("pun migrate: dry-run, pun.toml to write (preset empty, 1 format rule, 9 lint rules), ignore comments to rewrite in 0 files (from pint.json, "),
        "{head}"
    );
    assert!(rest.starts_with("pint.json migrate\nphpstan.neon migrate\nrector.php not-read not read: run `composer install` so Pun can read it with Rector; left untouched\nphpstan.neon unsupported format/casts/short-scalar: a PHPStan check that is this format rule in Pun; turn it on in [format.rules]\nphpstan.neon unsupported format/functions/backtick-to-shell-exec: a PHPStan check that is this format rule in Pun; turn it on in [format.rules]\nphpstan.neon unsupported format/whitespace/encoding: a PHPStan check that is this format rule in Pun; turn it on in [format.rules]\napp/Unknown.php ignore-not-migrated `@php-cs-fixer-ignore Acme/rule` left as is (no Pun rule for it)\nsend a PR for what is skipped: https://github.com/shipfastlabs/pun\n--- pun.toml\n"), "{rest}");
    let out = agent(&["migrate", "--from=rector"]);
    assert!(
        out.starts_with("pun migrate: ok, nothing to migrate, pun.toml not written ("),
        "{out}"
    );
    assert!(out.ends_with("s)\npint.json not-selected\nphpstan.neon not-selected\nrector.php not-read not read: run `composer install` so Pun can read it with Rector; left untouched\n"), "{out}");
    let d = project("agent-empty", &[]);
    let (out, _, _) = pun(&d, &["migrate", "--agent"]);
    assert!(out.starts_with("pun migrate: ok, starter pun.toml written (preset laravel, 115 format rules, 15 lint rules), ignore comments rewritten in 0 files (no pint.json, .php-cs-fixer.php, phpstan.neon, rector.php or mago.toml found, "), "{out}");
}

#[test]
fn checklist_on_a_terminal() {
    let d = project("tty", CHECKLIST);
    let out = pty(&d, "\\033[B \\r");
    assert!(
        out.contains("Migrate which configs? (space toggles, enter confirms)"),
        "{out}"
    );
    assert!(
        out.contains(
            "✔ Pint          pint.json\n✔ PHPStan       phpstan.neon\n✔ Rector        rector.php\n"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "✔ Pint          pint.json\n⬚ PHPStan       phpstan.neon\n✔ Rector        rector.php\n"
        ),
        "{out}"
    );
    assert!(
        out.ends_with(
            " ok pint.json     -> [format] preset empty, 1 rule changed, 1 rule -> [lint]\n - phpstan.neon  not selected\n ! rector.php    not read: run `composer install` so Pun can read it with Rector; left untouched\n ok Wrote pun.toml\n"
        ),
        "{out}"
    );
    assert_eq!(read(&d, "pun.toml"), TOML);
    std::fs::remove_file(d.join("pun.toml")).unwrap();
    let out = pty(&d, "\\033");
    assert!(
        out.ends_with(
            " - pint.json     not selected\n - phpstan.neon  not selected\n - rector.php    not selected\n - Nothing selected; pun.toml not written\n"
        ),
        "{out}"
    );
    assert!(!d.join("pun.toml").exists());
}

#[test]
fn starter_on_a_terminal() {
    let d = project("tty-empty", &[]);
    let out = pty(&d, "n");
    assert!(
        out.contains(
            "No pint.json, .php-cs-fixer.php, phpstan.neon, rector.php or mago.toml found.\n"
        ),
        "{out}"
    );
    assert!(out.contains("Write a starter pun.toml?"), "{out}");
    assert!(out.ends_with(" - Nothing written\n"), "{out}");
    assert!(!d.join("pun.toml").exists());
    let out = pty(&d, "\\r");
    assert!(
        out.ends_with(" ok Wrote starter pun.toml (preset laravel)\n"),
        "{out}"
    );
    assert!(read(&d, "pun.toml").starts_with("# Written by `pun migrate`: no pint.json, .php-cs-fixer.php, phpstan.neon, rector.php or mago.toml found; starter config.\n"));
}

#[test]
fn rewrites_phpstan_ignores() {
    let php = "<?php\n\n$a->foo(); // @phpstan-ignore method.notFound (a macro adds it), function.notFound\n/** @phpstan-ignore acme.rule */\n$b = 1;\n// @phpstan-ignore-next-line\n$c = 2;\ninclude 'x.php'; require 'y.php'; // @phpstan-ignore include.fileNotFound, require.fileNotFound\n";
    let d = project(
        "phpstan-ignores",
        &[
            (
                "phpstan.neon",
                "parameters:\n    level: 5\n    paths: [app]\n",
            ),
            ("app/A.php", php),
        ],
    );
    let (out, _, code) = pun(&d, &["migrate", "--dry-run", "--yes"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(
        out.contains("-$a->foo(); // @phpstan-ignore method.notFound (a macro adds it), function.notFound\n+$a->foo(); // @pun-ignore analyse/undefined/method,analyse/undefined/function -- a macro adds it\n"),
        "{out}"
    );
    // Two identifiers of one Pun rule name it once.
    assert!(
        out.contains(
            "+include 'x.php'; require 'y.php'; // @pun-ignore analyse/undefined/include-file\n"
        ),
        "{out}"
    );
    assert!(
        counts(&out).contains(" ok ignore comments  -> @pun-ignore in 1 file\n\n ! Not migrated\n   a PHPStan check that is this format rule in Pun; turn it on in [format.rules]\n     format/functions/backtick-to-shell-exec\n   Ignore comments left as is: `acme.rule` is not a PHPStan identifier Pun knows\n     app/A.php:4  `@phpstan-ignore acme.rule`\n   Ignore comments left as is: `pun analyse` reports no error on line 7 for it to name\n     app/A.php:6  `@phpstan-ignore-next-line`\n"),
        "{out}"
    );
    assert_eq!(read(&d, "app/A.php"), php);
}

/// `@phpstan-ignore-line` and `-next-line` name the rules of what they hid. PHPStan 2.3.6 at level 5 with
/// `reportIgnoresWithoutComments: true` reports, beside `ignore.allLineErrors`, `function.notFound` on lines 8, 9, 11,
/// 20, 24, 30 and 35, `method.nonObject` and `function.notFound` on 27, `array.invalidKey` on 29, and nothing on 13, 16
/// or 32; mapping.toml maps those to the IDs below. Line 16 is the one after the tag, inside the docblock, where no Pun
/// comment points. Without that parameter PHPStan reports lines 13, 16 and 32 as unmatched and the errors on 18, 21 and
/// 23, as `pun analyse` does after the migration. In C.php PHPStan reports `function.notFound` on 7, 9 and 10 with the
/// parameter, and without it line 8 as unmatched and the error on 9: the tag left on 7 still hides its error.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the report lists every kind of comment left as is"
)]
fn names_what_blanket_phpstan_ignores_hid() {
    const C: &str = "<?php\n\n/**\n * @php-cs-fixer-ignore array_syntax\n * @phpstan-ignore-next-line\n */\nundefined_c();\n/** @phpstan-ignore-line */\nundefined_d();\nundefined_e(); // @phpstan-ignore-line -- generated */\n";
    let php = "<?php\n\nnamespace App;\n\nfunction f(): void\n{\n    // @phpstan-ignore-next-line\n    undefined_one();\n    undefined_two(); // @phpstan-ignore-line\n    /** @phpstan-ignore-next-line */\n    undefined_three();\n    // @phpstan-ignore-next-line\n    $x = 1;\n    /**\n     * @phpstan-ignore-next-line\n     * @var int $y\n     */\n    $y = undefined_four();\n    // @phpstan-ignore-next-line because it is generated\n    undefined_five(\n        undefined_six()\n    );\n    undefined_seven(\n        undefined_eight() // @phpstan-ignore-line\n    );\n    // @phpstan-ignore-next-line\n    (new \\stdClass())->nope(undefined_nine());\n    // @phpstan-ignore-next-line\n    $a = [[] => 1];\n    /* @phpstan-ignore-line */ undefined_ten();\n    /**\n     * @phpstan-ignore-line\n     * @phpstan-ignore-next-line\n     */\n    undefined_eleven();\n}\n";
    let d = project(
        "phpstan-line-ignores",
        &[
            (
                "phpstan.neon",
                "parameters:\n    level: 5\n    paths: [app]\n",
            ),
            ("app/A.php", php),
            ("app/C.php", C),
            (
                "lib/B.php",
                "<?php\n\nundefined_b(); // @phpstan-ignore-line\n",
            ),
        ],
    );
    let (out, _, code) = pun(&d, &["migrate", "--yes"]);
    assert_eq!(code, Some(0), "{out}");
    let f = "analyse/undefined/function";
    let want = php
        .replacen(
            "// @phpstan-ignore-next-line\n",
            &format!("// @pun-ignore-next-line {f}\n"),
            1,
        )
        .replace(
            "// @phpstan-ignore-line\n    /**",
            &format!("// @pun-ignore {f}\n    /**"),
        )
        .replace(
            "/** @phpstan-ignore-next-line */",
            &format!("/** @pun-ignore-next-line {f} */"),
        )
        .replace(
            "@phpstan-ignore-next-line because",
            &format!("@pun-ignore-next-line {f} -- because"),
        )
        .replace(
            "undefined_eight() // @phpstan-ignore-line",
            &format!("undefined_eight() // @pun-ignore {f}"),
        )
        .replace(
            "// @phpstan-ignore-next-line\n    (new",
            &format!(
                "// @pun-ignore-next-line analyse/classes/invalid-method-callable,{f}\n    (new"
            ),
        )
        .replace(
            "// @phpstan-ignore-next-line\n    $a",
            "// @pun-ignore-next-line analyse/arrays/invalid-array-key\n    $a",
        )
        .replace(
            "/* @phpstan-ignore-line */",
            &format!("/* @pun-ignore {f} */"),
        );
    assert_eq!(read(&d, "app/A.php"), want);
    assert_eq!(
        read(&d, "app/C.php"),
        C.replace(
            "@php-cs-fixer-ignore array_syntax",
            "@pun-ignore-file format/arrays/syntax"
        )
        .replace("// @phpstan-ignore-line", &format!("// @pun-ignore {f}"))
    );
    assert_eq!(
        read(&d, "lib/B.php"),
        "<?php\n\nundefined_b(); // @phpstan-ignore-line\n"
    );
    // Each comment left as is is listed under its reason, the reasons in the order the files were read.
    let left = "   Ignore comments left as is: `pun analyse` reports no error on line 13 for it to name
     app/A.php:12  `@phpstan-ignore-next-line`
   Ignore comments left as is: it ignores line 16, where a Pun comment here can't point
     app/A.php:15  `@phpstan-ignore-next-line`
   Ignore comments left as is: a Pun comment holds one ignore
     app/A.php:32  `@phpstan-ignore-line`
     app/A.php:33  `@phpstan-ignore-next-line`
   Ignore comments left as is: the comment has a `@pun-ignore` already, and a Pun comment holds one ignore
     app/C.php:5  `@phpstan-ignore-next-line`
   Ignore comments left as is: it ignores line 8, where a Pun comment here can't point
     app/C.php:8  `@phpstan-ignore-line`
   Ignore comments left as is: `pun analyse` doesn't check this file
     lib/B.php:3  `@phpstan-ignore-line`
   Want them? Send a PR to https://github.com/shipfastlabs/pun
";
    assert!(out.contains(left), "{out}");
    let (out, _, code) = pun(&d, &["analyse", "--format=raw"]);
    assert_eq!(code, Some(1), "{out}");
    let root = d.canonicalize().unwrap();
    let at = |line: &str| format!("{}/app/A.php:{line}", root.display());
    let c = |line: &str| format!("{}/app/C.php:{line}", root.display());
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [
            format!("{}:No error to ignore is reported on line 13.", at("13")),
            format!("{}:No error to ignore is reported on line 16.", at("16")),
            format!("{}:Function undefined_four not found.", at("18")),
            format!("{}:Function undefined_six not found.", at("21")),
            format!("{}:Function undefined_seven not found.", at("23")),
            format!("{}:No error to ignore is reported on line 32.", at("32")),
            format!("{}:No error to ignore is reported on line 8.", c("8")),
            format!("{}:Function undefined_d not found.", c("9")),
        ]
    );
}

#[test]
fn offers_the_closest_preset() {
    let files = [
        (
            "phpstan.neon",
            "parameters:\n    level: 5\n    paths: [app]\n",
        ),
        ("app/A.php", "<?php\n\nundefined_fn();\n"),
    ];
    let d = project("tty-preset", &files);
    // Without --offer-preset, migrate writes the exact rules and doesn't ask.
    let out = pty(&d, "\\r");
    assert!(!out.contains("closest Pun preset"), "{out}");
    let toml = read(&d, "pun.toml");
    assert!(toml.contains("preset = \"phpstan/level_5\"\n"), "{toml}");
    assert!(!toml.contains("[analyse.rules]"), "{toml}");
    std::fs::remove_file(d.join("pun.toml")).unwrap();
    let offer = |keys: &str| {
        let (out, code) =
            common::pty::pty(&d, "migrate --offer-preset", keys, &[("NO_COLOR", "1")]);
        assert_eq!(code, Some(0), "{out}");
        common::strip_ansi(&out)
    };
    let out = offer("\\ry");
    assert!(
        out.contains("You're on PHPStan level 5; the closest Pun preset is `recommended`. Switch to it from `phpstan/level_5`? ("),
        "{out}"
    );
    assert!(out.contains(", switched to the preset\n"), "{out}");
    let toml = read(&d, "pun.toml");
    assert!(toml.contains("preset = \"recommended\"\n"), "{toml}");
    std::fs::remove_file(d.join("pun.toml")).unwrap();
    let out = offer("\\rn");
    assert!(!out.contains("switched to the preset"), "{out}");
    assert!(read(&d, "pun.toml").contains("preset = \"phpstan/level_5\"\n"));
}

/// `.oracle/<rel>` (scripts/oracle.sh), canonical; `None` skips the cases that need it.
fn oracle(rel: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.oracle")
        .join(rel);
    p.exists().then(|| p.canonicalize().unwrap())
}

#[test]
fn phpstan_baseline_extensions_options_and_ignores() {
    let Some(strict) = oracle("phpstan/vendor/phpstan/phpstan-strict-rules") else {
        return;
    };
    let neon = "includes:\n    - phpstan-baseline.neon\n    - vendor/phpstan/phpstan-strict-rules/rules.neon\n    - vendor/nesbot/carbon/extension.neon\n    - vendor/phpstan/phpstan-mockery/extension.neon\nparameters:\n    level: 9\n    paths:\n        - src\n    additionalConstructors:\n        - App\\Foo::init\n    treatPhpDocTypesAsCertain: false\n    ignoreErrors:\n        - '#Call to an undefined method#'\n";
    let d = project(
        "phpstan-extras",
        &[
            ("phpstan.neon", neon),
            (
                "phpstan-baseline.neon",
                "parameters:\n    ignoreErrors: []\n",
            ),
            ("src/a.php", "<?php\n"),
            // nesbot/carbon 3.15.1's extension.neon, which Pun has built in
            (
                "vendor/nesbot/carbon/extension.neon",
                "parameters:\n    bootstrapFiles:\n        - lazy/Carbon/UnprotectedDatePeriod.php\n\nservices:\n    -\n        class: Carbon\\PHPStan\\MacroExtension\n        tags:\n            - phpstan.broker.methodsClassReflectionExtension\n",
            ),
            (
                "vendor/nesbot/carbon/lazy/Carbon/UnprotectedDatePeriod.php",
                "<?php\n",
            ),
            // phpstan/phpstan-mockery 2.0.0's extension.neon, its services only
            (
                "vendor/phpstan/phpstan-mockery/extension.neon",
                "services:\n    -\n        class: PHPStan\\Mockery\\Type\\MockDynamicReturnTypeExtension\n        tags:\n            - phpstan.broker.dynamicStaticMethodReturnTypeExtension\n",
            ),
            // Locked, but phpstan.neon doesn't load it: its switch is off, as PHPStan runs none of its rules.
            (
                "composer.lock",
                r#"{"packages":[{"name":"nesbot/carbon","version":"3.15.1"}],"packages-dev":[{"name":"spaze/phpstan-disallowed-calls","version":"v4.15.1"}]}"#,
            ),
        ],
    );
    std::fs::create_dir_all(d.join("vendor/phpstan")).unwrap();
    std::os::unix::fs::symlink(strict, d.join("vendor/phpstan/phpstan-strict-rules")).unwrap();
    let (out, _, code) = pun(&d, &["migrate", "--yes"]);
    assert_eq!(code, Some(0), "{out}");
    let out = counts(&out);
    let line = out.lines().next().unwrap();
    assert!(
        line.starts_with(" ok phpstan.neon  -> [analyse] preset phpstan/level_9, N on, N off, "),
        "{out}"
    );
    assert!(
        line.ends_with(" rules -> [lint], 1 parameter -> rule options, 1 error pattern ignored"),
        "{out}"
    );
    assert!(
        out.contains("\n ! Not migrated\n   no pun.toml setting for this parameter yet\n     parameter additionalConstructors\n   a PHPStan check that is this format rule in Pun; turn it on in [format.rules]\n     format/functions/backtick-to-shell-exec\n"),
        "{out}"
    );
    assert!(
        out.contains("\n ! phpstan-baseline.neon was not copied. Run `pun analyse --generate-baseline` for a Pun baseline.\n"),
        "{out}"
    );
    assert!(
        !out.contains("nesbot/carbon") && !out.contains("phpstan-mockery"),
        "{out}"
    );
    let toml = read(&d, "pun.toml");
    assert!(
        toml.contains(
            "[extensions]\ncarbon = \"on\"\nmockery = \"on\"\ndisallowed-calls = \"off\"\n\n"
        ),
        "{toml}"
    );
    assert!(
        toml.contains(
            "\"control-flow/unhandled-match\" = { treat-php-doc-types-as-certain = false }\n"
        ) && !toml.contains("additional-constructors"),
        "{toml}"
    );
    assert!(
        toml.contains("[[analyse.ignore]]\nmessage = \"#Call to an undefined method#\"\n"),
        "{toml}"
    );
}

/// rector.php read through Rector 2.8.4 (`.oracle/rector-bin`): its paths become `[lint] exclude` for the files
/// outside them, a rule skipped for a path an override, and what Pun can't run is listed (a reason shared by more
/// than three rules once, with the count).
#[test]
fn rector_paths_skips_and_unsupported_rules() {
    let Some(vendor) = oracle("rector-bin/vendor") else {
        return;
    };
    let rector = "<?php\n\nuse Rector\\Config\\RectorConfig;\nuse Rector\\DeadCode\\Rector\\ClassMethod\\RemoveUselessParamTagRector;\n\nreturn RectorConfig::configure()\n    ->withPaths([__DIR__.'/src'])\n    ->withPreparedSets(deadCode: true)\n    ->withPhpSets(php53: true)\n    ->withSkip([RemoveUselessParamTagRector::class => [__DIR__.'/src/Sub']]);\n";
    let d = project(
        "rector",
        &[
            ("composer.json", "{\"require\":{\"php\":\"^8.4\"}}"),
            ("rector.php", rector),
            ("src/a.php", "<?php\n"),
            ("src/Sub/b.php", "<?php\n"),
            ("scripts/x.php", "<?php\n"),
            ("tools/deep/y.php", "<?php\n"),
            ("tools/z.php", "<?php\n"),
        ],
    );
    std::os::unix::fs::symlink(vendor, d.join("vendor")).unwrap();
    let (out, err, code) = pun(&d, &["migrate", "--yes"]);
    assert_eq!(code, Some(0), "{out}{err}");
    let out = counts(&out);
    assert!(
        out.starts_with(" ok rector.php  -> [lint] N on, N off, 3 paths excluded, 1 per-path skip\n                   sets rector/dead-code, rector/php-polyfills, rector/php-sets\n"),
        "{out}"
    );
    assert!(
        out.contains("\n   only in php52..php81 upgrade sets (below php82)\n     4 Rector rules\n"),
        "{out}"
    );
    assert!(out.contains(PR), "{out}");
    let toml = read(&d, "pun.toml");
    assert!(
        toml.contains("exclude = [\"rector.php\", \"scripts\", \"tools\"]\n"),
        "{toml}"
    );
    assert!(
        toml.contains("[[lint.overrides]]\nfiles = [\"src/Sub\"]\nrules = { \"phpdoc/no-useless-param\" = \"off\" }\n"),
        "{toml}"
    );
    assert!(
        toml.contains("\n\"control-flow/no-return-assignment\" = { skip-commented = true }\n"),
        "{toml}"
    );
    std::fs::remove_file(d.join("pun.toml")).unwrap();
    let whole = (rector.replace("__DIR__.'/src'", "__DIR__")).replace(
        "    ->withPreparedSets(deadCode: true)\n    ->withPhpSets(php53: true)\n",
        "",
    );
    std::fs::write(d.join("rector.php"), whole).unwrap();
    let (out, _, code) = pun(&d, &["migrate", "--yes"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(
        counts(&out).starts_with(" ok rector.php  -> [lint] N on, N off"),
        "{out}"
    );
    assert!(!out.contains("excluded"), "{out}");
    std::fs::remove_file(d.join("pun.toml")).unwrap();
    std::fs::write(d.join("rector.php"), "<?php\nreturn 1;\n").unwrap();
    let (_, err, code) = pun(&d, &["migrate", "--yes"]);
    assert_eq!(
        (err.as_str(), code),
        (
            "x can't read rector.php with Rector: rector.php must return RectorConfig::configure() or a closure taking RectorConfig\n",
            Some(2)
        )
    );
    std::fs::write(
        d.join("rector.php"),
        "<?php\nfunction f() {}\nfunction f() {}\n",
    )
    .unwrap();
    let (_, err, code) = pun(&d, &["migrate", "--yes"]);
    let file = d.join("rector.php");
    let file = file.display();
    assert_eq!(
        (err, code),
        (
            format!(
                "x can't read rector.php with Rector: Fatal error: Cannot redeclare function f() (previously declared in {file}:2) in {file} on line 3\n"
            ),
            Some(2)
        )
    );
}

#[test]
fn phpstan_ignore_of_a_format_check_is_left() {
    let php = "<?php\n\n$a = 1; // @phpstan-ignore whitespace.fileEnd\n";
    let d = project(
        "phpstan-format-ignore",
        &[
            (
                "phpstan.neon",
                "parameters:\n    level: 5\n    paths: [app]\n",
            ),
            ("app/A.php", php),
        ],
    );
    let (out, _, code) = pun(&d, &["migrate", "--dry-run", "--yes"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(
        out.contains("   Ignore comments left as is: `whitespace.fileEnd` is the format rule `format/tags/no-closing`, ignored per file\n     app/A.php:3  `@phpstan-ignore whitespace.fileEnd`\n"),
        "{out}"
    );
}

#[test]
fn checklist_marks_unsupported_configs() {
    let d = project("tty-mago", &[("pint.json", PINT), ("mago.toml", "")]);
    let out = pty(&d, "\\r");
    assert!(
        out.contains("Mago          mago.toml  (not supported yet)\n"),
        "{out}"
    );
}

#[test]
fn leaves_composer_json_and_its_lock_alone() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../pun-packages/tests/fixtures");
    let d = project("composer", &[("pint.json", PINT)]);
    let copied = std::process::Command::new("cp")
        .arg("-R")
        .arg(fixtures.join("project/."))
        .arg(&d)
        .status()
        .unwrap();
    assert!(copied.success());
    let (json, lock) = (read(&d, "composer.json"), read(&d, "composer.lock"));

    let (out, err, code) = pun(&d, &["migrate", "--yes"]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(!out.contains("composer"), "{out}");
    assert_eq!(read(&d, "pun.toml"), TOML);
    assert_eq!(
        (read(&d, "composer.json"), read(&d, "composer.lock")),
        (json, lock)
    );
    assert!(!d.join("pun.lock").exists() && !d.join("auth.json").exists());

    let (out, err, code) = pun(&d, &["install"]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(
        err.starts_with("Installing 2 packages from composer.lock\n")
            && !err.contains("out of date"),
        "{err}"
    );
    let expected =
        std::fs::read_to_string(fixtures.join("project-expected/composer/installed.php")).unwrap();
    assert_eq!(read(&d, "vendor/composer/installed.php"), expected);

    std::fs::write(d.join("pun.toml"), format!("{TOML}# mine\n")).unwrap();
    let (_, err, code) = pun(&d, &["migrate", "--yes"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("pun.toml exists and differs"), "{err}");
}

/// Rector prints "The per-version PHP set ... is deprecated" to stdout for each `SetList::PHP_8x` (2.8.4 in
/// `.oracle/rector-bin`, and 2.8.5, the latest, alike); migrate reads its own JSON after the warnings.
#[test]
fn rector_warnings_on_stdout() {
    let Some(vendor) = oracle("rector-bin/vendor") else {
        return;
    };
    let rector = "<?php\n\nuse Rector\\Config\\RectorConfig;\nuse Rector\\Set\\ValueObject\\SetList;\n\nreturn RectorConfig::configure()\n    ->withPaths([__DIR__.'/src'])\n    ->withSets([SetList::PHP_82, SetList::PHP_83, SetList::PHP_84]);\n";
    let d = project(
        "rector-deprecated-sets",
        &[
            ("composer.json", "{\"require\":{\"php\":\"^8.4\"}}"),
            ("rector.php", rector),
            ("src/a.php", "<?php\n"),
        ],
    );
    std::os::unix::fs::symlink(vendor, d.join("vendor")).unwrap();
    let (out, err, code) = pun(&d, &["migrate", "--yes"]);
    assert_eq!((err.as_str(), code), ("", Some(0)), "{out}");
    assert!(
        counts(&out).starts_with(" ok rector.php  -> [lint] N on, N off"),
        "{out}"
    );
    let toml = read(&d, "pun.toml");
    assert!(
        toml.contains("sets = [\"rector/php82\", \"rector/php83\", \"rector/php84\"]\n"),
        "{toml}"
    );
}

/// Issue #22: rector.php's settings that pun.toml has no equivalent for are listed under "Not migrated", by the
/// `RectorConfig` method that sets them, and the ones left at Rector's defaults are not (`withIndent(' ', 4)`).
#[test]
fn rector_settings_without_an_equivalent_are_listed() {
    let Some(vendor) = oracle("rector-bin/vendor") else {
        return;
    };
    let rector = "<?php\n\nuse Rector\\Config\\RectorConfig;\n\nreturn RectorConfig::configure()\n    ->withPaths([__DIR__.'/src'])\n    ->withImportNames()\n    ->withTreatClassesAsFinal()\n    ->withPhpVersion(80200)\n    ->withIndent(' ', 4);\n";
    let d = project(
        "rector-settings",
        &[
            ("composer.json", "{\"require\":{\"php\":\"^8.4\"}}"),
            ("rector.php", rector),
            ("src/a.php", "<?php\n"),
        ],
    );
    std::os::unix::fs::symlink(vendor, d.join("vendor")).unwrap();
    let (out, err, code) = pun(&d, &["migrate", "--yes"]);
    assert_eq!((err.as_str(), code), ("", Some(0)), "{out}");
    assert!(
        out.contains("\n   no project-wide setting in Pun: lint fixes write class names fully qualified, and `lint/imports/no-unused` removes unused imports\n     withImportNames()\n"),
        "{out}"
    );
    assert!(
        out.contains("\n   no project-wide setting in Pun; `lint/types/add-param-type-method-call` and `lint/php-8.3/add-const-type` take it as their `treat-classes-as-final` option\n     withTreatClassesAsFinal()\n"),
        "{out}"
    );
    assert!(
        out.contains("\n   Pun takes the PHP version from `require.php` in composer.json; set [php] version for another\n     withPhpVersion()\n"),
        "{out}"
    );
    assert!(!out.contains("withIndent()"), "{out}");
}

/// The PHP 8.2 set read through Rector 2.8.4 (`.oracle/rector-bin`): `SetList::PHP_82` lists the set's four rules
/// (`rector list-rules`), which become `[lint.rules]`, and `->withPhpSets(php82: true)` the `modernize` set, which
/// runs them. `pun lint --fix` then writes what `rector process` with `SetList::PHP_82` wrote for the same file.
#[test]
fn rector_php82_set_turns_on_the_php_8_2_rules() {
    let Some(vendor) = oracle("rector-bin/vendor") else {
        return;
    };
    let code = "<?php\n\nfinal class Money\n{\n    public function __construct(public readonly int $amount)\n    {\n    }\n\n    public function decode(string $s): string\n    {\n        new \\FilesystemIterator(__DIR__, \\FilesystemIterator::KEY_AS_FILENAME);\n\n        return utf8_decode(\"${s}\");\n    }\n}\n";
    let rector = "<?php\n\nfinal readonly class Money\n{\n    public function __construct(public int $amount)\n    {\n    }\n\n    public function decode(string $s): string\n    {\n        new \\FilesystemIterator(__DIR__, \\FilesystemIterator::KEY_AS_FILENAME | \\FilesystemIterator::SKIP_DOTS);\n\n        return mb_convert_encoding(\"{$s}\", 'ISO-8859-1');\n    }\n}\n";
    for (name, config, summary) in [
        (
            "rector-php82-set-list",
            "use Rector\\Set\\ValueObject\\SetList;\n\nreturn RectorConfig::configure()\n    ->withPaths([__DIR__.'/src'])\n    ->withSets([SetList::PHP_82]);\n",
            " ok rector.php  -> [lint] N on, N off",
        ),
        (
            "rector-php82-php-sets",
            "return RectorConfig::configure()\n    ->withPaths([__DIR__.'/src'])\n    ->withPhpSets(php82: true);\n",
            " ok rector.php  -> [lint] N on, N off, 1 path excluded\n                   sets rector/php-polyfills, rector/php-sets",
        ),
    ] {
        let d = project(
            name,
            &[
                ("composer.json", "{\"require\":{\"php\":\"^8.4\"}}"),
                (
                    "rector.php",
                    &format!("<?php\n\nuse Rector\\Config\\RectorConfig;\n{config}"),
                ),
                ("src/a.php", code),
            ],
        );
        std::os::unix::fs::symlink(&vendor, d.join("vendor")).unwrap();
        let (out, err, status) = pun(&d, &["migrate", "--yes"]);
        assert_eq!((err.as_str(), status), ("", Some(0)), "{out}");
        assert!(counts(&out).starts_with(summary), "{out}");
        let toml = read(&d, "pun.toml");
        for id in [
            "prefer-readonly-class",
            "no-utf8-encode-decode",
            "add-skip-dots-flag",
            "no-dollar-brace-interpolation",
        ] {
            // Both sets hold the rule, so pun.toml doesn't name it.
            let on = format!("\"php-8.2/{id}\" = \"error\"\n");
            assert!(!toml.contains(&on), "{name}: {toml}");
        }
        let sets = if name.ends_with("set-list") {
            "rector/php82"
        } else {
            "rector/php-polyfills\", \"rector/php-sets"
        };
        assert!(
            toml.contains(&format!("sets = [\"{sets}\"]\n")),
            "{name}: {toml}"
        );
        pun(&d, &["lint", "--fix"]);
        assert_eq!(read(&d, "src/a.php"), rector, "{name}");
    }
}

/// phpstan.neon alone, beside a pint.json the project keeps: pun.toml gets no [format] (Pint's defaults, as Pint
/// without a pint.json), `phpVersion` becomes [php] version, and the custom rule is named, for humans and agents.
#[test]
fn phpstan_alone_beside_a_format_config() {
    let neon = "parameters:\n    level: 0\n    paths: [app]\n    phpVersion: 80400\nrules:\n    - Support\\Lint\\NoSeederDataCreationInMigration\n";
    let ignored = "<?php\n\n// @php-cs-fixer-ignore array_syntax\n$a = array(1);\n";
    let files = [
        ("composer.json", "{\"require\":{\"php\":\"^8.4\"}}"),
        ("pint.json", PINT),
        ("phpstan.neon", neon),
        ("app/A.php", ignored),
    ];
    let d = project("phpstan-alone", &files);
    let (out, err, code) = pun(&d, &["migrate", "--from", "phpstan"]);
    let rule = "rule Support\\Lint\\NoSeederDataCreationInMigration";
    assert_eq!(
        (counts(&out).as_str(), err.as_str(), code),
        (
            format!(
                " - pint.json     not selected
 ok phpstan.neon  -> [analyse] preset phpstan/level_0, 1 rule -> [lint], phpVersion -> [php] version 8.4

 ! Not migrated
   a custom PHPStan rule; Pun runs no PHP rules
     {rule}
   a PHPStan check that is this format rule in Pun; turn it on in [format.rules]
     format/functions/backtick-to-shell-exec
   Want them? Send a PR to https://github.com/shipfastlabs/pun

 ok Wrote pun.toml
"
            )
            .as_str(),
            "",
            Some(0)
        )
    );
    let toml = read(&d, "pun.toml");
    assert!(
        toml.starts_with("# Written by `pun migrate` without [format]: pint.json not selected.\n[php]\nversion = \"8.4\"\n\n"),
        "{toml}"
    );
    assert!(!toml.lines().any(|l| l.starts_with("[format")), "{toml}");
    assert_eq!(read(&d, "app/A.php"), ignored);
    let (out, _, code) = pun(&d, &["config", "--format=json"]);
    assert_eq!(code, Some(0), "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["format"]["preset"], "laravel");
    std::fs::remove_file(d.join("pun.toml")).unwrap();
    let (out, _, code) = pun(&d, &["migrate", "--from", "phpstan", "--agent"]);
    assert_eq!(code, Some(0));
    assert!(
        out.contains(&format!(
            "\nphpstan.neon unsupported {rule}: a custom PHPStan rule; Pun runs no PHP rules\n"
        )),
        "{out}"
    );
}
