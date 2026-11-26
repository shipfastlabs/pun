use crate::finder::{Command, Exclusions};
use crate::render::{Look, Renderer, Summary};
use crate::report::Abort;
use crate::{Exit, agent, analyse_report, baseline, github, render, root};
use anyhow::Result;
use clap::Args;
use pun_config::rule_of_identifier;
use pun_config::{Config, Options};
use pun_lint::problem::ignore::Ignores;
use pun_lint::problem::{Level, LineIndex, Problem, Span};
use pun_phpstan::php::normalize_path;
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Args, Default, Clone)]
pub(crate) struct AnalyseArgs {
    /// The files or directories to analyse (default: `[analyse] paths`, else `[files] paths`, else the project).
    paths: Vec<PathBuf>,
    /// The configuration that should be used.
    #[arg(long)]
    config: Option<PathBuf>,
    /// PHPStan's error formats: raw, json, checkstyle, junit, gitlab, github, teamcity.
    #[arg(long, alias = "error-format", value_name = "FORMAT")]
    format: Option<String>,
    /// Write every error to the baseline (`[analyse] baseline`, else pun-baseline.toml) and exit 0.
    #[arg(long, value_name = "FILE", num_args = 0..=1)]
    #[expect(
        clippy::option_option,
        reason = "clap: the flag is absent, bare, or given a file"
    )]
    generate_baseline: Option<Option<PathBuf>>,
    /// The number of threads (default: one per core).
    #[arg(long)]
    max_processes: Option<std::num::NonZeroUsize>,
    /// Print each finding on one line.
    #[arg(long)]
    concise: bool,
    /// `raw`: add each error's identifier, as PHPStan's `-v` does.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
    /// Print nothing; the exit code is the result.
    #[arg(short, long)]
    quiet: bool,
    /// Color the output: auto, always or never.
    #[arg(long, value_enum, value_name = "WHEN")]
    color: Option<render::ColorChoice>,
    /// `GITHUB_ACTIONS=true`: human output gets GitHub annotations after it.
    #[arg(skip)]
    pub(crate) github_actions: bool,
}

impl AnalyseArgs {
    pub(crate) fn max_processes(&self) -> Option<std::num::NonZeroUsize> {
        self.max_processes
    }

    pub(crate) fn failure(&self) -> crate::report::Failure {
        let human = self.format.is_none() && !self.quiet && !agent::enabled();
        crate::report::Failure {
            pint: false,
            machine: !human,
            github_format: self.format.as_deref() == Some("github"),
            annotate: human && self.github_actions,
            color: self.color.unwrap_or(render::ColorChoice::Auto),
        }
    }
}

/// One error, as PHPStan reports it, with the Pun rule its identifier belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Finding {
    /// Absolute, as PHPStan's `json` and `raw` print it.
    pub(crate) file: PathBuf,
    /// Relative to the project root (PHPStan's simple relative path, the cwd being the root).
    pub(crate) rel: String,
    pub(crate) line: u32,
    pub(crate) identifier: String,
    pub(crate) rule: String,
    pub(crate) message: String,
    pub(crate) tip: Option<String>,
    pub(crate) can_be_ignored: bool,
    /// PHPStan's `isFixable()`.
    pub(crate) fixable: bool,
    /// The file that holds the error: `file` without the ` (in context of class …)` of an error in a trait. Kept
    /// apart from the label because a file's own name may hold that text.
    pub(crate) source: PathBuf,
    /// `source` relative to the project root.
    pub(crate) source_rel: String,
}

impl Finding {
    /// PHPStan's `canBeIgnored()`, preserved from the rule that reported the error.
    pub(crate) fn ignorable(&self) -> bool {
        self.can_be_ignored
    }

    pub(crate) fn source(&self) -> &Path {
        &self.source
    }

    pub(crate) fn source_rel(&self) -> &str {
        &self.source_rel
    }

    /// PHPStan's `getLine()`: none (0 here) for an `ignoreErrors` entry nothing matched.
    pub(crate) fn line(&self) -> Option<u32> {
        (self.line > 0).then_some(self.line)
    }
}

