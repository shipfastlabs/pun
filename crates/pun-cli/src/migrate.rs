use crate::render::{ColorChoice, Look, Role};
use crate::{Exit, agent, report, root};
use anyhow::{Context, Result, bail};
use pcre2::bytes::Regex;
use pun_config::migrate;
use std::fmt::Write as _;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

#[derive(clap::Args, Default)]
pub(crate) struct MigrateArgs {
    /// Print what would change, as a diff, and write nothing.
    #[arg(long)]
    dry_run: bool,
    /// The pint.json to read (default: ./pint.json).
    #[arg(long)]
    config: Option<PathBuf>,
    /// The preset to use instead of pint.json's, as `pint --preset` does.
    #[arg(long)]
    preset: Option<String>,
    /// Migrate only these, without asking (comma-separated).
    #[arg(long, value_enum, value_delimiter = ',')]
    from: Vec<Source>,
    /// Don't ask: migrate everything detected.
    #[arg(long, short)]
    yes: bool,
    /// On a terminal, offer to replace the analyse rules migrated from phpstan.neon with the closest Pun preset,
    /// showing how many problems that adds and removes.
    #[arg(long)]
    offer_preset: bool,
}

/// `FixerAnnotationAnalyzer`'s pattern, applied inside comment tokens only. Exactly one space after the name: it
/// splits there, so any other gap is an annotation upstream rejects, left for `pun format` to report.
static IGNORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^(\h*[*\/]+\h+@)php-cs-fixer-ignore ((?:@?[\w\/,])+)").unwrap()
});

/// `@phpstan-ignore id (comment), id`, as PHPStan's `IgnoreLexer` reads it (identifiers, each with an optional
/// parenthesised comment, comma-separated), up to the end of the comment or its line. `-line` and `-next-line` are
/// not this tag.
static PHPSTAN_IGNORE: LazyLock<Regex> = LazyLock::new(|| {
    let id = r"[\w.\-\\]+(?:\h*\([^()\n]*\))?";
    Regex::new(&format!(
        r"(?m)@phpstan-ignore(?![-\w])\h+({id}(?:\h*,\h*{id})*)(?=\h*(?:\*/|$))"
    ))
    .unwrap()
});

/// PHPStan's ignores without identifiers: every error of a line.
static PHPSTAN_IGNORE_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"@phpstan-ignore-(?:next-)?line\b").unwrap());

/// A docblock tag, as PHPStan finds the last one in a docblock: `@phpstan-ignore-next-line` ignores the line after the
/// docblock only when no tag follows it.
static PHPDOC_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)@(?:[a-z][a-z0-9-\\]+:)?[a-z][a-z0-9-\\]*|@[a-z_\\][a-z0-9_:\\]*[a-z_][a-z0-9_]*",
    )
    .unwrap()
});

#[derive(Clone, Copy, PartialEq, Debug, clap::ValueEnum)]
pub(crate) enum Source {
    Pint,
    PhpCsFixer,
    Phpstan,
    Rector,
    Mago,
}

impl Source {
    const ALL: [Source; 5] = [
        Source::Pint,
        Source::PhpCsFixer,
        Source::Phpstan,
        Source::Rector,
        Source::Mago,
    ];

