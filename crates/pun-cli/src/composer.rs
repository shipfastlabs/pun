//! `pun composer <command>`: Composer's commands that have no top-level verb.

use crate::Exit;
use crate::install::{InstallArgs, Project, apcu, project, scan};
use crate::render::{ColorChoice, Look, Role};
use anyhow::{Result, bail};
use clap::Parser;
use pun_packages::fetch::composer_dirs;
use pun_packages::install::{Options, dump_autoload};
use pun_packages::plugin_host::Dispatcher;
use pun_packages::scripts::SCRIPT_EVENTS;
use pun_packages::version::IgnorePlatform;
use serde_json::Value;
use std::path::PathBuf;

#[derive(clap::Args)]
pub(crate) struct ComposerArgs {
    #[command(flatten)]
    globals: Globals,
    #[command(subcommand)]
    cmd: ComposerCmd,
}

/// Composer's global options that Pun takes, before or after the command's name.
#[derive(clap::Args)]
pub(crate) struct Globals {
    /// Run in this directory instead of the current one.
    #[arg(short = 'd', long, value_name = "DIR", global = true)]
    pub(crate) working_dir: Option<PathBuf>,
    /// Ask no questions: `run-script` without a script name is then an error, `exec` without a binary lists them.
    #[arg(short = 'n', long, global = true)]
    pub(crate) no_interaction: bool,
}

#[derive(clap::Subcommand)]
enum ComposerCmd {
    /// Write the autoloader again from vendor/composer/installed.json, as `composer dump-autoload` does.
    #[command(visible_alias = "dumpautoload")]
    DumpAutoload(DumpArgs),
    /// Run a script from composer.json, as `composer run-script` does; a script also runs as `pun composer <script>`.
    #[command(visible_alias = "run")]
    RunScript(RunArgs),
    /// Run a binary of the bin dir or of composer.json's `bin`, as `composer exec` does.
    Exec(crate::exec::ExecArgs),
    /// Check composer.json and composer.lock, as `composer validate` does.
    Validate(crate::validate::ValidateArgs),
    #[command(external_subcommand)]
    Other(Vec<String>),
}

#[derive(clap::Args)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one bool per command-line flag"
)]
struct DumpArgs {
    /// Put the PSR-0 and PSR-4 classes in the class map too.
    #[arg(short = 'o', long)]
    optimize: bool,
    /// Load classes from the class map only (implies --optimize).
    #[arg(short = 'a', long)]
    classmap_authoritative: bool,
    /// Cache found and missing classes in APCu.
    #[arg(long)]
    apcu: bool,
    /// The APCu cache prefix (implies --apcu); without it each dump picks a random one.
    #[arg(long, value_name = "PREFIX")]
    apcu_prefix: Option<String>,
    /// Use the autoload-dev rules (default: as the last install did).
    #[arg(long, conflicts_with = "no_dev")]
    dev: bool,
    /// Leave out the autoload-dev rules and the dev packages (default: as the last install did).
    #[arg(long)]
    no_dev: bool,
    /// Skip the root package's scripts.
    #[arg(long)]
    no_scripts: bool,
    /// Skip the Composer plugins.
    #[arg(long)]
    no_plugins: bool,
    /// Leave this platform requirement out of `platform_check.php`; repeatable.
    #[arg(long, value_name = "REQ")]
    ignore_platform_req: Vec<String>,
    /// Write no `platform_check.php`.
    #[arg(long)]
    ignore_platform_reqs: bool,
}

#[derive(clap::Args)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one bool per command-line flag"
)]
struct RunArgs {
    /// The script to run.
    script: Option<String>,
    /// Arguments passed on to the script; put them after `--` when they start with `-`.
    args: Vec<String>,
    /// Stop each command after this many seconds, or 0 for never (default: `config.process-timeout`, else 300).
    #[arg(long, value_name = "SECONDS")]
    timeout: Option<String>,
    /// Run in dev mode (the default; `COMPOSER_DEV_MODE=1`).
    #[arg(long)]
    dev: bool,
    /// Run without dev mode (`COMPOSER_DEV_MODE=0`).
    #[arg(long)]
    no_dev: bool,
    /// List the scripts.
    #[arg(short = 'l', long)]
    list: bool,
    /// Skip the Composer plugins.
    #[arg(long)]
    no_plugins: bool,
}