/// What an analysis found.
pub(crate) struct Analysed {
    pub(crate) findings: Vec<Finding>,
    /// PHPStan's not-file-specific errors: `[[analyse.ignore]]` entries without a file path nothing matched.
    pub(crate) general: Vec<String>,
    /// The files analysed.
    pub(crate) files: usize,
    /// Baseline entries (counts) no error matched any more.
    pub(crate) unmatched_baseline: usize,
    /// `@pun-ignore` comments without a rule ID, as problems (`ignore-comment`).
    pub(crate) malformed: Vec<Problem>,
}

/// The syntax errors the engine did not report. It translates few of them into PHPParser's wording, and would
/// otherwise analyse the recovered tree and pass the file. These carry PHP's own message, from the grammar
/// `pun format` lints with, under PHPStan's identifier.
fn syntax_errors(files: &[PathBuf], reported: &[pun_analyse::Error]) -> Vec<pun_analyse::Error> {
    let reported: std::collections::HashSet<&str> = (reported.iter())
        .filter(|e| e.identifier == "phpstan.parse")
        .map(|e| e.file.as_str())
        .collect();
    (files.par_iter())
        .filter(|file| !reported.contains(&*file.to_string_lossy()))
        .filter_map(|file| {
            let err = pun_format::lint(&std::fs::read(file).ok()?).err()?;
            let message = String::from_utf8_lossy(&err.message);
            let message = message.strip_prefix("Parse error: ")?.trim_end_matches('.');
            let message = match message.strip_prefix("syntax error") {
                Some(rest) => format!("Syntax error{rest}"),
                None => message.to_string(),
            };
            Some(pun_analyse::Error {
                file: file.to_string_lossy().into_owned(),
                line: i64::from(err.line),
                identifier: "phpstan.parse".into(),
                message,
                tip: None,
                can_be_ignored: false,
                fixable: false,
            })
        })
        .collect()
}

/// The engine's rule for PHPStan's errors about ignores (`ignore.unmatched`, `ignore.count`, ...), which no rule
/// reports.
const UNMATCHED_IGNORE: &str = "unmatched-ignore";

fn run_error(message: String, hint: Option<String>) -> anyhow::Error {
    Abort::plain(message, hint).into()
}