    fn name(self) -> &'static str {
        ["Pint", "PHP-CS-Fixer", "PHPStan", "Rector", "Mago"][self as usize]
    }

    /// Its config files, in the order the tool looks for them. Pint's is `--config` when given.
    fn files(self) -> &'static [&'static str] {
        match self {
            Source::Pint => &["pint.json"],
            Source::PhpCsFixer => &[".php-cs-fixer.php", ".php-cs-fixer.dist.php"],
            Source::Phpstan => &["phpstan.neon", "phpstan.neon.dist", "phpstan.dist.neon"],
            Source::Rector => &["rector.php"],
            Source::Mago => &["mago.toml"],
        }
    }

    /// Pint and PHP-CS-Fixer give `[format]`, Rector `[lint]`, PHPStan `[analyse]`; Mago isn't migrated yet.
    fn supported(self) -> bool {
        matches!(
            self,
            Source::Pint | Source::PhpCsFixer | Source::Rector | Source::Phpstan
        )
    }

    /// Gives `[format]`.
    fn format(self) -> bool {
        matches!(self, Source::Pint | Source::PhpCsFixer)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Action {
    Migrate,
    /// Selected, but Pun can't migrate this tool yet: left untouched.
    Unsupported,
    NotSelected,
    /// PHP-CS-Fixer selected together with Pint: pint.json gives `[format]`, as Pint itself ignores the other.
    Covered,
    /// rector.php selected without `vendor/rector/rector`: Rector's config is PHP that only Rector can read.
    NoVendor,
}

/// The detected configs (tool, file) and the chosen tools (a subset of them) → what happens to each, in detection order.
fn plan(detected: &[(Source, String)], chosen: &[Source]) -> Vec<(Source, String, Action)> {
    let pint = chosen.contains(&Source::Pint);
    (detected.iter())
        .map(|(s, f)| {
            let action = match s {
                _ if !chosen.contains(s) => Action::NotSelected,
                Source::PhpCsFixer if pint => Action::Covered,
                _ if s.supported() => Action::Migrate,
                _ => Action::Unsupported,
            };
            (*s, f.clone(), action)
        })
        .collect()
}

/// The configs found in `root`, one per tool: (tool, file). `pun init` reads it too.
pub(crate) fn detect(root: &Path, pint: &Path) -> Vec<(Source, String)> {
    let pint = pint.display().to_string();
    (Source::ALL.iter())
        .filter_map(|&s| {
            let files = if s == Source::Pint {
                &[pint.as_str()][..]
            } else {
                s.files()
            };
            let f = files.iter().find(|f| root.join(f).exists())?;
            Some((s, f.to_string()))
        })
        .collect()
}

/// The checklist: every detected config, all checked. Esc or q selects nothing.
fn ask(detected: &[(Source, String)]) -> Result<Vec<Source>> {
    let width = detected.iter().map(|d| d.1.len()).max().unwrap_or(0);
    let items: Vec<String> = (detected.iter())
        .map(|(s, f)| {
            let note = if s.supported() {
                ""
            } else {
                "  (not supported yet)"
            };
            format!("{:<13} {f:<width$}{note}", s.name())
                .trim_end()
                .to_string()
        })
        .collect();
    let picked = dialoguer::MultiSelect::with_theme(&crate::init::theme())
        .with_prompt("Migrate which configs? (space toggles, enter confirms)")
        .items(&items)
        .defaults(&vec![true; items.len()])
        .report(false)
        .interact_on_opt(&dialoguer::console::Term::stdout())?;
    Ok(picked
        .unwrap_or_default()
        .into_iter()
        .map(|i| detected[i].0)
        .collect())
}

pub(crate) fn run(a: &MigrateArgs) -> Result<Exit> {
    let root = root()?;
    let pint = a.config.as_deref().unwrap_or(Path::new("pint.json"));
    let detected = detect(&root, pint);
    check_from(&a.from, &detected, pint)?;
    let interactive = !(a.yes || !a.from.is_empty() || agent::enabled())
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal();
    let mut report = Report::new(!agent::enabled(), a.dry_run);
    let nothing = format!(
        "no {}, .php-cs-fixer.php, phpstan.neon, rector.php or mago.toml found",
        pint.display()
    );
    let chosen = if !a.from.is_empty() {
        a.from.clone()
    } else if interactive && !detected.is_empty() {
        ask(&detected)?
    } else {
        detected.iter().map(|d| d.0).collect()
    };
    if detected.is_empty() && report.human {
        println!("N{}.", &nothing[1..]);
        if interactive && !starter()? {
            report.finish(Mark::Skip, "Nothing written".into(), &[], "");
            return Ok(Exit::Success);
        }
    }
    let rows = without_vendor(&root, plan(&detected, &chosen));
    let sources = Sources::of(&root, &rows);
    let formats: Vec<&str> = (detected.iter())
        .filter(|d| d.0.format())
        .map(|d| d.1.as_str())
        .collect();
    // A format config left out (the project keeps its formatter): pun.toml gets no [format], which means Pint's
    // defaults, as Pint without a pint.json.
    let no_format = sources.format.is_none() && !formats.is_empty();
    let migrates = rows.iter().any(|r| r.2 == Action::Migrate);
    if (chosen.is_empty() && !detected.is_empty()) || (no_format && !migrates) {
        hold(report, &rows, pint, chosen.is_empty());
        return Ok(Exit::Success);
    }
    let from = sources.format.as_ref().map(|s| rel(&root, &s.1));
    let arrow = report.look.glyph("→", "->");
    let body = body(&root, a, pint, &sources, no_format, &mut report)?;
    let header = match &from {
        Some(f) => format!(" from {f}."),
        None if no_format => format!(" without [format]: {} not selected.", formats.join(", ")),
        None if detected.is_empty() => format!(": {nothing}; starter config."),
        None => format!(
            ": no {} or .php-cs-fixer.php found; starter config.",
            pint.display()
        ),
    };
    let toml = format!(
        "# Written by `pun migrate`{header}\n{}",
        body.migration.toml
    );
    let path = root.join("pun.toml");
    if crate::finder::leaves(&root, &path) {
        bail!(
            "pun.toml is a symlink that leads outside the project; migrate doesn't read or write through it. Remove it, then run `pun migrate` again."
        );
    }
    // Not UTF-8 is still a pun.toml that differs, not one to overwrite.
    let old = std::fs::read(&path)
        .ok()
        .map(|old| String::from_utf8_lossy(&old).into_owned());
    if old.as_ref().is_some_and(|o| *o != toml) && !a.dry_run {
        bail!(
            "pun.toml exists and differs from what migrate writes; move it away first (`pun migrate --dry-run` shows the difference)"
        );
    }
    let cfg = pun_config::from_toml(&root, &toml)?;
    let phpstan = sources.phpstan.is_some();
    let mut edits = ignore_rewrites(&root, &cfg, !no_format, phpstan, &mut report);
    let rewritten = edits.len();
    let written = old.as_deref() != Some(&toml);
    if written {
        let old = old.unwrap_or_default().into_bytes();
        edits.insert(0, (path, old, toml.into_bytes()));
    }
    report.diffs = apply(&root, &edits, a.dry_run)?;
    let moved = body.moved(arrow);
    report.configs(&rows, pint, &moved.each_ref().map(String::as_str));
    if rewritten > 0 {
        let files = agent::count(rewritten, "file");
        report.line(
            Mark::Done,
            "ignore comments",
            match sources.phpstan {
                Some(_) => format!("{arrow} @pun-ignore in {files}"),
                None => format!("{arrow} @pun-ignore-file in {files}"),
            },
        );
    }
    let checked = match &from {
        Some(f) => format!("from {f}"),
        None if detected.is_empty() => nothing,
        None => String::new(),
    };
    let starter = from.is_none() && !no_format;
    report.summary(&cfg, starter, written, rewritten, &checked);
    Ok(Exit::Success)
}

/// A selected rector.php without `vendor/rector/rector`, or a phpstan.neon that includes from a missing `vendor/`,
/// can't be read.
fn without_vendor(
    root: &Path,
    rows: Vec<(Source, String, Action)>,
) -> Vec<(Source, String, Action)> {
    let rector_installed = root.join("vendor/rector/rector").is_dir();
    (rows.into_iter())
        .map(|(s, f, a)| match (s, a) {
            (Source::Rector, Action::Migrate) if !rector_installed => (s, f, Action::NoVendor),
            (Source::Phpstan, Action::Migrate) if needs_vendor(root, &root.join(&f)) => {
                (s, f, Action::NoVendor)
            }
            _ => (s, f, a),
        })
        .collect()
}

/// The configs migrated into each part of pun.toml.
struct Sources {
    /// Pint's or PHP-CS-Fixer's, for `[format]`.
    format: Option<(Source, PathBuf)>,
    rector: Option<PathBuf>,
    phpstan: Option<PathBuf>,
}

impl Sources {
    fn of(root: &Path, rows: &[(Source, String, Action)]) -> Sources {
        let find = |of: &dyn Fn(Source) -> bool| {
            (rows.iter())
                .find(|r| r.2 == Action::Migrate && of(r.0))
                .map(|r| (r.0, root.join(&r.1)))
        };
        Sources {
            format: find(&Source::format),
            rector: find(&|s| s == Source::Rector).map(|r| r.1),
            phpstan: find(&|s| s == Source::Phpstan).map(|r| r.1),
        }
    }
}

/// Nothing is written: nothing was selected, or nothing selected can be migrated.
fn hold(mut report: Report, rows: &[(Source, String, Action)], pint: &Path, nothing: bool) {
    let why = if nothing {
        "nothing selected"
    } else {
        "nothing to migrate"
    };
    report.configs(rows, pint, &["", "", ""]);
    let detail = [format!("{why}, pun.toml not written")];
    report.finish(
        Mark::Skip,
        format!("N{}; pun.toml not written", &why[1..]),
        &detail,
        "",
    );
}

/// pun.toml's tooling part, and what rector.php and phpstan.neon became.
struct Body {
    migration: migrate::Migration,
    lint: String,
    analyse: String,
}

impl Body {
    /// What each config's line says it became: the format config's, rector.php's and phpstan.neon's.
    fn moved(&self, arrow: &str) -> [String; 3] {
        let m = &self.migration;
        let mut moved = format!(
            "{arrow} [format] preset {}, {} changed",
            m.preset,
            agent::count(m.format_rules, "rule")
        );
        if m.lint_rules > 0 {
            let lint = agent::count(m.lint_rules, "rule");
            let _ = write!(moved, ", {lint} {arrow} [lint]");
        }
        [
            moved,
            // rector.php's sets line starts under `[lint]`.
            format!(
                "{arrow} {}",
                (self.lint).replace('\n', &format!("\n{:w$}", "", w = arrow.chars().count() + 1))
            ),
            format!("{arrow} {}", self.analyse),
        ]
    }
}

/// `[format]` from the format config (none with `no_format`), then `[lint]` from rector.php and `[analyse]` from
/// phpstan.neon.
fn body(
    root: &Path,
    a: &MigrateArgs,
    pint: &Path,
    sources: &Sources,
    no_format: bool,
    report: &mut Report,
) -> Result<Body> {
    let mut m = if no_format {
        migrate::Migration {
            toml: String::new(),
            parts: migrate::Parts::default(),
            unsupported: vec![],
            preset: "laravel",
            format_rules: 0,
            lint_rules: 0,
        }
    } else {
        convert(root, a, pint, sources.format.as_ref(), report)?
    };
    m.parts.extensions = migrate::extensions(&pun_config::composer::detect(root)?);
    m.toml = m.parts.render();
    let mut lint = String::new();
    if let Some(r) = &sources.rector {
        lint = rector_lint(root, r, &mut m.parts, report)?;
        m.toml = m.parts.render();
    }
    let mut analyse = String::new();
    if let Some(p) = &sources.phpstan {
        let level;
        (analyse, level) = phpstan_analyse(root, p, &mut m.parts, report)?;
        let terminal = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
        if a.offer_preset
            && terminal
            && !agent::enabled()
            && let Some(plain) = closest_preset(root, &m.parts, level)?
        {
            m.parts = plain;
            analyse.push_str(", switched to the preset");
        }
        m.toml = m.parts.render();
    }
    Ok(Body {
        migration: m,
        lint,
        analyse,
    })
}

/// `--from` names only detected configs.
fn check_from(from: &[Source], detected: &[(Source, String)], pint: &Path) -> Result<()> {
    for s in from {
        if !detected.iter().any(|d| d.0 == *s) {
            let files = if *s == Source::Pint {
                vec![pint.display().to_string()]
            } else {
                s.files()
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect()
            };
            bail!(
                "--from {}: no {} found",
                s.name().to_lowercase(),
                files.join(" or ")
            );
        }
    }
    Ok(())
}

/// No config at all, on a terminal: whether to write the starter pun.toml.
fn starter() -> Result<bool> {
    let yes = dialoguer::Confirm::with_theme(&crate::init::theme())
        .with_prompt("Write a starter pun.toml?")
        .default(true)
        .interact_on_opt(&dialoguer::console::Term::stdout())?;
    Ok(yes == Some(true))
}

/// pun.toml's body from the config that gives `[format]` (`source`), or from Pint's defaults (its laravel preset, or
/// `--preset`) when there is none. Pun runs a rule only with a native fixer; the rules it skips are reported.
fn convert(
    root: &Path,
    a: &MigrateArgs,
    pint: &Path,
    source: Option<&(Source, PathBuf)>,
    report: &mut Report,
) -> Result<migrate::Migration> {
    let fixers: std::collections::HashSet<&str> = crate::extensions::fixers()
        .iter()
        .map(|f| f.name())
        .collect();
    let runs = |name: &str| fixers.contains(name);
    let m = match source {
        Some((Source::PhpCsFixer, f)) => {
            let (rules, finder) = php_cs_fixer_config(root, f)?;
            migrate::from_rules(rules, &finder, &runs)?
        }
        Some((_, f)) => migrate::from_pint(f, root, a.preset.as_deref(), &runs)?,
        None => migrate::from_pint(&root.join(pint), root, a.preset.as_deref(), &runs)?,
    };
    let from = match source {
        Some((_, f)) => rel(root, f),
        None => format!("preset {}", a.preset.as_deref().unwrap_or("laravel")),
    };
    for u in &m.unsupported {
        let (name, why) = u.split_once(": ").unwrap();
        report.unsupported(name, why, format!("{from} unsupported {u}"));
    }
    Ok(m)
}

/// Each project file whose `@php-cs-fixer-ignore` comments (when the format config is migrated: a project that keeps
/// PHP-CS-Fixer keeps them), and, when the PHPStan config is migrated, `@phpstan-ignore` comments, in the files
/// `pun analyse` checks too, [`rewrite`] changes: (path, old, new). An annotation Pun has no rule for is reported.
fn ignore_rewrites(
    root: &Path,
    cfg: &pun_config::Config,
    fixer: bool,
    phpstan: bool,
    report: &mut Report,
) -> Vec<(PathBuf, Vec<u8>, Vec<u8>)> {
    let mut edits = vec![];
    let mut unknown = vec![];
    let mut files = crate::finder::discover(cfg, &[]).unwrap_or_default();
    let analysed: std::collections::HashSet<PathBuf> = if phpstan {
        let analysed = crate::finder::files(cfg, Some(crate::finder::Command::Analyse), &[]);
        analysed.unwrap_or_default().into_iter().collect()
    } else {
        std::collections::HashSet::new()
    };
    files.extend(analysed.iter().cloned());
    // A file outside the project (an absolute analyse path, or one a symlink leads to) isn't the project's to edit, and
    // `pun format` skips a symlinked file.
    files.retain(|f| !f.is_symlink() && !crate::finder::leaves(root, f));
    files.sort();
    files.dedup();
    let read: Vec<(PathBuf, Vec<u8>)> = (files.into_iter())
        .filter_map(|f| Some((std::fs::read(&f).ok()?, f)))
        .map(|(code, f)| (f, code))
        .collect();
    let blanket = |code: &[u8]| PHPSTAN_IGNORE_LINE.is_match(code).unwrap_or(false);
    let covered: Vec<PathBuf> = (read.iter())
        .filter(|(f, code)| phpstan && analysed.contains(f) && blanket(code))
        .map(|(f, _)| f.clone())
        .collect();
    let hidden = if covered.is_empty() {
        Ok(std::collections::HashMap::new())
    } else {
        crate::analyse::hidden_by_line_ignores(cfg, covered)
            .map_err(|e| format!("`pun analyse` couldn't run: {e}"))
    };
    for (f, code) in read {
        let lines: Hidden = phpstan.then(|| {
            if !analysed.contains(&f) {
                return Err("`pun analyse` doesn't check this file".to_string());
            }
            let path = pun_phpstan::php::normalize_path(&f.to_string_lossy());
            let all = hidden.as_ref().map_err(Clone::clone)?;
            let mine = (all.iter()).filter(|((p, _), _)| *p == path);
            Ok(mine
                .map(|((_, line), rules)| (*line, rules.clone()))
                .collect())
        });
        if let Some(new) = rewrite(&code, &rel(root, &f), fixer, lines.as_ref(), &mut unknown) {
            edits.push((f, code, new));
        }
    }
    for (path, note) in &unknown {
        // "`@tag ids` left as is (why)": the comments that share a reason are listed under it.
        let (tag, why) = note.split_once(" left as is (").expect("an ignore note");
        let why = format!("Ignore comments left as is: {}", &why[..why.len() - 1]);
        let finding = format!("{path} ignore-not-migrated {note}");
        report.unsupported(&format!("{path}  {tag}"), &why, finding);
    }
    edits
}

/// rector.php's `[lint]` added to `parts` (context/plan/migrate.md Build 4); the ✔ line's text. Rector's paths become
/// `[lint] exclude` entries for the files `pun lint` would otherwise check outside them.
fn rector_lint(
    root: &Path,
    config: &Path,
    parts: &mut migrate::Parts,
    report: &mut Report,
) -> Result<String> {
    let mut r = migrate::from_rector(root, &rector_config(root, config)?);
    let cfg = pun_config::from_toml(root, &parts.render())?;
    let files = crate::finder::files(&cfg, Some(crate::finder::Command::Lint), &[])?;
    r.exclude.extend(outside(root, &files, &r.paths));
    let done = migrate::compose(root, parts, &r)?;
    let mut skipped = r.unsupported;
    skipped.extend(done.unsupported);
    let mut reasons: Vec<(&str, usize)> = vec![];
    for (_, why) in &skipped {
        match reasons.iter_mut().find(|(w, _)| *w == why) {
            Some((_, n)) => *n += 1,
            None => reasons.push((why, 1)),
        }
    }
    let from = rel(root, config);
    for (why, n) in &reasons {
        let rules = skipped.iter().filter(|s| s.1 == *why);
        if *n > 3 {
            let finding = format!("{from} unsupported {n} Rector rules: {why}");
            report.unsupported(&format!("{n} Rector rules"), why, finding);
        } else {
            for (class, _) in rules {
                let finding = format!("{from} unsupported {class}: {why}");
                report.unsupported(class, why, finding);
            }
        }
    }
    let mut text = format!("[lint] {} on, {} off", done.on, done.off);
    if !r.exclude.is_empty() {
        let _ = write!(text, ", {} excluded", agent::count(r.exclude.len(), "path"));
    }
    if !r.overrides.is_empty() {
        let _ = write!(
            text,
            ", {}",
            agent::count(r.overrides.len(), "per-path skip")
        );
    }
    if !r.sets.is_empty() {
        let _ = write!(text, "\nsets {}", r.sets.join(", "));
    }
    Ok(text)
}

/// Whether phpstan.neon includes a file under `vendor/` that is not there (Larastan's extension.neon before
/// `composer install`): PHPStan cannot read such a config either.
fn needs_vendor(root: &Path, config: &Path) -> bool {
    let vendor = format!("File '{}", root.join("vendor").display());
    migrate::phpstan::from_phpstan(root, config).is_err_and(|e| {
        let e = e.to_string().replace("/./", "/");
        e.contains(&vendor) && e.ends_with("is missing or is not readable.")
    })
}

/// phpstan.neon's `[analyse]` added to `parts` (context/plan/migrate.md Build 5); the ✔ line's text. Baselines are
/// not copied: each gets the note to generate a Pun one.
fn phpstan_analyse(
    root: &Path,
    config: &Path,
    parts: &mut migrate::Parts,
    report: &mut Report,
) -> Result<(String, u8)> {
    let from = rel(root, config);
    let p = migrate::phpstan::from_phpstan(root, config)
        .with_context(|| format!("can't read {from} as a PHPStan config"))?;
    let arrow = report.look.glyph("→", "->");
    let done = migrate::phpstan::compose(root, parts, &p)?;
    for (name, why) in p.unsupported.iter().chain(&done.unsupported) {
        report.unsupported(name, why, format!("{from} unsupported {name}: {why}"));
    }
    // The PHPStan checks that became lint rules report through `pun lint`, which reads its own baseline.
    let run = if done.lint > 0 {
        "`pun analyse --generate-baseline` and `pun lint --generate-baseline` for Pun baselines"
    } else {
        "`pun analyse --generate-baseline` for a Pun baseline"
    };
    for b in &p.baselines {
        report
            .notes
            .push((b.clone(), format!("was not copied. Run {run}.")));
        (report.findings.1).push(format!("{b} not-copied run {run}"));
    }
    let mut text = format!("[analyse] preset {}", done.preset);
    if done.on + done.off > 0 {
        let _ = write!(text, ", {} on, {} off", done.on, done.off);
    }
    if done.lint > 0 {
        let _ = write!(text, ", {} {arrow} [lint]", agent::count(done.lint, "rule"));
    }
    if done.options > 0 {
        let parameters = agent::count(done.options, "parameter");
        let _ = write!(text, ", {parameters} {arrow} rule options");
    }
    if let Some(v) = &p.php_version {
        let _ = write!(text, ", phpVersion {arrow} [php] version {v}");
    }
    let ignores = p.ignores.len();
    if ignores > 0 {
        let _ = write!(text, ", {} ignored", agent::count(ignores, "error pattern"));
    }
    Ok((text, p.level))
}

/// On a terminal, after the exact rules (context/plan/migrate.md §3): when `parts` turns analyse rules on or off
/// beside its preset, offer the preset alone, with the problems the switch adds and drops (both configs analysed
/// on the project). `Some` is the preset alone, when taken.
fn closest_preset(
    root: &Path,
    parts: &migrate::Parts,
    level: u8,
) -> Result<Option<migrate::Parts>> {
    let mut plain = parts.clone();
    plain
        .analyse_rules
        .retain(|(_, v)| v != "\"on\"" && v != "\"off\"");
    plain.analyse = migrate::phpstan::with_pun_preset(&parts.analyse, level);
    let exact = pun_config::from_toml(root, &parts.render())?;
    let preset = pun_config::from_toml(root, &plain.render())?;
    let found = |cfg: &pun_config::Config| -> Result<std::collections::HashSet<_>> {
        let a = crate::analyse::analyse(cfg, &[], false)?;
        Ok((a.findings.into_iter())
            .map(|f| (f.rel, f.line, f.identifier, f.message))
            .collect())
    };
    let (before, after) = (found(&exact)?, found(&preset)?);
    let new = after.difference(&before).count();
    let gone = before.difference(&after).count();
    let prompt = format!(
        "You're on PHPStan level {level}; the closest Pun preset is `{}`. Switch to it from `phpstan/level_{level}`? ({new} new problem{}, {gone} fewer)",
        preset.analyse.preset,
        if new == 1 { "" } else { "s" },
    );
    let yes = dialoguer::Confirm::with_theme(&crate::init::theme())
        .with_prompt(prompt)
        .default(false)
        .interact_on_opt(&dialoguer::console::Term::stdout())?;
    Ok((yes == Some(true)).then_some(plain))
}

/// The shallowest path (relative to `root`) holding each of `files` that no Rector path covers, once each; nothing
/// when Rector checks the whole project (`.`).
fn outside(root: &Path, files: &[PathBuf], paths: &[String]) -> Vec<String> {
    if paths.is_empty() || paths.iter().any(|p| p == ".") {
        return vec![];
    }
    let under = |a: &str, b: &str| a == b || a.starts_with(&format!("{b}/"));
    let mut out: Vec<String> = vec![];
    for f in files {
        let f = rel(root, f);
        if paths.iter().any(|p| under(&f, p)) {
            continue;
        }
        let parts: Vec<&str> = f.split('/').collect();
        let a = (1..=parts.len())
            .map(|k| parts[..k].join("/"))
            .find(|a| !paths.iter().any(|p| under(p, a)))
            .unwrap();
        if !out.contains(&a) {
            out.push(a);
        }
    }
    out
}

/// A rector.php as Rector resolves it, read through the project's own Rector (the config is PHP code). Like
/// [`php_cs_fixer_config`], migrate is the only place Pun runs PHP.
fn rector_config(root: &Path, config: &Path) -> Result<migrate::RectorConfig> {
    read_php(root, config, include_str!("rector_config.php"), &[])
        .with_context(|| format!("can't read {} with Rector", rel(root, config)))
}

/// Writes the edits, or with `--dry-run` returns their diffs instead.
fn apply(root: &Path, edits: &[(PathBuf, Vec<u8>, Vec<u8>)], dry_run: bool) -> Result<String> {
    let mut diffs = String::new();
    for (f, old, new) in edits {
        if dry_run {
            let (old, new) = (String::from_utf8_lossy(old), String::from_utf8_lossy(new));
            diffs += &report::diff_bytes(old.as_bytes(), new.as_bytes(), Path::new(&rel(root, f)));
        } else {
            std::fs::write(f, new).with_context(|| format!("can't write {}", f.display()))?;
        }
    }
    Ok(diffs)
}

/// Where unsupported items are asked for (context/plan/migrate.md §3).
const PR: &str = "https://github.com/shipfastlabs/pun";

/// A line's mark: ✔ moved or written, ⚠ named and skipped, · left alone.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Mark {
    Done,
    Warn,
    Skip,
}

