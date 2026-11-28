use crate::baseline::{self, Baseline, Kind};
use crate::cache::{self, Cache};
use crate::lint_plan::{self, CliLevel, Engine, On, Plan};
use crate::render::{Look, RunError, Spinner};
use crate::report::{self, Abort, FileResult, Status};
use crate::{Exit, FormatArgs, root};
use anyhow::Result;
use pun_config::{Config, Options, Rule};
use pun_format::{LintError, Pipeline};
use pun_lint::problem::ignore::{Ignores, covers};
use pun_lint::problem::{LineIndex, Problem};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Stage {
    Format,
    Lint { fix: bool },
    Check { fix: bool },
}

impl Stage {
    fn fix(self) -> bool {
        matches!(self, Stage::Lint { fix: true } | Stage::Check { fix: true })
    }
}

#[derive(Clone, Default)]
pub(crate) struct LintOpts {
    pub(crate) levels: Vec<(CliLevel, String)>,
    pub(crate) deny_warnings: bool,
    pub(crate) max_warnings: Option<usize>,
    pub(crate) concise: bool,
    /// `pun lint --generate-baseline [file]`.
    #[expect(
        clippy::option_option,
        reason = "clap: the flag is absent, bare, or given a file"
    )]
    pub(crate) generate_baseline: Option<Option<PathBuf>>,
}

type Run = Result<Option<(Vec<u8>, Vec<&'static str>)>, LintError>;

fn run_file(p: &Pipeline, file: &Path, code: &[u8]) -> Result<Run> {
    let r = p.run(file, code);
    if !matches!(
        r,
        Err(LintError {
            after_fix: false,
            ..
        })
    ) {
        p.check_annotations(file, code)?;
    }
    Ok(r.map(|f| f.map(|f| (f.code, f.applied))))
}

pub(crate) fn load_config(a: &FormatArgs, root: &Path) -> Result<Config> {
    let opts = Options {
        config: a.config.clone(),
        no_config: a.no_config,
        preset: a.preset.clone(),
        ignore_php_constraint: crate::ignore_php_constraint(),
    };
    load(root, &opts)
}

pub(crate) fn load(root: &Path, opts: &Options) -> Result<Config> {
    pun_config::load(root, opts).map_err(config_error)
}

fn configure(cfg: &Config, rule: &mut dyn pun_lint::Rule) -> Result<()> {
    let id = rule.id();
    let base = (cfg.lint.rules.iter()).find(|r| r.id == id);
    let overridden = (cfg.lint.overrides.iter())
        .flat_map(|o| &o.rules)
        .any(|s| s.id == id && !s.options.is_null());
    let result = if overridden {
        Err("takes options in [lint.rules] only, not in [[lint.overrides]]".into())
    } else {
        rule.configure(base.map_or(&serde_json::Value::Null, |r| &r.options))
    };
    result.map_err(|e| config_error(anyhow::anyhow!("pun.toml: `{id}` {e}")))
}

/// A config error: Pun's message on stderr, or, with `--format` or `-q`, Pint's error box on stdout.
#[expect(clippy::needless_pass_by_value, reason = "a `map_err` callback")]
fn config_error(e: anyhow::Error) -> anyhow::Error {
    let mut abort = Abort::plain(format!("{e:#}"), None);
    abort.pint = true;
    abort.into()
}

struct Pipelines {
    cfg: Config,
    format: Vec<Rule>,
    built: Mutex<HashMap<Vec<String>, Arc<Pipeline>>>,
}

impl Pipelines {
    fn new(cfg: &Config, format: Vec<Rule>) -> Pipelines {
        Pipelines {
            cfg: cfg.clone(),
            format,
            built: Mutex::new(HashMap::new()),
        }
    }

    fn get(&self, with_format: bool, lint: &[Rule]) -> Result<Arc<Pipeline>> {
        let mut key = vec![with_format.to_string()];
        key.extend(lint.iter().map(|r| format!("{}={}", r.name, r.options)));
        if let Some(p) = self.built.lock().expect("pipelines").get(&key) {
            return Ok(p.clone());
        }
        let format = if with_format { &self.format[..] } else { &[] };
        let rules = format.iter().chain(lint).cloned().collect();
        let p = Arc::new(pipeline(&Config {
            rules,
            ..self.cfg.clone()
        })?);
        self.built.lock().expect("pipelines").insert(key, p.clone());
        Ok(p)
    }
}

struct Engines<'a> {
    stage: Stage,
    plan: Option<Plan>,
    linter: Option<pun_lint::Linter>,
    pipelines: Pipelines,
    lint_files: Option<HashSet<PathBuf>>,
    format_files: Option<HashSet<PathBuf>>,
    root: &'a Path,
}

struct Processed {
    result: FileResult,
    remaining: Option<FileResult>,
}

