#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a command-line tool prints its reports"
)]

mod agent;
mod analyse;
mod analyse_report;
mod baseline;
mod cache;
mod composer;
mod console;
mod exec;
mod extensions;
pub mod finder;
mod fix_report;
mod github;
mod init;
mod install;
mod lint_plan;
mod list_files;
mod migrate;
mod php_diff;
mod pint_error;
mod prettier;
mod render;
mod report;
mod run;
#[cfg(feature = "pprof")]
mod sampler;
mod self_update;
mod update;
mod validate;
mod watch;
mod workspace;
mod x;

use anyhow::Result;
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use pun_config::Options;
use run::{LintOpts, Stage};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "pun",
    bin_name = "pun",
    version,
    about = "Pun: format, lint and analyse PHP"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
    /// Use output for coding agents
    #[arg(long, global = true)]
    agent: bool,
    /// Use output for people
    #[arg(long, global = true, conflicts_with = "agent")]
    no_agent: bool,
}

/// `pun --help`'s commands, in groups.
const GROUPS: [(&str, &[&str]); 3] = [
    ("Code", &["format", "lint", "analyse", "check"]),
    (
        "Packages",
        &["install", "update", "require", "remove", "x", "composer"],
    ),
    (
        "Project",
        &[
            "init",
            "migrate",
            "config",
            "list-files",
            "extension",
            "explain",
            "self-update",
        ],
    ),
];

/// `cmd` and its subcommands with "Show help" and "Show version" for clap's "Print help" and "Print version", and a
/// shorter summary for the `help` subcommand.
fn plain_flags(mut cmd: clap::Command) -> clap::Command {
    // `mut_args` keeps each argument in place: `mut_arg` moves it last, which breaks a built command's lookup of
    // its positionals (`pun self-update 1.x` gave `1.x` to `--check`). The action, not the id, names clap's own
    // flags: `self-update`'s `[VERSION]` has the id `version` too.
    cmd = cmd.mut_args(|a| match a.get_action() {
        clap::ArgAction::Help => a
            .help("Show help")
            .long_help("Show help (see a summary with '-h')"),
        clap::ArgAction::Version => a.help("Show version"),
        _ => a,
    });
    let names: Vec<String> = cmd
        .get_subcommands()
        .map(|c| c.get_name().to_owned())
        .collect();
    for name in names {
        cmd = if name == "help" {
            cmd.mut_subcommand(name, |c| c.about("Show help for a command"))
        } else {
            cmd.mut_subcommand(name, plain_flags)
        };
    }
    cmd
}

/// `pun` with a `--help` that lists the commands in [`GROUPS`], each with its one-line summary.
fn command() -> clap::Command {
    use std::fmt::Write as _;
    let mut cmd = Cli::command();
    cmd.build();
    let cmd = plain_flags(cmd);
    let (header, literal) = (
        *cmd.get_styles().get_header(),
        *cmd.get_styles().get_literal(),
    );
    let width = GROUPS
        .iter()
        .flat_map(|(_, names)| names.iter())
        .map(|n| n.len())
        .max()
        .unwrap_or(0);
    let mut list = String::new();
    for (group, names) in GROUPS {
        let _ = writeln!(list, "{header}{group}:{header:#}");
        for name in names {
            let about = cmd
                .find_subcommand(name)
                .and_then(clap::Command::get_about)
                .map_or_else(String::new, ToString::to_string);
            let _ = writeln!(list, "  {literal}{name:width$}{literal:#}  {about}");
        }
        list.push('\n');
    }
    debug_assert_eq!(
        GROUPS.iter().map(|(_, n)| n.len()).sum::<usize>(),
        cmd.get_subcommands()
            .filter(|c| c.get_name() != "help")
            .count(),
        "every command is in a group"
    );
    cmd.help_template(format!(
        "{{about-with-newline}}\n{{usage-heading}} {{usage}}\n\n{list}{header}Options:{header:#}\n{{options}}{{after-help}}"
    ))
}