/// The project pun-analyse works on: the analysed files, Composer's autoloaded files for symbols (classmap
/// and `files` entries minus the excludes), the target PHP version, and `[analyse]` as PHPStan's parameters (the level, `stubFiles`,
/// `ignoreErrors`, Larastan, rule options). Only the enabled rules' identifiers are reported.
fn project(
    cfg: &Config,
    analysed: Vec<PathBuf>,
    analysed_paths: &[PathBuf],
) -> Result<pun_analyse::Project> {
    use pun_phpstan::ignore::IgnoredErrors;
    use pun_phpstan::{Array, Key, Value};
    // An identifier a lint rule owns (O5) is still reported here when a rule class that reports it runs: the lint
    // rules see no types, so they miss most of what PHPStan reports, and an error must show in lint or analyse.
    let lint_on: std::collections::HashSet<String> = (cfg.lint.rules.iter().map(|r| &r.id))
        .chain(
            (cfg.lint.overrides.iter().flat_map(|o| &o.rules))
                .filter(|s| s.level.is_some())
                .map(|s| &s.id),
        )
        .map(|id| format!("lint/{}", id.strip_prefix("lint/").unwrap_or(id)))
        .collect();
    let mut params = cfg
        .analyse
        .parameters_with_lint(&cfg.root, |r| lint_on.contains(r));
    pun_phpstan::config::validate_ignore_errors(&params)
        .map_err(|e| run_error(e.to_string(), None))?;
    let mut extensions = Array::new();
    extensions.push(Value::Str("php".into()));
    params.set(Key::Str("fileExtensions".into()), Value::Array(extensions));
    params.set(
        Key::Str("phpVersion".into()),
        Value::Int(i64::from(cfg.analyse_php_version.id())),
    );
    let mut project = pun_analyse::Project::from_config(&cfg.root, &params);
    // PHPStan's analysedPathsFromConfig: the configured paths. Their files that are not analysed still declare
    // symbols, as PHPStan's source locator finds them, and decide with the analysed paths which unmatched entries an
    // analysis of only some paths reports.
    let from_config: Vec<String> = (cfg.analyse.paths.iter())
        .map(|p| normalize_path(&cfg.root.join(p).to_string_lossy()))
        .collect();
    let analysed_paths: Vec<String> = analysed_paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if !from_config.is_empty() && analysed_paths != from_config {
        let analysing: std::collections::HashSet<&PathBuf> = analysed.iter().collect();
        let configured = crate::finder::files(cfg, Some(Command::Analyse), &[])?;
        (project.scanned).extend(configured.into_iter().filter(|f| !analysing.contains(f)));
    }
    let excluded = Exclusions::new(cfg, Command::Analyse)?;
    // PHPStan's PSR-4 and PSR-0 locators ignore excludePaths: an excluded class is still found by its name.
    project.scanned.retain(|p| !excluded.is_excluded(p));
    project.analysed = analysed;
    project.only_files = analysed_paths.iter().all(|p| Path::new(p).is_file());
    project.relative_path = pun_analyse::RelativePath::of(&analysed_paths);
    let ignores = (params.get_str("ignoreErrors").and_then(Value::as_array)).cloned();
    let ignores = ignores.unwrap_or_default();
    let pun_toml = cfg.pun_toml();
    let analyse_excluded: pun_phpstan::ignore::Excluded =
        std::sync::Arc::new(move |f: &str| excluded.is_excluded(Path::new(f)));
    let mut ignore_errors = IgnoredErrors::new(&ignores, cfg.analyse.report_unmatched)
        .with_rules(rule_of_identifier)
        .with_analysed_paths(&analysed_paths, &from_config, Some(analyse_excluded));
    if let Some(file) = pun_toml {
        ignore_errors = ignore_errors.with_config_entries(
            &file.to_string_lossy(),
            &ignores,
            &cfg.root.to_string_lossy(),
        );
    }
    project.ignore_errors = ignore_errors;
    let on: std::collections::HashSet<String> =
        cfg.analyse.rules.iter().map(|r| r.id.clone()).collect();
    let method_rules = pun_analyse::MethodRules {
        call_methods: on.contains("analyse/undefined/method"),
        overriding_method: on.contains("analyse/classes/incompatible-override"),
        consistent_constructor: on.contains("analyse/classes/inconsistent-constructor"),
        renamed_named_arguments: on.contains("analyse/types/renamed-parameter-in-subtype"),
    };
    project.method_rules = method_rules;
    project
        .load_phpat()
        .map_err(|e| run_error(format!("[extensions] phpat: {e}"), None))?;
    let routed = pun_config::identifiers_of_rules(|r| on.contains(r) || lint_on.contains(r));
    project.reported = Some(std::sync::Arc::new(move |id| {
        if method_rules.consistent_constructor
            && matches!(
                id,
                "method.childParameterType"
                    | "method.visibility"
                    | "parameter.byRef"
                    | "parameter.missing"
                    | "parameter.notByRef"
                    | "parameter.notOptional"
                    | "parameter.notVariadic"
                    | "parameter.variadic"
            )
        {
            return true;
        }
        rule_of_identifier(id).map_or_else(
            || id == "phpstan.parse" || id.starts_with("ignore."),
            |r| on.contains(r) || (r.starts_with("lint/") && routed.contains(id)),
        )
    }));
    Ok(project)
}

