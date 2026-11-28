//! `pun composer validate`: Composer 2.11.2's `ValidateCommand`, its output and exit codes included.

use crate::Exit;
use anyhow::Result;
use pun_packages::validate::{Checks, Findings, load_root, lock_errors, read, validate};
use serde_json::Value;
use std::path::Path;

#[derive(clap::Args)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one bool per command-line flag"
)]
pub(crate) struct ValidateArgs {
    /// The composer.json to check (default: `COMPOSER`, else ./composer.json).
    file: Option<String>,
    /// Leave out the warnings for overly strict or loose version constraints.
    #[arg(long)]
    no_check_all: bool,
    /// Check that composer.lock is up to date, even when `config.lock` is false.
    #[arg(long)]
    check_lock: bool,
    /// Report a stale composer.lock as warnings, not errors.
    #[arg(long)]
    no_check_lock: bool,
    /// Leave out what only stops the package from being published.
    #[arg(long)]
    no_check_publish: bool,
    /// Leave out the warning for a `version` field.
    #[arg(long)]
    no_check_version: bool,
    /// Also check the composer.json of every installed package.
    #[arg(short = 'A', long)]
    with_dependencies: bool,
    /// Exit 1 for warnings too.
    #[arg(long)]
    strict: bool,
}

/// `pun composer validate`, in `-d`'s directory (one of `pun composer`'s global options) when it is given.
pub(crate) fn run(a: &ValidateArgs, globals: &crate::composer::Globals) -> Result<Exit> {
    // Composer changes to `-d` and works from `getcwd()`: the physical, absolute path a dependency's file is named by.
    let base = match &globals.working_dir {
        Some(d) => d.canonicalize().map_err(|_| {
            anyhow::anyhow!(
                "Invalid working directory specified, {} does not exist.",
                d.display()
            )
        })?,
        None => std::env::current_dir()?,
    };
    // `Factory::getComposerFile`.
    let file = (a.file.clone())
        .or_else(|| {
            std::env::var("COMPOSER")
                .ok()
                .map(|f| f.trim().to_string())
                .filter(|f| !f.is_empty())
        })
        .unwrap_or_else(|| "./composer.json".into());
    let path = base.join(&file);
    if !path.exists() {
        eprintln!("{file} not found.");
        return Ok(Exit::Unreadable);
    }
    if std::fs::File::open(&path).is_err() {
        eprintln!("{file} is not readable.");
        return Ok(Exit::Unreadable);
    }
    let checks = Checks {
        constraints: !a.no_check_all,
        version: !a.no_check_version,
    };
    let findings = validate(&path, &file, checks);

    // `createComposerInstance`: a composer.json that doesn't load stops the command.
    let manifest = read(&path, &file)?;
    load_root(&manifest)?;
    let locking = manifest.get("config").and_then(|c| c.get("lock")) != Some(&Value::Bool(false));
    let lock_file = lock_file(&file);
    let lock_path = base.join(&lock_file);
    let mut lock = vec![];
    if lock_path.exists() {
        if locking {
            lock = lock_errors(
                &manifest,
                &std::fs::read(&path)?,
                &std::fs::read(&lock_path)?,
                &lock_file,
            )?;
        } else {
            // Composer reads its config twice here, and says so each time.
            for _ in 0..2 {
                eprintln!(
                    "{lock_file} is present but ignored as the \"lock\" config option is disabled."
                );
            }
        }
    }
    let check_lock = (!a.no_check_lock && locking) || a.check_lock;
    let report = Report {
        name: &file,
        findings,
        check_publish: !a.no_check_publish,
        lock: Some((check_lock, lock)),
    };
    let mut exit = report.print(a.strict);

    if a.with_dependencies {
        let vendor = pun_packages::install::dirs(&base.to_string_lossy(), &manifest).vendor;
        for package in pun_packages::installed::read_installed(Path::new(&vendor))? {
            // The install path of a library: `vendor/<name>`, whatever installed.json says.
            let dir = Path::new(&vendor).join(package.pretty_name());
            let file = dir.join("composer.json");
            if package.kind() == Some("metapackage") || !dir.is_dir() || !file.exists() {
                continue;
            }
            // The local repository holds a branch-aliased package and its alias, both at this path: checked once each.
            let times = if pun_packages::version::branch_alias(&package.0).is_some() {
                2
            } else {
                1
            };
            for _ in 0..times {
                let report = Report {
                    name: package.pretty_name(),
                    findings: validate(&file, &file.to_string_lossy(), checks),
                    check_publish: !a.no_check_publish,
                    lock: None,
                };
                exit = exit.max(report.print(a.strict));
            }
        }
    }
    Ok(match exit {
        0 => Exit::Success,
        1 => Exit::Failure,
        _ => Exit::RunError,
    })
}