#[derive(Subcommand)]
enum Cmd {
    /// Fix code style
    ///
    /// Formats the given paths, or the whole project. Use --test to check without changing files.
    Format(Box<FormatArgs>),
    /// Find code that could be written better
    ///
    /// Use --fix to apply the fixes.
    Lint(Box<LintCmd>),
    /// Run format, lint and analyse together
    ///
    /// Use --fix to apply the format and lint fixes.
    Check(Box<StageCmd>),
    /// Find bugs (never edits files)
    ///
    /// Also runs as `pun analyze`.
    #[command(visible_alias = "analyze")]
    Analyse(Box<analyse::AnalyseArgs>),
    /// Create pun.toml from Pint, PHP CS Fixer, Rector or PHPStan
    ///
    /// Reads pint.json, .php-cs-fixer.php, rector.php and phpstan.neon, and rewrites
    /// @php-cs-fixer-ignore comments.
    Migrate(migrate::MigrateArgs),
    /// Install dependencies from composer.lock
    ///
    /// Without a lock file, resolves one first.
    Install(install::InstallArgs),
    /// Update dependencies
    ///
    /// Updates every package, or only the ones named, then installs them.
    Update(update::UpdateArgs),
    /// Add a package
    ///
    /// Use --dev to add it as a dev dependency.
    Require(update::RequireArgs),
    /// Remove a package
    ///
    /// Also runs as `pun rm`.
    #[command(visible_alias = "rm")]
    Remove(update::RemoveArgs),
    /// Run a package's command without installing it
    ///
    /// Also runs as `punx`.
    X(x::XArgs),
    /// Run other Composer commands
    Composer(composer::ComposerArgs),
    /// Create pun.toml
    ///
    /// Asks for the code style, lint sets and analysis preset. Use --yes to take the defaults.
    Init(init::InitArgs),
    /// Update Pun
    ///
    /// Installs the latest release, or the version named.
    SelfUpdate(self_update::SelfUpdateArgs),
    /// Show the files Pun will check
    ListFiles(list_files::ListFilesArgs),
    /// Show the resolved config
    Config {
        /// Output format (default: toml).
        #[arg(long, value_enum)]
        format: Option<ConfigFormat>,
        /// The pun.toml to read (default: the one in the current directory).
        #[arg(long)]
        config: Option<PathBuf>,
        /// Resolve this built-in format preset without a pun.toml.
        #[arg(long)]
        preset: Option<String>,
    },
    /// List built-in extensions
    Extension {
        #[command(subcommand)]
        cmd: extensions::ExtensionCmd,
    },
    /// Explain a rule
    ///
    /// Shows its summary, group, sets, fix safety, upstream names and docs link.
    Explain {
        /// A rule ID (`format/arrays/syntax`), or an upstream or old name, which is answered with its Pun ID.
        rule: String,
    },
}

/// `pun lint` / `pun check`: `pun format`'s flags plus `--fix` and the lint levels.
#[derive(Args)]
struct StageCmd {
    #[command(flatten)]
    args: FormatArgs,
    /// Apply the fixes instead of reporting them.
    #[arg(long)]
    fix: bool,
    /// Turn a rule or group off (`-A lint/style`), after pun.toml; repeatable, applied in order with -W and -D.
    #[arg(short = 'A', long = "allow", value_name = "RULE")]
    allow: Vec<String>,
    /// Turn a rule or group on as a warning.
    #[arg(short = 'W', long = "warn", value_name = "RULE")]
    warn: Vec<String>,
    /// Turn a rule or group on as an error.
    #[arg(short = 'D', long = "deny", value_name = "RULE")]
    deny: Vec<String>,
    /// Exit 1 when there are warnings.
    #[arg(long)]
    deny_warnings: bool,
    /// Exit 1 when there are more than this many warnings.
    #[arg(long, value_name = "N")]
    max_warnings: Option<usize>,
    /// Print each problem on one line.
    #[arg(long)]
    concise: bool,
    /// `-A`, `-W` and `-D` in command-line order (filled in `main` from clap's indices).
    #[arg(skip)]
    levels: Vec<(lint_plan::CliLevel, String)>,
}