/// `pun composer <script>`: a script's own command, with the options Composer's `ScriptAliasCommand` and its global
/// options give it.
#[derive(clap::Parser)]
#[command(name = "pun composer")]
struct ScriptCommand {
    #[command(flatten)]
    globals: Globals,
    /// The script, by its name or an alias.
    script: String,
    /// Arguments passed on to the script.
    args: Vec<String>,
    /// Run in dev mode (the default).
    #[arg(long)]
    dev: bool,
    /// Run without dev mode.
    #[arg(long)]
    no_dev: bool,
    /// Skip the Composer plugins.
    #[arg(long)]
    no_plugins: bool,
}

impl ScriptCommand {
    /// `args` (the script's name first) as Composer reads them, which ignores the command's validation errors: the
    /// options up to the first one it doesn't know, which drops that one and every word after it, unless they follow
    /// `--`.
    fn read(args: &[String]) -> clap::error::Result<ScriptCommand> {
        use clap::error::{ContextKind, ContextValue, ErrorKind};
        let mut words: Vec<&str> = std::iter::once("pun composer")
            .chain(args.iter().map(String::as_str))
            .collect();
        loop {
            let error = match ScriptCommand::try_parse_from(&words) {
                Err(e) if e.kind() == ErrorKind::UnknownArgument => e,
                parsed => return parsed,
            };
            let Some(ContextValue::String(unknown)) = error.get(ContextKind::InvalidArg) else {
                return Err(error);
            };
            let end = words.iter().position(|w| *w == "--").unwrap_or(words.len());
            let at = words[..end].iter().position(|w| {
                *w == unknown.as_str()
                    || w.strip_prefix(unknown.as_str())
                        .is_some_and(|r| r.starts_with('='))
            });
            match at {
                Some(at) => words.truncate(at),
                None => return Err(error),
            }
        }
    }
}

pub(crate) fn run(a: ComposerArgs) -> Result<Exit> {
    let globals = a.globals;
    match a.cmd {
        ComposerCmd::DumpAutoload(d) => dump(&d, &globals),
        ComposerCmd::RunScript(r) => {
            let project = project(globals.working_dir.as_ref(), false)?;
            run_script(&project, &r, &globals, true)
        }
        ComposerCmd::Exec(e) => crate::exec::run(&e, &globals),
        ComposerCmd::Validate(v) => crate::validate::run(&v, &globals),
        ComposerCmd::Other(args) => {
            let name = args.first().map_or("", String::as_str);
            let parsed = ScriptCommand::read(&args);
            let dir = (parsed.as_ref().ok())
                .and_then(|c| c.globals.working_dir.clone())
                .or_else(|| globals.working_dir.clone());
            let found = project(dir.as_ref(), false)
                .ok()
                .and_then(|p| command_script(&p, name).map(|script| (p, script)));
            if let Some((project, script)) = found {
                let mut c = parsed.unwrap_or_else(|e| e.exit());
                c.globals.no_interaction |= globals.no_interaction;
                let run = RunArgs {
                    script: Some(script),
                    args: c.args,
                    timeout: None,
                    dev: c.dev,
                    no_dev: c.no_dev,
                    list: false,
                    no_plugins: c.no_plugins,
                };
                return run_script(&project, &run, &c.globals, false);
            }
            bail!("`pun composer {name}` is not built yet; Pun does not run the Composer CLI")
        }
    }
}