/// `Factory::getLockFile`: composer.json's name with `.lock` for `.json`, else with `.lock` added.
fn lock_file(file: &str) -> String {
    match file.strip_suffix(".json") {
        Some(stem) => format!("{stem}.lock"),
        None => format!("{file}.lock"),
    }
}

/// One file's result, as `ValidateCommand::outputResult` prints it.
struct Report<'a> {
    name: &'a str,
    findings: Findings,
    check_publish: bool,
    /// The root's lock errors, and whether they count as errors (`--check-lock`) or warnings; `None` for a
    /// dependency, which also gets no line for the schema's documentation.
    lock: Option<(bool, Vec<String>)>,
}

impl Report<'_> {
    /// Prints the result and returns its exit code: 2 for errors (publish and lock errors included when checked),
    /// else 1 for warnings under `--strict`, else 0.
    fn print(self, strict: bool) -> u8 {
        let Findings {
            errors,
            publish_errors,
            warnings,
        } = self.findings;
        let name = self.name;
        let (check_lock, lock) = self.lock.clone().unwrap_or_default();
        let publish = self.check_publish && !publish_errors.is_empty();
        let schema_url = self.lock.is_some();
        let mut show_schema_url = false;
        if !errors.is_empty() {
            eprintln!("{name} is invalid, the following errors/warnings were found:");
        } else if publish {
            eprintln!("{name} is valid for simple usage with Composer but has");
            eprintln!("strict errors that make it unable to be published as a package");
            show_schema_url = schema_url;
        } else if !warnings.is_empty() {
            eprintln!("{name} is valid, but with a few warnings");
            show_schema_url = schema_url;
        } else if !lock.is_empty() {
            let kind = if check_lock { "errors" } else { "warnings" };
            println!("{name} is valid but your composer.lock has some {kind}");
        } else {
            println!("{name} is valid");
        }
        if show_schema_url {
            eprintln!("See https://getcomposer.org/doc/04-schema.md for details on the schema");
        }

        let section = |title: &str, lines: &[String], out: &mut Vec<String>| {
            if !lines.is_empty() {
                out.push(title.to_string());
                out.extend(lines.iter().cloned());
            }
        };
        let dashed = |lines: &[String]| lines.iter().map(|l| format!("- {l}")).collect::<Vec<_>>();
        let mut shown_errors = vec![];
        section("# General errors", &dashed(&errors), &mut shown_errors);
        if publish {
            section(
                "# Publish errors",
                &dashed(&publish_errors),
                &mut shown_errors,
            );
        }
        let mut shown_warnings = vec![];
        section(
            "# General warnings",
            &dashed(&warnings),
            &mut shown_warnings,
        );
        if check_lock {
            section("# Lock file errors", &lock, &mut shown_errors);
        } else {
            section("# Lock file warnings", &lock, &mut shown_warnings);
        }
        for line in shown_errors.iter().chain(&shown_warnings) {
            eprintln!("{line}");
        }

        if shown_errors.is_empty() {
            u8::from(strict && !warnings.is_empty())
        } else {
            2
        }
    }
}