/// What migrate prints (context/plan/output.md §2): one line per detected config, a ⚠ line per item it skips with
/// the PR request under them, then the last line. Agent mode gets one summary line and a finding per config and item.
struct Report {
    human: bool,
    dry_run: bool,
    look: Look,
    /// One per detected config.
    configs: Vec<(Mark, String, String)>,
    /// What migrate couldn't move: (name, why).
    items: Vec<(String, String)>,
    /// Lines after the PR request: the baselines not copied, (file, text).
    notes: Vec<(String, String)>,
    /// Agent mode's findings: one per config, then one per item.
    findings: (Vec<String>, Vec<String>),
    /// `--dry-run`'s diffs, printed first.
    diffs: String,
}

impl Report {
    fn new(human: bool, dry_run: bool) -> Report {
        Report {
            human,
            dry_run,
            look: Look::stdout(ColorChoice::Auto),
            configs: vec![],
            items: vec![],
            notes: vec![],
            findings: (vec![], vec![]),
            diffs: String::new(),
        }
    }

    /// The line of each detected config; `moved` says what the format config and rector.php became.
    fn configs(&mut self, rows: &[(Source, String, Action)], pint: &Path, moved: &[&str; 3]) {
        for (s, f, action) in rows {
            let moved = match s {
                Source::Rector => moved[1],
                Source::Phpstan => moved[2],
                _ => moved[0],
            }
            .to_string();
            let (mark, kind, text) = match action {
                Action::Migrate if self.dry_run => (Mark::Done, "migrate", moved),
                Action::Migrate => (Mark::Done, "migrated", moved),
                Action::Unsupported => (
                    Mark::Warn,
                    "not-supported",
                    format!(
                        "{} config: not supported by Pun yet; left untouched",
                        s.name()
                    ),
                ),
                Action::NotSelected => (Mark::Skip, "not-selected", "not selected".into()),
                Action::Covered => (
                    Mark::Skip,
                    "covered",
                    format!(
                        "not migrated: {} gives [format], as in Pint",
                        pint.display()
                    ),
                ),
                Action::NoVendor => (
                    Mark::Warn,
                    "not-read",
                    match s {
                        Source::Phpstan => "not read: run `composer install` so Pun can read its vendor includes; left untouched",
                        _ => "not read: run `composer install` so Pun can read it with Rector; left untouched",
                    }
                    .into(),
                ),
            };
            self.findings.0.push(match action {
                Action::Migrate | Action::NotSelected => format!("{f} {kind}"),
                _ => format!("{f} {kind} {text}"),
            });
            if *action == Action::Unsupported {
                self.items.push((f.clone(), text));
            } else {
                self.configs.push((mark, f.clone(), text));
            }
        }
    }