#[expect(
    clippy::too_many_lines,
    reason = "long; to be split (code audit: functions over 100 lines)"
)]
pub(crate) fn stage(a: &FormatArgs, stage: Stage, lint: &LintOpts) -> Result<Exit> {
    struct StopProbe;
    impl Drop for StopProbe {
        fn drop(&mut self) {
            pun_fixers::prettier::stop();
        }
    }
    let started = std::time::Instant::now();
    let root = root()?;
    let cfg = load_config(a, &root)?;
    let plan = ported_plan(a, &cfg, lint)?;
    let lints = stage != Stage::Format;
    let formats = !matches!(stage, Stage::Lint { .. });
    let blade_on = formats
        && (cfg.rules.iter()).any(|r| r.name == "Pint/laravel_blade" && r.options.is_null());
    let _probe = StopProbe;
    let prettier = crate::prettier::ensure(&root, blade_on)?;
    let pipelines = Pipelines::new(&cfg, if formats { cfg.rules.clone() } else { vec![] });
    let base_token = if lints {
        token_rules(&plan.for_file(&root.join("-")), stage.fix(), false)
    } else {
        vec![]
    };
    let base = pipelines.get(formats, &base_token)?;
    if a.pun.explain {
        explain(&cfg, &base);
    }

    let stdin_path = a.paths.first().is_some_and(|p| p.as_os_str() == "-");
    if stdin_path || a.stdin_filename.is_some() {
        // Standard input is only fixed, never checked: without `--fix`, lint and check would pass it unread.
        if lints && !stage.fix() {
            let cmd = crate::agent::cmd();
            return Err(Abort::plain(
                format!("`pun {cmd}` can't check standard input"),
                Some(format!(
                    "pass the files to check, or `pun {cmd} --fix -` to fix the input"
                )),
            )
            .into());
        }
        if !a.stdin_filename.as_ref().is_some_and(|f| is_blade(f)) || a.test {
            pun_fixers::prettier::stop();
        }
        return stdin(a, &cfg, &base);
    }

    let dry_run = a.test;
    let selected = selected_files(a, &cfg)?;
    let (format_set, lint_set) = file_sets(a, &cfg, formats, lints, selected.as_ref())?;
    let mut files = format_set.clone();
    let known: HashSet<&PathBuf> = format_set.iter().collect();
    files.extend(lint_set.iter().filter(|f| !known.contains(f)).cloned());
    drop(known);
    let (natives, project) = if lints {
        native_rules(a, &cfg, &plan, selected.as_ref(), &lint_set)?
    } else {
        (vec![], None)
    };
    let cache = open_cache(a, stage, lint, &cfg, &prettier, project.as_deref());
    let engines = Engines {
        stage,
        linter: (!natives.is_empty()).then(|| pun_lint::Linter::new(natives, cfg.php_version())),
        lint_files: lints.then(|| lint_set.into_iter().collect()),
        format_files: match stage {
            Stage::Check { .. } => Some(format_set.into_iter().collect()),
            _ => None,
        },
        plan: lints.then_some(plan),
        pipelines,
        root: &root,
    };

    if blade_on && !files.iter().any(|f| is_blade(f)) {
        pun_fixers::prettier::stop();
    }
    let look = Look::stdout(a.color());
    let human = a.format.is_none() && !a.quiet && !crate::agent::enabled();
    let spinner = human
        .then(|| Spinner::stderr(files.len(), look, std::io::stderr().is_terminal()))
        .flatten();
    let process = |path: &PathBuf, code: std::io::Result<Vec<u8>>| -> Result<Processed> {
        let r = match (&cache, code) {
            (Some(cache), Ok(code)) => process_cached(a, &engines, cache, dry_run, path, code),
            (_, code) => process_file(a, &engines, dry_run, path, code),
        };
        if let Some(s) = &spinner {
            s.tick();
        }
        r
    };

    let all = process_in_parallel(&files, blade_on, process);
    drop(spinner);
    let processed: Vec<Processed> = all.into_iter().collect::<Result<_>>()?;

    let mut remaining = vec![];
    let mut results = vec![];
    for p in processed {
        remaining.extend(p.remaining);
        results.push(p.result);
    }
    if let Some(cache) = cache {
        cache.save(&results.iter().map(|r| r.rel.as_str()).collect());
    }

    let mut unmatched_lint = 0;
    if lints {
        if let Some(path) = &lint.generate_baseline
            && !results.iter().any(FileResult::is_error)
        {
            return generate_baseline(a, &cfg, path.as_deref(), &results, started);
        }
        if let Some(path) = &cfg.lint.baseline {
            let b = Baseline::read(&cfg.root.join(path), Kind::Lint)?;
            let mut left = b.left();
            let rediff = |r: &mut FileResult| rediff(a, &engines, r);
            let check_only = dry_run && !stage.fix();
            apply_baseline(
                &mut left,
                &mut results,
                &mut remaining,
                dry_run,
                check_only,
                rediff,
            )?;
            unmatched_lint = left.unmatched();
        }
    }

    let mut unmatched_analyse = 0;
    let mut general = vec![];
    let mut total = files.len();
    if let Stage::Check { .. } = stage
        && selected.as_ref().is_none_or(|(_, s)| !s.is_empty())
    {
        let paths = selected.as_ref().map_or(&a.paths, |(_, s)| s);
        let analysed =
            crate::analyse::analyse(&cfg, paths, true, crate::analyse::cache_on(a.pun.no_cache))?;
        unmatched_analyse = analysed.unmatched_baseline;
        general.clone_from(&analysed.general);
        total = total.max(analysed.files);
        add_findings(&mut results, &analysed, &root);
    }

    let ctx = report::Ctx {
        total,
        dry_run,
        remaining: &remaining,
        verbose: a.verbose > 0,
        color: a.color(),
        look,
        root: &root,
        elapsed: started.elapsed(),
        github: a.github_actions && human,
        concise: lint.concise,
        unmatched_baseline: (unmatched_lint, unmatched_analyse),
        general: &general,
    };
    report::emit(
        &ctx,
        &results,
        a.format.as_deref(),
        (a.quiet, !matches!(stage, Stage::Lint { .. })),
        a.output_to_file.as_deref(),
        a.output_format.as_deref(),
    )?;
    if lint.generate_baseline.is_some() && !a.quiet {
        // Reached only when a file failed: a baseline without that file's problems would hide too little.
        let failed = results.iter().filter(|r| r.is_error()).count();
        let look = Look::stderr(a.color());
        let glyph = look.paint(crate::render::Role::Warning, look.glyph("⚠", "!"));
        eprintln!(
            "{glyph} No baseline written: {} failed; fix {} and run `pun lint --generate-baseline` again",
            crate::agent::count(failed, "file"),
            if failed == 1 { "it" } else { "them" },
        );
    }
    let exit = exit(stage, a, lint, &ctx, &results);
    eprint!("{}", base.profile_report());
    Ok(exit)
}

/// The message of a token rule's baseline entry: those rules report a file, not each place they change.
const TOKEN_FIX: &str = "the rule's fix changes this file";