/// Analyse `paths` (or the configured paths) under `cfg`: enabled rules only, `@pun-ignore` comments and the
/// baseline applied (unless `baseline` is false, for `--generate-baseline`).
pub(crate) fn analyse(cfg: &Config, paths: &[PathBuf], use_baseline: bool) -> Result<Analysed> {
    // mimalloc returns freed pages to the OS after 100 ms instead of 1 s: the parse trees and scratch of one phase
    // are gone before the next one peaks, which takes 5-7% off the peak RSS of laravel-pint and larastan. A purge
    // already scheduled keeps its 1 s deadline, so the collection runs those now.
    // SAFETY: mi_option_set only stores the value, and mi_collect is safe to call at any time. 15 is
    // mi_option_purge_delay in mimalloc 3's mi_option_e, which libmimalloc-sys has no constant for.
    unsafe {
        libmimalloc_sys::mi_option_set(15, 100);
        libmimalloc_sys::mi_collect(true);
    }
    let files = crate::finder::files(cfg, Some(Command::Analyse), paths)?;
    let count = files.len();
    let configured: Vec<PathBuf> = if paths.is_empty() {
        cfg.analyse.paths.iter().map(PathBuf::from).collect()
    } else {
        paths.to_vec()
    };
    let mut paths: Vec<PathBuf> = (configured.iter())
        .map(|p| PathBuf::from(normalize_path(&cfg.root.join(p).to_string_lossy())))
        .collect();
    if paths.is_empty() {
        paths.push(cfg.root.clone());
    }
    let project = project(cfg, files, &paths)?;
    let analysis =
        pun_analyse::analyse_project(&project).map_err(|e| run_error(e.to_string(), None))?;
    let mut errors = analysis.errors;
    errors.extend(syntax_errors(&project.analysed, &errors));
    let mut general = analysis.general;
    let severe = errors.iter().any(|e| e.identifier == "phpstan.parse");
    if severe {
        let mut seen = std::collections::HashSet::new();
        errors.retain(|e| e.identifier == "phpstan.parse" && seen.insert(e.message.clone()));
        general.clear();
    }
    let analysed_files: std::collections::HashSet<String> = (project.analysed.iter())
        .map(|p| normalize_path(&p.to_string_lossy()))
        .collect();
    let mut findings: Vec<Finding> = (errors.into_iter())
        .map(|e| {
            let rule =
                rule_of_identifier(&e.identifier).unwrap_or(if e.identifier == "phpstan.parse" {
                    "parser-error"
                } else {
                    UNMATCHED_IGNORE
                });
            // An analysed file's name may itself hold ` (in context of `: only another label gets split.
            let whole = normalize_path(&e.file);
            let (source, file) = match e.file.rsplit_once(" (in context of ") {
                Some((path, class)) if !analysed_files.contains(&whole) => {
                    let path = normalize_path(path);
                    let file = format!("{path} (in context of {class}");
                    (PathBuf::from(path), file)
                }
                _ => (PathBuf::from(&whole), whole),
            };
            let file = PathBuf::from(file);
            Finding {
                rel: relative(&cfg.root, &file).replace('\\', "/"),
                source_rel: relative(&cfg.root, &source).replace('\\', "/"),
                source,
                file,
                line: u32::try_from(e.line).unwrap_or(0),
                identifier: e.identifier,
                rule: rule.to_string(),
                message: e.message,
                tip: e.tip,
                can_be_ignored: e.can_be_ignored,
                fixable: e.fixable,
            }
        })
        .collect();
    let malformed = if severe {
        Vec::new()
    } else {
        apply_ignores(&mut findings, &cfg.root)
    };
    let mut unmatched_baseline = 0;
    if !severe
        && use_baseline
        && let Some(path) = &cfg.analyse.baseline
    {
        let b = baseline::Baseline::read(&cfg.root.join(path))?;
        unmatched_baseline = b.filter(&mut findings);
    }
    Ok(Analysed {
        findings,
        general,
        files: count,
        unmatched_baseline,
        malformed,
    })
}

/// The rules of the errors that the `@phpstan-ignore-line` and `@phpstan-ignore-next-line` comments of `files` hide,
/// by (normalized path, line), for `pun migrate` to name in `@pun-ignore`. PHPStan's `reportIgnoresWithoutComments`
/// stops those comments ignoring: each line they cover gets `ignore.allLineErrors` beside the errors it had hidden.
pub(crate) fn hidden_by_line_ignores(
    cfg: &Config,
    files: Vec<PathBuf>,
) -> Result<std::collections::HashMap<(String, i64), Vec<&'static str>>> {
    let paths = files.clone();
    let mut project = project(cfg, files, &paths)?;
    project.options.set(
        pun_analyse::ProjectOptions::REPORT_IGNORES_WITHOUT_COMMENTS,
        true,
    );
    let errors = pun_analyse::analyse_project(&project)
        .map_err(|e| run_error(e.to_string(), None))?
        .errors;
    let at = |e: &pun_analyse::Error| (normalize_path(&e.file), e.line);
    let mut out: std::collections::HashMap<_, Vec<&str>> = (errors.iter())
        .filter(|e| e.identifier == "ignore.allLineErrors")
        .map(|e| (at(e), Vec::new()))
        .collect();
    for e in &errors {
        if let (Some(rules), Some(rule)) = (out.get_mut(&at(e)), rule_of_identifier(&e.identifier))
            && !rules.contains(&rule)
        {
            rules.push(rule);
        }
    }
    Ok(out)
}

fn relative(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .into_owned()
}

