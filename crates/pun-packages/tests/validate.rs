//! `pun_packages::validate` against `composer validate` 2.11.2, on the cases of `pun-cli/tests/fixtures/validate.json`
//! (`scripts/export-validate-cases.py`). The report's layout is pun-cli's; here each case's findings must be Composer's
//! "- " lines, every one and nothing else, and each run error must come from the step that loads the root package or its
//! lock, with Composer's lines in order.

use pun_packages::validate::{Checks, Findings, load_root, lock_errors, read, validate};
use serde_json::Value;
use std::path::Path;

const CASES: &str = include_str!("../../pun-cli/tests/fixtures/validate.json");

/// The cases whose result pun-cli's tests/validate.rs gives instead of Composer's: Composer crashes on them, and `time`
/// is read only in its documented forms.
const DIFFERENT: [&str; 7] = [
    "time",
    "schema-require-number",
    "schema-root-number",
    "schema-root-string",
    "schema-license-number",
    "schema-license-mixed-array",
    "schema-archive",
];

fn write(dir: &Path, case: &Value) {
    for (path, content) in case["files"].as_object().unwrap() {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let bytes: Vec<u8> = match content.get("hex").and_then(Value::as_str) {
            Some(hex) => (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect(),
            None => content.as_str().unwrap().as_bytes().to_vec(),
        };
        std::fs::write(file, bytes).unwrap();
    }
}

/// A line of `outputResult` that heads a file's result rather than continuing a finding.
fn heading(line: &str) -> bool {
    line.ends_with(" is invalid, the following errors/warnings were found:")
        || line.ends_with(" is valid for simple usage with Composer but has")
        || line == "strict errors that make it unable to be published as a package"
        || line.ends_with(" is valid, but with a few warnings")
        || line.starts_with("See https://getcomposer.org/doc/04-schema.md")
        || line.contains(" is present but ignored as the \"lock\" config option is disabled.")
}

/// Composer's `outputResult` lines that are findings: each "- " line and the lines that continue it, without the lock's.
fn composer_findings(stderr: &str) -> Vec<String> {
    let mut findings: Vec<String> = vec![];
    let mut lock = false;
    for line in stderr.lines() {
        if let Some(section) = line.strip_prefix("# ") {
            lock = section.starts_with("Lock file");
        } else if heading(line) {
            lock = false;
        } else if lock {
        } else if let Some(finding) = line.strip_prefix("- ") {
            findings.push(finding.to_string());
        } else if let Some(last) = findings.last_mut() {
            last.push('\n');
            last.push_str(line);
        }
    }
    findings
}

/// The case's working directory (`-d`, relative to the project) and composer.json (its argument, else `COMPOSER`, else
/// ./composer.json).
fn options<'a>(case: &'a Value, args: &[&'a str]) -> (&'a str, &'a str) {
    let mut working_dir = ".";
    let mut file = case["env"]["COMPOSER"]
        .as_str()
        .unwrap_or("./composer.json");
    let mut words = args.iter();
    while let Some(word) = words.next() {
        match *word {
            "-d" | "--working-dir" => working_dir = words.next().copied().unwrap_or("."),
            w if !w.starts_with('-') => file = w,
            _ => {}
        }
    }
    (working_dir, file)
}

/// The project's directory, or its `-d` directory.
fn within(dir: &Path, working_dir: &str) -> std::path::PathBuf {
    if working_dir == "." {
        dir.to_path_buf()
    } else {
        dir.join(working_dir)
    }
}

/// The packages `--with-dependencies` checks: those of vendor/composer/installed.json but metapackages, with a
/// `vendor/<name>/composer.json`; `prefix` is the working directory's path in the case's files.
fn dependencies(case: &Value, prefix: &str) -> Vec<String> {
    let files = case["files"].as_object().unwrap();
    let Some(installed) = files
        .get(&format!("{prefix}vendor/composer/installed.json"))
        .and_then(Value::as_str)
    else {
        return vec![];
    };
    let installed: Value = serde_json::from_str(installed).unwrap();
    (installed["packages"].as_array().unwrap().iter())
        .filter(|p| p["type"] != "metapackage")
        .map(|p| p["name"].as_str().unwrap().to_string())
        .filter(|name| files.contains_key(&format!("{prefix}vendor/{name}/composer.json")))
        .collect()
}

/// What the library finds for the case: the root's findings, then each dependency's, as `--with-dependencies` checks
/// `vendor/<name>/composer.json`.
fn library_findings(dir: &Path, case: &Value, args: &[&str]) -> Vec<String> {
    let checks = Checks {
        constraints: !args.contains(&"--no-check-all"),
        version: !args.contains(&"--no-check-version"),
    };
    let (working_dir, file) = options(case, args);
    let dir = within(dir, working_dir);
    let prefix = if working_dir == "." {
        String::new()
    } else {
        format!("{working_dir}/")
    };
    let mut all = vec![];
    let mut add = |f: Findings, publish: bool| {
        all.extend(f.errors);
        if publish {
            all.extend(f.publish_errors);
        }
        all.extend(f.warnings);
    };
    add(
        validate(&dir.join(file), file, checks),
        !args.contains(&"--no-check-publish"),
    );
    if args.contains(&"-A") || args.contains(&"--with-dependencies") {
        // Composer checks a branch-aliased package once more for its alias: as often as it heads a result.
        let output = format!(
            "{}{}",
            case["stdout"].as_str().unwrap(),
            case["stderr"].as_str().unwrap()
        );
        for name in dependencies(case, &prefix) {
            let times = output
                .lines()
                .filter(|l| l.starts_with(&format!("{name} is ")))
                .count();
            let path = dir.join("vendor").join(&name).join("composer.json");
            for _ in 1..times {
                add(
                    validate(&path, &path.to_string_lossy(), checks),
                    !args.contains(&"--no-check-publish"),
                );
            }
            add(
                validate(&path, &path.to_string_lossy(), checks),
                !args.contains(&"--no-check-publish"),
            );
        }
    }
    all
}

/// The error of loading the root package and its lock, as `ValidateCommand` does after its checks.
fn load(dir: &Path, file: &str) -> Option<String> {
    let path = dir.join(file);
    let manifest = match read(&path, file).and_then(|m| load_root(&m).map(|()| m)) {
        Ok(manifest) => manifest,
        Err(e) => return Some(format!("{e:#}")),
    };
    // `Factory::getLockFile`: `.json` becomes `.lock`; any other name gets `.lock` added.
    let lock_display = file
        .strip_suffix(".json")
        .map_or_else(|| format!("{file}.lock"), |stem| format!("{stem}.lock"));
    let lock = dir.join(&lock_display);
    let locking = manifest.get("config").and_then(|c| c.get("lock")) != Some(&Value::Bool(false));
    if locking && lock.exists() {
        let result = lock_errors(
            &manifest,
            &std::fs::read(&path).unwrap(),
            &std::fs::read(&lock).unwrap(),
            &lock_display,
        );
        return result.err().map(|e| format!("{e:#}"));
    }
    None
}

#[test]
fn findings_and_run_errors_match_composer() {
    let cases: Vec<Value> = serde_json::from_str(CASES).unwrap();
    let mut failures = vec![];
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let args: Vec<&str> = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        // A missing file (exit 3) and a missing `-d` directory are pun-cli's to report.
        if DIFFERENT.contains(&name) || case["exit"] == 3 || name == "working-dir-missing" {
            continue;
        }
        let dir =
            std::env::temp_dir().join(format!("pun-validate-lib-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir, case);
        let (working_dir, file) = options(case, &args);
        let loaded = load(&within(&dir, working_dir), file);
        if let Some(message) = case.get("message").and_then(Value::as_array) {
            let got = loaded.unwrap_or_default();
            let in_order = (message.iter().filter_map(Value::as_str))
                .try_fold(0, |from, line| {
                    got[from..].find(line).map(|at| from + at + line.len())
                })
                .is_some();
            if !in_order {
                failures.push(format!(
                    "=== {name}\nwant the lines of\n{message:?}\ngot\n{got}"
                ));
            }
        } else {
            let want = composer_findings(
                &case["stderr"]
                    .as_str()
                    .unwrap()
                    .replace("{dir}", &dir.to_string_lossy()),
            );
            let got = library_findings(&dir, case, &args);
            if loaded.is_some() || want != got {
                failures.push(format!(
                    "=== {name}\nwant\n{want:#?}\ngot\n{got:#?}\nload error: {loaded:?}"
                ));
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
    assert!(
        failures.is_empty(),
        "{} cases differ:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