/// The lint rule IDs among a file's fixers.
fn lint_ids(applied: &[&'static str]) -> impl Iterator<Item = &'static str> {
    (applied.iter())
        .map(|n| pun_config::rule_of(n).unwrap_or(n))
        .filter(|id| id.starts_with("lint/"))
}

/// Each lint problem of a dry run's `results` as (file, rule, message): the native rules' problems, and each token
/// rule that would change a file, once per file.
fn lint_problems(results: &[FileResult]) -> Vec<(&str, &str, &str)> {
    let mut out = vec![];
    for r in results {
        let problems = (r.problems.iter()).filter(|p| p.rule.starts_with("lint/"));
        out.extend(problems.map(|p| (r.rel.as_str(), p.rule.as_str(), p.message.as_str())));
        if r.status == Status::Fixed {
            out.extend(lint_ids(&r.applied).map(|id| (r.rel.as_str(), id, TOKEN_FIX)));
        }
    }
    out
}

/// Hides the lint problems the baseline covers, after the `@pun-ignore` comments: native problems, and the token
/// rules a dry run (or `--fix` left for later) would apply. Fixes `--fix` applied are not problems and stay. A file
/// that still changes without the hidden rules gets its diff from `rediff` when the run only checks (`check_only`).
fn apply_baseline(
    left: &mut baseline::Left,
    results: &mut [FileResult],
    remaining: &mut Vec<FileResult>,
    dry_run: bool,
    check_only: bool,
    mut rediff: impl FnMut(&mut FileResult) -> Result<()>,
) -> Result<()> {
    let drop_fixers = |r: &mut FileResult, left: &mut baseline::Left| {
        let rel = r.rel.clone();
        r.applied.retain(|n| {
            let id = pun_config::rule_of(n).unwrap_or(n);
            !(id.starts_with("lint/") && left.take(&rel, id, TOKEN_FIX))
        });
    };
    for r in results.iter_mut() {
        let rel = r.rel.clone();
        (r.problems)
            .retain(|p| !(p.rule.starts_with("lint/") && left.take(&rel, &p.rule, &p.message)));
        if dry_run && r.status == Status::Fixed {
            let before = r.applied.len();
            drop_fixers(r, left);
            if r.applied.is_empty() {
                no_changes(r);
            } else if check_only && r.applied.len() < before {
                rediff(r)?;
            }
        }
    }
    for r in remaining.iter_mut() {
        drop_fixers(r, left);
    }
    remaining.retain(|r| !r.applied.is_empty());
    Ok(())
}

fn no_changes(r: &mut FileResult) {
    r.status = Status::NoChanges;
    r.diff.clear();
    r.line = 0;
}

/// A checked file's diff without the token rules the baseline hides: its format rules and the token rules still in
/// `r.applied` run again on the file, which a check leaves as it was.
fn rediff(a: &FormatArgs, e: &Engines, r: &mut FileResult) -> Result<()> {
    let code = std::fs::read(&r.path)?;
    let (in_format, _) = scopes(e, &r.path);
    let on = (e.plan.as_ref()).map_or_else(BTreeMap::new, |plan| plan.for_file(&r.path));
    let mut token = token_rules(&on, false, false);
    token.retain(|t| r.applied.iter().any(|n| *n == t.name));
    let p = e.pipelines.get(in_format, &token)?;
    let fixed = isolated(|| run_file(&p, &r.path, &code))
        .ok()
        .transpose()?
        .and_then(Result::ok)
        .flatten();
    match fixed {
        Some((new, _)) => {
            r.diff = shown_diff(a, &r.path, Path::new(&r.rel), &code, &new);
            r.line = crate::github::first_changed_line(&code, &new);
        }
        None => no_changes(r),
    }
    Ok(())
}

/// The unified diff of a file from `old` to `new` (`-v`), named by its relative path in agent mode.
fn shown_diff(a: &FormatArgs, path: &Path, rel: &Path, old: &[u8], new: &[u8]) -> String {
    if a.verbose == 0 {
        return String::new();
    }
    report::diff_bytes(old, new, if crate::agent::enabled() { rel } else { path })
}

/// `pun lint --generate-baseline [file]`: every lint problem into the baseline; nothing is reported, the exit code
/// is 0.
fn generate_baseline(
    a: &FormatArgs,
    cfg: &Config,
    path: Option<&Path>,
    results: &[FileResult],
    started: std::time::Instant,
) -> Result<Exit> {
    let path = match path {
        Some(path) => path.to_path_buf(),
        None => PathBuf::from(
            cfg.lint
                .baseline
                .as_deref()
                .unwrap_or(Kind::Lint.default_file()),
        ),
    };
    let problems = lint_problems(results);
    Baseline::of(problems.iter().copied()).write(&cfg.root.join(&path), Kind::Lint)?;
    let counted = crate::agent::count(problems.len(), "problem");
    let files = problems.iter().filter(|p| p.2 == TOKEN_FIX).count();
    let files_note = (files > 0).then(|| {
        let entries = crate::render::plural(files, "entry", "entries");
        format!(
            "{entries} for PHP-CS-Fixer rules: each counts a file, not its problems, so a new problem in a listed file stays hidden"
        )
    });
    let shown = path.display();
    if a.quiet {
        return Ok(Exit::Success);
    }
    if crate::agent::enabled() {
        let mut detail = vec![format!("baseline {shown} written with {counted}")];
        detail.extend(files_note);
        print!("{}", crate::agent::render("ok", &detail, "", &[], false));
        return Ok(Exit::Success);
    }
    let mut message = format!("Baseline generated with {counted} in {shown}");
    if cfg.lint.baseline.as_deref() != Some(path.to_string_lossy().as_ref()) {
        let _ = write!(
            message,
            "; add `baseline = \"{shown}\"` under [lint] in pun.toml to use it"
        );
    }
    let r = crate::render::Renderer::new(Look::stdout(a.color()), &cfg.root);
    let elapsed = started.elapsed().as_secs_f64();
    print!("{}", r.done(&format!("{message} ({elapsed:.2}s)")));
    if let Some(note) = files_note {
        println!("  {note}");
    }
    Ok(Exit::Success)
}

/// The lint plan, or the config error that names every format and lint rule Pun hasn't ported.
fn ported_plan(a: &FormatArgs, cfg: &Config, lint: &LintOpts) -> Result<Plan> {
    let plan = Plan::new(cfg, &lint.levels).map_err(config_error)?;
    let mut unported = Pipeline::new(&cfg.rules, &crate::extensions::fixers)?.not_ported;
    if let Err(lint) = &plan {
        unported.extend_from_slice(lint);
    }
    if unported.is_empty()
        && let Ok(plan) = plan
    {
        return Ok(plan);
    }
    Err(unported_error(a, cfg, &unported).into())
}

/// The files `--dirty`, `--diff` or `--staged` selects from git, when one of them is given: the files to format, then
/// the changed files the other stages pick theirs from.
fn selected_files(a: &FormatArgs, cfg: &Config) -> Result<Option<(Vec<PathBuf>, Vec<PathBuf>)>> {
    let selection = if a.dirty {
        Some(crate::finder::GitSelect::Dirty)
    } else if let Some(b) = &a.diff {
        Some(crate::finder::GitSelect::Diff(b.clone()))
    } else {
        a.pun.staged.then_some(crate::finder::GitSelect::Staged)
    };
    Ok(match &selection {
        Some(sel) => Some(crate::finder::git_selection(cfg, sel)?),
        None => None,
    })
}

/// The files to format and the files to lint.
fn file_sets(
    a: &FormatArgs,
    cfg: &Config,
    formats: bool,
    lints: bool,
    selected: Option<&(Vec<PathBuf>, Vec<PathBuf>)>,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let format_set = match (formats, selected) {
        (false, _) => vec![],
        (true, Some((files, _))) => files.clone(),
        (true, None) => crate::finder::discover(cfg, &a.paths)?,
    };
    let lint_set = match (lints, selected) {
        (false, _) => vec![],
        (true, Some((_, files))) => {
            let all: HashSet<PathBuf> =
                crate::finder::files(cfg, Some(crate::finder::Command::Lint), &[])?
                    .into_iter()
                    .collect();
            files.iter().filter(|f| all.contains(*f)).cloned().collect()
        }
        (true, None) => crate::finder::files(cfg, Some(crate::finder::Command::Lint), &a.paths)?,
    };
    Ok((format_set, lint_set))
}

type Natives = Vec<Box<dyn pun_lint::Rule>>;

/// The configured native lint rules the plan turns on, with the project's codebase scanned for them, and the files
/// that scan reads.
fn native_rules(
    a: &FormatArgs,
    cfg: &Config,
    plan: &Plan,
    selected: Option<&(Vec<PathBuf>, Vec<PathBuf>)>,
    lint_set: &[PathBuf],
) -> Result<(Natives, Option<Vec<PathBuf>>)> {
    let on = plan.mentioned();
    let mut natives: Vec<_> = (crate::extensions::lint_rules().into_iter())
        .filter(|r| on.contains(r.id()))
        .collect();
    for r in &mut natives {
        configure(cfg, r.as_mut())?;
    }
    if natives.is_empty() {
        return Ok((natives, None));
    }
    let project = match (selected, a.paths.is_empty()) {
        (None, true) => lint_set.to_vec(),
        _ => crate::finder::files(cfg, Some(crate::finder::Command::Lint), &[])?,
    };
    project_codebase(cfg, project.clone());
    Ok((natives, Some(project)))
}

/// The result cache of this run, unless `--no-cache`, `PUN_NO_CACHE` or `PUN_PROFILE` turns it off. The run key covers
/// everything besides the file that can change a file's result; `project` (the files the lint rules' project scan
/// reads) adds a fingerprint that the files whose linting reads the project are keyed on as well.
fn open_cache(
    a: &FormatArgs,
    stage: Stage,
    lint: &LintOpts,
    cfg: &Config,
    prettier: &str,
    project: Option<&[PathBuf]>,
) -> Option<Cache> {
    let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty() && v != "0");
    if a.pun.no_cache || set("PUN_NO_CACHE") || std::env::var_os("PUN_PROFILE").is_some() {
        return None;
    }
    let file = Cache::default_file(&cfg.root)?;
    let exe = std::env::current_exe().ok()?;
    let mut key = cache::Key::default();
    let flags = (a.test, a.repair, &a.config, a.no_config, &a.preset);
    let env = ["PUN_IGNORE_PHP_CONSTRAINT"];
    (key.add("pun", env!("CARGO_PKG_VERSION")))
        .add("exe", cache::stat(&exe))
        .add("stage", format!("{stage:?}"))
        .add("flags", format!("{flags:?}"))
        .add("levels", format!("{:?}", lint.levels))
        .add(
            "view",
            format!("{:?}", (a.verbose > 0, crate::agent::enabled())),
        )
        .add("config", format!("{cfg:?}"))
        .add("env", format!("{:?}", env.map(std::env::var_os)))
        .add("prettier", prettier);
    if !prettier.is_empty() {
        for f in PRETTIER_FILES {
            key.stat(&cfg.root.join(f));
        }
    }
    let project = project.map(|files| project_fingerprint(&cfg.root, files));
    Some(Cache::open(file, &cfg.root, key.digest(), project))
}

/// The files besides the installed packages that can change what Prettier does to a Blade file.
const PRETTIER_FILES: [&str; 12] = [
    "package.json",
    "package-lock.json",
    "bun.lock",
    "bun.lockb",
    "pnpm-lock.yaml",
    "yarn.lock",
    "node_modules/.package-lock.json",
    "node_modules/tailwindcss/package.json",
    "tailwind.config.js",
    "tailwind.config.cjs",
    "tailwind.config.mjs",
    "tailwind.config.ts",
];

/// Composer's files that change when the packages or the autoloader change, and Pest's global `uses()`.
const COMPOSER_FILES: [&str; 9] = [
    "tests/Pest.php",
    "composer.json",
    "composer.lock",
    "vendor/composer/installed.json",
    "vendor/composer/autoload_classmap.php",
    "vendor/composer/autoload_files.php",
    "vendor/composer/autoload_namespaces.php",
    "vendor/composer/autoload_psr4.php",
    "vendor/composer/autoload_static.php",
];

/// The size and modification time of every file the project scan can read outside the installed packages (the
/// scanned files and the project's own autoloaded ones), and of Composer's files.
fn project_fingerprint(root: &Path, files: &[PathBuf]) -> cache::Digest {
    let mut paths = files.to_vec();
    paths.extend(autoloaded(root));
    paths.extend(linked_packages(root));
    let stats: Vec<Vec<u8>> = paths.par_iter().map(|p| cache::stat(p)).collect();
    let mut key = cache::Key::default();
    for (p, s) in paths.iter().zip(stats) {
        key.add(&p.to_string_lossy(), s);
    }
    for f in COMPOSER_FILES {
        key.stat(&root.join(f));
    }
    key.digest()
}

/// The PHP files of the packages Composer linked into `vendor/` (`path` repositories): their source changes in place,
/// without a change to Composer's files, and the project scan reads it.
fn linked_packages(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let mut entries: Vec<_> = (std::fs::read_dir(dir).into_iter().flatten())
            .flatten()
            .collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for e in entries {
            let p = e.path();
            // A linked package's own installed packages and assets aren't the package.
            let skip = e.file_name() == "vendor" || e.file_name() == "node_modules";
            match e.file_type() {
                Ok(t) if t.is_dir() && !skip => walk(&p, out),
                Ok(t) if t.is_file() && p.extension().is_some_and(|x| x == "php") => out.push(p),
                _ => {}
            }
        }
    }
    let mut out = vec![];
    let vendors = (std::fs::read_dir(root.join("vendor")).into_iter().flatten()).flatten();
    let mut packages: Vec<PathBuf> = vendors
        .flat_map(|v| (std::fs::read_dir(v.path()).into_iter().flatten()).flatten())
        .filter(|p| p.file_type().is_ok_and(|t| t.is_symlink()))
        .map(|p| p.path())
        .collect();
    packages.sort();
    for p in packages {
        walk(&p, &mut out);
    }
    out
}