/// `pun lint`: the stage flags plus `--generate-baseline`.
#[derive(Args)]
struct LintCmd {
    #[command(flatten)]
    stage: StageCmd,
    /// Write every lint problem to the baseline (`[lint] baseline`, else pun-lint-baseline.toml) and exit 0.
    #[arg(long, value_name = "FILE", num_args = 0..=1, conflicts_with = "fix")]
    #[expect(
        clippy::option_option,
        reason = "clap: the flag is absent, bare, or given a file"
    )]
    generate_baseline: Option<Option<PathBuf>>,
}

#[derive(Clone, Copy, PartialEq, clap::ValueEnum)]
enum ConfigFormat {
    Toml,
    Json,
}

/// Pint's `DefaultCommand` flags, plus Pun's own.
#[derive(Args, Default, Clone)]
#[command(about = "Fix the coding style of the given path")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one bool per command-line flag"
)]
pub(crate) struct FormatArgs {
    /// The paths to run on (`-` reads stdin).
    paths: Vec<PathBuf>,
    /// The configuration that should be used.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Disable loading any configuration file.
    #[arg(long)]
    no_config: bool,
    /// The preset that should be used.
    #[arg(long)]
    preset: Option<String>,
    /// Test for code style errors without fixing them.
    #[arg(long, alias = "check")]
    test: bool,
    /// Fix code style errors but exit with status 1 if there were any changes made.
    #[arg(long)]
    repair: bool,
    /// Only fix files that have changed since branching off from the given branch.
    #[arg(long, value_name = "BRANCH")]
    diff: Option<String>,
    /// Only fix files that have uncommitted changes.
    #[arg(long)]
    dirty: bool,
    /// The output format that should be used (json, agent, checkstyle, junit, gitlab, github).
    #[arg(long)]
    format: Option<String>,
    /// Output the test results to a file at this path.
    #[arg(long)]
    output_to_file: Option<PathBuf>,
    /// The format that should be used when outputting the test results to a file.
    #[arg(long)]
    output_format: Option<String>,
    /// File path context for stdin input.
    #[arg(long)]
    stdin_filename: Option<PathBuf>,
    /// The number of threads (default: one per core).
    #[arg(long)]
    max_processes: Option<NonZeroUsize>,
    /// Print more detail: every finding under a coding agent, and the error class of a run error.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
    /// Print nothing; the exit code is the result.
    #[arg(short, long)]
    quiet: bool,
    #[expect(
        clippy::doc_markdown,
        reason = "this doc comment is the --help text, printed as written"
    )]
    /// Color the output: auto (on for a terminal, off when piped or with NO_COLOR), always or never.
    #[arg(long, value_enum, value_name = "WHEN")]
    color: Option<render::ColorChoice>,
    #[command(flatten)]
    pun: PunArgs,
    /// `GITHUB_ACTIONS=true`, read once in `main`: human output gets GitHub annotations after it.
    #[arg(skip)]
    github_actions: bool,
}

impl FormatArgs {
    fn color(&self) -> render::ColorChoice {
        self.color.unwrap_or(render::ColorChoice::Auto)
    }

    /// How a failed run of `pun format`, `lint` or `check` shows its error.
    fn failure(&self) -> report::Failure {
        let human = self.format.is_none() && !self.quiet && !agent::enabled();
        report::Failure {
            pint: true,
            machine: !human,
            github_format: self.format.as_deref().and_then(|f| f.parse().ok())
                == Some(report::ReportFormat::Github),
            annotate: human && self.github_actions,
            color: self.color(),
        }
    }
}

/// `pun format`'s flags that Pint doesn't have.
#[derive(Args, Default, Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one bool per command-line flag"
)]
pub(crate) struct PunArgs {
    /// Only fix files with staged changes (Pun).
    #[arg(long)]
    staged: bool,
    /// Print the fixers the run applies, in priority order (to stderr).
    #[arg(long)]
    explain: bool,
    /// Keep running and re-format files as they change (Pun).
    #[arg(long, conflicts_with = "stdin_filename")]
    watch: bool,
    /// Check every file, without reading or writing the result cache (Pun; also `PUN_NO_CACHE=1`).
    #[arg(long)]
    no_cache: bool,
}

