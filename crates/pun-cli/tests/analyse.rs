//! `pun analyse` against PHPStan 2.3.5 (`.oracle/phpstan`, scripts/oracle.sh): every `--format` (PHPStan's
//! `--error-format`) byte for byte and the exit code, on a project with errors and on a clean one. Without the
//! oracle (or `php`) the comparisons are skipped. The baseline and ignore cases are derived by hand from
//! context/plan/analyse.md Build 2 and rules.md §4.
mod common;

use common::{PUN, clean_env, project};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

const FORMATS: [&str; 7] = [
    "raw",
    "json",
    "checkstyle",
    "junit",
    "gitlab",
    "github",
    "teamcity",
];

const ERRORS: &str = "<?php\n\nnamespace App;\n\nfunction f(): void\n{\n    undefined_fn();\n    new Missing();\n}\n\nclass B\n{\n    public function g(): void\n    {\n        $this->nope();\n    }\n}\n";
const CLEAN: &str = "<?php\n\nnamespace App;\n\nfunction f(): int\n{\n    return 1;\n}\n";
const NON_IGNORABLE: &str = "<?php\nfunction duplicate($value, $value): void {}\nundefined_fn();\n";

fn phpstan() -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.oracle/phpstan/vendor/phpstan/phpstan/phpstan.phar");
    p.exists().then(|| p.canonicalize().unwrap())
}

fn sample(name: &str, source: &str) -> PathBuf {
    project(
        "analyse",
        name,
        &[
            (
                "phpstan.neon",
                "parameters:\n    level: 0\n    paths:\n        - src\n",
            ),
            ("pun.toml", "[analyse]\npaths = [\"src\"]\n"),
            ("src/a.php", source),
        ],
    )
}

fn run(dir: &Path, program: &[&str], args: &[&str]) -> (String, Option<i32>) {
    let out = clean_env(&mut Command::new(program[0]), &["PATH", "HOME"])
        .current_dir(dir)
        .env("COLUMNS", "80")
        .env("PUN_AGENT", "0")
        .args(&program[1..])
        .args(args)
        .output()
        .unwrap();
    (String::from_utf8(out.stdout).unwrap(), out.status.code())
}

fn formats_match_phpstan(name: &str, source: &str) {
    let Some(phar) = phpstan() else { return };
    let phar = phar.to_str().unwrap();
    let d = sample(name, source);
    for f in FORMATS {
        let format = format!("--error-format={f}");
        let want = run(
            &d,
            &[
                "php",
                phar,
                "analyse",
                "--no-progress",
                "-c",
                "phpstan.neon",
            ],
            &[&format],
        );
        let got = run(&d, &[PUN, "analyse"], &[&format]);
        assert_eq!(got, want, "{name} {f}");
    }
    let want = run(
        &d,
        &[
            "php",
            phar,
            "analyse",
            "--no-progress",
            "-c",
            "phpstan.neon",
            "-v",
        ],
        &["--error-format=raw"],
    );
    assert_eq!(
        run(&d, &[PUN, "analyse"], &["--error-format=raw", "-v"]),
        want,
        "{name} raw -v"
    );
}

#[test]
fn formats_match_phpstan_errors() {
    formats_match_phpstan("errors", ERRORS);
}

#[test]
fn formats_match_phpstan_clean() {
    formats_match_phpstan("clean", CLEAN);
}

#[test]
fn formats_match_phpstan_non_ignorable() {
    formats_match_phpstan("non-ignorable", NON_IGNORABLE);
}

#[test]
fn formats_match_phpstan_trait_context() {
    formats_match_phpstan(
        "trait-context",
        "<?php\n\nnamespace App\\Models;\n\ntrait T\n{\n    public function make(): static\n    {\n        return new static();\n    }\n}\n\nclass C\n{\n    use T;\n}\n",
    );
}

#[test]
fn anonymous_class_names_match_phpstan() {
    let Some(phar) = phpstan() else { return };
    let source = |class: &str, function: &str| {
        format!(
            "<?php\n\nnamespace App;\n\nclass {class}\n{{\n    public function __construct(public int $x)\n    {{\n    }}\n}}\n\nfunction {function}(): void\n{{\n    new class('a') extends {class} {{}};\n}}\n"
        )
    };
    let d = project(
        "analyse",
        "anonymous-names",
        &[
            (
                "phpstan.neon",
                "parameters:\n    level: max\n    paths:\n        - src\n",
            ),
            (
                "pun.toml",
                "[analyse]\npreset = \"max\"\npaths = [\"src\"]\n",
            ),
            ("src/Sub/a.php", &source("R", "f")),
            ("src/b.php", &source("Q", "g")),
        ],
    );
    let want = run(
        &d,
        &[
            "php",
            phar.to_str().unwrap(),
            "analyse",
            "--no-progress",
            "-c",
            "phpstan.neon",
        ],
        &["--error-format=raw"],
    );
    assert!(
        want.0.contains("App\\R@anonymous/Sub/a.php:14 constructor"),
        "{}",
        want.0
    );
    assert_eq!(run(&d, &[PUN, "analyse"], &["--error-format=raw"]), want);
}

#[test]
fn unused_traits_follow_the_analysed_paths() {
    let Some(phar) = phpstan() else { return };
    let d = project(
        "analyse",
        "unused-trait",
        &[
            (
                "phpstan.neon",
                "parameters:\n    level: max\n    paths:\n        - src\n",
            ),
            (
                "pun.toml",
                "[analyse]\npreset = \"max\"\npaths = [\"src\"]\n",
            ),
            (
                "src/a.php",
                "<?php\n\nnamespace App;\n\ntrait Unused\n{\n}\n",
            ),
        ],
    );
    let phpstan = [
        "php",
        phar.to_str().unwrap(),
        "analyse",
        "--no-progress",
        "-c",
        "phpstan.neon",
    ];
    let want = run(&d, &phpstan, &["--error-format=raw"]);
    assert!(
        want.0.contains("Trait App\\Unused is used zero times"),
        "{}",
        want.0
    );
    assert_eq!(run(&d, &[PUN, "analyse"], &["--error-format=raw"]), want);
    let want = run(&d, &phpstan, &["--error-format=raw", "src/a.php"]);
    assert!(!want.0.contains("Trait App\\Unused"), "{}", want.0);
    assert_eq!(
        run(&d, &[PUN, "analyse"], &["--error-format=raw", "src/a.php"]),
        want
    );
}

#[test]
fn formats_match_phpstan_dumps() {
    formats_match_phpstan(
        "dumps",
        "<?php\n\\PHPStan\\dumpType(1, true); // @phpstan-ignore phpstan.dumpType\n\\PHPStan\\dumpPhpDocType(['value' => 2]);\n",
    );
}