/// The PHP files composer.json's `autoload` and `autoload-dev` name outside `vendor/`.
fn autoloaded(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let mut entries: Vec<_> = (std::fs::read_dir(dir).into_iter().flatten())
            .flatten()
            .map(|e| e.path())
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|e| e == "php") {
                out.push(p);
            }
        }
    }
    let code = std::fs::read(root.join("composer.json")).unwrap_or_default();
    let json: serde_json::Value = serde_json::from_slice(&code).unwrap_or_default();
    let mut named = vec![];
    for section in ["autoload", "autoload-dev"] {
        let s = &json[section];
        for kind in ["psr-4", "psr-0"] {
            for dirs in s[kind].as_object().into_iter().flat_map(|m| m.values()) {
                named.extend(dirs.as_str());
                named.extend(
                    dirs.as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|d| d.as_str()),
                );
            }
        }
        for kind in ["classmap", "files"] {
            named.extend(
                s[kind]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|d| d.as_str()),
            );
        }
    }
    let vendor = root.join("vendor");
    let mut out = vec![];
    for name in named {
        let p = root.join(name);
        if p.starts_with(&vendor) {
            continue;
        }
        if p.is_dir() {
            walk(&p, &mut out);
        } else {
            out.push(p);
        }
    }
    out
}

/// Every file, in order: the Blade files, largest first, on lanes of their own (each waits on a Prettier worker),
/// the PHP files on the thread pool as the readers bring them in.
fn process_in_parallel(
    files: &[PathBuf],
    blade_on: bool,
    process: impl Fn(&PathBuf, std::io::Result<Vec<u8>>) -> Result<Processed> + Sync,
) -> Vec<Result<Processed>> {
    let (mut blade, mut php): (Vec<_>, Vec<_>) =
        (files.iter().cloned().enumerate()).partition(|(_, f)| blade_on && is_blade(f));
    blade.sort_by_cached_key(|(_, f)| {
        std::cmp::Reverse(std::fs::metadata(f).map_or(0, |m| m.len()))
    });
    // Largest first too, so the last files to start are small ones and the threads finish together.
    php.par_sort_by_cached_key(|(_, f)| {
        std::cmp::Reverse(std::fs::metadata(f).map_or(0, |m| m.len()))
    });
    let lanes = (blade.len() / 32)
        .clamp(1, rayon::current_num_threads())
        .min(blade.len());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let php_paths: Vec<PathBuf> = php.iter().map(|(_, f)| f.clone()).collect();
    let (mut done, php_done) = std::thread::scope(|s| {
        let lanes: Vec<_> = (0..lanes)
            .map(|_| {
                s.spawn(|| {
                    let mut out = vec![];
                    while let Some((i, f)) =
                        blade.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
                    {
                        out.push((*i, process(f, std::fs::read(f))));
                    }
                    out
                })
            })
            .collect();
        let php_done = read_in_parallel(&php_paths, &process);
        let done: Vec<_> = (lanes.into_iter())
            .flat_map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .collect();
        (done, php_done)
    });
    done.extend(php.iter().map(|(i, _)| *i).zip(php_done));
    done.sort_by_key(|d| d.0);
    done.into_iter().map(|d| d.1).collect()
}

