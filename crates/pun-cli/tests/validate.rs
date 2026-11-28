//! `pun composer validate` against `composer validate` 2.11.2 (the Composer the plugin host pins). Each case in
//! `fixtures/validate.json` (written by `scripts/export-validate-cases.py`) is a small project with Composer's exit code,
//! stdout and stderr for it. Pun must print the same bytes and exit the same way, except where Composer stops with an
//! exception (its "In <file> line <n>:" block, exit 1): that is a run error, which Pun prints in its own form with
//! Composer's message lines, in order, and exits 2.

mod common;

use common::{PUN, scratch};
use serde_json::Value;
use std::process::Command;

const CASES: &str = include_str!("fixtures/validate.json");

/// What Pun prints in a case where it differs from Composer on purpose.
enum Instead {
    /// A report: the exit code, stdout and stderr.
    Report(i32, &'static str, &'static str),
    /// A run error (exit 2) with these lines, in order.
    RunError(&'static [&'static str]),
}

const SCHEMA: &str = "\"./composer.json\" does not match the expected JSON schema:";

/// The cases where Pun differs from Composer on purpose, and what it prints instead.
///
/// `time` (context/PLAN.md §3): Composer reports a `time` that PHP's `DateTime` can't parse; Pun reads only the
/// documented and ISO 8601 forms and leaves any other value unreported.
///
/// The others are the cases where Composer crashes (`crash` in the fixture): a value of the wrong type reaches a PHP
/// function before Composer loads the root package, whose schema check would have refused it. Pun reports what Composer
/// gives without the crash, taken from Composer 2.11.2's own `JsonFile::validateJsonSchema` (`LAX_SCHEMA`) and
/// `ValidatingArrayLoader` on the same input.
const DIFFERENT: &[(&str, Instead)] = &[
    ("time", Instead::Report(0, "./composer.json is valid\n", "")),
    (
        "schema-require-number",
        Instead::RunError(&[
            SCHEMA,
            " - require.acme/a : Integer value found, but a string is required",
        ]),
    ),
    (
        "schema-root-number",
        Instead::RunError(&[SCHEMA, " - Integer value found, but an object is required"]),
    ),
    (
        "schema-root-string",
        Instead::RunError(&[SCHEMA, " - String value found, but an object is required"]),
    ),
    (
        "schema-license-number",
        Instead::RunError(&[
            SCHEMA,
            " - license : Integer value found, but a string or an array is required",
        ]),
    ),
    (
        "schema-license-mixed-array",
        Instead::Report(
            0,
            "",
            "./composer.json is valid, but with a few warnings\nSee https://getcomposer.org/doc/04-schema.md for details on the schema\n# General warnings\n- License 5 should be a string.\n",
        ),
    ),
    (
        "schema-archive",
        Instead::RunError(&[
            SCHEMA,
            " - archive.name : Integer value found, but a string is required",
            " - archive.exclude : String value found, but an array is required",
        ]),
    ),
];

struct Outcome {
    exit: Option<i32>,
    stdout: String,
    stderr: String,
}

fn validate(case: &Value) -> Outcome {
    let name = case["name"].as_str().unwrap();
    let dir = scratch("validate", name);
    for (path, content) in case["files"].as_object().unwrap() {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        // A file that isn't UTF-8 comes as {"hex": "..."}.
        let bytes = match content.get("hex").and_then(Value::as_str) {
            Some(hex) => (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect(),
            None => content.as_str().unwrap().as_bytes().to_vec(),
        };
        std::fs::write(&file, bytes).unwrap();
    }
    let args: Vec<&str> = (case["args"].as_array().unwrap().iter())
        .map(|a| a.as_str().unwrap())
        .collect();
    let out = common::clean_env(&mut Command::new(PUN), &["PATH"])
        .args(["composer", "validate"])
        .args(&args)
        .current_dir(&dir)
        .env("PUN_AGENT", "0")
        .env("HOME", &dir)
        .envs(
            (case
                .get("env")
                .and_then(Value::as_object)
                .into_iter()
                .flatten())
            .map(|(k, v)| (k, v.as_str().unwrap())),
        )
        .output()
        .unwrap();
    // The fixture writes the project's directory as `{dir}`.
    let here = dir.canonicalize().unwrap().to_string_lossy().into_owned();
    std::fs::remove_dir_all(&dir).unwrap();
    Outcome {
        exit: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).replace(&here, "{dir}"),
        stderr: String::from_utf8_lossy(&out.stderr).replace(&here, "{dir}"),
    }
}

/// How a run differs from a Composer run error: Pun must exit 2 with Composer's message lines, in order, in stderr.
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

/// How a run differs from Composer's report: the same exit code, stdout and stderr.
fn report_difference(case: &Value, got: &Outcome) -> Option<String> {
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
fn validate_matches_composer() {
    let cases: Vec<Value> = serde_json::from_str(CASES).unwrap();
    let mut failures = vec![];
    for case in &cases {
        let got = validate(case);
        let name = case["name"].as_str().unwrap();
        let instead = DIFFERENT.iter().find(|d| d.0 == name).map(|d| &d.1);
        let difference = if let Some(Instead::Report(exit, stdout, stderr)) = instead {
            ((Some(*exit), *stdout, *stderr) != (got.exit, got.stdout.as_str(), got.stderr.as_str())).then(|| {
                format!(
                    "Pun's own result, want exit {exit}\nstdout:\n{stdout}\nstderr:\n{stderr}\ngot exit {:?}\nstdout:\n{}\nstderr:\n{}",
                    got.exit, got.stdout, got.stderr
                )
            })
        } else if let Some(Instead::RunError(lines)) = instead {
            run_error_difference(lines, &got)
        } else if let Some(crash) = case.get("crash").and_then(Value::as_str) {
            Some(format!(
                "Composer crashes here ({crash}): add what Pun does instead to DIFFERENT"
            ))
        } else if let Some(message) = case.get("message").and_then(Value::as_array) {
            let lines: Vec<&str> = message.iter().map(|l| l.as_str().unwrap()).collect();
            run_error_difference(&lines, &got)
        } else {
            report_difference(case, &got)
        };
        if let Some(difference) = difference {
            failures.push(format!("=== {name}\n{difference}"));
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

/// A composer.json that exists but can't be read: Composer 2.11.2 prints "./composer.json is not readable." and exits 3.
/// (The fixture can't hold such a file.) Root reads it anyway, so the test then has nothing to check.
#[test]
fn unreadable_file() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("validate", "unreadable");
    let file = dir.join("composer.json");
    std::fs::write(&file, "{\"name\": \"acme/app\"}\n").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&file).is_err() {
        let out = common::clean_env(&mut Command::new(PUN), &["PATH"])
            .args(["composer", "validate"])
            .current_dir(&dir)
            .env("PUN_AGENT", "0")
            .output()
            .unwrap();
        assert_eq!(
            (
                out.status.code(),
                String::from_utf8_lossy(&out.stdout).into_owned(),
                String::from_utf8_lossy(&out.stderr).into_owned()
            ),
            (
                Some(3),
                String::new(),
                "./composer.json is not readable.\n".into()
            )
        );
    }
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}
