//! `pun composer suggests`: Composer 2.11.2's `SuggestsCommand`, its output and exit codes included.

use crate::Exit;
use crate::composer::Globals;
use crate::platform_reqs::{ComposerJson, composer_json};
use crate::render::{ColorChoice, Look};
use anyhow::Result;
use pun_packages::platform_reqs::{Style, root_package};
use pun_packages::resolver::platform::probe_cached;
use pun_packages::suggests::{Options, suggests};
use std::path::PathBuf;

#[derive(clap::Args)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one bool per command-line flag"
)]
pub(crate) struct SuggestsArgs {
    /// The packages whose suggestions to show (all of them, transitive ones included); without, the root's and its
    /// dependencies'.
    #[arg(value_name = "PACKAGES")]
    packages: Vec<String>,
    /// Group by the package that suggests (the default).
    #[arg(long)]
    by_package: bool,
    /// Group by the suggested package.
    #[arg(long)]
    by_suggestion: bool,
    /// Show the suggestions of every dependency, transitive ones included.
    #[arg(short = 'a', long)]
    all: bool,
    /// Show only the names of the suggested packages.
    #[arg(long)]
    list: bool,
    /// Leave out the suggestions of the packages only require-dev needs (from composer.lock).
    #[arg(long)]
    no_dev: bool,
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

pub(crate) fn run(a: &SuggestsArgs, globals: &Globals) -> Result<Exit> {
    let ComposerJson {
        base,
        manifest,
        lock_file,
    } = composer_json(globals)?;
    let root = root_package(&manifest, &base, &|name| std::env::var(name).ok());
    let vendor = pun_packages::install::dirs(&base.to_string_lossy(), &root.manifest).vendor;
    // The real platform: Composer leaves out the suggestions it already has.
    let probe = probe_cached(
        &crate::install::php_binary(),
        pun_config::cache_dir().as_deref(),
    )?;
    let found = suggests(
        &base.join(&lock_file),
        &format!("./{lock_file}"),
        &root,
        &PathBuf::from(vendor),
        &probe,
        &a.packages,
        Options {
            by_package: a.by_package,
            by_suggestion: a.by_suggestion,
            all: a.all,
            list: a.list,
            no_dev: a.no_dev,
            no_plugins: a.no_plugins,
        },
    )?;
    // `-q` prints nothing, Composer's errors included; the exit code still tells.
    if a.quiet {
        return Ok(if found.errors.is_empty() {
            Exit::Success
        } else {
            Exit::Failure
        });
    }
    let (out_color, err_color) = if a.ansi || a.no_ansi {
        (a.ansi, a.ansi)
    } else {
        (
            Look::stdout(ColorChoice::Auto).colored(),
            Look::stderr(ColorChoice::Auto).colored(),
        )
    };
    for warning in root.warning.iter().chain(&found.warnings) {
        eprintln!("{}", Style::Warning.paint(warning, err_color));
    }
    if !found.errors.is_empty() {
        for error in &found.errors {
            eprintln!("{}", Style::Error.paint(error, err_color));
        }
        return Ok(Exit::Failure);
    }
    print!("{}", found.text(out_color));
    Ok(Exit::Success)
}