/// A run error when a file crashed a fixer or a rule, else a failure for what `--test`, `--repair` and the lint
/// levels count against the run.
fn exit(
    stage: Stage,
    a: &FormatArgs,
    lint: &LintOpts,
    ctx: &report::Ctx,
    results: &[FileResult],
) -> Exit {
    let crashed = (results.iter()).any(|r| matches!(r.status, Status::Exception | Status::Lint));
    let changed = results.iter().any(|r| r.status == Status::Fixed);
    let failed = if stage == Stage::Format {
        ((ctx.dry_run || a.repair) && changed) || results.iter().any(FileResult::is_error)
    } else {
        let c = report::counts(ctx, results);
        c.errors > 0
            || c.need_formatting > 0
            || (a.repair && changed)
            || (lint.deny_warnings && c.warnings > 0)
            || lint.max_warnings.is_some_and(|m| c.warnings > m)
    };
    if crashed {
        Exit::RunError
    } else if failed {
        Exit::Failure
    } else {
        Exit::Success
    }
}

fn project_codebase(cfg: &Config, files: Vec<PathBuf>) {
    let cfg = cfg.clone();
    pun_lint::rules::facts::set_project_scan(move || {
        let threads = rayon::current_num_threads();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .stack_size(crate::STACK)
            .build();
        let pool = pool.expect("the project scan's thread pool");
        std::thread::scope(|s| {
            s.spawn(|| pool.install(|| scan_project(&cfg, &files)))
                .join()
        })
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    });
}

fn scan_project(cfg: &Config, files: &[PathBuf]) -> Arc<pun_lint::rules::facts::ProjectCodebase> {
    use pun_lint::rules::facts;
    use pun_phpstan::{Array, Key, Value};
    std::thread::spawn(|| {
        pun_phpstan::symbols::symbols(pun_phpstan::symbols::PhpVersion::new(8, 4))
    });
    let read = || -> Vec<(String, String)> {
        (files.par_iter())
            .filter_map(|f| {
                let rel = f.strip_prefix(&cfg.root).unwrap_or(f);
                Some((
                    rel.to_string_lossy().into_owned(),
                    std::fs::read_to_string(f).ok()?,
                ))
            })
            .collect()
    };
    let autoloaded = || {
        let mut extensions = Array::new();
        extensions.push(Value::Str("php".into()));
        let mut params = Array::new();
        params.set(Key::Str("fileExtensions".into()), Value::Array(extensions));
        let project = pun_analyse::Project::from_config(&cfg.root, &params);
        (project, composer_classmap(&cfg.root))
    };
    let (sources, (project, classmap)) = rayon::join(read, autoloaded);
    let mut sources = sources;
    for path in composer_files(&cfg.root) {
        if let Ok(code) = std::fs::read_to_string(&path) {
            sources.push((path.to_string_lossy().into_owned(), code));
        }
    }
    let class_loader = cfg.root.join("vendor/composer/ClassLoader.php");
    let mut by_class: HashMap<&[u8], &Path> = HashMap::new();
    by_class.insert(b"composer\\autoload\\classloader", &class_loader);
    for (class, path) in &classmap {
        by_class.insert(class, path);
    }
    for a in &project.autoloaded {
        by_class.entry(&a.class).or_insert(&a.path);
    }
    let psr0 = project.autoloaded.iter().any(|a| a.psr0);
    let autoload = |class: &[u8]| {
        let path = by_class.get(class).or_else(|| {
            let at = class.iter().rposition(|b| *b == b'\\').map_or(0, |i| i + 1);
            let mut dirs = class.to_vec();
            dirs[at..]
                .iter_mut()
                .filter(|b| **b == b'_')
                .for_each(|b| *b = b'\\');
            psr0.then(|| by_class.get(dirs.as_slice())).flatten()
        })?;
        Some((
            path.to_string_lossy().into_owned(),
            std::fs::read_to_string(path).ok()?,
        ))
    };
    facts::project_codebase_autoloaded(&sources, cfg.php_version(), &autoload)
}

fn composer_classmap(root: &Path) -> Vec<(Vec<u8>, PathBuf)> {
    (composer_map(root, "autoload_classmap.php").into_iter())
        .map(|(class, path)| {
            let class = class.replace("\\\\", "\\").to_ascii_lowercase();
            (class.into_bytes(), path)
        })
        .collect()
}

fn composer_files(root: &Path) -> Vec<PathBuf> {
    let vendor = root.join("vendor");
    (composer_map(root, "autoload_files.php").into_iter())
        .map(|(_, path)| path)
        .filter(|path| path.starts_with(&vendor))
        .collect()
}

fn composer_map(root: &Path, file: &str) -> Vec<(String, PathBuf)> {
    let vendor = root.join("vendor");
    let code = std::fs::read(vendor.join("composer").join(file)).unwrap_or_default();
    (String::from_utf8_lossy(&code).lines())
        .filter_map(|line| {
            let (class, path) = line.trim().strip_prefix('\'')?.split_once("' => $")?;
            let (dir, path) = path.strip_suffix("',")?.split_once(" . '/")?;
            let base = if dir == "vendorDir" {
                vendor.as_path()
            } else {
                root
            };
            Some((class.to_string(), base.join(path)))
        })
        .collect()
}

const READERS: usize = 3;

fn read_in_parallel<T: Send>(
    files: &[PathBuf],
    process: impl Fn(&PathBuf, std::io::Result<Vec<u8>>) -> T + Sync,
) -> Vec<T> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let (tx, rx) = std::sync::mpsc::sync_channel(64);
    std::thread::scope(|s| {
        for _ in 0..READERS.min(files.len()) {
            let (tx, next) = (tx.clone(), &next);
            s.spawn(move || {
                while let Some(path) = files.get(next.fetch_add(1, Ordering::Relaxed))
                    && tx.send((path, std::fs::read(path))).is_ok()
                {}
            });
        }
        drop(tx);
        let index: HashMap<&PathBuf, usize> =
            files.iter().enumerate().map(|(i, f)| (f, i)).collect();
        let mut out: Vec<(usize, T)> = (rx.into_iter().par_bridge())
            .map(|(path, code)| (index[path], process(path, code)))
            .collect();
        out.sort_by_key(|o| o.0);
        out.into_iter().map(|o| o.1).collect()
    })
}

fn token_rules(on: &BTreeMap<String, On>, fix: bool, pending: bool) -> Vec<Rule> {
    (on.iter())
        .filter_map(|(id, o)| match lint_plan::engine(id) {
            Some(Engine::Token(fixer)) if (!fix && !pending) || (fix && o.fix != pending) => {
                Some(Rule {
                    name: fixer.to_string(),
                    options: o.options.clone(),
                })
            }
            _ => None,
        })
        .collect()
}

fn static_id(rule: &str) -> &'static str {
    (lint_plan::native_rules().get(rule).copied()).unwrap_or(lint_plan::UNUSED_IGNORE)
}