/// What a run tells the shell. Pint's 0 and 1, and 2 when Pun couldn't run, where Pint exits 1.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub(crate) enum Exit {
    Success = 0,
    /// Problems found, or files changed under `--test` or `--repair`.
    Failure = 1,
    RunError = 2,
    /// `pun composer validate`: the composer.json is missing or unreadable (Composer's exit 3).
    Unreadable = 3,
    /// The lock doesn't meet composer.json's requirements (Composer's `Installer::ERROR_LOCK_FILE_INVALID`).
    LockInvalid = 4,
}

impl From<Exit> for ExitCode {
    fn from(exit: Exit) -> ExitCode {
        ExitCode::from(exit as u8)
    }
}

/// The stack of every thread that parses, lints or analyses: they recurse as deep as the syntax tree, and a long
/// chain of concatenations or `&&` overflows the 8 MiB of a main thread and Rust's 2 MiB default.
pub(crate) const STACK: usize = 36 * 1024 * 1024;

/// Runs `pun` on the process's arguments, on a thread with a stack deep enough for the parser, and returns its exit
/// code.
///
/// # Panics
///
/// When the thread can't start, or when the run panics outside a file's isolated fixing.
pub fn main() -> ExitCode {
    let run = std::thread::Builder::new()
        .name("main".into())
        .stack_size(STACK)
        .spawn(run)
        .expect("the main thread starts");
    run.join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

fn run() -> ExitCode {
    #[cfg(feature = "pprof")]
    let sampler = sampler::start();
    let mut args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if (args.first().map(std::path::Path::new))
        .and_then(std::path::Path::file_stem)
        .is_some_and(|n| n == "punx")
    {
        args.insert(1, "x".into());
    }
    let matches = command().get_matches_from(args);
    let mut cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    if let Some((_, sub)) = matches.subcommand() {
        match &mut cli.cmd {
            Cmd::Lint(cmd) => cmd.stage.levels = levels(sub),
            Cmd::Check(cmd) => cmd.levels = levels(sub),
            _ => {}
        }
    }
    let name = matches.subcommand_name().unwrap_or_default();
    let flag = (cli.agent || cli.no_agent).then_some(cli.agent);
    agent::init(flag, name);
    let github_actions = std::env::var_os("GITHUB_ACTIONS").is_some_and(|v| v == "true");
    let mut cmd = cli.cmd;
    let other = report::Failure {
        pint: false,
        machine: false,
        github_format: false,
        annotate: false,
        color: render::ColorChoice::Auto,
    };
    let (threads, failure) = match &mut cmd {
        Cmd::Format(a) => {
            a.github_actions = github_actions;
            (a.max_processes, a.failure())
        }
        Cmd::Lint(cmd) => {
            cmd.stage.args.github_actions = github_actions;
            (cmd.stage.args.max_processes, cmd.stage.args.failure())
        }
        Cmd::Check(cmd) => {
            cmd.args.github_actions = github_actions;
            (cmd.args.max_processes, cmd.args.failure())
        }
        Cmd::Analyse(a) => {
            a.github_actions = github_actions;
            (a.max_processes(), a.failure())
        }
        _ => (None, other),
    };
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(threads.map_or(0, NonZeroUsize::get))
        .stack_size(STACK)
        .build_global();
    let result = match cmd {
        Cmd::Format(a) if a.pun.watch => watch::run(*a),
        Cmd::Format(a) => run::stage(&a, Stage::Format, &LintOpts::default()),
        Cmd::Lint(cmd) => {
            lint_or_check(cmd.stage, cmd.generate_baseline, |fix| Stage::Lint { fix })
        }
        Cmd::Check(cmd) => lint_or_check(*cmd, None, |fix| Stage::Check { fix }),
        Cmd::Analyse(a) => analyse::run(&a),
        Cmd::Migrate(a) => migrate::run(&a),
        Cmd::Init(a) => init::run(a),
        Cmd::Install(a) => install::run(&a),
        Cmd::Update(a) => update::run(a),
        Cmd::Require(a) => update::require(a),
        Cmd::Remove(a) => update::remove(a),
        Cmd::X(a) => x::run(&a),
        Cmd::Composer(a) => composer::run(a),
        Cmd::SelfUpdate(a) => self_update::run(&a),
        Cmd::ListFiles(a) => list_files::run(a),
        Cmd::Config {
            format,
            config,
            preset,
            ..
        } => show_config(format, config, preset),
        Cmd::Extension { cmd } => extensions::run(cmd),
        Cmd::Explain { rule } => pun_config::explain(&rule).map(|t| {
            print!("{t}");
            Exit::Success
        }),
    };
    #[cfg(feature = "pprof")]
    if let Some(s) = sampler {
        s.report();
    }
    result.unwrap_or_else(|e| report::fail(e, &failure)).into()
}

/// `pun lint` / `pun check`: a `pun format` run over other rules; without `--fix` nothing is written.
#[expect(
    clippy::option_option,
    reason = "clap: `--generate-baseline` is absent, bare, or given a file"
)]
fn lint_or_check(
    cmd: StageCmd,
    generate_baseline: Option<Option<PathBuf>>,
    stage: fn(bool) -> Stage,
) -> Result<Exit> {
    let mut a = cmd.args;
    a.test |= !cmd.fix;
    let lint = LintOpts {
        levels: cmd.levels,
        deny_warnings: cmd.deny_warnings,
        max_warnings: cmd.max_warnings,
        concise: cmd.concise,
        generate_baseline,
    };
    run::stage(&a, stage(cmd.fix), &lint)
}