/// Drop the findings `@pun-ignore` comments cover; the malformed comments come back as problems.
fn apply_ignores(findings: &mut Vec<Finding>, root: &Path) -> Vec<Problem> {
    let mut by_file: BTreeMap<PathBuf, Vec<Finding>> = BTreeMap::new();
    for f in findings.drain(..) {
        by_file.entry(f.source().to_path_buf()).or_default().push(f);
    }
    let mut malformed = vec![];
    for (file, mut list) in by_file {
        let source = std::fs::read(&file).unwrap_or_default();
        let source = String::from_utf8_lossy(&source);
        let mut ignores = Ignores::parse(&source);
        if !ignores.is_empty() {
            list.retain(|f| !f.ignorable() || !ignores.ignores(&f.rule, f.line as usize));
            let rel = relative(root, &file);
            malformed.extend(ignores.malformed.iter().map(|m| ignore_problem(&rel, m)));
        }
        findings.extend(list);
    }
    findings.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    malformed
}

/// A malformed `@pun-ignore` comment, as an error.
/// The engine's rule for an `@pun-ignore` comment without a rule ID (rules.md §4).
const INVALID_IGNORE: &str = "invalid-ignore-comment";

pub(crate) fn ignore_problem(rel: &str, m: &pun_lint::problem::ignore::Malformed) -> Problem {
    Problem {
        rule: INVALID_IGNORE.into(),
        path: rel.into(),
        span: m.span,
        message: m.message.clone(),
        help: Some("name the rule IDs it ignores: `@pun-ignore lint/group/rule`".into()),
        level: Level::Error,
        fix: None,
    }
}

/// A finding as a problem: at the first non-blank column of its line (PHPStan reports lines only), the tip as help.
pub(crate) fn problem(f: &Finding, source: &str, index: &LineIndex) -> Problem {
    let at = (index.line_range(f.line.max(1) as usize)).map_or(0, |r| {
        r.start + source[r.clone()].len() - source[r].trim_start().len()
    });
    Problem {
        rule: f.rule.clone(),
        path: f.source_rel().into(),
        span: Span::new(at, at),
        message: f.message.clone(),
        help: f.tip.clone(),
        level: Level::Error,
        fix: None,
    }
}

pub(crate) fn run(a: &AnalyseArgs) -> Result<Exit> {
    let started = Instant::now();
    let root = root()?;
    let opts = Options {
        config: a.config.clone(),
        ignore_php_constraint: crate::ignore_php_constraint(),
        ..Default::default()
    };
    let cfg = crate::run::load(&root, &opts)?;
    if cfg.analyse.rules.is_empty() {
        let hint = "set [analyse] preset, or turn rules on under [analyse.rules]";
        return Err(run_error("No rules detected".into(), Some(hint.into())));
    }
    if let Some(path) = &a.generate_baseline {
        return generate_baseline(a, &cfg, path.as_deref(), started);
    }
    let analysed = analyse(&cfg, &a.paths, true)?;
    present(a, &cfg, &analysed, started)
}

fn present(a: &AnalyseArgs, cfg: &Config, analysed: &Analysed, started: Instant) -> Result<Exit> {
    let root = &cfg.root;
    let errors = analysed.findings.len() + analysed.malformed.len() + analysed.general.len();
    match a.format.as_deref() {
        // `-q` prints nothing, as PHPStan's `--quiet` (with any error format, under a coding agent too).
        _ if a.quiet => {}
        Some(format) => {
            let cx = analyse_report::Context {
                verbose: a.verbose > 0 || agent::enabled(),
                general: &analysed.general,
                config: cfg.pun_toml().map(|s| relative(root, s)),
            };
            let findings = with_malformed(analysed, root);
            print!(
                "{}",
                analyse_report::render(format, &findings, &cx)
                    .map_err(|e| unsupported_format(&e))?
            );
        }
        None if agent::enabled() => print!("{}", agent_report(analysed, a.verbose > 0)),
        None => {
            let look = Look::stdout(a.color.unwrap_or(render::ColorChoice::Auto));
            print!(
                "{}",
                human(analysed, root, look, a.concise, started.elapsed())
            );
            if a.github_actions {
                print!("{}", annotations(analysed));
            }
        }
    }
    Ok(if errors > 0 {
        Exit::Failure
    } else {
        Exit::Success
    })
}