    /// A line under the configs (the rewritten ignore comments).
    fn line(&mut self, mark: Mark, name: &str, text: String) {
        self.configs.push((mark, name.into(), text));
    }

    /// Something migrate names and skips.
    fn unsupported(&mut self, name: &str, why: &str, finding: String) {
        self.items.push((name.into(), why.into()));
        self.findings.1.push(finding);
    }

    /// Prints everything, ending with `last` (humans) or the summary line built from `detail` and `checked` (agents).
    /// The last line, and agent mode's detail: what happened to pun.toml (a `starter` one without a format config)
    /// and the ignore comments.
    fn summary(
        self,
        cfg: &pun_config::Config,
        starter: bool,
        written: bool,
        rewritten: usize,
        checked: &str,
    ) {
        let toml = if starter {
            "starter pun.toml"
        } else {
            "pun.toml"
        };
        let state = match (written, self.dry_run) {
            (false, _) => "already up to date",
            (true, true) => "to write",
            (true, false) => "written",
        };
        let ignores = if self.dry_run {
            "to rewrite"
        } else {
            "rewritten"
        };
        let detail = vec![
            format!(
                "{toml} {} (preset {}, {}, {})",
                state.trim_start_matches("already "),
                cfg.preset,
                agent::count(cfg.rules.len(), "format rule"),
                agent::count(cfg.lint.rules.len(), "lint rule")
            ),
            format!(
                "ignore comments {ignores} in {}",
                agent::count(rewritten, "file")
            ),
        ];
        let preset = if starter {
            format!(" (preset {})", cfg.preset)
        } else {
            String::new()
        };
        let (mark, last) = match (written, self.dry_run) {
            (false, true) => (Mark::Skip, format!("Dry run: {toml} is already up to date")),
            (false, false) => (Mark::Done, format!("{toml} is already up to date")),
            (true, true) => (Mark::Skip, format!("Dry run: would write {toml}{preset}")),
            (true, false) => (Mark::Done, format!("Wrote {toml}{preset}")),
        };
        self.finish(mark, last, &detail, checked);
    }