/// PHPStan's `table` and `prettyJson` are not Pun formats.
#[test]
fn unknown_format_is_a_run_error() {
    let d = sample("unknown", CLEAN);
    for format in [
        "--format=nope",
        "--format=table",
        "--error-format=prettyJson",
    ] {
        let (out, code) = run(&d, &[PUN, "analyse"], &[format]);
        assert_eq!((out.as_str(), code), ("", Some(2)), "{format}");
    }
}

#[test]
fn parser_failures_are_severe_not_configurable_rule_errors() {
    let Some(phar) = phpstan() else { return };
    let phar = phar.to_str().unwrap();
    let d = sample("parser-severe", "<?php\n#[\\self] class InvalidA {}\n");
    common::write_files(
        &d,
        &[
            ("src/b.php", "<?php\n#[\\self] class InvalidB {}\n"),
            ("src/c.php", "<?php\nnew \\parent;\n"),
            ("src/control.php", ERRORS),
            (
                "phpstan.neon",
                "parameters:\n    level: 0\n    paths: [src]\n    ignoreErrors:\n        - '#^never matches$#'\n        -\n            identifier: phpstan.parse\n",
            ),
            (
                "pun.toml",
                "[analyse]\npaths = [\"src\"]\n[[analyse.ignore]]\nmessage = \"#^never matches$#\"\n[[analyse.ignore]]\nidentifier = \"phpstan.parse\"\n[analyse.rules]\n\"analyse/undefined/function\" = \"off\"\n",
            ),
        ],
    );
    for format in FORMATS {
        let arg = format!("--error-format={format}");
        let want = run(
            &d,
            &[
                "php",
                phar,
                "analyse",
                "--no-progress",
                "-c",
                "phpstan.neon",
            ],
            &[&arg],
        );
        assert_eq!(want.1, Some(1));
        assert_eq!(run(&d, &[PUN, "analyse"], &[&arg]), want, "{format}");
    }
    std::fs::write(
        d.join("src/a.php"),
        "<?php\n#[\\self] // @pun-ignore\nclass InvalidA {}\n",
    )
    .unwrap();
    let (out, code) = run(&d, &[PUN, "analyse"], &["--format=json"]);
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(code, Some(1));
    assert_eq!(json["totals"]["file_errors"], 2, "{out}");
    assert_eq!(json["totals"]["errors"], 0, "{out}");
    let (_, code) = run(&d, &[PUN, "analyse"], &["--generate-baseline", "--quiet"]);
    assert_eq!(code, Some(1));
    assert!(!d.join("pun-baseline.toml").exists());
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn syntax_errors_fail_the_run() {
    let d = sample("syntax-errors", "<?php\n$a = 1\n$b = 2;\n");
    common::write_files(
        &d,
        &[
            ("src/b.php", "<?php\nfunction f() {\n"),
            ("src/control.php", ERRORS),
        ],
    );
    let (out, code) = run(&d, &[PUN, "analyse"], &["--format=json"]);
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(code, Some(1));
    let mut found: Vec<(String, String, u64)> = (json["files"].as_object().unwrap().iter())
        .flat_map(|(file, entry)| {
            let name = file.rsplit('/').next().unwrap().to_string();
            (entry["messages"].as_array().unwrap().iter()).map(move |m| {
                let message = m["message"].as_str().unwrap().to_string();
                (name.clone(), message, m["line"].as_u64().unwrap())
            })
        })
        .collect();
    found.sort();
    let want = [
        (
            "a.php",
            "Syntax error, unexpected variable \"$b\" on line 3",
            3,
        ),
        ("b.php", "Unclosed '{' on line 2 on line 3", 3),
    ];
    let want: Vec<_> = (want.iter())
        .map(|(f, m, l)| (f.to_string(), m.to_string(), *l))
        .collect();
    assert_eq!(found, want, "{out}");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn non_ignorable_errors_survive_comments_and_baselines() {
    let d = sample("non-ignorable-baseline", NON_IGNORABLE);
    let source = NON_IGNORABLE
        .replace(
            "void {}",
            "void {} // @pun-ignore analyse/functions/duplicate-parameter",
        )
        .replace(
            "undefined_fn();",
            "undefined_fn(); // @pun-ignore analyse/undefined/function",
        );
    std::fs::write(d.join("src/a.php"), source).unwrap();
    let (out, code) = run(&d, &[PUN, "analyse"], &["--format=json"]);
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(code, Some(1), "{out}");
    assert_eq!(json["totals"]["file_errors"], 1, "{out}");
    let file = d.join("src/a.php").to_string_lossy().into_owned();
    assert_eq!(
        json["files"][&file]["messages"][0]["identifier"], "parameter.duplicate",
        "{out}"
    );
    assert_eq!(
        json["files"][&file]["messages"][0]["ignorable"], false,
        "{out}"
    );

    std::fs::write(d.join("src/a.php"), NON_IGNORABLE).unwrap();
    let (out, code) = run(
        &d,
        &[PUN, "analyse"],
        &["--generate-baseline", "--color=never"],
    );
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("Baseline generated with 1 error in"), "{out}");
    let mut baseline = std::fs::read_to_string(d.join("pun-baseline.toml")).unwrap();
    assert!(
        baseline.contains("Function undefined_fn not found."),
        "{baseline}"
    );
    assert!(!baseline.contains("Redefinition"), "{baseline}");
    baseline.push_str("\n[[issues]]\nfile = 'src/a.php'\ncode = 'analyse/functions/duplicate-parameter'\nmessage = 'Redefinition of parameter $value.'\ncount = 1\n");
    std::fs::write(d.join("pun-baseline.toml"), baseline).unwrap();
    std::fs::write(
        d.join("pun.toml"),
        "[analyse]\npaths = [\"src\"]\nbaseline = \"pun-baseline.toml\"\n",
    )
    .unwrap();
    let (out, code) = run(&d, &[PUN, "analyse"], &["--format=json"]);
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(code, Some(1), "{out}");
    assert_eq!(json["totals"]["file_errors"], 1, "{out}");
    assert_eq!(
        json["files"][&file]["messages"][0]["identifier"], "parameter.duplicate",
        "{out}"
    );
}

#[test]
fn baseline_covers_the_errors_it_lists() {
    let d = sample("baseline", ERRORS);
    let (out, code) = run(
        &d,
        &[PUN, "analyse"],
        &["--generate-baseline", "--color=never"],
    );
    assert_eq!(code, Some(0), "{out}");
    assert!(
        out.contains("Baseline generated with 3 errors in pun-baseline.toml"),
        "{out}"
    );
    let baseline = std::fs::read_to_string(d.join("pun-baseline.toml")).unwrap();
    assert!(baseline.contains("variant = \"loose\""), "{baseline}");
    assert!(
        baseline.contains("message = 'Call to an undefined method App\\B::nope().'"),
        "{baseline}"
    );
    assert_eq!(run(&d, &[PUN, "analyse"], &["--format=raw"]).1, Some(1));
    std::fs::write(
        d.join("pun.toml"),
        "[analyse]\npaths = [\"src\"]\nbaseline = \"pun-baseline.toml\"\n",
    )
    .unwrap();
    assert_eq!(
        run(&d, &[PUN, "analyse"], &["--format=raw"]),
        (String::new(), Some(0))
    );
    std::fs::write(
        d.join("src/a.php"),
        ERRORS.replace("$this->nope();", "nope2();"),
    )
    .unwrap();
    let (out, code) = run(&d, &[PUN, "analyse"], &["--color=never"]);
    assert!(out.contains("nope2 not found."), "{out}");
    assert!(
        out.contains(
            "1 baseline entry no longer matches: run `pun analyse --generate-baseline` to drop it\n"
        ),
        "{out}"
    );
    assert_eq!(code, Some(1));
    std::fs::write(d.join("src/a.php"), CLEAN).unwrap();
    let (out, _) = run(&d, &[PUN, "analyse"], &["--color=never"]);
    assert!(
        out.contains("! 3 baseline entries no longer match: run `pun analyse --generate-baseline` to drop them\n"),
        "{out}"
    );
}

#[test]
fn ignore_comments_drop_errors() {
    let source = ERRORS
        .replace(
            "    undefined_fn();",
            "    undefined_fn(); // @pun-ignore analyse/undefined/function",
        )
        .replace(
            "    new Missing();",
            "    // @pun-ignore\n    new Missing();",
        );
    let d = sample("ignores", &source);
    let (out, code) = run(&d, &[PUN, "analyse"], &["--format=raw"]);
    let lines: Vec<&str> = out
        .lines()
        .map(|l| l.rsplit_once("/src/").unwrap().1)
        .collect();
    // The bare `@pun-ignore` ignores nothing and is an error itself.
    assert_eq!(
        lines,
        [
            "a.php:8:`@pun-ignore` needs one or more rule IDs",
            "a.php:9:Instantiated class App\\Missing not found.",
            "a.php:16:Call to an undefined method App\\B::nope().",
        ]
    );
    assert_eq!(code, Some(1));
}

#[test]
fn ignore_comments_drop_errors_in_traits() {
    // PHPStan reports `a.php (in context of class App\C):9:Call to an undefined method App\C::nope().`
    let source = "<?php\n\nnamespace App;\n\ntrait T\n{\n    public function f(): void\n    {\n        $this->nope(); // @pun-ignore analyse/undefined/method\n    }\n}\n\nclass C\n{\n    use T;\n}\n";
    let d = sample("trait-ignores", source);
    assert_eq!(
        run(&d, &[PUN, "analyse"], &["--format=raw"]),
        (String::new(), Some(0))
    );
}

#[test]
fn unused_variables_are_reported() {
    // PHPStan 2.3.5, level max with bleedingEdge (featureToggles.unusedVariable), on the same file.
    let d = project(
        "analyse",
        "unused-variables",
        &[
            (
                "pun.toml",
                "[analyse]\npreset = \"max\"\npaths = [\"src\"]\nbleeding-edge = true\n",
            ),
            (
                "src/a.php",
                "<?php\n\nfunction f(string $value): string\n{\n    $account = 'x';\n    [$a, $b] = explode('@', $value);\n\n    return $value . $b;\n}\n",
            ),
        ],
    );
    let (out, code) = run(&d, &[PUN, "analyse"], &["--format=raw"]);
    let lines: Vec<&str> = out
        .lines()
        .map(|l| l.rsplit_once("/src/").unwrap().1)
        .collect();
    assert_eq!(
        lines,
        [
            "a.php:5:Variable $account is never read.",
            "a.php:6:Variable $a is never read.",
        ]
    );
    assert_eq!(code, Some(1));
}

#[test]
fn identifiers_lint_owns_are_reported_too() {
    // PHPStan 2.3.6 at level 5 (`--error-format=raw`) on the same file. Lint rules own these identifiers (rules.md
    // O5) but see no types, so analyse still reports them.
    let source = "<?php\n\nnamespace App;\n\nfinal class Probe\n{\n    private int $unused = 1;\n\n    private int $written = 0;\n\n    public function __construct(private Probe $next) {}\n\n    public function nullsafe(): int\n    {\n        $this->written = 5;\n\n        return $this->next?->count();\n    }\n\n    public function count(): int\n    {\n        return 1;\n    }\n\n    public function conditions(int $a): int\n    {\n        $b = 1;\n        if ($b === 1) {\n            return 2;\n        }\n\n        return $a;\n    }\n\n    public function both(int $a): bool\n    {\n        $t = true;\n\n        return $t && $a > 1;\n    }\n\n    private function wide(): int|string\n    {\n        return 1;\n    }\n\n    public function callWide(): int|string\n    {\n        return $this->wide();\n    }\n}\n";
    let d = project(
        "analyse",
        "lint-owned",
        &[
            (
                "pun.toml",
                "[analyse]\npreset = \"recommended\"\npaths = [\"src\"]\n\n[lint.rules]\n\"complexity/no-redundant-nullsafe\" = \"error\"\n\"dead-code/no-unreachable-code\" = \"error\"\n",
            ),
            ("src/Probe.php", source),
        ],
    );
    let (out, code) = run(&d, &[PUN, "analyse"], &["--format=raw"]);
    let lines: Vec<&str> = out
        .lines()
        .map(|l| l.rsplit_once("/src/").unwrap().1)
        .collect();
    assert_eq!(
        lines,
        [
            "Probe.php:7:Property App\\Probe::$unused is never read, only written.",
            "Probe.php:9:Property App\\Probe::$written is never read, only written.",
            "Probe.php:17:Using nullsafe method call on non-nullable type App\\Probe. Use -> instead.",
            "Probe.php:28:Strict comparison using === between 1 and 1 will always evaluate to true.",
            "Probe.php:32:Unreachable statement - code above always terminates.",
            "Probe.php:39:Left side of && is always true.",
            "Probe.php:42:Method App\\Probe::wide() never returns string so it can be removed from the return type.",
        ]
    );
    assert_eq!(code, Some(1));
}

fn pun(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (String, String, Option<i32>) {
    let out = clean_env(&mut Command::new(PUN), &["PATH", "HOME"])
        .current_dir(dir)
        .env("COLUMNS", "80")
        .env("PUN_AGENT", "0")
        .envs(env.iter().copied())
        .arg("analyse")
        .args(args)
        .output()
        .unwrap();
    let s = |b: Vec<u8>| String::from_utf8(b).unwrap();
    (s(out.stdout), s(out.stderr), out.status.code())
}

const MALFORMED: &str = "<?php\n\nnamespace App;\n\nfunction f(): void\n{\n    undefined_fn();\n    new Missing(); // @pun-ignore\n}\n";

/// A file whose name holds ` (in context of ` is not a trait's error label: its excerpt comes from the file itself,
/// not from the file its name starts with.
#[test]
fn file_names_like_trait_labels_show_their_own_source() {
    let d = sample("context-name", CLEAN);
    let named = "src/s (in context of x.php";
    common::write_files(
        &d,
        &[
            (named, "<?php\n\nundefined_fn();\n"),
            ("secret.txt", "line one\nline two\nTOP SECRET\n"),
        ],
    );
    std::os::unix::fs::symlink(d.join("secret.txt"), d.join("src/s")).unwrap();
    let (out, _, code) = pun(&d, &["--color=never"], &[]);
    assert_eq!(code, Some(1));
    assert!(!out.contains("SECRET"), "{out}");
    assert!(out.contains("3| undefined_fn();"), "{out}");
    assert!(out.contains(&format!(",- {named}:3:1")), "{out}");
    let out = clean_env(&mut Command::new(PUN), &["PATH", "HOME"])
        .current_dir(&d)
        .env("PUN_AGENT", "0")
        .args(["check", "--color=never"])
        .output()
        .unwrap();
    let out = String::from_utf8(out.stdout).unwrap();
    assert!(
        out.contains(&format!("   {named}:3:1  analyse/undefined/function\n")),
        "{out}"
    );
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn malformed_ignores_show_in_every_output() {
    let d = sample("malformed", MALFORMED);
    let (out, _, code) = pun(&d, &["--color=never"], &[]);
    assert!(
        out.contains(" x invalid-ignore-comment  `@pun-ignore` needs one or more rule IDs\n  ,- src/a.php:8:23\n"),
        "{out}"
    );
    assert_eq!(code, Some(1));
    let (out, _, _) = pun(&d, &["--color=never"], &[("GITHUB_ACTIONS", "true")]);
    let annotations: Vec<&str> = out.lines().filter(|l| l.starts_with("::")).collect();
    assert_eq!(
        annotations,
        [
            "::error file=src/a.php,line=7,col=5,title=analyse/undefined/function::Function undefined_fn not found.",
            "::error file=src/a.php,line=8,col=5,title=analyse/classes/invalid-new::Instantiated class App\\Missing not found.",
            "::error file=src/a.php,line=8,col=23,title=invalid-ignore-comment::`@pun-ignore` needs one or more rule IDs",
        ]
    );
    assert_eq!(
        pun(&d, &["-q"], &[]),
        (String::new(), String::new(), Some(1))
    );
    // The machine formats count the malformed comment too, so they agree with the exit code.
    let (out, _, code) = pun(&d, &["--format=json"], &[]);
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(code, Some(1));
    assert_eq!(json["totals"]["file_errors"], 3, "{out}");
    let messages = json["files"].as_object().unwrap().values().next().unwrap()["messages"].clone();
    assert_eq!(
        messages[2],
        serde_json::json!({
            "message": "`@pun-ignore` needs one or more rule IDs",
            "line": 8,
            "ignorable": false,
            "tip": "name the rule IDs it ignores: `@pun-ignore lint/group/rule`",
            "identifier": "invalid-ignore-comment",
        })
    );
    let (out, _, _) = pun(&d, &["--format=junit"], &[]);
    assert!(out.contains("<testsuite failures=\"3\""), "{out}");
    assert!(
        out.contains("<testcase name=\"src/a.php:8\"><failure type=\"ERROR\" message=\"`@pun-ignore` needs one or more rule IDs\" /></testcase>"),
        "{out}"
    );
}

#[test]
fn generate_baseline_to_a_named_file() {
    let d = sample("baseline-path", ERRORS);
    let (out, _, code) = pun(&d, &["--generate-baseline", "b.toml", "--color=never"], &[]);
    assert!(
        out.contains("Baseline generated with 3 errors in b.toml; add `baseline = \"b.toml\"` under [analyse] in pun.toml to use it ("),
        "{out}"
    );
    assert_eq!(code, Some(0));
    assert!(d.join("b.toml").exists());
    assert!(!d.join("pun-baseline.toml").exists());
    // The configured baseline is the default file, and needs no hint.
    let toml = "[analyse]\npaths = [\"src\"]\nbaseline = \"b.toml\"\n";
    std::fs::write(d.join("pun.toml"), toml).unwrap();
    std::fs::remove_file(d.join("b.toml")).unwrap();
    let (out, _, code) = pun(&d, &["--generate-baseline", "--color=never"], &[]);
    assert!(
        out.contains("Baseline generated with 3 errors in b.toml ("),
        "{out}"
    );
    assert_eq!(code, Some(0));
    assert!(d.join("b.toml").exists());
    std::fs::write(d.join("pun.toml"), "[analyse]\npaths = [\"src\"]\n").unwrap();
    assert_eq!(
        pun(&d, &["--generate-baseline", "-q"], &[]),
        (String::new(), String::new(), Some(0))
    );
    assert!(d.join("pun-baseline.toml").exists());
}

#[test]
fn run_errors_exit_2() {
    use std::os::unix::fs::PermissionsExt;
    let d = sample("bad-config", CLEAN);
    std::fs::write(d.join("pun.toml"), "[analyse]\nnope = 1\n").unwrap();
    let (out, err, code) = pun(&d, &[], &[]);
    assert_eq!((out.as_str(), code), ("", Some(2)));
    assert!(err.contains("unknown field `nope`"), "{err}");
    let d = sample("bad-baseline", ERRORS);
    std::fs::write(
        d.join("pun.toml"),
        "[analyse]\npaths = [\"src\"]\nbaseline = \"b.toml\"\n",
    )
    .unwrap();
    let b = d.join("b.toml");
    let hint = "  run `pun analyse --generate-baseline` to write it again\n";
    let (out, err, code) = pun(&d, &["--color=never"], &[]);
    assert_eq!(
        (out.as_str(), err, code),
        (
            "",
            format!(
                "x {}  can't read the baseline: No such file or directory (os error 2)\n{hint}",
                b.display()
            ),
            Some(2)
        )
    );
    std::fs::write(&b, "x = \n").unwrap();
    let (_, err, code) = pun(&d, &["--color=never"], &[]);
    assert_eq!(
        (err, code),
        (
            format!(
                "x {}  invalid baseline: string values must be quoted, expected literal string\n{hint}",
                b.display()
            ),
            Some(2)
        )
    );
    std::fs::create_dir(d.join("dir.toml")).unwrap();
    let (_, err, code) = pun(
        &d,
        &["--generate-baseline", "dir.toml", "--color=never"],
        &[],
    );
    assert_eq!(
        (err, code),
        (
            format!(
                "x can't write the baseline to {}: Is a directory (os error 21)\n",
                d.join("dir.toml").display()
            ),
            Some(2)
        )
    );
    // PHPStan's "Invalid entries in ignoreErrors" (its output on the same entries as ignoreErrors) is a run error.
    let d = sample("bad-ignores", CLEAN);
    std::fs::write(
        d.join("pun.toml"),
        "[analyse]\npaths = [\"src\"]\n\n[[analyse.ignore]]\nmessage = \"#(#\"\npaths = [\"src/a.php\"]\n\n[[analyse.ignore]]\nmessage = \"#^Nope#\"\npaths = [\"src/nothere.php\"]\n",
    )
    .unwrap();
    let (out, err, code) = pun(&d, &["--color=never"], &[]);
    assert_eq!((out.as_str(), code), ("", Some(2)));
    for line in [
        "Invalid entries in ignoreErrors:".to_string(),
        "Compilation failed: missing closing parenthesis at offset 1 in pattern: #(#".into(),
        format!(
            "Path \"{}\" is neither a directory, nor a file path, nor a fnmatch pattern.",
            d.join("src/nothere.php").display()
        ),
    ] {
        assert!(err.contains(&line), "{err}");
    }
    let d = sample("unreadable", CLEAN);
    let file = d.join("src/a.php");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&file).is_err() {
        let (out, _, code) = pun(&d, &[], &[]);
        assert_eq!((out.as_str(), code), ("", Some(2)));
    }
}

#[test]
fn teamcity_matches_phpstan() {
    let Some(phar) = phpstan() else { return };
    let phar = phar.to_str().unwrap();
    let outside = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("analyse-outside-{}", std::process::id()));
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("o.php"), "<?php\nnope2();\n").unwrap();
    let outside = outside.display();
    let long = "<?php\nthis_is_an_extremely_long_function_name_to_wrap();\n";
    #[expect(
        clippy::type_complexity,
        reason = "a table of test cases: name, files, working directory, paths"
    )]
    let cases: [(&str, &[(&str, &str)], &str, &str); 5] = [
        (
            "escapes",
            &[("src/it's [a|b]/x.php", "<?php\nnope();\n")],
            "",
            "src",
        ),
        (
            "file",
            &[("src/a.php", long), ("src/b.php", "<?php\nnope();\n")],
            "",
            "src/a.php",
        ),
        (
            "outside",
            &[
                ("src/a.php", "<?php\nnope();\n"),
                ("lib/b.php", "<?php\nnope3();\n"),
            ],
            "",
            &format!("src\", \"lib\", \"{outside}"),
        ),
        (
            "outside-first",
            &[("src/a.php", "<?php\nnope();\n")],
            "",
            &format!("{outside}\", \"src"),
        ),
        (
            "parent",
            &[
                ("proj/src/a.php", "<?php\nnope();\n"),
                ("x/o.php", "<?php\n\u{20000}();\n\u{30000}();\n"),
            ],
            "proj",
            "../x\", \"src",
        ),
    ];
    for (name, files, cwd, paths) in cases {
        let d = project("analyse", name, files).join(cwd);
        let list: Vec<&str> = paths.split("\", \"").collect();
        let neon: String = list.iter().fold(String::new(), |mut out, p| {
            let _ = writeln!(out, "        - {p}");
            out
        });
        std::fs::write(
            d.join("phpstan.neon"),
            format!("parameters:\n    level: 0\n    paths:\n{neon}"),
        )
        .unwrap();
        std::fs::write(
            d.join("pun.toml"),
            format!("[analyse]\npaths = [\"{paths}\"]\n"),
        )
        .unwrap();
        let format = ["--error-format=teamcity"];
        let want = run(
            &d,
            &[
                "php",
                phar,
                "analyse",
                "--no-progress",
                "-c",
                "phpstan.neon",
            ],
            &format,
        );
        assert_eq!(run(&d, &[PUN, "analyse"], &format), want, "{name}");
    }
}

