//! `pun migrate` + `pun format` against the real Pint (`php .oracle/pint/builds/pint`) on the same pint.json: same
//! exit code, resulting files and machine-format stdout for every flag combination below, run in the same directory
//! (so diff headers and absolute paths agree). Human output is Pun's own: those runs compare the files each tool
//! names (tests/common). Rules: only fully ported ones, so the output must be byte-identical. Skipped without the
//! oracle. Pun-only behaviour (`--explain`, `config`, unported rules, no pun.toml) is checked by hand-derived
//! expectations at the end.
mod common;

use common::{PUN, migrate, oracle, scratch, write_files};

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const PINT_JSON: &str = r#"{"preset":"empty","rules":{"array_syntax":true,"no_trailing_whitespace":true,"trim_array_spaces":true,"whitespace_after_comma_in_array":true}}"#;
const LONG: &str = "app/Long/VeryLongDirectoryNameToForceTruncationOfTheDescription/File.php";

fn files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("composer.json", r#"{"require":{"php":"^8.4"}}"#),
        ("pint.json", PINT_JSON),
        ("app/Fix.php", "<?php\n$a = array(1,2);   \n"),
        ("app/Clean.php", "<?php\n$a = [1];\n"),
        ("app/Empty.php", ""),
        ("app/with spaçe.php", "<?php\n$b = array( 3 );\n"),
        (LONG, "<?php\n$c = array( 1,2 );\n"),
    ]
}

fn reset(dir: &Path, files: &[(&str, &str)]) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        if e.file_name() != ".git" {
            let p = e.path();
            if p.is_dir() && !p.is_symlink() {
                std::fs::remove_dir_all(p).unwrap();
            } else {
                std::fs::remove_file(p).unwrap();
            }
        }
    }
    write_files(dir, files);
    if dir.join("app/Fix.php").exists() {
        std::os::unix::fs::symlink("Fix.php", dir.join("app/Link.php")).unwrap();
    }
}

#[derive(Debug, PartialEq)]
struct Run {
    stdout: String,
    code: Option<i32>,
    files: BTreeMap<String, Vec<u8>>,
}

