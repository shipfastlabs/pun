//! `pun composer check-platform-reqs`: Composer 2.11.2's `CheckPlatformReqsCommand`, its output and exit codes included.

use crate::Exit;
use crate::composer::Globals;
use crate::render::{ColorChoice, Look};
use anyhow::{Result, bail};
use pun_packages::platform_reqs::{Options, Report, Source, Style, check, root_versioned};
use pun_packages::resolver::platform::probe_cached;
use serde_json::{Map, Value};
use std::path::PathBuf;

#[derive(clap::Args)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one bool per command-line flag"
)]
pub(crate) struct PlatformReqsArgs {
    /// Leave out the requirements of require-dev and of the dev packages.
    #[arg(long)]
    no_dev: bool,
    /// Check the packages of composer.lock, not those installed in the vendor dir.
    #[arg(long)]
    lock: bool,
    /// The output: text or json (another value is text, as in Composer).
    #[arg(short = 'f', long, value_name = "FORMAT", default_value = "text")]
    format: String,
    /// Print nothing; the exit code still tells.
    #[arg(short = 'q', long)]
    quiet: bool,
    /// Composer's verbosity (-v, -vv, -vvv); Pun prints no debug output.
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    verbose: u8,
    /// Colour the output (it wins over `--no-ansi`, as in Composer).
    #[arg(long)]
    ansi: bool,
    /// Don't colour the output.
    #[arg(long)]
    no_ansi: bool,
    /// Load no plugins: the packages count where Composer's own installer puts them, in the vendor dir.
    #[arg(long)]
    no_plugins: bool,
    /// Composer's option to skip scripts; this command runs none.
    #[arg(long)]
    no_scripts: bool,
    /// Composer's option to skip its cache; this command uses none.
    #[arg(long)]
    no_cache: bool,
}

impl PlatformReqsArgs {
    /// Whether to colour what goes to stdout and to stderr: `--ansi`, else not with `--no-ansi`, else each a
    /// terminal's.
    fn colors(&self) -> (bool, bool) {
        if self.ansi || self.no_ansi {
            return (self.ansi, self.ansi);
        }
        (
            Look::stdout(ColorChoice::Auto).colored(),
            Look::stderr(ColorChoice::Auto).colored(),
        )
    }
}

/// The project a Composer command reads, as `Factory` finds it: its directory (`-d`, else the current one), its
/// composer.json (`COMPOSER`, else composer.json) and the lock named after it.
pub(crate) struct ComposerJson {
    pub(crate) base: PathBuf,
    pub(crate) manifest: Map<String, Value>,
    pub(crate) lock_file: String,
}

/// [`ComposerJson`], with Composer's errors for a missing directory and a composer.json that is missing, unreadable or
/// not an object.
pub(crate) fn composer_json(globals: &Globals) -> Result<ComposerJson> {
    // Composer changes to `-d` and works from `getcwd()`.
    let base = match &globals.working_dir {
        Some(d) => d.canonicalize().map_err(|_| {
            anyhow::anyhow!(
                "Invalid working directory specified, {} does not exist.",
                d.display()
            )
        })?,
        None => std::env::current_dir()?,
    };
    // `Factory::getComposerFile`: `COMPOSER`, else composer.json; the lock is named after it.
    let named = std::env::var("COMPOSER")
        .ok()
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty());
    let file = named.clone().unwrap_or_else(|| "composer.json".into());
    let path = base.join(&file);
    if !path.exists() {
        let missing = match &named {
            Some(file) => format!("Composer could not find the config file: {file}"),
            None => format!(
                "Composer could not find a composer.json file in {}",
                base.display()
            ),
        };
        bail!(
            "{missing}\nTo initialize a project, please create a composer.json file. See https://getcomposer.org/basic-usage"
        );
    }
    let Ok(text) = std::fs::read(&path) else {
        bail!("The file \"./{file}\" is not readable.");
    };
    // Composer adds jsonlint's description of a JSON error, and checks the rest against its schema (context/PLAN.md
    // §3). A value that isn't an object fails the schema's first check.
    let manifest: Map<String, Value> = match serde_json::from_slice(&text) {
        Ok(Value::Object(manifest)) => manifest,
        Ok(other) => bail!(
            "\"./{file}\" does not match the expected JSON schema:\n - {} value found, but an object is required",
            match other {
                Value::Null => "NULL",
                Value::Bool(_) => "Boolean",
                Value::Number(n) if n.is_i64() || n.is_u64() => "Integer",
                Value::Number(_) => "Double",
                Value::String(_) => "String",
                _ => "Array",
            }
        ),
        Err(_) => bail!("\"./{file}\" does not contain valid JSON"),
    };
    let lock_file = match file.strip_suffix(".json") {
        Some(stem) => format!("{stem}.lock"),
        None => format!("{file}.lock"),
    };
    Ok(ComposerJson {
        base,
        manifest,
        lock_file,
    })
}

pub(crate) fn run(a: &PlatformReqsArgs, globals: &Globals) -> Result<Exit> {
    let ComposerJson {
        base,
        manifest,
        lock_file,
    } = composer_json(globals)?;
    let (manifest, root_warning) =
        root_versioned(&manifest, &base, &|name| std::env::var(name).ok());
    let vendor = pun_packages::install::dirs(&base.to_string_lossy(), &manifest).vendor;
    // The real platform: Composer checks it with `config.platform` left out.
    let probe = probe_cached(
        &crate::install::php_binary(),
        pun_config::cache_dir().as_deref(),
    )?;
    let report = check(
        &base.join(&lock_file),
        &format!("./{lock_file}"),
        &manifest,
        &PathBuf::from(vendor),
        &probe,
        Options {
            no_dev: a.no_dev,
            lock: a.lock,
            no_plugins: a.no_plugins,
        },
    )?;
    // `-q` prints nothing, the JSON included.
    if !a.quiet {
        print(a, &report, root_warning.as_ref());
    }
    Ok(match report.exit {
        0 => Exit::Success,
        1 => Exit::Failure,
        _ => Exit::RunError,
    })
}

/// The warnings and the first line on stderr, then the table or the JSON on stdout.
fn print(a: &PlatformReqsArgs, report: &Report, root_warning: Option<&String>) {
    let (out_color, err_color) = a.colors();
    for warning in root_warning.into_iter().chain(&report.warnings) {
        eprintln!("{}", Style::Warning.paint(warning, err_color));
    }
    let non_dev = if a.no_dev { "non-dev " } else { "" };
    let first = match report.source {
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
    eprintln!("{}", first.0.paint(&first.1, err_color));
    if a.format == "json" {
        println!("{}", report.json());
    } else {
        print!("{}", report.text(out_color));
    }
}