fn filter_problems(
    on: &BTreeMap<String, On>,
    rel: &str,
    source: &str,
    problems: &mut Vec<Problem>,
) {
    problems.retain(|p| on.contains_key(&p.rule));
    for p in problems.iter_mut() {
        let o = &on[&p.rule];
        p.level = o.level;
        if !o.fix {
            p.fix = None;
        }
    }
    let mut ignores = Ignores::parse(source);
    if ignores.is_empty() {
        return;
    }
    ignores.retain(problems, &LineIndex::new(source));
    let runs = |id: &str, native: bool| {
        (on.keys()).any(|r| {
            covers(id, r)
                && match lint_plan::engine(r) {
                    Some(Engine::Native) => native,
                    Some(Engine::Token(_)) => !native,
                    _ => false,
                }
        })
    };
    if let Some(o) = on.get(lint_plan::UNUSED_IGNORE) {
        for u in ignores.unused(source, |id| runs(id, true) && !runs(id, false)) {
            problems.push(Problem {
                rule: lint_plan::UNUSED_IGNORE.into(),
                path: rel.into(),
                span: u.span,
                message: format!("`{}` suppresses no problem here", u.id),
                help: Some("remove it".into()),
                level: o.level,
                fix: o.fix.then_some(u.fix),
            });
        }
    }
    problems.extend((ignores.malformed.iter()).map(|m| crate::analyse::ignore_problem(rel, m)));
    problems.sort_by_key(|p| p.span.start);
}

const FIX_ROUNDS: usize = 10;

/// Whether the run formats and whether it lints `path`.
fn scopes(e: &Engines, path: &PathBuf) -> (bool, bool) {
    let in_format = match &e.format_files {
        Some(f) => f.contains(path),
        None => !matches!(e.stage, Stage::Lint { .. }),
    };
    let in_lint = e.lint_files.as_ref().is_some_and(|f| f.contains(path));
    (in_format, in_lint)
}

/// `process_file` through the result cache: a file the cache holds has the cached result without a check, and the
/// result of a check goes into the cache, unless the check wrote the file, failed to run or printed a note.
fn process_cached(
    a: &FormatArgs,
    e: &Engines,
    cache: &Cache,
    dry_run: bool,
    path: &PathBuf,
    code: Vec<u8>,
) -> Result<Processed> {
    if code.is_empty() || path.is_symlink() {
        return process_file(a, e, dry_run, path, Ok(code));
    }
    let rel = path.strip_prefix(e.root).unwrap_or(path).to_string_lossy();
    let content = cache::Key::default()
        .add("path", path.as_os_str().as_encoded_bytes())
        .add("scopes", format!("{:?}", scopes(e, path)))
        .add("code", &code)
        .digest();
    if let Some(found) = cache.hit(&rel, &content) {
        let result = if found.is_empty() {
            Some(FileResult::new(path.clone(), rel.to_string()))
        } else {
            cache::decode(&found, path.clone(), rel.to_string(), &code)
        };
        if let Some(result) = result {
            return Ok(Processed {
                result,
                remaining: None,
            });
        }
    }
    let (processed, reads) =
        pun_lint::rules::facts::tracking(|| process_file(a, e, dry_run, path, Ok(code.clone())));
    if let Ok(p) = &processed {
        let r = &p.result;
        let kept = match r.status {
            Status::NoChanges => true,
            Status::Fixed => dry_run,
            _ => false,
        };
        if kept && r.message.is_empty() && p.remaining.is_none() && !reads.noted {
            let clean = r.status == Status::NoChanges && r.problems.is_empty();
            let found = if clean {
                vec![]
            } else {
                cache::encode(r, &code)
            };
            cache.keep(&rel, &content, reads.project, found);
        }
    }
    processed
}