/// The findings, and the malformed `@pun-ignore` comments as errors that can't be ignored: they fail the run, so
/// every format reports them (PHPStan has no such comment, so its formats are unchanged without one).
fn with_malformed(analysed: &Analysed, root: &Path) -> Vec<Finding> {
    if analysed.malformed.is_empty() {
        return analysed.findings.clone();
    }
    let sources = sources(analysed);
    let mut findings = analysed.findings.clone();
    for m in &analysed.malformed {
        let source = root_join(analysed, &m.path);
        let source = if source.is_absolute() {
            source
        } else {
            root.join(source)
        };
        let line = sources
            .get(&root_join(analysed, &m.path))
            .map_or(0, |(_, index)| index.line_col(m.span.start).line);
        let rel = m.path.to_string_lossy().replace('\\', "/");
        findings.push(Finding {
            file: source.clone(),
            rel: rel.clone(),
            line: u32::try_from(line).unwrap_or(0),
            identifier: INVALID_IGNORE.into(),
            rule: INVALID_IGNORE.into(),
            message: m.message.clone(),
            tip: m.help.clone(),
            can_be_ignored: false,
            fixable: false,
            source,
            source_rel: rel,
        });
    }
    findings.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    findings
}

fn unsupported_format(e: &anyhow::Error) -> anyhow::Error {
    run_error(e.to_string(), None)
}

/// `--generate-baseline [file]`: every error into the baseline; nothing is reported, the exit code is 0.
fn generate_baseline(
    a: &AnalyseArgs,
    cfg: &Config,
    path: Option<&Path>,
    started: Instant,
) -> Result<Exit> {
    let path = match path {
        Some(path) => path.to_path_buf(),
        None => PathBuf::from(cfg.analyse.baseline.as_deref().unwrap_or(baseline::DEFAULT)),
    };
    let mut analysed = analyse(cfg, &a.paths, false)?;
    if analysed
        .findings
        .iter()
        .any(|e| e.identifier == "phpstan.parse")
    {
        return present(a, cfg, &analysed, started);
    }
    analysed.findings.retain(Finding::ignorable);
    let b = baseline::Baseline::of(&analysed.findings);
    b.write(&cfg.root.join(&path))?;
    let n = analysed.findings.len();
    let errors = agent::count(n, "error");
    let shown = path.display();
    if a.quiet {
        return Ok(Exit::Success);
    }
    if agent::enabled() {
        let detail = [format!("baseline {shown} written with {errors}")];
        print!("{}", agent::render("ok", &detail, "", &[], false));
        return Ok(Exit::Success);
    }
    let look = Look::stdout(a.color.unwrap_or(render::ColorChoice::Auto));
    let r = Renderer::new(look, &cfg.root);
    let mut message = format!("Baseline generated with {errors} in {shown}");
    if cfg.analyse.baseline.as_deref() != Some(path.to_string_lossy().as_ref()) {
        let _ = write!(
            message,
            "; add `baseline = \"{shown}\"` under [analyse] in pun.toml to use it"
        );
    }
    print!(
        "{}",
        r.done(&format!(
            "{message} ({:.2}s)",
            started.elapsed().as_secs_f64()
        ))
    );
    Ok(Exit::Success)
}

/// Problem blocks, sorted by file and line, then the summary; a note when baseline entries no longer match.
pub(crate) fn human(
    analysed: &Analysed,
    root: &Path,
    look: Look,
    concise: bool,
    elapsed: Duration,
) -> String {
    let mut r = Renderer::new(look, root);
    r.concise = concise;
    let mut out = problem_blocks(&r, analysed);
    let glyph = r.look.paint(render::Role::Error, r.look.glyph("✖", "x"));
    for g in &analysed.general {
        let _ = writeln!(out, "{glyph} {g}");
    }
    if !out.is_empty() {
        out.push('\n');
    }
    if analysed.unmatched_baseline > 0 {
        out.push_str(&baseline_note(&r, analysed.unmatched_baseline));
    }
    out.push_str(&r.summary(&Summary {
        errors: analysed.findings.len() + analysed.malformed.len() + analysed.general.len(),
        warnings: 0,
        fixable: 0,
        fix_command: "",
        need_formatting: 0,
        files: analysed.files,
        elapsed,
    }));
    out
}