    fn finish(mut self, mark: Mark, last: String, detail: &[String], checked: &str) {
        if !self.items.is_empty() {
            self.findings
                .1
                .push(format!("send a PR for what is skipped: {PR}"));
        }
        if !self.human {
            let result = if self.dry_run { "dry-run" } else { "ok" };
            let findings = [&self.findings.0[..], &self.findings.1[..]].concat();
            print!(
                "{}",
                agent::render(result, detail, checked, &findings, false)
            );
            print!("{}", self.diffs);
            return;
        }
        let mut out = std::mem::take(&mut self.diffs);
        out += &self.block(&self.configs);
        let warn = self.look.paint(Role::Warning, self.look.glyph("⚠", "!"));
        if !self.items.is_empty() {
            let _ = writeln!(out, "\n {warn} Not migrated");
            // One heading per reason, in the order they came, with what it applies to under it.
            let mut reasons: Vec<&str> = vec![];
            for (_, why) in &self.items {
                if !reasons.contains(&why.as_str()) {
                    reasons.push(why);
                }
            }
            for why in reasons {
                let _ = writeln!(out, "   {why}");
                for (name, _) in self.items.iter().filter(|i| i.1 == why) {
                    let _ = writeln!(out, "     {}", self.look.paint(Role::Dim, name));
                }
            }
            let _ = writeln!(out, "   Want them? Send a PR to {PR}");
        }
        for (name, text) in &self.notes {
            writeln!(out, " {warn} {name} {text}").unwrap();
        }
        if !self.items.is_empty() || !self.notes.is_empty() {
            out.push('\n');
        }
        out += &self.block(&[(mark, last, String::new())]);
        print!("{out}");
    }

