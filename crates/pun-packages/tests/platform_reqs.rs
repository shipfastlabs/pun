//! `pun_packages::platform_reqs` against `composer check-platform-reqs` 2.11.2, on the cases and the platform of
//! `pun-cli/tests/fixtures/check_platform_reqs.json` (`scripts/export-platform-reqs-cases.py`): the table or JSON each
//! case prints, its exit code, where its packages came from, and the errors that stop it.

use pun_packages::platform_reqs::{Options, Report, Source, Style, check, root_versioned};
use serde_json::{Map, Value};
use std::path::Path;

const FIXTURE: &str = include_str!("../../pun-cli/tests/fixtures/check_platform_reqs.json");

/// The cases pun-cli's `tests/check_platform_reqs.rs` checks only in part, for the reasons it gives there: the first line
/// of a JSON error, a schema check to come, and the lock Composer crashes on.
const FIRST_LINE_ONLY: &[&str] = &[
    "installed-invalid-json",
    "lock-invalid-json",
    "global-quiet-error",
    "vendor-lock-invalid-json",
    "vendor-lock-invalid-json-no-plugins",
    "lock-installed-invalid-json",
];
const NEEDS_THE_SCHEMA: &[&str] = &["composer-json-schema"];
const AS_ANOTHER: &[(&str, &str)] = &[("lock-not-an-object", "lock-without-packages-dev")];

fn write(dir: &Path, case: &Value) {
    for (path, content) in case["files"].as_object().unwrap() {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content.as_str().unwrap()).unwrap();
    }
}

/// What a case asks for, from its arguments and `before`, Composer's global options before the command's name.
struct Asked {
    working_dir: String,
    options: Options,
    json: bool,
    color: bool,
    quiet: bool,
}

fn asked(case: &Value) -> Asked {
    let mut words = (case
        .get("before")
        .and_then(Value::as_array)
        .into_iter()
        .flatten())
    .chain(case["args"].as_array().unwrap())
    .map(|a| a.as_str().unwrap());
    let mut asked = Asked {
        working_dir: ".".to_string(),
        options: Options::default(),
        json: false,
        color: false,
        quiet: false,
    };
    while let Some(word) = words.next() {
        match word {
            "-d" => asked.working_dir = words.next().unwrap().to_string(),
            "--format" | "-f" => asked.json = words.next() == Some("json"),
            w if w.starts_with("--format=") => asked.json = w == "--format=json",
            "--no-dev" => asked.options.no_dev = true,
            "--lock" => asked.options.lock = true,
            "--no-plugins" => asked.options.no_plugins = true,
            "--ansi" => asked.color = true,
            "-q" | "--quiet" => asked.quiet = true,
            _ => {}
        }
    }
    asked
}

/// What Composer prints on stderr before the table: the warnings, then the line that says where the packages came
/// from, in its styles with `color`.
fn stderr(report: &Report, root_warning: Option<&String>, no_dev: bool, color: bool) -> String {
    let non_dev = if no_dev { "non-dev " } else { "" };
    let (style, first) = match report.source {
        Source::Lock => (
            Style::Info,
            format!("Checking {non_dev}platform requirements using the lock file"),
        ),
        Source::Vendor => (
            Style::Info,
            format!("Checking {non_dev}platform requirements for packages in the vendor dir"),
        ),
        Source::NoVendor => (
            Style::Warning,
            format!(
                "No vendor dir present, checking {non_dev}platform requirements from the lock file"
            ),
        ),
    };
    let mut out = String::new();
    for warning in root_warning.into_iter().chain(&report.warnings) {
        out += &Style::Warning.paint(warning, color);
        out.push('\n');
    }
    out + &style.paint(&first, color) + "\n"
}

/// The root package `manifest` with its version and Composer's warning, in the recorder's environment:
/// `COMPOSER_ROOT_VERSION=1.0.0`, unless the case leaves it out or sets its own.
fn versioned(
    case: &Value,
    manifest: &Map<String, Value>,
    base: &Path,
) -> (Map<String, Value>, Option<String>) {
    let root_version = match case["env"].get("COMPOSER_ROOT_VERSION") {
        Some(Value::String(v)) => Some(v.clone()),
        Some(_) => None,
        None => Some("1.0.0".to_string()),
    };
    root_versioned(manifest, base, &|name| {
        (name == "COMPOSER_ROOT_VERSION")
            .then(|| root_version.clone())
            .flatten()
    })
}

