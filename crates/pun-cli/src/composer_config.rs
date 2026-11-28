//! `pun composer config`: Composer 2.11.2's `ConfigCommand`, its output, its edits and its errors included.

use crate::Exit;
use anyhow::{Context, Result, bail};
use pun_packages::composer_config::{Dirs, Options, run};
use std::path::Path;
use std::rc::Rc;

#[derive(clap::Args)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one bool per command-line flag"
)]
pub(crate) struct ConfigArgs {
    /// The setting to read or write, such as `process-timeout`, `allow-plugins.vendor/plugin` or `extra.key`.
    setting_key: Option<String>,
    /// The value(s) to write.
    setting_value: Vec<String>,
    /// Use the global config.json (in `COMPOSER_HOME`).
    #[arg(short = 'g', long)]
    global: bool,
    /// Open an editor (Pun doesn't).
    #[arg(short = 'e', long)]
    editor: bool,
    /// The auth file (only used with --editor).
    #[arg(short = 'a', long)]
    auth: bool,
    /// Remove the setting.
    #[arg(long)]
    unset: bool,
    /// List every setting.
    #[arg(short = 'l', long)]
    list: bool,
    /// Another composer.json or config.json.
    #[arg(short = 'f', long, value_name = "FILE")]
    file: Option<String>,
    /// Show `*-dir` settings as absolute paths.
    #[arg(long)]
    absolute: bool,
    /// Read the value as JSON (for extra.*, policy.*.ignore, audit.ignore and repositories).
    #[arg(short = 'j', long)]
    json: bool,
    /// Merge the JSON value with the current one.
    #[arg(short = 'm', long)]
    merge: bool,
    /// Add a repository last, not first.
    #[arg(long)]
    append: bool,
    /// Show where each value comes from.
    #[arg(long)]
    source: bool,
    /// Composer's option to skip plugins; this command runs none.
    #[arg(long)]
    no_plugins: bool,
}

/// `Factory::getDataDir`: `COMPOSER_HOME`, else the home dir, or XDG's data dir where Composer uses XDG.
fn data_dir(home: &Path) -> String {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if let Some(composer_home) = env("COMPOSER_HOME") {
        return composer_home;
    }
    let user = env("HOME").unwrap_or_default();
    let xdg = Path::new("/etc/xdg").is_dir()
        || std::env::vars_os().any(|(k, _)| k.to_string_lossy().starts_with("XDG_"));
    if home != Path::new(&format!("{user}/.composer")) && xdg {
        let data = env("XDG_DATA_HOME").unwrap_or_else(|| format!("{user}/.local/share"));
        return format!("{data}/composer");
    }
    home.to_string_lossy().into_owned()
}

/// `pun composer config`, in `-d`'s directory when it is given, as Composer changes into it first.
pub(crate) fn run_config(a: &ConfigArgs, globals: &crate::composer::Globals) -> Result<Exit> {
    let here = match &globals.working_dir {
        Some(d) => d
            .canonicalize()
            .with_context(|| format!("can't open {}", d.display()))?,
        None => std::env::current_dir()?,
    };
    let Some((home, cache)) = pun_packages::fetch::composer_dirs() else {
        bail!("can't find Composer's home: set COMPOSER_HOME or HOME");
    };
    let dirs = Dirs {
        data: data_dir(&home),
        home: home.to_string_lossy().into_owned(),
        cache: cache.to_string_lossy().into_owned(),
    };
    let options = Options {
        global: a.global,
        editor: a.editor,
        file: a.file.clone(),
        unset: a.unset,
        list: a.list,
        absolute: a.absolute,
        json: a.json,
        merge: a.merge,
        append: a.append,
        source: a.source,
    };
    let outcome = run(
        &here,
        &dirs,
        &(Rc::new(|k: &str| std::env::var(k).ok()) as pun_packages::composer_config::Env),
        options,
        a.setting_key.as_deref(),
        &a.setting_value,
    )?;
    print!("{}", outcome.stdout);
    eprint!("{}", outcome.stderr);
    Ok(Exit::Success)
}