    /// Lines with their texts in one column, two spaces after the longest name (up to 36 characters).
    fn block(&self, lines: &[(Mark, String, String)]) -> String {
        let width = (lines.iter())
            .filter(|l| !l.2.is_empty())
            .map(|l| l.1.chars().count())
            .filter(|w| *w <= 36)
            .max()
            .unwrap_or(0);
        let mut out = String::new();
        for (mark, name, text) in lines {
            let (glyph, role) = match mark {
                Mark::Done => (self.look.glyph("✔", "ok"), Role::Ok),
                Mark::Warn => (self.look.glyph("⚠", "!"), Role::Warning),
                Mark::Skip => (self.look.glyph("·", "-"), Role::Dim),
            };
            // A text's later lines start under its first.
            let pad = width.saturating_sub(name.chars().count());
            let indent = " ".repeat(glyph.chars().count() + name.chars().count() + pad + 4);
            let glyph = self.look.paint(role, glyph);
            let text = text.replace('\n', &format!("\n{indent}"));
            let text = match mark {
                Mark::Skip => self.look.paint(Role::Dim, &text),
                _ => text,
            };
            let line = format!(" {glyph} {name}{:pad$}  {text}", "");
            writeln!(out, "{}", line.trim_end()).unwrap();
        }
        out
    }
}

/// For the `@phpstan-ignore-line` and `-next-line` comments of a file whose PHPStan config is migrated: the rules of
/// the errors each line they cover had ([`crate::analyse::hidden_by_line_ignores`]), or why they are not known.
type Hidden = Option<Result<std::collections::HashMap<i64, Vec<&'static str>>, String>>;