#[expect(
    clippy::too_many_lines,
    reason = "long; to be split (code audit: functions over 100 lines)"
)]
fn process_file(
    a: &FormatArgs,
    e: &Engines,
    dry_run: bool,
    path: &PathBuf,
    code: std::io::Result<Vec<u8>>,
) -> Result<Processed> {
    let rel = path
        .strip_prefix(e.root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned();
    let mut r = FileResult::new(path.clone(), rel.clone());
    let done = |result: FileResult| Processed {
        result,
        remaining: None,
    };
    if path.is_symlink() {
        r.status = Status::Link;
        return Ok(done(r));
    }
    let code = code.map_err(|_| crate::pint_error::PintError::Read(path.clone()))?;
    if code.is_empty() {
        r.status = Status::Skipped;
        return Ok(done(r));
    }
    let (in_format, in_lint) = scopes(e, path);
    let on = match (in_lint, &e.plan) {
        (true, Some(plan)) => plan.for_file(path),
        _ => BTreeMap::new(),
    };
    let mut file_ignores = match std::str::from_utf8(&code).ok() {
        Some(t) if !on.is_empty() => Ignores::parse(t),
        _ => Ignores::default(),
    };
    let token_on: BTreeMap<String, On> = (on.iter())
        .filter(|(id, _)| matches!(lint_plan::engine(id), Some(Engine::Token(_))))
        .filter(|(id, _)| !file_ignores.ignores_file(id))
        .map(|(id, o)| (id.clone(), o.clone()))
        .collect();
    let fix = e.stage.fix();
    let unchanged_on_disk = dry_run && !fix;
    let rel_path = Path::new(&rel);

    let mut now = code.clone();
    let mut applied: Vec<&'static str> = vec![];
    let mut problems: Vec<Problem> = vec![];
    let native_on = (on.keys()).any(|id| {
        matches!(
            lint_plan::engine(id),
            Some(Engine::Native | Engine::Builtin)
        )
    });
    let token = token_rules(&token_on, fix, false);
    let rounds = if fix && !on.is_empty() { FIX_ROUNDS } else { 1 };
    for round in 1..=rounds {
        let before = (round == 2).then(|| now.clone());
        let format = in_format && round != 2;
        let mut native_syntax_error = false;
        let text = std::str::from_utf8(&now).ok().map(str::to_owned);
        if let (Some(linter), None, true) = (&e.linter, &text, native_on) {
            // A file that isn't UTF-8 is still checked, on its lossy text, so its rules can't pass it unread. Their
            // fixes would write that text back, so none is offered.
            let lossy = String::from_utf8_lossy(&now);
            match linter.check(rel_path, &lossy) {
                Ok(mut ps) => {
                    filter_problems(&on, &rel, &lossy, &mut ps);
                    for p in &mut ps {
                        p.fix = None;
                    }
                    problems = ps;
                }
                Err(pun_lint::LintError::Syntax { .. }) => native_syntax_error = true,
                Err(err) => {
                    r.status = Status::Lint;
                    r.message = err.to_string();
                    return Ok(done(r));
                }
            }
        }
        if let (Some(linter), Some(text), true) = (&e.linter, text.as_deref(), native_on) {
            let filter = |src: &str, ps: &mut Vec<Problem>| filter_problems(&on, &rel, src, ps);
            let outcome = if fix {
                linter.fix_with(rel_path, text, filter).map(|f| {
                    for p in &f.applied {
                        let id = static_id(&p.rule);
                        if !applied.contains(&id) {
                            applied.push(id);
                        }
                    }
                    now = f.source.into_bytes();
                    f.remaining
                })
            } else {
                linter.check(rel_path, text).map(|mut ps| {
                    filter_problems(&on, &rel, text, &mut ps);
                    ps
                })
            };
            match outcome {
                Ok(ps) => problems = ps,
                Err(pun_lint::LintError::Syntax { .. }) => native_syntax_error = true,
                Err(err) => {
                    r.status = Status::Lint;
                    r.message = err.to_string();
                    return Ok(done(r));
                }
            }
        }

        let token_changed = if format || !token.is_empty() {
            let pipeline = e.pipelines.get(format, &token)?;
            let outcome = match isolated(|| run_file(&pipeline, path, &now)) {
                Ok(run) => run?,
                Err(message) => {
                    r.status = Status::Exception;
                    r.message = message;
                    return Ok(done(r));
                }
            };
            match outcome {
                Err(err) => {
                    r.status = if err.after_fix || round > 1 {
                        Status::Lint
                    } else {
                        Status::Invalid
                    };
                    r.message = crate::console::carry(&err.message);
                    return Ok(done(r));
                }
                Ok(None) => false,
                Ok(Some((new, fixers))) => {
                    for fixer in fixers {
                        if !applied.contains(&fixer) {
                            applied.push(fixer);
                        }
                    }
                    if !unchanged_on_disk
                        && !problems.is_empty()
                        && let (Some(linter), Ok(t)) = (&e.linter, std::str::from_utf8(&new))
                        && let Ok(mut ps) = linter.check(rel_path, t)
                    {
                        filter_problems(&on, &rel, t, &mut ps);
                        problems = ps;
                    }
                    now = new;
                    true
                }
            }
        } else {
            if native_syntax_error && let Err(err) = pun_format::lint(&code) {
                r.status = Status::Invalid;
                r.message = crate::console::carry(&err.message);
                return Ok(done(r));
            }
            false
        };
        let again = match before {
            Some(before) => before != now,
            None => token_changed,
        };
        if !again {
            break;
        }
    }
    let warned: Vec<&'static str> = (token_on.iter())
        .filter(|(_, o)| o.level == pun_lint::problem::Level::Warning)
        .filter_map(|(id, _)| lint_plan::token_rules().get(id.as_str()).copied())
        .collect();
    r.warned.clone_from(&warned);

    let is_format = |n: &&str| pun_config::rule_of(n).unwrap_or(n).starts_with("format/");
    if dry_run
        && in_format
        && matches!(e.stage, Stage::Check { .. })
        && now != code
        && applied.iter().any(|n| !is_format(n))
    {
        // A fixer that undoes another's no-op change (unary spacing, then the space after `!`) counts as applied
        // once a lint fix changes the file, so the file needs formatting only if the format rules alone change it.
        let p = e.pipelines.get(true, &[])?;
        let formatted = isolated(|| run_file(&p, path, &code))
            .ok()
            .transpose()?
            .and_then(Result::ok)
            .flatten();
        let formatted: Vec<&str> = formatted.into_iter().flat_map(|(_, f)| f).collect();
        applied.retain(|n| !is_format(n) || formatted.contains(n));
        for n in formatted {
            if !applied.contains(&n) {
                applied.push(n);
            }
        }
    }

    let diff = |old: &[u8], new: &[u8]| shown_diff(a, path, rel_path, old, new);
    if now != code {
        r.diff = diff(&code, &now);
        r.line = crate::github::first_changed_line(&code, &now);
        r.applied = applied;
        r.status = Status::Fixed;
        if !dry_run {
            replace(path, &now).map_err(|_| crate::pint_error::PintError::Write(path.clone()))?;
        }
    }
    if !problems.is_empty() {
        r.source =
            String::from_utf8_lossy(if unchanged_on_disk { &code } else { &now }).into_owned();
        r.problems = problems;
    }

    let pending = if fix {
        token_rules(&token_on, true, true)
    } else {
        vec![]
    };
    let left = if pending.is_empty() {
        None
    } else {
        let p = e.pipelines.get(false, &pending)?;
        isolated(|| run_file(&p, path, &now))
            .ok()
            .transpose()?
            .and_then(Result::ok)
            .flatten()
    };
    let remaining = left.map(|(new, fixers)| {
        let mut f = FileResult::new(path.clone(), rel.clone());
        f.diff = diff(&now, &new);
        f.line = crate::github::first_changed_line(&now, &new);
        f.applied = fixers;
        f.status = Status::Fixed;
        f.warned = warned;
        f
    });
    Ok(Processed {
        result: r,
        remaining,
    })
}

/// Swaps `bytes` in for the file at `path` through a private sibling that takes the file's group and mode before the
/// rename. Without such a sibling (a read-only directory, or a group the sibling can't take) the file is rewritten in
/// place, which keeps its inode and so its owner, group and mode.
fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::MetadataExt;
    let target = std::fs::OpenOptions::new().write(true).open(path)?;
    let meta = target.metadata()?;
    drop(target);
    let Ok((tmp, mut file)) = private_sibling(path) else {
        return std::fs::write(path, bytes);
    };
    let grouped = file.metadata().is_ok_and(|m| m.gid() == meta.gid())
        || std::os::unix::fs::fchown(&file, None, Some(meta.gid())).is_ok();
    if !grouped {
        drop(file);
        drop(std::fs::remove_file(&tmp));
        return std::fs::write(path, bytes);
    }
    let written = (file.write_all(bytes))
        .and_then(|()| file.set_permissions(meta.permissions()))
        .and_then(|()| std::fs::rename(&tmp, path));
    written.inspect_err(|_| drop(std::fs::remove_file(&tmp)))
}

/// Writes a file Pun generates (a report, a baseline) at `path`, unless `path` is a symlink in the project (the
/// current directory) that leads out of it, or nowhere: a checkout can carry such a link to aim the write elsewhere.
/// A link outside the project, such as `/dev/stdout`, is the caller's own choice and is written through.
pub(crate) fn write_generated(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let root = std::env::current_dir()?.canonicalize()?;
    let abs = root.join(path);
    let linked = abs
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink());
    let in_project =
        (abs.parent().and_then(|p| p.canonicalize().ok())).is_some_and(|p| p.starts_with(&root));
    if linked && in_project && crate::finder::leaves(&root, &abs) {
        return Err(std::io::Error::other(
            "it is a symlink that leads outside the project",
        ));
    }
    std::fs::write(path, bytes)
}

/// A new file beside `path` that only its owner can open, under a random name. It is created exclusively, so an entry
/// already at that name (a planted symlink included) fails the call instead of receiving the write.
pub(crate) fn private_sibling(path: &Path) -> std::io::Result<(PathBuf, std::fs::File)> {
    use std::hash::{BuildHasher, Hasher};
    use std::os::unix::fs::OpenOptionsExt;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let random = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let tmp = path.with_file_name(format!(".{name}.pun-{}-{random:016x}", std::process::id()));
    let file = (std::fs::OpenOptions::new().write(true).create_new(true))
        .mode(0o600)
        .open(&tmp)?;
    Ok((tmp, file))
}

fn add_findings(results: &mut Vec<FileResult>, analysed: &crate::analyse::Analysed, root: &Path) {
    let mut at: HashMap<PathBuf, usize> = (results.iter().enumerate())
        .map(|(i, r)| (r.path.clone(), i))
        .collect();
    let mut index_of = |results: &mut Vec<FileResult>, file: &Path| -> usize {
        let i = *at.entry(file.to_path_buf()).or_insert_with(|| {
            let rel = file.strip_prefix(root).unwrap_or(file).to_string_lossy();
            results.push(FileResult::new(file.to_path_buf(), rel.into_owned()));
            results.len() - 1
        });
        let r = &mut results[i];
        if r.source.is_empty() {
            r.source =
                String::from_utf8_lossy(&std::fs::read(file).unwrap_or_default()).into_owned();
        }
        i
    };
    // One line index per file, not per finding: a file can have thousands of findings.
    let mut indexes: HashMap<usize, LineIndex> = HashMap::new();
    for f in &analysed.findings {
        let i = index_of(results, f.source());
        let r = &mut results[i];
        if f.rule == "parser-error" && matches!(r.status, Status::Invalid | Status::Lint) {
            continue;
        }
        let index = indexes
            .entry(i)
            .or_insert_with(|| LineIndex::new(&r.source));
        let p = crate::analyse::problem(f, &r.source, index);
        r.problems.push(p);
    }
    for m in &analysed.malformed {
        let i = index_of(results, &root.join(&m.path));
        let r = &mut results[i];
        if !r
            .problems
            .iter()
            .any(|q| q.rule == m.rule && q.span == m.span)
        {
            r.problems.push(m.clone());
        }
    }
    for r in results.iter_mut() {
        r.problems.sort_by_key(|p| p.span.start);
    }
}