fn run(
    program: &Path,
    pre: &[&str],
    dir: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    stdin: &[u8],
) -> (Run, String) {
    let mut c = Command::new(program);
    common::clean_env(&mut c, &["PATH", "HOME"]);
    c.args(pre).args(args).current_dir(dir);
    let tmp = dir.with_extension("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    c.env("COLUMNS", "80")
        .env("TMPDIR", &tmp)
        .env("PUN_AGENT", "0")
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let out = child.wait_with_output().unwrap();
    let files = common::snapshot(dir, |r| r == ".git" || r == "pun.toml");
    (
        Run {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            code: out.status.code(),
            files,
        },
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn pun_format(dir: &Path, args: &[&str], env: &[(&str, &str)], stdin: &[u8]) -> (Run, String) {
    migrate(dir);
    run(Path::new(PUN), &["format"], dir, args, env, stdin)
}

struct Ctx {
    oracle: PathBuf,
}

impl Ctx {
    fn pint(&self, dir: &Path, args: &[&str], env: &[(&str, &str)], stdin: &[u8]) -> (Run, String) {
        run(
            Path::new("php"),
            &[self.oracle.to_str().unwrap()],
            dir,
            args,
            env,
            stdin,
        )
    }
    fn same(
        &self,
        dir: &Path,
        tree: &[(&str, &str)],
        pre: &[&str],
        args: &[&str],
        env: &[(&str, &str)],
        stdin: &[u8],
    ) {
        self.same_as(dir, tree, pre, args, args, env, stdin);
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "`same` plus the arguments Pint gets"
    )]
    fn same_as(
        &self,
        dir: &Path,
        tree: &[(&str, &str)],
        pre: &[&str],
        args: &[&str],
        pint_args: &[&str],
        env: &[(&str, &str)],
        stdin: &[u8],
    ) {
        reset(dir, tree);
        let (mut want, _) = self.pint(dir, pint_args, env, stdin);
        reset(dir, tree);
        migrate(dir);
        let pre = if pre.is_empty() { &["format"][..] } else { pre };
        let (mut got, err) = run(Path::new(PUN), pre, dir, args, env, stdin);
        if common::human(args) {
            (want.stdout, got.stdout) = common::human_pair(
                &want.stdout,
                &got.stdout,
                want.files.keys(),
                got.files.keys(),
            );
        }
        assert_eq!(got, want, "{pre:?} {args:?} {env:?} (our stderr: {err})");
    }
    fn same_error(&self, dir: &Path, tree: &[(&str, &str)], args: &[&str], message: &str) {
        reset(dir, tree);
        let (mut want, werr) = self.pint(dir, args, &[], b"");
        reset(dir, tree);
        let (got, err) = pun_format(dir, args, &[], b"");
        assert_eq!(want.code, Some(1), "{args:?}: {werr}");
        assert!(want.stdout.contains(message), "{}", want.stdout);
        assert_eq!(err, format!("x {message}\n"), "{args:?}");
        (want.stdout, want.code) = (String::new(), Some(2));
        assert_eq!(got, want, "{args:?}");
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one test walks every case of the behaviour it checks"
)]
fn matches_pint() {
    let Some(oracle) = oracle() else { return };
    let cx = Ctx { oracle };
    let dir = scratch("cli", "p");
    let tree = files();
    for args in [
        &[][..],
        &["--test"],
        &["--test", "-v"],
        &["-v"],
        &["--repair"],
        &["--format=json"],
        &["--test", "--format=json"],
        &["--test", "--format=json", "-v"],
        &["--format=agent"],
        &["--test", "--format=agent"],
        &["-q"],
        &["--test", "-q"],
        &[
            "--test",
            "--output-to-file=report.json",
            "--output-format=json",
        ],
        &["--format=junit", "--output-to-file=report.xml"],
        &["app/Fix.php"],
        &["app/Clean.php"],
        &["app/Clean.php", "--format=agent"],
        &["app/Clean.php", "--test", "--format=json"],
        &["app", "--test", "--max-processes=2"],
        &["--test", "--preset=empty"],
    ] {
        cx.same(&dir, &tree, &[], args, &[], b"");
    }
    reset(&dir, &tree);
    let (want, _) = cx.pint(&dir, &["--test"], &[], b"");
    reset(&dir, &tree);
    migrate(&dir);
    let (got, err) = run(Path::new(PUN), &["check"], &dir, &[], &[], b"");
    assert_eq!(
        (got.code, &got.files),
        (want.code, &want.files),
        "check (our stderr: {err})"
    );
    for file in [
        "app/Fix.php",
        "app/Long/VeryLongDirectoryNameToForceTruncationOfTheDescription/File.php",
        "app/with spaçe.php",
    ] {
        assert!(
            want.stdout.contains(file.get(..20).unwrap_or(file)),
            "{}",
            want.stdout
        );
        assert!(got.stdout.contains(file), "{}", got.stdout);
    }
    cx.same_as(
        &dir,
        &tree,
        &["format"],
        &["--check", "--format=json"],
        &["--test", "--format=json"],
        &[],
        b"",
    );

    for args in [
        &["-"][..],
        &["--stdin-filename=app/x.php", "-"],
        &["--stdin-filename=storage/x.php", "-"],
    ] {
        cx.same(&dir, &tree, &[], args, &[], b"<?php\n$a = array(1,2);\n");
        cx.same(&dir, &tree, &[], args, &[], b"<?php\nfunction (\n");
        cx.same(&dir, &tree, &[], args, &[], b"<?php\n$a = [1];\n");
    }

    for flag in ["dirty", "diff=main"] {
        let message = format!(
            "The [--{}] option is only available when using Git.",
            flag.split('=').next().unwrap()
        );
        cx.same_error(&dir, &tree, &[&format!("--{flag}"), "--test"], &message);
    }

    cx.same_error(
        &dir,
        &tree,
        &["nope"],
        r#"The path "nope" is not readable."#,
    );
    let mut bad_in = tree.clone();
    bad_in[1] = (
        "pint.json",
        r#"{"preset":"empty","rules":{"array_syntax":true},"in":["nodir"]}"#,
    );
    cx.same_error(
        &dir,
        &bad_in,
        &[],
        r#"The "nodir" directory does not exist."#,
    );
    let env: &[(&str, &str)] = &[];
    for (mode, args) in [(0o444, &[][..]), (0o000, &["--test"])] {
        let locked = || {
            reset(&dir, &tree);
            std::fs::set_permissions(
                dir.join("app/Fix.php"),
                std::os::unix::fs::PermissionsExt::from_mode(mode),
            )
            .unwrap();
        };
        locked();
        let (want, _) = cx.pint(&dir, args, env, b"");
        locked();
        let (got, err) = pun_format(&dir, args, env, b"");
        let fix = dir.join("app/Fix.php");
        let message = match mode {
            0o444 => format!(
                r#"Cannot write to file "{}" as it is not writable."#,
                fix.display()
            ),
            _ => format!(r#"Failed to read content from "{}"."#, fix.display()),
        };
        assert!(want.stdout.starts_with("\n  "), "{}", want.stdout);
        assert_eq!(want.code, Some(1));
        assert_eq!(
            (got.stdout, err, got.code),
            (String::new(), format!("x {message}\n"), Some(2))
        );
    }
    std::fs::set_permissions(
        dir.join("app/Fix.php"),
        std::os::unix::fs::PermissionsExt::from_mode(0o644),
    )
    .unwrap();

    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args([
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(&dir)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    };
    reset(&dir, &tree);
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "."]);
    git(&["commit", "-qm", "init"]);
    let mut changed = tree.clone();
    changed.push(("app/New.php", "<?php\n$n = array(1);\n"));
    cx.same(&dir, &tree, &[], &["--dirty"], &[], b"");
    cx.same(&dir, &changed, &[], &["--dirty", "--test"], &[], b"");
    cx.same(
        &dir,
        &changed,
        &[],
        &["--diff=main", "--test", "--format=json"],
        &[],
        b"",
    );
}

#[test]
fn blade_like_pint() {
    let Some(oracle) = oracle() else { return };
    let Some(modules) = common::prettier::node_modules() else {
        eprintln!("skipping: no prettier in crates/pun-fixers/node_modules (bun install there)");
        return;
    };
    let dir = scratch("cli", "blade");
    for (preset, blade, check) in [
        (
            "empty",
            "<div>\n@if(!$a)\n{{ !$b }}\n@endif\n</div>\n",
            "@if (!$a)",
        ),
        ("laravel", "\n  \n", ""),
    ] {
        let pint_json = format!(r#"{{"preset":"{preset}","rules":{{"Pint/laravel_blade":true}}}}"#);
        let tree = [
            ("composer.json", r#"{"require":{"php":"^8.4"}}"#),
            ("pint.json", &pint_json),
            ("resources/v.blade.php", blade),
        ];
        let mut runs = vec![];
        for pint in [true, false] {
            reset(&dir, &tree);
            std::os::unix::fs::symlink(&modules, dir.join("node_modules")).unwrap();
            runs.push(if pint {
                run(
                    Path::new("php"),
                    &[oracle.to_str().unwrap()],
                    &dir,
                    &["--format=json"],
                    &[],
                    b"",
                )
                .0
            } else {
                pun_format(&dir, &["--format=json"], &[], b"").0
            });
        }
        assert_eq!(runs[1], runs[0], "preset {preset}");
        let out = String::from_utf8_lossy(&runs[0].files["resources/v.blade.php"]).into_owned();
        assert!(
            if check.is_empty() {
                out.is_empty()
            } else {
                out.contains(check)
            },
            "{out:?}"
        );
    }
}

#[test]
fn diffs_like_pint() {
    let Some(oracle) = oracle() else { return };
    let cx = Ctx { oracle };
    let dir = scratch("cli", "diff");
    let big: String = std::iter::once("<?php\n".to_string())
        .chain((0..1000).map(|i| format!("$a{i} = array({i});\n$b = {};\n", i % 7)))
        .collect();
    let tree = [
        ("composer.json", r#"{"require":{"php":"^8.4"}}"#),
        ("pint.json", r#"{"preset":"laravel"}"#),
        (
            "app/Imports.php",
            include_str!("fixtures/dirty_imports.php"),
        ),
        ("app/NoEol.php", "<?php\n\n$a = array(1);\n$b = 2;"),
        ("app/OneLine.php", "<?php $a = array(1);"),
        ("app/Crlf.php", "<?php\r\n\r\n$a = array(1);\n"),
        ("app/Big.php", &big),
    ];
    cx.same(
        &dir,
        &tree,
        &[],
        &["--test", "-v", "--format=json"],
        &[],
        b"",
    );
}

/// `"=" = "align"` re-tokenizes each file mid-run; on these two (from #39 and #40) the transformers then read block
/// pairs left over from the previous code, so `format` hung on the first and panicked on the second. Pint's
/// `no_unused_imports` is off because in Pun it is a lint rule.
#[test]
fn aligns_equals_like_pint() {
    let Some(oracle) = oracle() else { return };
    let cx = Ctx { oracle };
    let dir = scratch("cli", "align");
    let tree = [
        ("composer.json", r#"{"require":{"php":"^8.4"}}"#),
        (
            "pint.json",
            r#"{"preset":"laravel","rules":{"no_unused_imports":false,"binary_operator_spaces":{"default":"single_space","operators":{"=>":"align","=":"align","|":"no_space"}}}}"#,
        ),
        (
            "app/Hang.php",
            include_str!("fixtures/align_equals_hang.php"),
        ),
        (
            "app/Crash.php",
            include_str!("fixtures/align_equals_crash.php"),
        ),
    ];
    for args in [&[][..], &["--test", "--format=json"]] {
        cx.same(&dir, &tree, &[], args, &[], b"");
    }
}

/// A report or baseline path that is a symlink in the project leading out of it is refused, and the file it leads to
/// keeps its content; a link outside the project (`/dev/stdout`) is the user's own and is written through.
#[test]
fn generated_files_are_not_written_through_a_link_out_of_the_project() {
    let dir = scratch("cli", "links");
    let outside = scratch("cli", "outside");
    write_files(
        &dir,
        &[
            ("pun.toml", "[format]\npreset = \"empty\"\n"),
            ("app/A.php", "<?php\n\n$a = 1;\n"),
        ],
    );
    std::fs::write(outside.join("keep"), "keep\n").unwrap();
    for name in ["pun.xml", "pun-baseline.toml"] {
        std::os::unix::fs::symlink(outside.join("keep"), dir.join(name)).unwrap();
    }
    let report = ["--output-to-file=pun.xml", "--output-format=checkstyle"];
    let (out, err, code) = pun(&dir, &[&["format", "--test"][..], &report].concat());
    assert_eq!(code, Some(2), "{out}{err}");
    assert!(
        err.contains("it is a symlink that leads outside the project"),
        "{err}"
    );
    let (out, err, code) = pun(&dir, &["analyse", "--generate-baseline"]);
    assert_eq!(code, Some(2), "{out}{err}");
    assert!(
        err.contains("it is a symlink that leads outside the project"),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(outside.join("keep")).unwrap(),
        "keep\n"
    );
    let to_stdout = ["--output-to-file=/dev/stdout", "--output-format=checkstyle"];
    let (out, err, code) = pun(&dir, &[&["format", "--test"][..], &to_stdout].concat());
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("<checkstyle"), "{out}");
}

fn pun(dir: &Path, args: &[&str]) -> (String, String, Option<i32>) {
    pun_env(dir, args, &[])
}

fn pun_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (String, String, Option<i32>) {
    let out = common::clean_env(&mut Command::new(PUN), &["PATH"])
        .args(args)
        .current_dir(dir)
        .env("COLUMNS", "80")
        .env("PUN_AGENT", "0")
        .envs(env.iter().copied())
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code(),
    )
}

#[test]
fn list_files() {
    let dir = scratch("cli", "list");
    reset(&dir, &[]);
    for f in [
        "app/A.php",
        "app/B.php",
        "tests/T.php",
        "storage/S.php",
        "vendor/v/V.php",
    ] {
        std::fs::create_dir_all(dir.join(f).parent().unwrap()).unwrap();
        std::fs::write(dir.join(f), "<?php\n").unwrap();
    }
    std::fs::write(
        dir.join("pun.toml"),
        "[files]\npaths = [\"app\", \"tests\"]\nexclude = [\"storage/**\"]\n\n[analyse]\nexclude = [\"tests/**\"]\n",
    )
    .unwrap();
    let ok = |s: &str| (s.to_string(), String::new(), Some(0));
    assert_eq!(
        pun(&dir, &["list-files"]),
        ok("app/A.php\napp/B.php\ntests/T.php\n")
    );
    assert_eq!(
        pun(&dir, &["list-files", "--command", "analyse"]),
        ok("app/A.php\napp/B.php\n")
    );
    assert_eq!(
        pun(&dir, &["list-files", "-0", "tests"]),
        ok("tests/T.php\0")
    );
    assert_eq!(
        pun(
            &dir,
            &["list-files", "--zero-terminate", "--command=lint", "app"]
        ),
        ok("app/A.php\0app/B.php\0")
    );
    assert_eq!(
        pun(
            &dir,
            &["list-files", "--format=json", "--command", "analyse"]
        ),
        ok("[\"app/A.php\",\"app/B.php\"]\n")
    );
    assert_eq!(
        pun_env(
            &dir,
            &["list-files", "--format", "json", "tests"],
            &[("PUN_AGENT", "1")]
        ),
        ok("[\"tests/T.php\"]\n")
    );
    let (out, summary, code) = pun_env(
        &dir,
        &["list-files", "--command", "analyse"],
        &[("PUN_AGENT", "1")],
    );
    assert!(
        summary.starts_with("pun list-files: 2 files, analyse set ("),
        "{summary}"
    );
    assert_eq!((out.as_str(), code), ("app/A.php\napp/B.php\n", Some(0)));
    let (_, summary, _) = pun_env(&dir, &["list-files", "tests"], &[("PUN_AGENT", "1")]);
    assert!(summary.starts_with("pun list-files: 1 file ("), "{summary}");
    assert_eq!(
        pun(&dir, &["list-files", "--format=txt"]),
        (
            String::new(),
            "x --format=txt: expected `json`\n".into(),
            Some(2)
        )
    );
    assert_eq!(
        pun(&dir, &["list-files", "nope"]),
        (
            String::new(),
            "x The path \"nope\" is not readable.\n".into(),
            Some(2)
        )
    );
    assert_eq!(pun(&dir, &["format", "--list-files"]).2, Some(2));
    // `-0` keeps a name that isn't UTF-8 byte for byte, so `xargs -0` gets the file and not a lookalike.
    let latin1 =
        <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(b"tests/caf\xe9.php");
    std::fs::write(dir.join(latin1), "<?php\n").unwrap();
    let out = common::clean_env(&mut Command::new(PUN), &["PATH"])
        .args(["list-files", "-0", "tests"])
        .current_dir(&dir)
        .env("PUN_AGENT", "0")
        .output()
        .unwrap();
    assert_eq!(out.stdout, b"tests/T.php\0tests/caf\xe9.php\0");
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one test walks every case of the behaviour it checks"
)]
fn pun_only_behaviour() {
    let dir = scratch("cli", "own");
    reset(&dir, &files());
    for cmd in [
        &["format"][..],
        &["lint"],
        &["check", "--fix"],
        &["config"],
        &["list-files"],
    ] {
        let (out, err, code) = pun(&dir, cmd);
        assert_eq!(
            (out.as_str(), err.as_str(), code),
            (
                "",
                "x No pun.toml found. Run `pun migrate`, or `pun init` for a new project.\n",
                Some(2)
            ),
            "{cmd:?}"
        );
    }
    let (_, _, code) = pun(&dir, &["format", "--test", "--preset=empty"]);
    assert_eq!(code, Some(0));
    let (_, _, code) = pun(&dir, &["format", "--test", "--no-config"]);
    assert_eq!(code, Some(1));
    migrate(&dir);
    let (out, _, code) = pun(&dir, &["list-files", "--command", "format"]);
    let listed: Vec<&str> = out.lines().collect();
    assert_eq!(
        listed,
        [
            "app/Clean.php",
            "app/Empty.php",
            "app/Fix.php",
            "app/Link.php",
            LONG,
            "app/with spaçe.php"
        ]
    );
    assert_eq!(code, Some(0));

    let (_, err, code) = pun(&dir, &["format", "--test", "--explain"]);
    assert_eq!(code, Some(1));
    let lines: Vec<&str> = err.lines().collect();
    assert_eq!(lines[0], "native (PHP 8.4), preset empty, 4 rules");
    assert_eq!(lines.len(), 5);
    assert!(
        lines[1..].iter().all(|l| l.starts_with("  native ")),
        "{err}"
    );
    for rule in [
        "array_syntax",
        "no_trailing_whitespace",
        "trim_array_spaces",
        "whitespace_after_comma_in_array",
    ] {
        assert!(
            lines.iter().any(|l| l.ends_with(&format!("  {rule}"))),
            "{err}"
        );
    }

    let (_, err, _) = pun(&dir, &["format", "--test", "-v", "app/Clean.php"]);
    assert_eq!(err, "");

    // No PHP fallback: a rule Pun hasn't ported stops every command, named under its Pun ID (exit 2).
    let one = "[format]\npreset = \"empty\"\n[format.rules]\n\"arrays/syntax\" = true\n[lint.rules]\n\"laravel/prefer-pivot-class\" = \"error\"\n";
    std::fs::write(dir.join("pun.toml"), one).unwrap();
    let unported = "x pun.toml  rule not supported by Pun yet: `lint/laravel/prefer-pivot-class`\n  remove it from pun.toml to run the other rules\n";
    for cmd in [&["format", "--test"][..], &["lint"], &["check", "--fix"]] {
        let (out, err, code) = pun(&dir, cmd);
        assert_eq!(
            (out.as_str(), err.as_str(), code),
            ("", unported, Some(2)),
            "{cmd:?}"
        );
    }
    // Lint rules count too, even for `pun format` with no format rules.
    std::fs::write(
        dir.join("pun.toml"),
        "[format]\npreset = \"empty\"\n[lint.rules]\n\"laravel/prefer-pivot-class\" = \"error\"\n",
    )
    .unwrap();
    let (_, err, code) = pun(&dir, &["format", "--test"]);
    let rules = "rule not supported by Pun yet: `lint/laravel/prefer-pivot-class`";
    assert_eq!((err.as_str(), code), (unported, Some(2)));
    // Agent mode: the same message as its one-line error, same exit code.
    let (out, _, code) = pun_env(&dir, &["lint"], &[("PUN_AGENT", "1")]);
    let toml = dir.join("pun.toml");
    assert_eq!(
        (out, code),
        (
            format!("pun lint: error: {}: {rules}\n", toml.display()),
            Some(2)
        )
    );
    // A named lint rule Pun can't run yet stops the run too; a preset skips the rules Pun can't run yet.
    std::fs::write(
        dir.join("pun.toml"),
        "[format]\npreset = \"empty\"\n[lint]\npreset = \"strict\"\n[lint.rules]\n\"laravel/prefer-pivot-class\" = \"error\"\n",
    )
    .unwrap();
    let (_, err, code) = pun(&dir, &["lint"]);
    assert_eq!((err.as_str(), code), (unported, Some(2)));
    // `self-update` checks the version it is given before it downloads anything.
    assert_eq!(
        pun(&dir, &["self-update", "1.x"]),
        (
            String::new(),
            "x not a release version: v1.x\n".into(),
            Some(2)
        )
    );

    std::fs::write(
        dir.join("pun.toml"),
        "[format]\npreset = \"empty\"\n[lint]\npreset = \"strict\"\n",
    )
    .unwrap();
    let (out, err, code) = pun_env(&dir, &["lint", "app/Clean.php"], &[("PUN_AGENT", "1")]);
    assert_eq!((err.as_str(), code), ("", Some(1)));
    assert!(
        out.ends_with("\napp/Clean.php lint/statements/require-strict-types [fix:unsafe] Force a strict types mode in all files.\n"),
        "{out}"
    );
    std::fs::remove_file(dir.join("pun.toml")).unwrap();
    for args in [
        &["format", "--test", "--preset=per"][..],
        &["check", "--preset=symfony"],
        &["lint", "--preset=per"],
        &["config", "--preset=symfony"],
    ] {
        let (out, err, code) = pun(&dir, args);
        assert_eq!(
            (out.as_str(), err.as_str(), code),
            ("", "x Preset not found.\n", Some(2)),
            "{args:?}"
        );
    }
    std::fs::write(dir.join("pun.toml"), "[format]\npreset = \"symfony\"\n").unwrap();
    let (_, err, code) = pun(&dir, &["format", "--test"]);
    assert_eq!(code, Some(2), "{err}");

    std::fs::write(dir.join("pun.toml"), "[format]\nnope = 1\n").unwrap();
    let (out, err, code) = pun(&dir, &["format", "--test", "app/Clean.php"]);
    assert_eq!((out.as_str(), code), ("", Some(2)));
    assert!(
        err.starts_with(&format!("x invalid {}: ", dir.join("pun.toml").display())),
        "{err}"
    );

    reset(&dir, &files());
    migrate(&dir);

    let (out, err, code) = pun(&dir, &["format", "--staged", "--test", "--format=json"]);
    assert_eq!((err.as_str(), code), ("", Some(2)));
    assert!(
        out.contains("\n  The [--staged] option is only available when using Git.  \n"),
        "{out}"
    );

    let (out, _, code) = pun(&dir, &["config", "--format=json"]);
    assert_eq!(code, Some(0));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["format"]["preset"], "empty");
    assert_eq!(v["format"]["rules"]["arrays/syntax"], true);
    let (out, _, _) = pun(&dir, &["config", "--preset=psr12", "--config=pun.toml"]);
    let t: toml::Value = toml::from_str(&out).unwrap();
    assert_eq!(t["format"]["preset"].as_str(), Some("psr12"));
    let (_, err, code) = pun(&dir, &["config", "--preset=nope"]);
    assert_eq!((err.as_str(), code), ("x Preset not found.\n", Some(2)));
}

/// Pint's error box for a one-line message: Symfony's `renderThrowable` block, two spaces either side.
fn pint_box(message: &str) -> String {
    let blank = " ".repeat(message.chars().count() + 4);
    format!("\n{blank}\n  {message}  \n{blank}\n\n")
}

/// A config error is a run error: under `--format` or `-q` it is Pint's error box on stdout, else Pun's message on
/// stderr; exit 2 either way.
#[test]
fn config_errors_box_under_format_and_quiet() {
    let dir = scratch("cli", "config-box");
    reset(&dir, &files());
    migrate(&dir);
    let wide = [("COLUMNS", "300")];
    let root = dir.display();
    let missing = format!("can't read {root}/missing.toml: No such file or directory (os error 2)");
    // Pun has no symfony preset (Pint does), so it is an unknown one
    let symfony = "Preset not found.";
    for (args, message) in [
        (
            &["format", "--config=missing.toml", "--test", "--format=json"][..],
            missing.as_str(),
        ),
        (&["lint", "--config=missing.toml", "-q"], &missing),
        (
            &["format", "--preset=symfony", "--test", "--format=json"],
            symfony,
        ),
        (
            &["check", "--preset=symfony", "--format=checkstyle"],
            symfony,
        ),
        (
            &["format", "--preset=nope", "--test", "-q"],
            "Preset not found.",
        ),
    ] {
        assert_eq!(
            pun_env(&dir, args, &wide),
            (pint_box(message), String::new(), Some(2)),
            "{args:?}"
        );
    }
    std::fs::write(
        dir.join("pun.toml"),
        "[format]\npreset = \"empty\"\n[format.rules]\n\"nope/x\" = true\n",
    )
    .unwrap();
    let unknown = format!("{root}/pun.toml: `nope/x` is not a format rule");
    assert_eq!(
        pun_env(&dir, &["format", "--test", "--format=json"], &wide),
        (pint_box(&unknown), String::new(), Some(2))
    );
    assert_eq!(
        pun_env(&dir, &["format", "--test"], &wide),
        (String::new(), format!("x {unknown}\n"), Some(2))
    );
    std::fs::write(dir.join("pun.toml"), "x = [\n").unwrap();
    let (out, err, code) = pun_env(&dir, &["format", "--test", "--format=json"], &wide);
    assert_eq!((err.as_str(), code), ("", Some(2)));
    let first = format!("\n  invalid {root}/pun.toml: TOML parse error at line 1, column 6");
    assert!(out.starts_with('\n') && out.contains(&first), "{out:?}");
}

/// The flags dropped before the first beta (Pint's `--bail`, `--cache-file` and `--ansi`/`--no-ansi`, Pun's
/// `--stdin` and `--since`) are clap's unknown arguments on every command that takes `pun format`'s flags, and
/// `--max-processes` takes only a positive number.
#[test]
fn removed_flags_are_unknown_arguments() {
    let dir = scratch("cli", "removed");
    reset(&dir, &files());
    migrate(&dir);
    for cmd in ["format", "lint", "check"] {
        for flag in [
            "--ansi",
            "--no-ansi",
            "--bail",
            "--cache-file=c",
            "--stdin",
            "--since=main",
        ] {
            let (out, err, code) = pun(&dir, &[cmd, "--test", flag]);
            let name = flag.split('=').next().unwrap();
            assert!(
                out.is_empty()
                    && code == Some(2)
                    && err.starts_with(&format!("error: unexpected argument '{name}' found")),
                "{cmd} {flag}: {err}"
            );
        }
        for n in ["0", "-1", "abc", "2abc"] {
            let (out, err, code) = pun(&dir, &[cmd, "--test", &format!("--max-processes={n}")]);
            assert!(
                out.is_empty()
                    && code == Some(2)
                    && err.starts_with(&format!(
                        "error: invalid value '{n}' for '--max-processes <MAX_PROCESSES>'"
                    )),
                "{cmd} {n}: {err}"
            );
        }
    }
    let formats = "  use json, agent, checkstyle, junit, gitlab or github\n";
    for f in ["txt", "xml"] {
        let unsupported = format!("Format [{f}] is not supported.");
        assert_eq!(
            pun(&dir, &["format", "--test", &format!("--format={f}")]),
            (pint_box(&unsupported), String::new(), Some(2))
        );
        let (_, err, code) = pun(
            &dir,
            &[
                "format",
                "--test",
                "--output-to-file=r",
                &format!("--output-format={f}"),
            ],
        );
        assert_eq!(
            (err, code),
            (format!("x {unsupported}\n{formats}"), Some(2))
        );
    }
}

/// The custom-fixer fixture, loading the real PHP-CS-Fixer (`.oracle/php-cs-fixer`) through its autoloader.
fn php_cs_fixer_tree(oracle: &Path) -> Vec<(&'static str, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/php-cs-fixer-custom");
    let read = |f: &str| std::fs::read_to_string(src.join(f)).unwrap();
    let real = oracle
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("php-cs-fixer/vendor/autoload.php");
    let autoload = format!("<?php\nreturn require '{}';\n", real.display());
    vec![
        ("composer.json", r#"{"require":{"php":"^8.4"}}"#.into()),
        (".php-cs-fixer.php", read(".php-cs-fixer.php")),
        ("UpperTodoFixer.php", read("UpperTodoFixer.php")),
        ("app/A.php", "<?php\n$a = \"x\";\n$b = sizeof([]);\n".into()),
        ("vendor/autoload.php", autoload),
        ("vendor/friendsofphp/php-cs-fixer/.keep", String::new()),
    ]
}

/// A notice the config writes straight to STDOUT (past the helper's output buffer) comes before the helper's JSON:
/// migrate writes the same pun.toml as without it.
#[test]
fn migrates_php_cs_fixer_config_past_stdout_notices() {
    let Some(oracle) = oracle() else { return };
    let dir = scratch("cli", "pcf-notice");
    let tree = php_cs_fixer_tree(&oracle);
    let tree: Vec<(&str, &str)> = tree.iter().map(|(a, b)| (*a, b.as_str())).collect();
    reset(&dir, &tree);
    let (_, _, code) = pun(&dir, &["migrate"]);
    assert_eq!(code, Some(0));
    let toml = std::fs::read_to_string(dir.join("pun.toml")).unwrap();
    reset(&dir, &tree);
    let noisy = std::fs::read_to_string(dir.join(".php-cs-fixer.php"))
        .unwrap()
        .replacen(
            "<?php\n",
            "<?php\nfwrite(STDOUT, \"Deprecated: a notice on stdout\\n\");\n",
            1,
        );
    std::fs::write(dir.join(".php-cs-fixer.php"), noisy).unwrap();
    let (_, err, code) = pun(&dir, &["migrate"]);
    assert_eq!((err.as_str(), code), ("", Some(0)));
    assert_eq!(std::fs::read_to_string(dir.join("pun.toml")).unwrap(), toml);
}

/// Projects that keep PHP-CS-Fixer out of their own vendor/ (Composer, Monolog: a global install; nikic/php-parser:
/// tools/composer.json; Guzzle: bamarni's vendor-bin/) migrate as with it in vendor/: through the tool directory's
/// autoloader, or the `php-cs-fixer` on PATH (here the oracle's source bin, whose autoloader sits next to it).
#[test]
fn migrates_php_cs_fixer_config_without_it_in_vendor() {
    let Some(oracle) = oracle() else { return };
    let dir = scratch("cli", "pcf-tool");
    let tree = php_cs_fixer_tree(&oracle);
    let tree: Vec<(&str, &str)> = tree.iter().map(|(a, b)| (*a, b.as_str())).collect();
    reset(&dir, &tree);
    assert_eq!(pun(&dir, &["migrate"]).2, Some(0));
    let want = std::fs::read_to_string(dir.join("pun.toml")).unwrap();
    for tool in ["tools", "vendor-bin/php-cs-fixer"] {
        let moved: Vec<(String, &str)> = (tree.iter())
            .map(|(f, c)| match f.strip_prefix("vendor/") {
                Some(rest) => (format!("{tool}/vendor/{rest}"), *c),
                None => ((*f).to_string(), *c),
            })
            .collect();
        let moved: Vec<(&str, &str)> = moved.iter().map(|(f, c)| (f.as_str(), *c)).collect();
        reset(&dir, &moved);
        let (_, err, code) = pun(&dir, &["migrate"]);
        assert_eq!((err.as_str(), code), ("", Some(0)), "{tool}");
        assert_eq!(
            std::fs::read_to_string(dir.join("pun.toml")).unwrap(),
            want,
            "{tool}"
        );
    }
    let no_vendor: Vec<(&str, &str)> = (tree.iter().copied())
        .filter(|(f, _)| !f.starts_with("vendor/"))
        .collect();
    reset(&dir, &no_vendor);
    let bin_dir = oracle.ancestors().nth(3).unwrap().join("php-cs-fixer");
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let (_, err, code) = pun_env(&dir, &["migrate"], &[("PATH", path.as_str())]);
    assert_eq!((err.as_str(), code), ("", Some(0)));
    assert_eq!(std::fs::read_to_string(dir.join("pun.toml")).unwrap(), want);
}

#[test]
fn migrates_php_cs_fixer_config() {
    let Some(oracle) = oracle() else { return };
    let dir = scratch("cli", "pcf");
    let tree = php_cs_fixer_tree(&oracle);
    let tree: Vec<(&str, &str)> = tree.iter().map(|(a, b)| (*a, b.as_str())).collect();
    reset(&dir, &tree);
    std::fs::write(
        dir.join(".php-cs-fixer.php"),
        "<?php\nthrow new Exception('bad config');\n",
    )
    .unwrap();
    let (_, err, code) = pun(&dir, &["migrate"]);
    assert_eq!(
        (err.as_str(), code),
        (
            "x can't read .php-cs-fixer.php with PHP-CS-Fixer: bad config\n",
            Some(2)
        )
    );
    assert!(!dir.join("pun.toml").exists());
    reset(&dir, &tree);
    std::fs::remove_dir_all(dir.join("vendor/friendsofphp")).unwrap();
    let (_, err, code) = pun(&dir, &["migrate"]);
    assert_eq!(
        (err.as_str(), code),
        (
            "x can't read .php-cs-fixer.php with PHP-CS-Fixer: no PHP-CS-Fixer in vendor/, tools/ or vendor-bin/, and no `php-cs-fixer` on PATH: install it with `composer require --dev friendsofphp/php-cs-fixer`\n",
            Some(2)
        )
    );
    reset(&dir, &tree);
    let skipped = "\n ! Not migrated\n   not a PHP-CS-Fixer or Pint rule Pun knows (a custom fixer or rule set)\n     Acme/upper_todo\n   Want them? Send a PR to https://github.com/shipfastlabs/pun\n\n ok Wrote pun.toml\n";
    let (out, err, code) = pun(&dir, &["migrate"]);
    assert_eq!((err.as_str(), code), ("", Some(0)));
    assert!(
        out.starts_with(" ok .php-cs-fixer.php  -> [format] preset empty, ")
            && out.ends_with(skipped),
        "{out}"
    );
    let toml = std::fs::read_to_string(dir.join("pun.toml")).unwrap();
    assert!(
        toml.starts_with("# Written by `pun migrate` from .php-cs-fixer.php.\n[files]\npaths = [\"app\"]\n\n[format]\npreset = \"empty\"\n\n[format.rules]\n")
            && toml.contains("\n\"strings/single-quote\" = true\n"),
        "{toml}"
    );
    reset(&dir, &tree);
    let (out, _, code) = pun_env(&dir, &["migrate"], &[("PUN_AGENT", "1")]);
    let (first, rest) = out.split_once('\n').unwrap();
    assert_eq!(code, Some(0));
    assert!(
        first.starts_with("pun migrate: ok, pun.toml written (preset empty, "),
        "{first}"
    );
    assert_eq!(
        rest,
        ".php-cs-fixer.php migrated\n.php-cs-fixer.php unsupported Acme/upper_todo: not a PHP-CS-Fixer or Pint rule Pun knows (a custom fixer or rule set)\nsend a PR for what is skipped: https://github.com/shipfastlabs/pun\n"
    );
    reset(&dir, &tree);
    std::fs::create_dir_all(dir.join("legacy")).unwrap();
    std::fs::write(dir.join("legacy/L.php"), "<?php\n$b = sizeof([]);\n").unwrap();
    std::fs::write(
        dir.join(".php-cs-fixer.php"),
        "<?php\nreturn (new PhpCsFixer\\Config())->setRiskyAllowed(true)->setRules(['single_quote' => true, 'no_alias_functions' => true])\n    ->setFinder(PhpCsFixer\\Finder::create()->in(__DIR__)->name('*.php')->exclude('legacy')->notName('*.inc')->depth('< 3')->ignoreVCSIgnored(true));\n",
    )
    .unwrap();
    let (out, err, code) = pun(&dir, &["migrate"]);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(
        out,
        " ok .php-cs-fixer.php  -> [format] preset empty, 1 rule changed, 1 rule -> [lint]\n\n ! Not migrated\n   no pun.toml key for it\n     Finder depth()\n     Finder ignoreVCSIgnored()\n   Want them? Send a PR to https://github.com/shipfastlabs/pun\n\n ok Wrote pun.toml\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("pun.toml")).unwrap(),
        "# Written by `pun migrate` from .php-cs-fixer.php.\n[files]\nexclude = [\"legacy\"]\n\n[format]\npreset = \"empty\"\nnot-name = [\"*.inc\"]\n\n[format.rules]\n\"strings/single-quote\" = true\n\n[lint.rules]\n\"style/no-alias-function\" = \"error\"\n\n[[lint.overrides]]\nfiles = [\"**/*.inc\"]\n\n[lint.overrides.rules]\n\"style/no-alias-function\" = \"off\"\n"
    );
    pun(&dir, &["format"]);
    let a = || std::fs::read_to_string(dir.join("app/A.php")).unwrap();
    assert_eq!(a(), "<?php\n$a = 'x';\n$b = sizeof([]);\n");
    pun(&dir, &["check", "--fix"]);
    assert_eq!(a(), "<?php\n$a = 'x';\n$b = count([]);\n");
    assert_eq!(
        std::fs::read_to_string(dir.join("legacy/L.php")).unwrap(),
        "<?php\n$b = sizeof([]);\n"
    );
}

#[test]
fn extension_list_says_why_each_is_on() {
    let dir = scratch("cli", "extensions");
    write_files(
        &dir,
        &[
            (
                "composer.lock",
                r#"{"packages":[{"name":"laravel/framework","version":"v12.4.1"}],"packages-dev":[{"name":"pestphp/pest","version":"v4.1.0"}]}"#,
            ),
            ("pun.toml", "[extensions]\nlaravel = \"on\"\n"),
        ],
    );
    // A locked package alone turns nothing on: pest is off, and the list says how to turn it on.
    let want = r#"laravel           on   Laravel's rules and Larastan's analysis: on in pun.toml
pest              off  Rules for Pest tests: pestphp/pest v4.1.0 is locked; turn it on with pest = "on" in [extensions]
phpunit           off  Rules for PHPUnit tests: not in pun.toml
carbon            off  Carbon's macros and DatePeriodBase in analyse: not in pun.toml
mockery           off  phpstan-mockery's mock types in analyse: not in pun.toml
disallowed-calls  off  The calls and uses your config forbids, in analyse: not in pun.toml
type-coverage     off  Minimum shares of declared types, in analyse: not in pun.toml
"#;
    assert_eq!(
        pun(&dir, &["extension", "list"]),
        (want.into(), String::new(), Some(0))
    );
    let (out, _, code) = pun(&dir, &["extension", "list", "--format=json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let on: Vec<_> = (v.as_array().unwrap().iter())
        .map(|e| {
            (
                e["id"].as_str().unwrap(),
                e["on"].as_bool().unwrap(),
                e["switch"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        (on, code),
        (
            vec![
                ("laravel", true, Some("on")),
                ("pest", false, None),
                ("phpunit", false, None),
                ("carbon", false, None),
                ("mockery", false, None),
                ("disallowed-calls", false, None),
                ("type-coverage", false, None),
            ],
            Some(0)
        )
    );
    std::fs::remove_file(dir.join("pun.toml")).unwrap();
    let (out, _, code) = pun(&dir, &["extension", "list"]);
    assert!(
        out.starts_with("laravel           off ") && out.contains("\npest              off "),
        "{out}"
    );
    assert_eq!(code, Some(0));
    let (_, err, code) = pun(&dir, &["extension", "list", "--format=xml"]);
    assert_eq!(
        (err.as_str(), code),
        ("x --format=xml: expected `json`\n", Some(2))
    );
}

#[test]
fn config_prints_resolved_schema() {
    let dir = scratch("cli", "config");
    let toml = r#"[php]
version = "8.5"

[files]
paths = ["app", "tests"]
exclude = ["storage/**"]

[format]
preset = "empty"
exclude = ["app/Generated/**"]
not-path = ["app/Legacy.php"]

[format.rules]
"arrays/syntax" = true
"operators/concat-spacing" = { spacing = "one" }

[lint]
exclude = ["database/**"]

[lint.groups]
tags = "warn"

[lint.rules]
"style/no-alias-function" = { fix = "off" }
"dead-code/no-self-assignment" = "error"

[[lint.overrides]]
files = ["tests/**"]
rules = { "dead-code/no-self-assignment" = "off" }

[analyse]
preset = "basic"
exclude = ["tests/**"]
baseline = "pun-baseline.toml"

[analyse.rules]
arrays = "off"
attributes = "off"
casing = "off"
casts = "off"
classes = "off"
comments = "off"
control-flow = "off"
dead-code = "off"
debug = "off"
deprecated = "off"
functions = "off"
imports = "off"
operators = "off"
phpdoc = "off"
statements = "off"
strings = "off"
types = "off"
undefined = "off"
"undefined/method" = { check-function-name-case = true }
"#;
    std::fs::write(dir.join("pun.toml"), toml).unwrap();
    let want = r#"# Resolved from pun.toml.

[php]
version = "8.5"

[files]
paths = ["app", "tests"]
exclude = ["storage/**"]

[extensions]
laravel = "off"
pest = "off"
phpunit = "off"
carbon = "off"
mockery = "off"
disallowed-calls = "off"
type-coverage = "off"

[format]
preset = "empty"
exclude = ["storage/**", "app/Generated/**"]
not-path = ["app/Legacy.php"]
not-name = []

[format.rules]
"arrays/syntax" = true
"operators/concat-spacing" = "one"

[lint]
sets = []
exclude = ["storage/**", "database/**"]
ecosystems = []

[lint.rules]
"dead-code/no-self-assignment" = "error"
"tags/no-invalid-open-tag" = "warn"

[lint.rules."style/no-alias-function"]
fix = "off"

[[lint.overrides]]
files = ["tests/**"]

[lint.overrides.rules]
"dead-code/no-self-assignment" = "off"

[analyse]
preset = "basic"
paths = ["app", "tests"]
exclude = ["storage/**", "tests/**"]
switches = []
ecosystems = []
baseline = "pun-baseline.toml"

[analyse.rules."undefined/method"]
check-function-name-case = true
"#;
    assert_eq!(
        pun(&dir, &["config"]),
        (want.into(), String::new(), Some(0))
    );
    let (out, _, code) = pun(
        &scratch("cli", "config-none"),
        &["config", "--preset=empty"],
    );
    assert!(out.starts_with("[php]\nversion = \"8.3\"\n"), "{out}");
    assert_eq!(code, Some(0));
}

#[test]
fn explain_rule() {
    let dir = scratch("cli", "explain");
    let text = "lint/style/no-alias-function
Master functions shall be used instead of aliases.

group     style: One consistent way to write the same thing
preset    strict
fix       unsafe
upstream  no_alias_functions (php-cs-fixer)
          no-alias-function (mago-linter)
docs      https://punphp.com/docs/rules/lint/style/no-alias-function
";
    let (out, err, code) = pun(&dir, &["explain", "lint/style/no-alias-function"]);
    assert_eq!(out, text);
    assert_eq!((err.as_str(), code), ("", Some(0)));
    let (out, _, code) = pun(&dir, &["explain", "no_alias_functions"]);
    let note = "`no_alias_functions` is `lint/style/no-alias-function` in Pun.\n\n";
    assert!(out.starts_with(&format!("{note}{text}")), "{out}");
    assert_eq!(code, Some(0));
    assert_eq!(
        pun(&dir, &["explain", "nope"]),
        (
            String::new(),
            "x `nope` is not a Pun rule or an upstream rule name\n".into(),
            Some(2)
        )
    );
}

#[test]
fn usage_line_says_pun_under_another_file_name() {
    let dir = std::env::temp_dir().join(format!("pun-usage-name-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("pun-0.1.0-x86_64-unknown-linux-musl");
    std::fs::copy(PUN, &copy).unwrap();
    let help = |args: &[&str]| {
        let out = Command::new(&copy)
            .args(args)
            .env("PUN_AGENT", "0")
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    let (top, sub) = (help(&["--help"]), help(&["init", "--help"]));
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(top.contains("Usage: pun [OPTIONS] <COMMAND>\n"), "{top}");
    assert!(sub.contains("Usage: pun init [OPTIONS]\n"), "{sub}");
}