/// `code` with, when `fixer`, each `@php-cs-fixer-ignore a,b` in a comment turned into
/// `@pun-ignore-file format/x/a,lint/y/b`, and with `phpstan` (`Some`), each `@phpstan-ignore` into
/// [`phpstan_ignore`]'s `@pun-ignore` and each `@phpstan-ignore-line` or `-next-line` into [`line_ignore`]'s; `None`
/// when nothing changes. One naming a rule Pun has no rule for, or that can't be converted, stays as it is, reported
/// in `unknown` as (path, note).
fn rewrite(
    code: &[u8],
    path: &str,
    fixer: bool,
    phpstan: Option<&Result<std::collections::HashMap<i64, Vec<&'static str>>, String>>,
    unknown: &mut Vec<(String, String)>,
) -> Option<Vec<u8>> {
    let has = |needle: &[u8]| code.windows(needle.len()).any(|w| w == needle);
    let phpstan = phpstan.filter(|_| has(b"@phpstan-ignore"));
    let fixer = fixer && has(b"php-cs-fixer-ignore");
    if !fixer && phpstan.is_none() {
        return None;
    }
    let mut out = Vec::with_capacity(code.len());
    let (mut line, mut offset) = (1, 0);
    for t in pun_tokens::Tokens::from_code(code).as_slice() {
        let (first, at) = (line, offset);
        line += newlines(t.get_content());
        offset += t.get_content().len();
        if !t.is_comment() {
            out.extend_from_slice(t.get_content());
            continue;
        }
        let mut text = t.get_content().to_vec();
        let mut done = Vec::with_capacity(text.len());
        let mut last = 0;
        for c in IGNORE.captures_iter(&text).flatten().filter(|_| fixer) {
            let ids: Option<Vec<&str>> = (c[2].split(|&b| b == b','))
                .map(|r| std::str::from_utf8(r).ok().and_then(pun_config::rule_of))
                .collect();
            let Some(ids) = ids else {
                let rules = String::from_utf8_lossy(&c[2]);
                let note =
                    format!("`@php-cs-fixer-ignore {rules}` left as is (no Pun rule for it)");
                unknown.push((path.to_string(), note));
                continue;
            };
            let m = c.get(0).unwrap();
            done.extend_from_slice(&text[last..m.start()]);
            done.extend_from_slice(&c[1]);
            done.extend_from_slice(b"pun-ignore-file ");
            done.extend_from_slice(ids.join(",").as_bytes());
            last = m.end();
        }
        done.extend_from_slice(&text[last..]);
        if let Some(hidden) = phpstan {
            text = done;
            done = Vec::with_capacity(text.len());
            last = 0;
            for c in PHPSTAN_IGNORE.captures_iter(&text).flatten() {
                let list = String::from_utf8_lossy(&c[1]).into_owned();
                let m = c.get(0).unwrap();
                match phpstan_ignore(&list) {
                    Ok(new) => {
                        done.extend_from_slice(&text[last..m.start()]);
                        done.extend_from_slice(new.as_bytes());
                        last = m.end();
                    }
                    Err(why) => {
                        let at = format!("{path}:{}", first + newlines(&text[..m.start()]));
                        let note = format!("`@phpstan-ignore {list}` left as is ({why})");
                        unknown.push((at, note));
                    }
                }
            }
            done.extend_from_slice(&text[last..]);
            let comment = at..at + t.get_content().len();
            let own_line = own_line(code, comment);
            done = line_ignore(done, first, own_line, hidden, path, unknown);
        }
        out.extend_from_slice(&done);
    }
    (out != code).then_some(out)
}

fn newlines(text: &[u8]) -> i64 {
    i64::try_from(pun_tokens::php_compat::substr_count(text, b"\n")).unwrap_or(i64::MAX)
}

/// Whether the comment at `span` of `code` has nothing but blanks before it on its first line and after it on its last,
/// as crates/pun-lint/src/problem/ignore.rs decides where a plain `@pun-ignore` points.
fn own_line(code: &[u8], span: std::ops::Range<usize>) -> bool {
    let start = code[..span.start]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let end =
        (code[span.end..].iter().position(|&b| b == b'\n')).map_or(code.len(), |i| span.end + i);
    let blank = |b: &[u8]| b.iter().all(u8::is_ascii_whitespace);
    blank(&code[start..span.start]) && blank(&code[span.end..end])
}