/// Composer 2.11.2's commands, by each name and alias (`Application::all()`), with their descriptions. A script with one
/// of these names is skipped (`A script named show would override a Composer command and has been skipped`), and
/// `run-script --list` shows the command's description beside it.
const COMPOSER_COMMANDS: [(&str, &str); 53] = [
    (
        "_complete",
        "Internal command to provide shell completion suggestions",
    ),
    ("about", "Shows a short information about Composer"),
    ("archive", "Creates an archive of this composer package"),
    (
        "audit",
        "Checks for security vulnerability advisories for installed packages",
    ),
    (
        "browse",
        "Opens the package's repository URL or homepage in your browser",
    ),
    (
        "bump",
        "Increases the lower limit of your composer.json requirements to the currently installed versions",
    ),
    ("cc", "Clears composer's internal package cache"),
    (
        "check-platform-reqs",
        "Check that platform requirements are satisfied",
    ),
    ("clear-cache", "Clears composer's internal package cache"),
    ("clearcache", "Clears composer's internal package cache"),
    ("completion", "Dump the shell completion script"),
    ("config", "Sets config options"),
    (
        "create-project",
        "Creates new project from a package into given directory",
    ),
    (
        "depends",
        "Shows which packages cause the given package to be installed",
    ),
    ("diagnose", "Diagnoses the system to identify common errors"),
    ("dump-autoload", "Dumps the autoloader"),
    ("dumpautoload", "Dumps the autoloader"),
    ("exec", "Executes a vendored binary/script"),
    (
        "fund",
        "Discover how to help fund the maintenance of your dependencies",
    ),
    (
        "global",
        "Allows running commands in the global composer dir ($COMPOSER_HOME)",
    ),
    ("help", "Display help for a command"),
    (
        "home",
        "Opens the package's repository URL or homepage in your browser",
    ),
    (
        "i",
        "Installs the project dependencies from the composer.lock file if present, or falls back on the composer.json",
    ),
    ("info", "Shows information about packages"),
    (
        "init",
        "Creates a basic composer.json file in current directory",
    ),
    (
        "install",
        "Installs the project dependencies from the composer.lock file if present, or falls back on the composer.json",
    ),
    (
        "licenses",
        "Shows information about licenses of dependencies",
    ),
    ("list", "List commands"),
    (
        "outdated",
        "Shows a list of installed packages that have updates available, including their latest version",
    ),
    (
        "policy",
        "Manages custom dependency policies and their sources",
    ),
    (
        "prohibits",
        "Shows which packages prevent the given package from being installed",
    ),
    (
        "r",
        "Adds required packages to your composer.json and installs them",
    ),
    (
        "reinstall",
        "Uninstalls and reinstalls the given package names",
    ),
    (
        "remove",
        "Removes a package from the require or require-dev",
    ),
    ("repo", "Manages repositories"),
    ("repository", "Manages repositories"),
    (
        "require",
        "Adds required packages to your composer.json and installs them",
    ),
    ("rm", "Removes a package from the require or require-dev"),
    ("run", "Runs the scripts defined in composer.json"),
    ("run-script", "Runs the scripts defined in composer.json"),
    ("search", "Searches for packages"),
    ("self-update", "Updates composer.phar to the latest version"),
    ("selfupdate", "Updates composer.phar to the latest version"),
    ("show", "Shows information about packages"),
    ("status", "Shows a list of locally modified packages"),
    ("suggests", "Shows package suggestions"),
    (
        "u",
        "Updates your dependencies to the latest version according to composer.json, and updates the composer.lock file",
    ),
    (
        "uninstall",
        "Removes a package from the require or require-dev",
    ),
    (
        "update",
        "Updates your dependencies to the latest version according to composer.json, and updates the composer.lock file",
    ),
    (
        "upgrade",
        "Updates your dependencies to the latest version according to composer.json, and updates the composer.lock file",
    ),
    ("validate", "Validates a composer.json and composer.lock"),
    (
        "why",
        "Shows which packages cause the given package to be installed",
    ),
    (
        "why-not",
        "Shows which packages prevent the given package from being installed",
    ),
];

/// The Composer command `name` names, by its name or an alias, as `Application::find` gives it for a script.
fn composer_command(name: &str) -> Option<&'static str> {
    (COMPOSER_COMMANDS.iter())
        .find(|(command, _)| *command == name)
        .map(|(_, description)| *description)
}