#[test]
fn check_reports_analyse_findings() {
    let d = project(
        "analyse",
        "check",
        &[
            (
                "pun.toml",
                "[format]\npreset = \"empty\"\nexclude = [\"lib\"]\n\n[lint]\nexclude = [\"lib\"]\n\n[analyse]\npaths = [\"src\", \"lib\"]\nbaseline = \"b.toml\"\n",
            ),
            ("src/a.php", "<?php\n\nnope(); // @pun-ignore\n"),
            ("lib/b.php", "<?php\n\nnope2();\n"),
        ],
    );
    assert_eq!(
        pun(&d, &["--generate-baseline", "b.toml", "-q"], &[]).2,
        Some(0)
    );
    std::fs::write(d.join("lib/b.php"), "<?php\n\nnope3();\n").unwrap();
    let check = |env: &[(&str, &str)]| {
        let out = clean_env(&mut Command::new(PUN), &["PATH", "HOME"])
            .current_dir(&d)
            .env("PUN_AGENT", "0")
            .envs(env.iter().copied())
            .args(["check", "--color=never"])
            .output()
            .unwrap();
        (String::from_utf8(out.stdout).unwrap(), out.status.code())
    };
    let (out, code) = check(&[("PUN_AGENT", "1")]);
    let lines: Vec<&str> = out.lines().skip(1).collect();
    assert_eq!(
        lines,
        [
            "lib/b.php:3:1 analyse/undefined/function Function nope3 not found.",
            "src/a.php:3:12 invalid-ignore-comment `@pun-ignore` needs one or more rule IDs",
        ]
    );
    assert_eq!(code, Some(1));
    let (out, _) = check(&[]);
    assert!(
        out.contains("! 1 baseline entry no longer matches: run `pun analyse --generate-baseline` to drop it\n"),
        "{out}"
    );
}