/// How Pun's report for `case` differs from Composer's, if it does.
fn difference(fixture: &Value, case: &Value) -> Option<String> {
    let name = case["name"].as_str().unwrap();
    let like = AS_ANOTHER
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, other)| {
            (fixture["cases"].as_array().unwrap().iter())
                .find(|c| c["name"] == *other)
                .expect("the case it stands for")
        });
    let asked = asked(case);
    let dir = std::env::temp_dir().join(format!("pun-platform-lib-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    write(&dir, case);
    // The project's directory, or its `-d` directory (no `/./` in the paths it names).
    let base = if asked.working_dir == "." {
        dir.clone()
    } else {
        dir.join(&asked.working_dir)
    };
    let file = case["env"]["COMPOSER"].as_str().unwrap_or("composer.json");
    let lock_file = file
        .strip_suffix(".json")
        .map_or_else(|| format!("{file}.lock"), |stem| format!("{stem}.lock"));
    // A missing or unparsable composer.json is pun-cli's to report.
    let Some(manifest) = std::fs::read(base.join(file))
        .ok()
        .and_then(|text| serde_json::from_slice::<Map<String, Value>>(&text).ok())
    else {
        std::fs::remove_dir_all(&dir).unwrap();
        return None;
    };
    let (manifest, warning) = versioned(case, &manifest, &base);
    let vendor = (manifest
        .get("config")
        .and_then(|c| c.get("vendor-dir"))
        .and_then(Value::as_str))
    .unwrap_or("vendor")
    .to_string();
    let result = check(
        &base.join(&lock_file),
        &format!("./{lock_file}"),
        &manifest,
        &base.join(vendor),
        &fixture["probe"],
        asked.options,
    );
    // The fixture writes the project's directory as `{dir}`.
    let here = dir.canonicalize().unwrap().to_string_lossy().into_owned();
    std::fs::remove_dir_all(&dir).unwrap();
    match (
        result,
        like.unwrap_or(case)
            .get("message")
            .and_then(Value::as_array),
    ) {
        (Err(e), Some(message)) => {
            let got = format!("{e:#}")
                .replace(&here, "{dir}")
                .replace(&*dir.to_string_lossy(), "{dir}");
            let mut lines: Vec<&str> = message.iter().filter_map(Value::as_str).collect();
            if FIRST_LINE_ONLY.contains(&name) {
                lines.truncate(1);
            }
            (!lines.iter().all(|l| got.contains(l)))
                .then(|| format!("want the lines {lines:?}\ngot {got}"))
        }
        (Ok(report), None) => {
            let stdout = if asked.json {
                format!("{}\n", report.json())
            } else {
                report.text(asked.color)
            };
            let stderr = stderr(&report, warning.as_ref(), asked.options.no_dev, asked.color);
            // `-q` prints nothing; Pun's CLI leaves both out.
            let got = if asked.quiet {
                (i64::from(report.exit), String::new(), String::new())
            } else {
                (i64::from(report.exit), stdout, stderr)
            };
            let want = (
                case["exit"].as_i64().unwrap(),
                case["stdout"].as_str().unwrap().to_string(),
                case["stderr"].as_str().unwrap().to_string(),
            );
            (got != want).then(|| format!("want {want:#?}\ngot {got:#?}"))
        }
        (Err(e), None) => Some(format!("an error Composer doesn't give: {e:#}")),
        (Ok(_), Some(message)) => Some(format!("no error, where Composer gives {message:?}")),
    }
}

#[test]
fn reports_as_composer_does() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let failures: Vec<String> = (fixture["cases"].as_array().unwrap().iter())
        .filter(|case| !NEEDS_THE_SCHEMA.contains(&case["name"].as_str().unwrap()))
        .filter_map(|case| {
            difference(&fixture, case)
                .map(|d| format!("=== {}\n{d}", case["name"].as_str().unwrap()))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} cases differ:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// In a git repository, without `version` or `COMPOSER_ROOT_VERSION`, the root's version is git's: on `main`,
/// `dev-main`, as Composer's `VersionGuesser` gives it, without the warning about 1.0.0.
#[test]
fn root_version_from_git() {
    let dir = std::env::temp_dir().join(format!("pun-platform-git-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["commit", "-q", "--allow-empty", "-m", "one"],
    ] {
        let status = std::process::Command::new("git")
            .args([
                "-c",
                "user.email=a@b",
                "-c",
                "user.name=a",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(&dir)
            .env_remove("GIT_DIR")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }
    let manifest = serde_json::json!({"name": "acme/app"})
        .as_object()
        .unwrap()
        .clone();
    let (versioned, warning) = root_versioned(&manifest, &dir, &|_| None);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(versioned["version"], "dev-main");
    assert_eq!(warning, None);
}
