//! `pun composer licenses`: Composer 2.11.2's `LicensesCommand`, its output and exit codes included.

use crate::Exit;
use crate::composer::Globals;
use crate::platform_reqs::{ComposerJson, composer_json};
use crate::render::{ColorChoice, Look};
use anyhow::Result;
use pun_packages::licenses::{Format, Options, report};
use pun_packages::platform_reqs::{Style, root_package};
use std::path::PathBuf;

#[derive(clap::Args)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one bool per command-line flag"
)]
pub(crate) struct LicensesArgs {
    /// The output: text, json or summary.
    #[arg(short = 'f', long, value_name = "FORMAT", default_value = "text")]
    format: String,
    /// Leave out the packages only require-dev needs.
    #[arg(long)]
    no_dev: bool,
    /// Show the packages of composer.lock, not those installed in the vendor dir.
    #[arg(long)]
    locked: bool,
    /// Colour the output (it wins over `--no-ansi`, as in Composer).
    #[arg(long)]
    ansi: bool,
    /// Don't colour the output.
    #[arg(long)]
    no_ansi: bool,
    /// Print nothing; the exit code still tells.
    #[arg(short = 'q', long)]
    quiet: bool,
    /// Composer's verbosity (-v, -vv, -vvv); Pun prints no debug output.
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    verbose: u8,
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

pub(crate) fn run(a: &LicensesArgs, globals: &Globals) -> Result<Exit> {
    let ComposerJson {
        base,
        manifest,
        lock_file,
    } = composer_json(globals)?;
    let root = root_package(&manifest, &base, &|name| std::env::var(name).ok());
    let vendor = pun_packages::install::dirs(&base.to_string_lossy(), &root.manifest).vendor;
    let report = report(
        &base.join(&lock_file),
        &format!("./{lock_file}"),
        &root,
        &PathBuf::from(vendor),
        Options {
            no_dev: a.no_dev,
            locked: a.locked,
            no_plugins: a.no_plugins,
        },
    )?;
    // Composer checks the format once it has read the packages.
    let format = Format::parse(&a.format)?;
    // `-q` prints nothing.
    if a.quiet {
        return Ok(Exit::Success);
    }
    let (out_color, err_color) = if a.ansi || a.no_ansi {
        (a.ansi, a.ansi)
    } else {
        (
            Look::stdout(ColorChoice::Auto).colored(),
            Look::stderr(ColorChoice::Auto).colored(),
        )
    };
    for warning in root.warning.iter().chain(&report.warnings) {
        eprintln!("{}", Style::Warning.paint(warning, err_color));
    }
    match format {
        Format::Text => print!("{}", report.text(out_color)),
        Format::Json => println!("{}", report.json()),
        Format::Summary => print!("{}", report.summary(out_color)),
    }
    Ok(Exit::Success)
}