/// The script `name` runs as a command, by its own name or one of its `scripts-aliases`; Composer makes no command of
/// a script named after an event, and a Composer command or alias of that name wins over the script and its aliases.
fn command_script(project: &Project, name: &str) -> Option<String> {
    if composer_command(name).is_some() {
        return None;
    }
    let scripts = project.manifest.get("scripts")?.as_object()?;
    let aliases = project.manifest.get("scripts-aliases");
    scripts
        .keys()
        .filter(|s| !SCRIPT_EVENTS.contains(&s.as_str()))
        .find(|s| {
            *s == name
                || (aliases
                    .and_then(|a| a.get(s.as_str()))
                    .and_then(Value::as_array))
                .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(name)))
        })
        .cloned()
}

/// Runs `r.script`; `echo` prints each command first, as `run-script` does and a script's own command doesn't.
fn run_script(project: &Project, r: &RunArgs, globals: &Globals, echo: bool) -> Result<Exit> {
    if r.list {
        list_scripts(project);
        return Ok(Exit::Success);
    }
    let script = match &r.script {
        Some(s) => s.clone(),
        None if globals.no_interaction => bail!("Missing required argument \"script\""),
        None => match pick(project)? {
            Some(s) => s,
            None => bail!("Missing required argument \"script\""),
        },
    };
    let defined = (project.manifest.get("scripts").and_then(Value::as_object))
        .is_some_and(|s| s.contains_key(&script));
    if !defined {
        bail!("Script \"{script}\" is not defined in this package");
    }
    let timeout = match &r.timeout {
        // PHP's `ctype_digit`, then `(int)`, which stops at PHP_INT_MAX.
        Some(t) if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) => {
            Some(t.parse::<u64>().unwrap_or(i64::MAX.unsigned_abs()))
        }
        Some(_) => bail!("Timeout value must be numeric and positive if defined, or 0 for forever"),
        None => None,
    };
    let mut events = Dispatcher::new(
        &project.root,
        &project.manifest,
        r.dev || !r.no_dev,
        true,
        !r.no_plugins,
        composer_dirs().map(|(_, cache)| cache).as_deref(),
        &|k| std::env::var(k).ok(),
    );
    events.echo(echo);
    if let Some(seconds) = timeout {
        events.timeout(seconds);
    }
    finish(events.dispatch_script(&script, &r.args), false)
}

/// How a run of scripts ends: a script that failed prints its line, then each reference that led to it, and exits
/// with its code, as Composer does (with `color`, in its error style); any other error is a run error.
pub(crate) fn finish(result: pun_packages::error::Result<()>, color: bool) -> Result<Exit> {
    match result {
        Ok(()) => Ok(Exit::Success),
        Err(e) => match e.script_code() {
            Some(code) => {
                let mut lines = vec![e.to_string()];
                let mut cause = std::error::Error::source(&e);
                while let Some(c) = cause {
                    lines.push(c.to_string());
                    cause = c.source();
                }
                for line in lines.iter().rev() {
                    if color {
                        eprintln!("\x1b[37;41m{line}\x1b[39;49m");
                    } else {
                        eprintln!("{line}");
                    }
                }
                std::process::exit(code)
            }
            None => Err(e.into()),
        },
    }
}

/// Composer asks which script to run when none is named, in a terminal and with scripts to choose from; `None` when
/// it can't ask, or the question was cancelled.
fn pick(project: &Project) -> Result<Option<String>> {
    let rows = script_rows(project);
    let width = rows.iter().map(|r| r.0.chars().count()).max().unwrap_or(0);
    let items: Vec<String> = (rows.iter())
        .map(|(name, description)| {
            format!("{name:width$}  {description}")
                .trim_end()
                .to_string()
        })
        .collect();
    Ok(select("Script to run", &items)?.map(|i| rows[i].0.clone()))
}

