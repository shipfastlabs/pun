//! `pun composer suggests` against `composer suggests` 2.11.2 (the Composer the plugin host pins).
//! `fixtures/suggests.json` (written by `scripts/export-suggests-cases.py`) holds the platform Composer ran on, as
//! Pun's probe of the same PHP, and per case a small project, the words after `composer`, and Composer's exit code,
//! stdout and stderr. Composer leaves out suggestions the platform already has, so Pun gets that platform from a
//! `PHP_BINARY` that prints the probe, and the cases hold on any machine. Pun must print the same bytes and exit the
//! same way, except where Composer stops with an error of its own (exit 1 with an exception): that is a run error,
//! which Pun prints in its own form with Composer's lines, in order, and exits 2.

mod common;

use common::{PUN, scratch};
use serde_json::Value;
use std::process::Command;

const FIXTURE: &str = include_str!("fixtures/suggests.json");

/// Composer follows the first line of a JSON error with jsonlint's description of it, which Pun gives once it shares
/// `validate`'s port of jsonlint (context/PLAN.md §3): only that first line is asked for.
const FIRST_LINE_ONLY: &[&str] = &["lock-invalid-json"];

struct Outcome {
    exit: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run(case: &Value, probe: &Value) -> Outcome {
    let name = case["name"].as_str().unwrap();
    let dir = scratch("suggests", name);
    for (path, content) in case["files"].as_object().unwrap() {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, content.as_str().unwrap()).unwrap();
    }
    // A PHP that reports the recorded platform, whatever it is asked.
    let bin = scratch("suggests", &format!("{name}-php"));
    std::fs::write(bin.join("probe.json"), probe.to_string()).unwrap();
    let php = bin.join("php");
    std::fs::write(
        &php,
        format!("#!/bin/sh\ncat '{}'\n", bin.join("probe.json").display()),
    )
    .unwrap();
    std::fs::set_permissions(&php, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
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
        .env("COMPOSER_ROOT_VERSION", "1.0.0")
        .env("PHP_BINARY", &php);
    // The case's own environment, where a null leaves a variable out.
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
    std::fs::remove_dir_all(&bin).unwrap();
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
fn suggests_matches_composer() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    let mut failures = vec![];
    for case in cases {
        let got = run(case, &fixture["probe"]);
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
