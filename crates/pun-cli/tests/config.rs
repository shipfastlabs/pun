//! `pun composer config` against `composer config` 2.11.2 (the Composer the plugin host pins). `fixtures/config.json`
//! (written by `scripts/export-config-cases.py`) holds per case a project and a `COMPOSER_HOME`, the words after
//! `composer`, its environment, and Composer's exit code, stdout and stderr, and every file of both afterwards. Pun
//! must print the same bytes, exit the same way and leave the same files, byte for byte, except where Composer stops
//! with an error of its own (exit 1 with an exception): that is a run error, which Pun prints in its own form with
//! Composer's lines, in order, and exits 2. The `.htaccess` Composer writes in `COMPOSER_HOME` on every command isn't
//! `config`'s, and isn't compared.

mod common;

use common::{PUN, scratch};
use serde_json::{Map, Value};
use std::path::Path;
use std::process::Command;

const FIXTURE: &str = include_str!("fixtures/config.json");

/// Composer follows the first line with lines Pun gives once it shares `validate`'s ports (context/PLAN.md §3):
/// jsonlint's description of a JSON error, or the schema's errors behind "Failed to update …, reverting to the
/// original content". Only that first line is asked for.
const FIRST_LINE_ONLY: &[&str] = &[
    "set-extra-invalid-json",
    "set-repository-one-value",
    "fallback-auth",
];

/// Composer checks the file it wrote against its schema, and puts the old one back when it doesn't hold, which Pun does
/// once it shares `validate`'s port of the schema (context/PLAN.md §3).
const NEEDS_THE_SCHEMA: &[&str] = &["set-suffix-null"];

struct Outcome {
    exit: Option<i32>,
    stdout: String,
    stderr: String,
    after: Map<String, Value>,
    home_after: Map<String, Value>,
}

fn write(base: &Path, files: &Value) {
    for (path, content) in files.as_object().unwrap() {
        let file = base.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, content.as_str().unwrap()).unwrap();
    }
}

/// Every file under `base` but caches and `.htaccess`, by its path, with the temporary directories as placeholders.
fn tree(base: &Path, placeholders: &dyn Fn(&str) -> String) -> Map<String, Value> {
    let mut out = Map::new();
    let mut dirs = vec![base.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                if name != "cache" {
                    dirs.push(path);
                }
            } else if name != ".htaccess" {
                let relative = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let text = std::fs::read_to_string(&path).unwrap();
                out.insert(relative, Value::String(placeholders(&text)));
            }
        }
    }
    out
}

fn run(case: &Value) -> Outcome {
    let name = case["name"].as_str().unwrap();
    let dir = scratch("config", name);
    let user = scratch("config", &format!("{name}-user"));
    let home = user.join(".composer");
    std::fs::create_dir_all(&home).unwrap();
    write(&dir, &case["files"]);
    write(&home, &case["home_files"]);
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
        .env("HOME", &user)
        .env("COMPOSER_HOME", &home)
        .env("COMPOSER_CACHE_DIR", user.join(".cache"))
        .env("COMPOSER_ROOT_VERSION", "1.0.0")
        .env("COLUMNS", "500");
    // The case's own environment, where a null leaves a variable out.
    for (key, value) in (case.get("env").and_then(Value::as_object).into_iter()).flatten() {
        match value.as_str() {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    let out = command.output().unwrap();
    // The fixture writes COMPOSER_HOME as `{home}`, the project as `{dir}` and HOME as `{user}`.
    let canonical = |p: &Path| p.canonicalize().unwrap().to_string_lossy().into_owned();
    let paths = [
        (canonical(&home), "{home}"),
        (home.to_string_lossy().into_owned(), "{home}"),
        (canonical(&dir), "{dir}"),
        (dir.to_string_lossy().into_owned(), "{dir}"),
        (canonical(&user), "{user}"),
        (user.to_string_lossy().into_owned(), "{user}"),
    ];
    let placeholders = |text: &str| {
        (paths.iter()).fold(text.to_string(), |text, (path, placeholder)| {
            text.replace(path.as_str(), placeholder)
        })
    };
    let outcome = Outcome {
        exit: out.status.code(),
        stdout: placeholders(&String::from_utf8_lossy(&out.stdout)),
        stderr: placeholders(&String::from_utf8_lossy(&out.stderr)),
        after: tree(&dir, &placeholders),
        home_after: tree(&home, &placeholders),
    };
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&user).unwrap();
    outcome
}

/// How the files left behind differ from Composer's.
fn files_difference(case: &Value, got: &Outcome) -> Option<String> {
    let mut want_home = case["home_after"].as_object().unwrap().clone();
    want_home.remove(".htaccess");
    let want = (case["after"].as_object().unwrap(), &want_home);
    (want != (&got.after, &got.home_after)).then(|| {
        format!(
            "files, want:\n{:#?}\n{:#?}\ngot:\n{:#?}\n{:#?}",
            want.0, want.1, got.after, got.home_after
        )
    })
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
fn config_matches_composer() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    let mut failures = vec![];
    for case in cases {
        if NEEDS_THE_SCHEMA.contains(&case["name"].as_str().unwrap()) {
            continue;
        }
        let got = run(case);
        let output = match case.get("message").and_then(Value::as_array) {
            Some(message) => {
                let mut lines: Vec<&str> = message.iter().map(|l| l.as_str().unwrap()).collect();
                if FIRST_LINE_ONLY.contains(&case["name"].as_str().unwrap()) {
                    lines.truncate(1);
                }
                run_error_difference(&lines, &got)
            }
            None => result_difference(case, &got),
        };
        let differences: Vec<String> = [output, files_difference(case, &got)]
            .into_iter()
            .flatten()
            .collect();
        if !differences.is_empty() {
            failures.push(format!(
                "=== {}\n{}",
                case["name"].as_str().unwrap(),
                differences.join("\n")
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

/// Composer's `--editor` opens an editor on the file; Pun doesn't, and says so (a run error), leaving it as it was.
#[test]
fn config_refuses_the_editor() {
    let dir = scratch("config", "editor");
    let user = scratch("config", "editor-user");
    std::fs::write(
        dir.join("composer.json"),
        "{\n    \"name\": \"acme/app\"\n}\n",
    )
    .unwrap();
    let out = common::clean_env(&mut Command::new(PUN), &["PATH"])
        .args(["composer", "config", "--editor"])
        .current_dir(&dir)
        .env("PUN_AGENT", "0")
        .env("HOME", &user)
        .env("COMPOSER_HOME", user.join(".composer"))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let after = std::fs::read_to_string(dir.join("composer.json")).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&user).unwrap();
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("doesn't open an editor (--editor)"),
        "{stderr}"
    );
    assert_eq!(after, "{\n    \"name\": \"acme/app\"\n}\n");
}