/// Asks which of `items` to take, in a terminal and with items to choose from; `None` when it can't ask, or the
/// question was cancelled.
pub(crate) fn select(prompt: &str, items: &[String]) -> Result<Option<usize>> {
    use std::io::IsTerminal;
    if items.is_empty()
        || !std::io::stdin().is_terminal()
        || !std::io::stderr().is_terminal()
        || crate::agent::enabled()
    {
        return Ok(None);
    }
    Ok(dialoguer::Select::with_theme(&crate::init::theme())
        .with_prompt(prompt)
        .items(items)
        .default(0)
        .interact_on_opt(&dialoguer::console::Term::stderr())?)
}

/// Each script with the description Composer gives it: its `scripts-descriptions` entry, else the default one; a
/// script named after an event has none.
fn script_rows(project: &Project) -> Vec<(String, String)> {
    let Some(scripts) = project.manifest.get("scripts").and_then(Value::as_object) else {
        return vec![];
    };
    let descriptions = project.manifest.get("scripts-descriptions");
    (scripts.keys())
        .map(|name| {
            let description = if SCRIPT_EVENTS.contains(&name.as_str()) {
                String::new()
            } else if let Some(command) = composer_command(name) {
                command.to_string()
            } else if let Some(d) = (descriptions.and_then(|d| d.get(name))).and_then(Value::as_str)
            {
                d.to_string()
            } else {
                format!("Runs the {name} script as defined in composer.json")
            };
            (name.clone(), description)
        })
        .collect()
}

/// Composer's `run-script --list`: `scripts:` on stderr, then each script and its description in a table on stdout.
fn list_scripts(project: &Project) {
    let rows: Vec<(String, String)> = (script_rows(project).into_iter())
        .map(|(name, description)| (format!("  {name}"), description))
        .collect();
    if rows.is_empty() {
        return;
    }
    let width = |f: fn(&(String, String)) -> &String| {
        rows.iter().map(|r| f(r).chars().count()).max().unwrap_or(0)
    };
    let (names, descriptions) = (width(|r| &r.0), width(|r| &r.1));
    eprintln!("scripts:");
    for (name, description) in &rows {
        println!("{name:names$} {description:descriptions$} ");
    }
}

fn dump(d: &DumpArgs, globals: &Globals) -> Result<Exit> {
    let project = project(globals.working_dir.as_ref(), false)?;
    let options = Options {
        scan: scan(d.optimize, d.classmap_authoritative),
        apcu: apcu(d.apcu, d.apcu_prefix.as_ref()),
        no_plugins: d.no_plugins,
        ignore_platform: IgnorePlatform::new(d.ignore_platform_req.clone(), d.ignore_platform_reqs),
        cache: pun_config::cache_dir(),
        ..Options::default()
    };
    let dev = if d.dev {
        Some(true)
    } else if d.no_dev {
        Some(false)
    } else {
        None
    };
    let mut events = |event: &str, dev_mode: bool| {
        let a = InstallArgs {
            no_dev: !dev_mode,
            no_scripts: d.no_scripts,
            no_plugins: d.no_plugins,
            ..InstallArgs::default()
        };
        project.dispatch(&a, event)
    };
    let report = dump_autoload(
        &project.root,
        &project.manifest,
        project.lock.as_ref(),
        &options,
        dev,
        &mut events,
    )?;
    let look = Look::stderr(ColorChoice::Auto);
    let ok = look.paint(Role::Ok, look.glyph("✔", "+"));
    let warn = look.paint(Role::Warning, look.glyph("⚠", "!"));
    let noun = if report.classes == 1 {
        "class"
    } else {
        "classes"
    };
    eprintln!(" {ok} autoloader: {} {noun}", report.classes);
    for w in &report.warnings {
        eprintln!(" {warn} {w}");
    }
    if report.missing {
        eprintln!(
            " {warn} Not all dependencies are installed: run `pun install` to install the missing ones"
        );
        return Ok(Exit::Failure);
    }
    Ok(Exit::Success)
}