/// The problem blocks of an analysis (also `pun check`'s analyse part).
pub(crate) fn problem_blocks(r: &Renderer, analysed: &Analysed) -> String {
    let sources = sources(analysed);
    let mut problems: Vec<(Problem, &str, &LineIndex)> = vec![];
    for f in &analysed.findings {
        let (source, index) = &sources[f.source()];
        problems.push((problem(f, source, index), source, index));
    }
    for m in &analysed.malformed {
        let (source, index) = &sources[&root_join(analysed, &m.path)];
        problems.push((m.clone(), source, index));
    }
    problems.sort_by(|a, b| (&a.0.path, a.0.span.start).cmp(&(&b.0.path, b.0.span.start)));
    r.problems(problems.iter().map(|(p, s, i)| (p, *s, *i)))
}

/// The absolute path of a malformed comment's file: the findings carry absolute paths, malformed ones relative.
fn root_join(analysed: &Analysed, rel: &Path) -> PathBuf {
    (analysed.findings.iter())
        .find(|f| Path::new(f.source_rel()) == rel)
        .map_or_else(|| rel.to_path_buf(), |f| f.source().to_path_buf())
}

/// Each file with a finding or a malformed comment, read once: its source and line index.
fn sources(analysed: &Analysed) -> BTreeMap<PathBuf, (String, LineIndex)> {
    let mut out = BTreeMap::new();
    let files = (analysed.findings.iter().map(|f| f.source().to_path_buf())).chain(
        analysed
            .malformed
            .iter()
            .map(|m| root_join(analysed, &m.path)),
    );
    for file in files {
        out.entry(file.clone()).or_insert_with(|| {
            let source =
                String::from_utf8_lossy(&std::fs::read(&file).unwrap_or_default()).into_owned();
            let index = LineIndex::new(&source);
            (source, index)
        });
    }
    out
}

pub(crate) fn baseline_note(r: &Renderer, unmatched: usize) -> String {
    let (entries, verb, them) = match unmatched {
        1 => ("entry", "matches", "it"),
        _ => ("entries", "match", "them"),
    };
    let glyph = r.look.paint(render::Role::Warning, r.look.glyph("⚠", "!"));
    format!(
        "{glyph} {unmatched} baseline {entries} no longer {verb}: run `pun analyse --generate-baseline` to drop {them}\n"
    )
}

/// GitHub annotations after the human output (`GITHUB_ACTIONS=true`): one per finding, at its line.
pub(crate) fn annotations(analysed: &Analysed) -> String {
    let sources = sources(analysed);
    let mut out = String::new();
    for f in &analysed.findings {
        let (source, index) = &sources[f.source()];
        out.push_str(&github::problem(&problem(f, source, index), index));
    }
    for m in &analysed.malformed {
        let (_, index) = &sources[&root_join(analysed, &m.path)];
        out.push_str(&github::problem(m, index));
    }
    out
}

/// Agent mode: the summary line, then `path:line rule message` per finding, all of them with `-v`.
fn agent_report(analysed: &Analysed, all: bool) -> String {
    let items = agent_items(analysed);
    let errors = items.len();
    let (result, detail) = match errors {
        0 => ("pass", vec![]),
        n => {
            let files: std::collections::BTreeSet<&str> =
                analysed.findings.iter().map(|f| f.rel.as_str()).collect();
            let what = format!(
                "{} in {}",
                agent::count(n, "error"),
                agent::count(files.len(), "file")
            );
            ("fail", vec![what])
        }
    };
    let mut detail = detail;
    if analysed.unmatched_baseline > 0 {
        detail.push(format!(
            "{} no longer matched",
            agent::count(analysed.unmatched_baseline, "baseline entry")
                .replace("entrys", "entries")
        ));
    }
    let checked = format!("{} checked", analysed.files);
    agent::render(result, &detail, &checked, &items, all)
}

/// One agent line per finding: `path:line rule message`.
pub(crate) fn agent_items(analysed: &Analysed) -> Vec<String> {
    let mut items: Vec<String> = (analysed.findings.iter())
        .map(|f| format!("{}:{} {} {}", f.rel, f.line, f.rule, f.message))
        .collect();
    items.extend(
        (analysed.malformed.iter())
            .map(|m| format!("{} {} {}", m.path.display(), m.rule, m.message)),
    );
    items.extend(
        analysed
            .general
            .iter()
            .map(|g| format!("{UNMATCHED_IGNORE} {g}")),
    );
    items
}