/// `text`, a comment starting on line `first`, with its `@phpstan-ignore-line` turned into `@pun-ignore` and its
/// `@phpstan-ignore-next-line` into `@pun-ignore-next-line`, naming the rules of the errors on the line PHPStan ignores
/// (`hidden`), with any text after the tag as the reason. It stays as it is, noted in `unknown`, when that line has
/// none, when the Pun comment would cover another line (crates/pun-lint/src/problem/ignore.rs), or with two tags in one
/// comment, which Pun reads only the first of.
fn line_ignore(
    text: Vec<u8>,
    first: i64,
    own_line: bool,
    hidden: &Result<std::collections::HashMap<i64, Vec<&'static str>>, String>,
    path: &str,
    unknown: &mut Vec<(String, String)>,
) -> Vec<u8> {
    let tags: Vec<_> = PHPSTAN_IGNORE_LINE.find_iter(&text).flatten().collect();
    let [m] = tags.as_slice() else {
        for m in &tags {
            let tag = String::from_utf8_lossy(m.as_bytes());
            let at = format!("{path}:{}", first + newlines(&text[..m.start()]));
            let why = "a Pun comment holds one ignore";
            unknown.push((at, format!("`{tag}` left as is ({why})")));
        }
        return text;
    };
    let tag = String::from_utf8_lossy(m.as_bytes()).into_owned();
    let next = tag.ends_with("next-line");
    let tag_line = first + newlines(&text[..m.start()]);
    let last = first + newlines(&text);
    let doc = text.starts_with(b"/**");
    // The line PHPStan ignores (crates/pun-phpstan/src/comments.rs) and the one the Pun comment would.
    let ignored = match (doc, next) {
        (false, true) => last + 1,
        (false, false) => last,
        (true, true) if !PHPDOC_TAG.is_match(&text[m.end()..]).unwrap_or(true) => last + 1,
        (true, true) => tag_line + 1,
        (true, false) => tag_line,
    };
    let pun = match (next, own_line) {
        (true, _) | (false, true) => last + 1,
        (false, false) => first,
    };
    let why = match hidden {
        _ if text.windows(11).any(|w| w == b"@pun-ignore") => {
            "the comment has a `@pun-ignore` already, and a Pun comment holds one ignore"
                .to_string()
        }
        _ if ignored != pun => {
            format!("it ignores line {ignored}, where a Pun comment here can't point")
        }
        Err(why) => why.clone(),
        Ok(lines) => match lines.get(&ignored).filter(|r| !r.is_empty()) {
            Some(rules) => {
                let end = text[m.end()..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(text.len(), |i| m.end() + i);
                let rest = &text[m.end()..end];
                let kept = rest.strip_suffix(b"*/").unwrap_or(rest).trim_ascii_end();
                let after = end - (rest.len() - kept.len());
                let reason = kept.trim_ascii_start();
                let reason = reason.strip_prefix(b"--").unwrap_or(reason).trim_ascii();
                let mut out = text[..m.start()].to_vec();
                out.extend_from_slice(if next {
                    b"@pun-ignore-next-line "
                } else {
                    b"@pun-ignore "
                });
                out.extend_from_slice(rules.join(",").as_bytes());
                if !reason.is_empty() {
                    out.extend_from_slice(b" -- ");
                    out.extend_from_slice(reason);
                    out.extend_from_slice(&text[after..]);
                    return out;
                }
                out.extend_from_slice(&text[m.end()..]);
                return out;
            }
            None => format!("`pun analyse` reports no error on line {ignored} for it to name"),
        },
    };
    unknown.push((
        format!("{path}:{tag_line}"),
        format!("`{tag}` left as is ({why})"),
    ));
    text
}

/// `@phpstan-ignore`'s list (`a.b (why), c.d`) as `@pun-ignore <rule IDs> -- <comments>`: each identifier's Pun rule
/// (rules/mapping.toml), once each; the comments joined by `; `. An identifier with no analyse or lint rule in Pun
/// (format rules are ignored per file) is the error.
fn phpstan_ignore(list: &str) -> Result<String, String> {
    let mut ids: Vec<&str> = vec![];
    let mut comments = vec![];
    let mut rest = list;
    while !rest.is_empty() {
        let end = rest.find([',', '(']).unwrap_or(rest.len());
        let identifier = rest[..end].trim();
        rest = &rest[end..];
        if let Some(r) = rest.strip_prefix('(') {
            let close = r.find(')').unwrap();
            comments.push(r[..close].trim());
            rest = &r[close + 1..];
        }
        rest = rest.trim_start().trim_start_matches(',').trim_start();
        match pun_config::rule_of_identifier(identifier) {
            Some(id) if !id.starts_with("format/") => {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            Some(id) => {
                return Err(format!(
                    "`{identifier}` is the format rule `{id}`, ignored per file"
                ));
            }
            None => {
                return Err(format!(
                    "`{identifier}` is not a PHPStan identifier Pun knows"
                ));
            }
        }
    }
    let mut out = format!("@pun-ignore {}", ids.join(","));
    if !comments.is_empty() {
        let _ = write!(out, " -- {}", comments.join("; "));
    }
    Ok(out)
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).display().to_string()
}

type Object = serde_json::Map<String, serde_json::Value>;

/// A `.php-cs-fixer.php`'s rules and Finder, read through the project's own PHP-CS-Fixer (the config is PHP code and
/// may register custom fixers). Migrate is the only place Pun runs PHP, and only to read the old config.
fn php_cs_fixer_config(root: &Path, config: &Path) -> Result<(Object, Object)> {
    let read = || -> Result<(Object, Object)> {
        let Some(fixer) = php_cs_fixer(root) else {
            bail!(
                "no PHP-CS-Fixer in vendor/, tools/ or vendor-bin/, and no `php-cs-fixer` on PATH: install it with `composer require --dev friendsofphp/php-cs-fixer`"
            );
        };
        let script = include_str!("php_cs_fixer_rules.php");
        let mut v: Object = read_php(root, config, script, &[fixer])?;
        let mut take = |k: &str| serde_json::from_value(v.remove(k).unwrap_or_default());
        Ok((take("rules")?, take("finder")?))
    };
    read().with_context(|| format!("can't read {} with PHP-CS-Fixer", rel(root, config)))
}

/// The PHP-CS-Fixer that reads the config, where projects install it: the autoloader of the project's `vendor/`, of
/// a tool directory (`tools/`, `tools/<name>/`, bamarni's `vendor-bin/<name>/`), or else Phive's `tools/php-cs-fixer`
/// or the `php-cs-fixer` on PATH (a PHAR or Composer's bin; the helper finds its autoloader as its bin does).
fn php_cs_fixer(root: &Path) -> Option<PathBuf> {
    let subdirs = |dir: &str| -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = (std::fs::read_dir(root.join(dir)).into_iter().flatten())
            .filter_map(|e| Some(e.ok()?.path().join("vendor")))
            .collect();
        found.sort();
        found
    };
    let vendors = [root.join("vendor"), root.join("tools/vendor")]
        .into_iter()
        .chain(subdirs("tools"))
        .chain(subdirs("vendor-bin"));
    let mut vendors = vendors.filter(|v| v.join("friendsofphp/php-cs-fixer").is_dir());
    let on_path = || {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|d| d.join("php-cs-fixer"))
            .find(|f| f.is_file())
    };
    (vendors.next().map(|v| v.join("autoload.php")))
        .or_else(|| Some(root.join("tools/php-cs-fixer")).filter(|f| f.is_file()))
        .or_else(on_path)
}

/// What the PHP helpers print before their JSON or error message; anything before it (a tool's warning) is not theirs.
const MARKER: &[u8] = b"\0pun-migrate\0";

/// Runs `script` on `config` with the project's autoloader (and `extra` arguments) and reads the JSON it prints after
/// [`MARKER`].
fn read_php<T: serde::de::DeserializeOwned>(
    root: &Path,
    config: &Path,
    script: &str,
    extra: &[PathBuf],
) -> Result<T> {
    let out = std::process::Command::new("php")
        .args([
            "-d",
            "display_errors=stderr",
            "-d",
            "log_errors=0",
            "-d",
            "fatal_error_backtraces=0",
            "-r",
            script,
            "--",
        ])
        .arg(root.join("vendor/autoload.php"))
        .arg(config)
        .args(extra)
        .current_dir(root)
        .output()
        .context("it needs `php` on PATH")?;
    let at = (out.stdout.windows(MARKER.len())).rposition(|w| w == MARKER);
    let own = at.map(|at| &out.stdout[at + MARKER.len()..]);
    match own {
        _ if !out.status.success() && own.is_none_or(<[u8]>::is_empty) => {
            bail!("{}", String::from_utf8_lossy(&out.stderr).trim())
        }
        Some(own) if !out.status.success() => bail!("{}", String::from_utf8_lossy(own)),
        Some(own) => {
            let mut json = serde_json::Deserializer::from_slice(own);
            Ok(serde::Deserialize::deserialize(&mut json)?)
        }
        None => bail!("the config ended the PHP process before Pun could read it"),
    }
}

#[cfg(test)]
mod tests {
    use super::{Action::*, Source::*, *};

    /// The checklist's answer becomes the plan: hand-derived from the rules in `plan`'s match.
    #[test]
    fn plan_follows_the_selection() {
        let all: Vec<(Source, String)> = Source::ALL
            .iter()
            .map(|&s| (s, s.files()[0].into()))
            .collect();
        let actions = |chosen: &[Source]| -> Vec<Action> {
            plan(&all, chosen).into_iter().map(|r| r.2).collect()
        };
        assert_eq!(
            actions(&Source::ALL),
            [Migrate, Covered, Migrate, Migrate, Unsupported]
        );
        assert_eq!(
            actions(&[PhpCsFixer, Mago]),
            [NotSelected, Migrate, NotSelected, NotSelected, Unsupported]
        );
        assert_eq!(actions(&[]), [NotSelected; 5]);
    }
}