/// `-A`, `-W` and `-D` in the order they were given: later ones win.
fn levels(m: &clap::ArgMatches) -> Vec<(lint_plan::CliLevel, String)> {
    use lint_plan::CliLevel;
    let mut out: Vec<(usize, CliLevel, String)> = vec![];
    for (id, level) in [
        ("allow", CliLevel::Allow),
        ("warn", CliLevel::Warn),
        ("deny", CliLevel::Deny),
    ] {
        if let (Some(values), Some(indices)) = (m.get_many::<String>(id), m.indices_of(id)) {
            out.extend(values.zip(indices).map(|(v, i)| (i, level, v.clone())));
        }
    }
    out.sort_by_key(|l| l.0);
    out.into_iter().map(|(_, l, v)| (l, v)).collect()
}

/// `PUN_IGNORE_PHP_CONSTRAINT`: the harness runs corpora whose composer.json allows PHP below Pun's floor (8.3).
fn ignore_php_constraint() -> bool {
    std::env::var_os("PUN_IGNORE_PHP_CONSTRAINT").is_some()
}

fn root() -> Result<PathBuf> {
    Ok(std::env::current_dir()?.canonicalize()?)
}

fn show_config(
    fmt: Option<ConfigFormat>,
    config: Option<PathBuf>,
    preset: Option<String>,
) -> Result<Exit> {
    let root = root()?;
    let cfg = pun_config::load(
        &root,
        &Options {
            config,
            preset,
            ignore_php_constraint: ignore_php_constraint(),
            ..Default::default()
        },
    )?;
    let v = cfg.show();
    let sources: Vec<String> = (cfg.sources.iter())
        .map(|s| s.strip_prefix(&root).unwrap_or(s).display().to_string())
        .collect();
    let toml = if sources.is_empty() {
        toml::to_string(&v)?
    } else {
        format!(
            "# Resolved from {}.\n\n{}",
            sources.join(", "),
            toml::to_string(&v)?
        )
    };
    if fmt.is_none() && agent::enabled() {
        let detail = [
            format!("preset {}", cfg.preset),
            format!("PHP {}", cfg.php),
            agent::count(cfg.rules.len(), "format rule"),
            agent::count(cfg.lint.rules.len(), "lint rule"),
        ];
        let from = format!("from {}", sources.join(", "));
        eprint!("{}", agent::render("ok", &detail, &from, &[], false));
        print!("{toml}");
    } else if fmt == Some(ConfigFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        print!("{toml}");
    }
    Ok(Exit::Success)
}