/// An `[[analyse.ignore]]` entry without a path that nothing matched fails `pun check` as it fails `pun analyse`.
#[test]
fn check_fails_on_unmatched_ignores_without_a_file() {
    let d = project(
        "analyse",
        "check-general",
        &[
            (
                "pun.toml",
                "[format]\npreset = \"empty\"\n\n[analyse]\npaths = [\"src\"]\n\n[[analyse.ignore]]\nraw-message = \"Not here\"\n",
            ),
            ("src/a.php", CLEAN),
        ],
    );
    let message = "Ignored error \"Not here\" was not matched in reported errors.";
    assert_eq!(pun(&d, &["--format=raw"], &[]).2, Some(1));
    let check = |env: &[(&str, &str)]| {
        let out = clean_env(&mut Command::new(PUN), &["PATH", "HOME"])
            .current_dir(&d)
            .env("PUN_AGENT", "0")
            .envs(env.iter().copied())
            .args(["check", "--color=never"])
            .output()
            .unwrap();
        (String::from_utf8(out.stdout).unwrap(), out.status.code())
    };
    let (out, code) = check(&[]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains(&format!("   x {message}\n")), "{out}");
    let (out, code) = check(&[("PUN_AGENT", "1")]);
    assert_eq!(code, Some(1), "{out}");
    assert_eq!(
        out.lines().skip(1).collect::<Vec<_>>(),
        [format!("unmatched-ignore {message}")],
        "{out}"
    );
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn ignores_match_phpstan() {
    let neon = "parameters:\n    level: 0\n    paths:\n        - src\n    ignoreErrors:\n        - message: '#^Function undefined_fn not found\\.$#'\n          path: src/a.php\n        - identifier: method.notFound\n          path: src/a.php\n          count: 2\n        - message: '#^Never$#'\n          path: src/a.php\n        - rawMessage: 'Nope'\n          paths: [src/*, lib/*]\n        - message: '#^Never again$#'\n          path: src/a.php\n          reportUnmatched: false\n        - rawMessage: 'Not here'\n          reportUnmatched: false\n";
    let toml = "[analyse]\npaths = [\"src\"]\npreset = \"basic\"\n\n[[analyse.ignore]]\nmessage = '#^Function undefined_fn not found\\.$#'\npaths = [\"src/a.php\"]\n\n[[analyse.ignore]]\nidentifier = \"method.notFound\"\npaths = [\"src/a.php\"]\ncount = 2\n\n[[analyse.ignore]]\nmessage = \"#^Never$#\"\npaths = [\"src/a.php\"]\n\n[[analyse.ignore]]\nraw-message = \"Nope\"\npaths = [\"src/*\", \"lib/*\"]\n\n[[analyse.ignore]]\nmessage = \"#^Never again$#\"\npaths = [\"src/a.php\"]\nreport-unmatched = false\n\n[[analyse.ignore]]\nraw-message = \"Not here\"\nreport-unmatched = false\n";
    let d = project(
        "analyse",
        "ignores",
        &[
            ("phpstan.neon", neon),
            ("pun.toml", toml),
            ("src/a.php", ERRORS),
        ],
    );
    let (out, _, code) = pun(&d, &["--color=never"], &[]);
    assert_eq!(code, Some(1));
    let general = format!(
        "Ignored error \"Nope\" in paths: {0}/src/*, {0}/lib/* was not matched in reported errors.",
        d.display()
    );
    assert!(out.contains(&format!("x {general}\n")), "{out}");
    assert!(out.contains("4 errors"), "{out}");
    let (out, _, _) = pun(&d, &[], &[("PUN_AGENT", "1")]);
    assert!(
        out.contains(&format!("unmatched-ignore {general}")),
        "{out}"
    );
    let (out, _, code) = pun(&d, &["--generate-baseline", "--color=never"], &[]);
    assert!(out.contains("Baseline generated with 1 error in"), "{out}");
    assert_eq!(code, Some(0));
    std::fs::remove_file(d.join("pun-baseline.toml")).unwrap();
    // Only a file on the command line: PHPStan 2.3.6's UnmatchedIgnoredErrorJudge still reports the unmatched entries
    // of src/a.php, which declares no trait and depends on no file left out; the entry for `src/*, lib/*` is not
    // judged. This is 2.3.6's output (`phpstan analyse -c phpstan.neon --error-format=raw src/a.php`); the oracle,
    // 2.3.5, reports no `ignore.unmatched` when only files are analysed.
    let a = d.join("src/a.php");
    let a = a.display();
    let want = format!(
        "{a}:?:Ignored error pattern #^Never$# in path {a} was not matched in reported errors.\n\
         {a}:8:Instantiated class App\\Missing not found.\n\
         {a}:15:Ignored error pattern method.notFound in path {a} is expected to occur 2 times, but occurred only 1 time.\n"
    );
    let got = run(&d, &[PUN, "analyse"], &["--error-format=raw", "src/a.php"]);
    assert_eq!(got, (want, Some(1)));
    let Some(phar) = phpstan() else { return };
    let phar = phar.to_str().unwrap();
    for f in FORMATS {
        let format = format!("--error-format={f}");
        let phpstan = [
            "php",
            phar,
            "analyse",
            "--no-progress",
            "-c",
            "phpstan.neon",
        ];
        let mut want = run(&d, &phpstan, &[&format, "-v"]);
        want.0 = want.0.replace("file='phpstan.neon'", "file='pun.toml'");
        let got = run(&d, &[PUN, "analyse"], &[&format, "-v"]);
        assert_eq!(got, want, "{f}");
    }
}

/// Only some files of the configured paths: PHPStan 2.3.6 reports the unmatched entry of src/a.php but not the one of
/// src/b.php, whose class uses a trait in src/t.php, which is not analysed (`phpstan analyse -c phpstan.neon
/// --error-format=raw src/a.php src/b.php` with the same entries; 2.3.5 reports neither).
#[test]
fn unmatched_ignores_of_files_left_out() {
    let toml = "[analyse]\npaths = [\"src\"]\npreset = \"basic\"\n\n[[analyse.ignore]]\nmessage = \"#^Never$#\"\npaths = [\"src/a.php\"]\n\n[[analyse.ignore]]\nmessage = \"#^Never$#\"\npaths = [\"src/b.php\"]\n";
    let d = project(
        "analyse",
        "ignores-files-left-out",
        &[
            ("pun.toml", toml),
            ("src/a.php", CLEAN),
            (
                "src/b.php",
                "<?php\n\nnamespace App;\n\nclass B\n{\n    use T;\n}\n",
            ),
            ("src/t.php", "<?php\n\nnamespace App;\n\ntrait T\n{\n}\n"),
        ],
    );
    let a = d.join("src/a.php");
    let a = a.display();
    let got = run(
        &d,
        &[PUN, "analyse"],
        &["--error-format=raw", "src/a.php", "src/b.php"],
    );
    let want = format!(
        "{a}:?:Ignored error pattern #^Never$# in path {a} was not matched in reported errors.\n"
    );
    assert_eq!(got, (want, Some(1)));
}

#[test]
fn no_rules_detected() {
    let d = sample("no-rules", ERRORS);
    let groups = "arrays attributes casing casts classes comments control-flow dead-code debug deprecated functions imports laravel operators phpdoc statements strings types undefined";
    let off: String = groups.split(' ').fold(String::new(), |mut out, g| {
        let _ = writeln!(out, "{g} = \"off\"");
        out
    });
    std::fs::write(
        d.join("pun.toml"),
        format!("[analyse]\npaths = [\"src\"]\n\n[analyse.rules]\n{off}"),
    )
    .unwrap();
    let (out, err, code) = pun(&d, &["--color=never"], &[]);
    assert_eq!((out.as_str(), code), ("", Some(2)));
    assert!(err.starts_with("x No rules detected\n"), "{err}");
}

/// PHPStan's fixable errors (a named argument to a `@no-named-arguments` function, at level 5 and the
/// `recommended` preset): json's `"fixable": true` on the reported ones, byte for byte with PHPStan on the same
/// ignoreErrors.
#[test]
fn fixable_errors_match_phpstan() {
    let Some(phar) = phpstan() else { return };
    let phar = phar.to_str().unwrap();
    let named = |f: &str, n: &str| {
        format!("<?php\n\n/** @no-named-arguments */\nfunction {f}(int $a): void {{}}\n\n{n}\n")
    };
    for (name, paths) in [("both", "src"), ("ignored-only", "src/sub")] {
        let d = project(
            "analyse",
            &format!("fixable-{name}"),
            &[
                (
                    "phpstan.neon",
                    &format!(
                        "parameters:\n    level: 5\n    paths: [{paths}]\n    ignoreErrors:\n        - identifier: argument.named\n          path: src/sub/b.php\n"
                    ),
                ),
                (
                    "pun.toml",
                    &format!(
                        "[analyse]\npaths = [\"{paths}\"]\n\n[[analyse.ignore]]\nidentifier = \"argument.named\"\npaths = [\"src/sub/b.php\"]\n"
                    ),
                ),
                ("src/a.php", &named("f", "f(a: 1);\nf(a: 2);")),
                ("src/sub/b.php", &named("g", "g(a: 3);")),
            ],
        );
        let phpstan = [
            "php",
            phar,
            "analyse",
            "--no-progress",
            "-c",
            "phpstan.neon",
        ];
        let want = run(&d, &phpstan, &["--error-format=json"]);
        assert_eq!(
            want.0.contains("\"fixable\":true"),
            name == "both",
            "{}",
            want.0
        );
        assert_eq!(
            run(&d, &[PUN, "analyse"], &["--error-format=json"]),
            want,
            "{name}"
        );
    }
}

/// Composer's autoloaded classes are symbols (analyse.md §3): the classes under the psr-4 directory are found, also
/// one in an `[analyse] exclude`, as PHPStan's PSR-4 locator ignores `excludePaths`. PHPStan 2.3.5 at level 0 with
/// `excludePaths: [app/Other.php]`, with or without `composer dump-autoload`, reports only `Missing` too.
#[test]
fn composer_autoloaded_classes_are_symbols() {
    let d = project(
        "analyse",
        "autoload",
        &[
            (
                "composer.json",
                r#"{"autoload":{"psr-4":{"App\\":"app/"}}}"#,
            ),
            (
                "pun.toml",
                "[analyse]\npreset = \"basic\"\npaths = [\"src\"]\nexclude = [\"app/Other.php\"]\n",
            ),
            ("app/Kept.php", "<?php\n\nnamespace App;\n\nclass Kept {}\n"),
            (
                "app/Other.php",
                "<?php\n\nnamespace App;\n\nclass Other {}\n",
            ),
            (
                "src/a.php",
                "<?php\n\nnew \\App\\Kept();\nnew \\App\\Other();\nnew \\App\\Missing();\n",
            ),
        ],
    );
    let (out, code) = run(&d, &[PUN, "analyse"], &["--error-format=raw"]);
    assert_eq!(
        (out, code),
        (
            format!(
                "{}:5:Instantiated class App\\Missing not found.\n",
                d.join("src/a.php").display()
            ),
            Some(1)
        )
    );
}

/// `[analyse] report-unmatched = false` is PHPStan's `reportUnmatchedIgnoredErrors: false`: PHPStan 2.3.5 reports
/// neither the unused `@phpstan-ignore-line` nor the entry that matches nothing with it, and both without it.
#[test]
fn report_unmatched_false_reports_no_unmatched_ignores() {
    let toml = "[analyse]\npreset = \"basic\"\npaths = [\"src\"]\n{global}\n[[analyse.ignore]]\nmessage = \"#^Nothing matches this$#\"\n";
    for (global, want) in [("", Some(1)), ("report-unmatched = false\n", Some(0))] {
        let toml = toml.replace("{global}", global);
        let d = project(
            "analyse",
            &format!("report-unmatched-{}", want.unwrap()),
            &[
                ("pun.toml", toml.as_str()),
                (
                    "src/a.php",
                    "<?php\n\nfunction f(int $a): int\n{\n    return $a; // @phpstan-ignore-line\n}\n",
                ),
            ],
        );
        let (out, code) = run(&d, &[PUN, "analyse"], &["--error-format=raw"]);
        assert_eq!(code, want, "{out}");
        let reported = [
            "No error to ignore is reported on line 5.",
            "#^Nothing matches this$#",
        ];
        for message in reported {
            assert_eq!(out.contains(message), want == Some(1), "{out}");
        }
    }
}

/// `-q` prints nothing and keeps the exit code, as PHPStan 2.3.6's `analyse -q` (no output with the table and json
/// formats, under `CLAUDECODE=1`, and with `--generate-baseline`, which exits 0).
#[test]
fn quiet_prints_nothing() {
    let d = sample("quiet", ERRORS);
    let none = (String::new(), String::new());
    let agent: &[(&str, &str)] = &[("PUN_AGENT", "1")];
    for (args, env) in [
        (&["-q"][..], &[][..]),
        (&["--quiet", "--format=json"], &[]),
        (&["-q"], agent),
    ] {
        let (out, err, code) = pun(&d, args, env);
        assert_eq!(((out, err), code), (none.clone(), Some(1)), "{args:?}");
    }
    let (out, err, code) = pun(&d, &["-q", "--generate-baseline"], agent);
    assert_eq!(((out, err), code), (none, Some(0)));
    assert!(d.join("pun-baseline.toml").exists());
}

#[test]
fn scan_and_bootstrap_files_declare_symbols() {
    // PHPStan 2.3.5 at level 1 (the basic preset's highest) reports these four errors without scanFiles,
    // scanDirectories and bootstrapFiles, and none with them
    const SOURCE: &str = "<?php\n\nnamespace App;\n\nclass Collector extends \\DebugBar\\DataCollector\\DataCollector\n{\n}\n\nfunction f(): void\n{\n    debug_helper();\n    echo LEGACY_FLAG;\n    new \\Legacy\\Old();\n}\n";
    let files = |toml: &str| {
        project(
            "analyse",
            if toml.contains("scan") {
                "scan-on"
            } else {
                "scan-off"
            },
            &[
                ("pun.toml", toml),
                ("src/a.php", SOURCE),
                (
                    "stubs/debugbar.php",
                    "<?php\n\nnamespace DebugBar\\DataCollector;\n\nabstract class DataCollector\n{\n}\n",
                ),
                (
                    "legacy/Old.php",
                    "<?php\n\nnamespace Legacy;\n\nclass Old\n{\n}\n",
                ),
                (
                    "tests/bootstrap.php",
                    "<?php\n\nfunction debug_helper(): void\n{\n}\n\ndefine('LEGACY_FLAG', 1);\n",
                ),
            ],
        )
    };
    let d = files("[analyse]\npreset = \"basic\"\npaths = [\"src\"]\n");
    let (out, _, code) = pun(&d, &["--format=raw"], &[]);
    assert_eq!(
        (out.replace(&format!("{}/", d.display()), "").as_str(), code),
        (
            "src/a.php:5:Class App\\Collector extends unknown class DebugBar\\DataCollector\\DataCollector.\nsrc/a.php:11:Function debug_helper not found.\nsrc/a.php:12:Constant LEGACY_FLAG not found.\nsrc/a.php:13:Instantiated class Legacy\\Old not found.\n",
            Some(1)
        )
    );
    let d = files(
        "[analyse]\npreset = \"basic\"\npaths = [\"src\"]\nscan = [\"stubs/debugbar.php\", \"legacy\"]\nbootstrap = [\"tests/bootstrap.php\"]\n",
    );
    assert_eq!(
        pun(&d, &["--format=raw"], &[]),
        (String::new(), String::new(), Some(0))
    );
}

const PHPAT_TEST: &str = "<?php\n\nnamespace Arch;\n\nuse PHPat\\Selector\\Selector;\nuse PHPat\\Test\\Builder\\Rule;\nuse PHPat\\Test\\PHPat;\n\nfinal class LayerTest\n{\n    public function testDomainIsPure(): Rule\n    {\n        return PHPat::rule()\n            ->classes(Selector::inNamespace('App\\Domain'))\n            ->shouldNotDependOn()\n            ->classes(Selector::inNamespace('App\\Infra'))\n            ->because('Keep the domain pure');\n    }\n}\n";

fn phpat_project(name: &str, extension: &str) -> PathBuf {
    project(
        "analyse",
        name,
        &[
            (
                "composer.json",
                r#"{"autoload": {"psr-4": {"App\\": "src/"}}, "autoload-dev": {"psr-4": {"Arch\\": "arch/"}}}"#,
            ),
            (
                "pun.toml",
                &format!(
                    "[analyse]\npreset = \"phpstan/level_0\"\npaths = [\"src\"]\n\n[extensions]\nphpat = {extension}\n"
                ),
            ),
            (
                "src/Domain/Order.php",
                "<?php\n\nnamespace App\\Domain;\n\nuse App\\Infra\\Db;\n\nfinal class Order\n{\n    public function save(Db $db): void\n    {\n        // @phpstan-ignore phpat.testDomainIsPure\n        $copy = new Db();\n    }\n}\n",
            ),
            (
                "src/Infra/Db.php",
                "<?php\n\nnamespace App\\Infra;\n\nclass Db\n{\n}\n",
            ),
            ("arch/LayerTest.php", PHPAT_TEST),
        ],
    )
}

/// phpat's tests run inside `pun analyse`: PHPStan 2.3.5 with phpat 0.12.5 on the same project (level 0, the test
/// class tagged `phpat.test`) prints this JSON and exits 1. The `@phpstan-ignore` comment covers the second error.
#[test]
fn phpat_tests_run_in_analyse() {
    let d = phpat_project("phpat", r#"{ tests = ["Arch\\LayerTest"] }"#);
    let (out, code) = run(&d, &[PUN, "analyse"], &["--error-format=json"]);
    let file = d.join("src/Domain/Order.php").canonicalize().unwrap();
    let want = format!(
        r#"{{"totals":{{"errors":0,"file_errors":1}},"files":{{"{}":{{"errors":1,"messages":[{{"message":"App\\Domain\\Order should not depend on App\\Infra\\Db","line":9,"ignorable":true,"tip":"Keep the domain pure","identifier":"phpat.testDomainIsPure"}}]}}}},"errors":[]}}"#,
        file.display()
    );
    assert_eq!((out, code), (want, Some(1)));
    // Switched off as a group, phpat reports nothing, so the ignore comment matches no error, as in PHPStan without
    // phpat.
    std::fs::write(
        d.join("pun.toml"),
        "[analyse]\npreset = \"phpstan/level_0\"\npaths = [\"src\"]\n\n[analyse.rules]\nphpat = \"off\"\n\n[extensions]\nphpat = { tests = [\"Arch\\\\LayerTest\"] }\n",
    )
    .unwrap();
    let (out, code) = run(&d, &[PUN, "analyse"], &["--error-format=json"]);
    let want = format!(
        r#"{{"totals":{{"errors":0,"file_errors":1}},"files":{{"{}":{{"errors":1,"messages":[{{"message":"No error with identifier phpat.testDomainIsPure is reported on line 12.","line":12,"ignorable":false,"identifier":"ignore.unmatchedIdentifier","fixable":true}}]}}}},"errors":[]}}"#,
        file.display()
    );
    assert_eq!((out, code), (want, Some(1)));
}

/// A phpat test Pun can't find or read stops the run (exit 2) with an error that names it.
#[test]
fn unreadable_phpat_test_is_a_run_error() {
    let d = phpat_project("phpat-missing", r#"{ tests = ["Arch\\Missing"] }"#);
    let out = clean_env(&mut Command::new(PUN), &["PATH", "HOME"])
        .current_dir(&d)
        .env("PUN_AGENT", "0")
        .args(["analyse", "--error-format=json"])
        .output()
        .unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("[extensions] phpat: phpat test class Arch\\Missing was not found"),
        "{stderr}"
    );
}
