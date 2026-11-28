//! `pun composer licenses` against `composer licenses` 2.11.2 (the Composer the plugin host pins).
//! `fixtures/licenses.json` (written by `scripts/export-licenses-cases.py`) holds per case a small project, the words
//! after `composer`, and Composer's exit code, stdout and stderr. Pun must print the same bytes and exit the same way,
//! except where Composer stops with an error of its own (exit 1): that is a run error, which Pun prints in its own form
//! with Composer's lines, in order, and exits 2.

mod common;

use common::{PUN, scratch};
use serde_json::Value;
use std::process::Command;

const FIXTURE: &str = include_str!("fixtures/licenses.json");

/// Composer follows the first line of a JSON error with jsonlint's description of it, which Pun gives once it shares
/// `validate`'s port of jsonlint (context/PLAN.md §3): only that first line is asked for.
const FIRST_LINE_ONLY: &[&str] = &["locked-invalid-lock"];

/// A case's `git`: its files as the one commit of a repository on `main`, made as the recorder makes it (the same
/// author, committer and dates), so it has the same hash.
fn git_commit(dir: &std::path::Path) {
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["add", "-A"],
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "one"],
    ] {
        let status = common::clean_env(&mut Command::new("git"), &["PATH"])
            .args(args)
            .current_dir(dir)
            .env("HOME", dir)
            .envs([
                ("GIT_AUTHOR_NAME", "a"),
                ("GIT_AUTHOR_EMAIL", "a@b"),
                ("GIT_COMMITTER_NAME", "a"),
                ("GIT_COMMITTER_EMAIL", "a@b"),
                ("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z"),
                ("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z"),
            ])
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }
}

struct Outcome {
    exit: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run(case: &Value) -> Outcome {
    let dir = scratch("licenses", case["name"].as_str().unwrap());
    for (path, content) in case["files"].as_object().unwrap() {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, content.as_str().unwrap()).unwrap();
    }
    if case["git"].as_bool() == Some(true) {
        git_commit(&dir);
    }
    let mut command = Command::new(PUN);
    common::clean_env(&mut command, &["PATH"])
        .arg("composer")
        .args(
            case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap()),
        )
        .current_dir(&dir)
        .env("PUN_AGENT", "0")
        .env("HOME", &dir)
        // The recorder's environment: COMPOSER_ROOT_VERSION=1.0.0 and the case's own, where a null leaves it out.
        .env("COMPOSER_ROOT_VERSION", "1.0.0");
    for (key, value) in (case.get("env").and_then(Value::as_object).into_iter()).flatten() {
        match value.as_str() {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    let out = command.output().unwrap();
    // The fixture writes the project's directory as `{dir}`.
    let here = dir.canonicalize().unwrap().to_string_lossy().into_owned();
    std::fs::remove_dir_all(&dir).unwrap();
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace(&here, "{dir}");
    Outcome {
        exit: out.status.code(),
        stdout: text(&out.stdout),
        stderr: text(&out.stderr),
    }
}

/// How a run differs from a Composer error: Pun must exit 2 with Composer's lines, in order, in stderr.
fn run_error_difference(lines: &[&str], got: &Outcome) -> Option<String> {
    let in_order = (lines.iter())
        .try_fold(0, |from, line| {
            got.stderr[from..]
                .find(line)
                .map(|at| from + at + line.len())
        })
        .is_some();
    (got.exit != Some(2) || !got.stdout.is_empty() || !in_order).then(|| {
        format!(
            "run error, want exit 2 and these lines in stderr:\n{}\ngot exit {:?}\nstdout:\n{}\nstderr:\n{}",
            lines.join("\n"),
            got.exit,
            got.stdout,
            got.stderr
        )
    })
}

/// How a run differs from Composer's result: the same exit code, stdout and stderr.
fn result_difference(case: &Value, got: &Outcome) -> Option<String> {
    let want = (
        case["exit"].as_i64().and_then(|c| i32::try_from(c).ok()),
        case["stdout"].as_str().unwrap(),
        case["stderr"].as_str().unwrap(),
    );
    (want != (got.exit, got.stdout.as_str(), got.stderr.as_str())).then(|| {
        format!(
            "want exit {:?}\nstdout:\n{}\nstderr:\n{}\ngot exit {:?}\nstdout:\n{}\nstderr:\n{}",
            want.0, want.1, want.2, got.exit, got.stdout, got.stderr
        )
    })
}

/// Every case, so one run reports every difference rather than the first.
#[test]
fn licenses_matches_composer() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    let mut failures = vec![];
    for case in cases {
        let got = run(case);
        let difference = match case.get("message").and_then(Value::as_array) {
            Some(message) => {
                let mut lines: Vec<&str> = message.iter().map(|l| l.as_str().unwrap()).collect();
                if FIRST_LINE_ONLY.contains(&case["name"].as_str().unwrap()) {
                    lines.truncate(1);
                }
                run_error_difference(&lines, &got)
            }
            None => result_difference(case, &got),
        };
        if let Some(difference) = difference {
            failures.push(format!(
                "=== {}\n{difference}",
                case["name"].as_str().unwrap()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ from Composer:\n\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
}