fn pipeline(cfg: &Config) -> Result<Pipeline> {
    let mut p = Pipeline::new(&cfg.rules, &crate::extensions::fixers)?;
    p.set_preset(&cfg.preset);
    if std::env::var_os("PUN_PROFILE").is_some() {
        p.enable_profile();
    }
    Ok(p)
}

fn unported_error(a: &FormatArgs, cfg: &Config, names: &[String]) -> Abort {
    let ids: Vec<String> = (names.iter())
        .map(|n| format!("`{}`", pun_config::rule_of(n).unwrap_or(n)))
        .collect();
    let file = (cfg.root).join(a.config.as_deref().unwrap_or(Path::new("pun.toml")));
    let rules = if ids.len() == 1 { "rule" } else { "rules" };
    let what = format!("{rules} not supported by Pun yet: {}", ids.join(", "));
    let rel = file.strip_prefix(&cfg.root).unwrap_or(&file).to_path_buf();
    let them = if ids.len() == 1 { "it" } else { "them" };
    let error = RunError {
        hint: Some(format!(
            "remove {them} from {} to run the other rules",
            rel.display()
        )),
        path: Some(rel),
        line: None,
        message: what.clone(),
    };
    let mut abort = Abort::pun(format!("{}: {what}", file.display()), error);
    abort.pint = true;
    abort
}

fn explain(cfg: &Config, p: &Pipeline) {
    let mut err = std::io::stderr().lock();
    let _ = writeln!(
        err,
        "native (PHP {}), preset {}, {} rules",
        cfg.php,
        cfg.preset,
        cfg.rules.len()
    );
    for f in p.fixers() {
        let _ = writeln!(err, "  native      {:>4}  {}", f.priority(), f.name());
    }
}

fn is_blade(path: &Path) -> bool {
    path.to_string_lossy().ends_with(".blade.php")
}

fn stdin(a: &FormatArgs, cfg: &Config, engine: &Pipeline) -> Result<Exit> {
    let mut code = Vec::new();
    std::io::stdin().read_to_end(&mut code)?;
    let mut out = std::io::stdout().lock();
    let ctx = a
        .stdin_filename
        .clone()
        .unwrap_or_else(|| "stdin.php".into());
    let fixed = if a.test {
        None
    } else {
        fix_text(cfg, engine, &ctx, a.stdin_filename.is_some(), &code)?
    };
    out.write_all(fixed.as_deref().unwrap_or(&code))?;
    Ok(Exit::Success)
}

fn fix_text(
    cfg: &Config,
    engine: &Pipeline,
    path: &Path,
    check_excluded: bool,
    code: &[u8],
) -> Result<Option<Vec<u8>>> {
    if check_excluded && crate::finder::is_path_excluded(cfg, path) {
        return Ok(None);
    }
    let Ok(run) = isolated(|| run_file(engine, path, code)) else {
        return Ok(None);
    };
    Ok(run?.ok().flatten().map(|(new, _)| new))
}

thread_local! {
    static ISOLATED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn isolated<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !ISOLATED.get() {
                default(info);
            }
        }));
    });
    ISOLATED.set(true);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    ISOLATED.set(false);
    r.map_err(|p| {
        p.downcast_ref::<&str>()
            .map(std::string::ToString::to_string)
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn isolated_panics() {
        assert_eq!(
            super::isolated(|| panic!("boom")),
            Err::<(), _>("boom".into())
        );
        // A formatted message (a `String` payload, as the Blade fixer's and the preg exceptions' are).
        let n = 2;
        assert_eq!(
            super::isolated(|| panic!("boom {n}")),
            Err::<(), _>("boom 2".into())
        );
    }

    #[test]
    fn replace_swaps_the_file_in() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("pun-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.php");
        let mode = |m| std::fs::Permissions::from_mode(m);
        let names = || -> Vec<_> {
            let entries = std::fs::read_dir(&dir).unwrap();
            entries.map(|e| e.unwrap().file_name()).collect()
        };
        std::fs::write(&file, "old").unwrap();
        std::fs::set_permissions(&file, mode(0o640)).unwrap();
        super::replace(&file, b"new").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"new");
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(names(), ["a.php"]);

        std::fs::set_permissions(&dir, mode(0o555)).unwrap();
        super::replace(&file, b"in place").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"in place");
        std::fs::set_permissions(&dir, mode(0o755)).unwrap();

        std::fs::set_permissions(&file, mode(0o444)).unwrap();
        assert!(super::replace(&file, b"refused").is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"in place");
        assert_eq!(names(), ["a.php"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The staging file is new, owner-only and never an entry that was already there: a symlink planted at the name
    /// staging once used (`.a.php.pun-<pid>`) leaves its target alone, and the sibling starts at 0600 whatever the mode
    /// of the file it stands in for.
    #[test]
    fn replace_stages_in_a_new_private_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("pun-stage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (file, outside) = (dir.join("a.php"), dir.join("outside"));
        std::fs::write(&file, "old").unwrap();
        std::fs::write(&outside, "keep").unwrap();
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o600)).unwrap();
        let planted = dir.join(format!(".a.php.pun-{}", std::process::id()));
        std::os::unix::fs::symlink(&outside, &planted).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        super::replace(&file, b"new").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"new");
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&outside), 0o600);
        assert_eq!(mode(&file), 0o644);
        let (tmp, _file) = super::private_sibling(&file).unwrap();
        assert_eq!(mode(&tmp), 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// In a setgid directory a new file takes the directory's group; the replacement keeps the old file's group, so
    /// the mode bits it copies still apply to the group they applied to. Needs a second group to put the directory in.
    #[test]
    fn replace_keeps_the_group_in_a_setgid_directory() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = std::env::temp_dir().join(format!("pun-group-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.php");
        std::fs::write(&file, "old").unwrap();
        let own = std::fs::metadata(&file).unwrap().gid();
        let ids = std::process::Command::new("id").arg("-G").output().unwrap();
        let other = String::from_utf8_lossy(&ids.stdout)
            .split_whitespace()
            .filter_map(|g| g.parse::<u32>().ok())
            .find(|&g| g != own);
        let Some(other) = other else {
            std::fs::remove_dir_all(&dir).unwrap();
            return;
        };
        std::os::unix::fs::chown(&dir, None, Some(other)).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o2755)).unwrap();
        assert_eq!(std::fs::metadata(dir.join(".")).unwrap().gid(), other);
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
        super::replace(&file, b"new").unwrap();
        let meta = std::fs::metadata(&file).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"new");
        assert_eq!(
            (meta.gid(), meta.permissions().mode() & 0o7777),
            (own, 0o640)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
