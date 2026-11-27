//! `pun composer exec` against `composer exec` 2.11.2 (the Composer the plugin host pins), and the `run-script` cases
//! that share its way of running a command. `fixtures/exec.json` (written by `scripts/export-exec-cases.py`) holds per
//! case a small project, the words after `composer`, and Composer's exit code, stdout and stderr. Pun must print the
//! same bytes and exit the same way, except where Composer stops with an error of its own (exit 1): that is a run
//! error, which Pun prints in its own form with Composer's lines, in order, and exits 2. Where stderr starts with what
//! /bin/sh itself says (`stderr_tail`), which differs between systems, only Composer's lines after it count. A result
//! that differs between macOS (bash as /bin/sh) and Linux (dash) is under the case's `macos` and `linux`.

mod common;

use common::{PUN, scratch};
use serde_json::Value;
use std::process::Command;

const FIXTURE: &str = include_str!("fixtures/exec.json");

struct Outcome {
    exit: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run(case: &Value) -> Outcome {
    let dir = scratch("exec", case["name"].as_str().unwrap());
    for (path, content) in case["files"].as_object().unwrap() {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, content.as_str().unwrap()).unwrap();
    }
    for path in case["executable"].as_array().unwrap() {
        std::fs::set_permissions(
            dir.join(path.as_str().unwrap()),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
    }
    let out = common::clean_env(&mut Command::new(PUN), &["PATH"])
        .arg("composer")
        .args(
            case["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap()),
        )
        .current_dir(dir.join(case["cwd"].as_str().unwrap()))
        .env("PUN_AGENT", "0")
        .env("HOME", &dir)
        .env("COMPOSER_ROOT_VERSION", "1.0.0")
        .output()
        .unwrap();
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

/// How a run differs from Composer's result: the same exit code, stdout and stderr (with `stderr_tail`, the same
/// stderr after its first line, which /bin/sh writes).
fn result_difference(case: &Value, got: &Outcome) -> Option<String> {
    let want_stderr = case["stderr"].as_str().unwrap();
    let stderr_matches = if case["stderr_tail"].as_bool().unwrap() {
        let tail = want_stderr.split_once('\n').map_or("", |(_, rest)| rest);
        got.stderr.ends_with(tail) && got.stderr.lines().count() == want_stderr.lines().count()
    } else {
        got.stderr == want_stderr
    };
    let want = (
        case["exit"].as_i64().and_then(|c| i32::try_from(c).ok()),
        case["stdout"].as_str().unwrap(),
    );
    (want != (got.exit, got.stdout.as_str()) || !stderr_matches).then(|| {
        format!(
            "want exit {:?}\nstdout:\n{}\nstderr:\n{}\ngot exit {:?}\nstdout:\n{}\nstderr:\n{}",
            want.0, want.1, want_stderr, got.exit, got.stdout, got.stderr
        )
    })
}

/// Every case, so one run reports every difference rather than the first.
#[test]
fn exec_matches_composer() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    let mut failures = vec![];
    let system = if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    for case in cases {
        // This system's results over the shared ones; a null drops one (a `message` the other system has).
        let mut case = case.clone();
        if let Some(Value::Object(results)) = case.as_object_mut().unwrap().remove(system) {
            let case = case.as_object_mut().unwrap();
            for (key, value) in results {
                if value.is_null() {
                    case.remove(&key);
                } else {
                    case.insert(key, value);
                }
            }
        }
        let case = &case;
        let got = run(case);
        let difference = match case.get("message").and_then(Value::as_array) {
            Some(message) => {
                let lines: Vec<&str> = message.iter().map(|l| l.as_str().unwrap()).collect();
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

/// In a terminal, without a binary, Composer asks which binary to run (Pun asks with its own picker); the second one
/// here, which then runs.
#[test]
fn exec_asks_which_binary_in_a_terminal() {
    let dir = scratch("exec", "asks");
    std::fs::create_dir_all(dir.join("vendor/bin")).unwrap();
    std::fs::write(dir.join("composer.json"), r#"{"name":"acme/app"}"#).unwrap();
    for name in ["first", "second"] {
        let bin = dir.join("vendor/bin").join(name);
        std::fs::write(&bin, format!("#!/bin/sh\necho ran {name}\n")).unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
    }
    let (out, code) = common::pty::pty(&dir, "composer exec", "\\033[B\\r", &[]);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("Binary to run"), "{out}");
    assert!(out.trim_end().ends_with("ran second"), "{out:?}");
}

/// Composer's `--help`, `--version` and `--profile` after the binary print its help, its version or its timings;
/// Pun doesn't take them there and says so (a run error).
#[test]
fn exec_refuses_composer_options_it_does_not_take() {
    let dir = scratch("exec", "refuses");
    std::fs::write(dir.join("composer.json"), r#"{"name":"acme/app"}"#).unwrap();
    for option in ["--help", "-h", "-V", "--version", "--profile"] {
        let out = common::clean_env(&mut Command::new(PUN), &["PATH"])
            .args(["composer", "exec", "tool", option])
            .current_dir(&dir)
            .env("PUN_AGENT", "0")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{option}: {stderr}");
        let name = match option {
            "-h" => "help",
            "-V" => "version",
            o => o.trim_start_matches('-'),
        };
        assert!(
            stderr.contains(&format!("`pun composer exec` doesn't take --{name}")),
            "{option}: {stderr}"
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
