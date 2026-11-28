//! `composer suggests` (Composer 2.11.2's `SuggestsCommand` and `SuggestedPackagesReporter`): the packages the root
//! and its installed (or locked) dependencies suggest, but those already installed, by package, by suggestion, or as
//! a list.

use crate::error::{Result, bail};
use crate::platform_reqs::{Root, installed, locked};
use crate::resolver::package::{Origin, Package, load};
use crate::resolver::platform::from_probe;
use serde_json::Value;
use std::fmt::Write as _;
use std::path::Path;

/// `suggests`'s options.
#[derive(Debug, Clone, Copy, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one bool per command-line flag"
)]
pub struct Options {
    /// `--by-package`.
    pub by_package: bool,
    /// `--by-suggestion`.
    pub by_suggestion: bool,
    /// `--all`: the suggestions of every package, not only of the root's own dependencies.
    pub all: bool,
    /// `--list`.
    pub list: bool,
    /// `--no-dev`.
    pub no_dev: bool,
    /// `--no-plugins`.
    pub no_plugins: bool,
}

/// One suggestion: the package that suggests (its pretty name), the one it suggests, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Suggestion {
    source: String,
    target: String,
    reason: String,
}

pub struct Suggests {
    /// Composer's warnings about the plugins it didn't load.
    pub warnings: Vec<String>,
    /// The named packages that aren't installed, or that `--no-dev` leaves out: Composer's errors (exit 1).
    pub errors: Vec<String>,
    shown: Vec<Suggestion>,
    /// How many more `--all` would show; `None` when it already shows all (`--all`, or named packages).
    more: Option<usize>,
    options: Options,
}

/// What `suggests` finds for the root package `root`, its lock at `lock_file` (named `lock_display` in messages) and
/// its vendor dir `vendor`, on the platform `probe` describes, for the packages named in `filter` (all when empty).
pub fn suggests(
    lock_file: &Path,
    lock_display: &str,
    root: &Root,
    vendor: &Path,
    probe: &Value,
    filter: &[String],
    options: Options,
) -> Result<Suggests> {
    let mut arena: Vec<Package> = vec![];
    let mut root_data = root.manifest.clone();
    root_data.entry("name").or_insert_with(|| "__root__".into());
    let root_id = *load(root_data, Origin::Root, &mut arena)?
        .last()
        .expect("the root");
    let local = installed(
        &mut arena,
        root_id,
        lock_file,
        lock_display,
        vendor,
        options.no_plugins,
    )?;
    // `Locker::isLocked`: a lock with `packages`. Composer follows a JSON error with jsonlint's description of it
    // (context/PLAN.md §3).
    let lock = match std::fs::read(lock_file) {
        Ok(text) => match serde_json::from_slice::<Value>(&text) {
            Ok(lock) => Some(lock).filter(|l| l.get("packages").is_some_and(|p| !p.is_null())),
            Err(_) => bail!("\"{lock_display}\" does not contain valid JSON"),
        },
        Err(_) => None,
    };
    // The root, the platform (with the lock's platform overrides, else `config.platform`), then the locked or the
    // installed packages.
    let overrides = match &lock {
        Some(lock) => lock
            .get("platform-overrides")
            .and_then(Value::as_object)
            .cloned(),
        None => (root.manifest.get("config").and_then(|c| c.get("platform")))
            .and_then(Value::as_object)
            .cloned(),
    };
    let mut repository = vec![root_id];
    for data in from_probe(probe, overrides.as_ref())? {
        repository.extend(load(data, Origin::Platform, &mut arena)?);
    }
    match &lock {
        Some(_) => repository.extend(locked(
            lock_file,
            lock_display,
            !options.no_dev,
            &mut arena,
        )?),
        None => repository.extend(local.ids),
    }

    let normalized: Vec<String> = filter.iter().map(|f| f.to_ascii_lowercase()).collect();
    // From a lock, with --no-dev, a named package may be one it leaves out.
    let with_dev = if lock.is_some() && options.no_dev && !normalized.is_empty() {
        Some(locked(lock_file, lock_display, true, &mut arena)?)
    } else {
        None
    };
    let errors = errors(&arena, &repository, with_dev.as_deref(), filter);
    let suggestions = collect(&arena, &repository, root_id, &normalized);
    let only_dependents = normalized.is_empty() && !options.all;
    let shown = filtered(
        &arena,
        &repository,
        &suggestions,
        only_dependents.then_some(root_id),
    );
    let more = only_dependents
        .then(|| filtered(&arena, &repository, &suggestions, None).len() - shown.len());
    Ok(Suggests {
        warnings: local.warnings,
        errors,
        shown,
        more,
        options,
    })
}

/// Composer's errors for the packages named in `filter` that aren't in `repository` by name: not installed, or (in
/// `with_dev`, the lock's packages with packages-dev) left out by --no-dev.
fn errors(
    arena: &[Package],
    repository: &[usize],
    with_dev: Option<&[usize]>,
    filter: &[String],
) -> Vec<String> {
    let has = |ids: &[usize], name: &str| ids.iter().any(|&id| arena[id].name == name);
    let (mut missing, mut excluded) = (vec![], vec![]);
    for given in filter {
        let name = given.to_ascii_lowercase();
        if has(repository, &name) {
            continue;
        }
        if with_dev.is_some_and(|ids| has(ids, &name)) {
            excluded.push(given.clone());
        } else {
            missing.push(given.clone());
        }
    }
    let list = |names: &[String]| names.join("\", \"");
    let mut errors = vec![];
    match missing.len() {
        0 => {}
        1 => errors.push(format!("Package \"{}\" is not installed.", missing[0])),
        _ => errors.push(format!(
            "Packages \"{}\" are not installed.",
            list(&missing)
        )),
    }
    match excluded.len() {
        0 => {}
        1 => errors.push(format!(
            "Package \"{}\" is excluded by --no-dev.",
            excluded[0]
        )),
        _ => errors.push(format!(
            "Packages \"{}\" are excluded by --no-dev.",
            list(&excluded)
        )),
    }
    errors
}

/// Each package's suggestions, of the named packages only when any (`normalized`); the root's twice, as Composer
/// adds it again after `repository`.
fn collect(
    arena: &[Package],
    repository: &[usize],
    root: usize,
    normalized: &[String],
) -> Vec<Suggestion> {
    let mut suggestions = vec![];
    for &id in repository.iter().chain([&root]) {
        let package = &arena[id];
        if !normalized.is_empty() && !normalized.contains(&package.name) {
            continue;
        }
        let data = package.data();
        for (target, reason) in
            (data.get("suggest").and_then(Value::as_object).into_iter()).flatten()
        {
            suggestions.push(Suggestion {
                source: package.pretty_name.clone(),
                target: target.clone(),
                reason: reason.as_str().unwrap_or_default().to_string(),
            });
        }
    }
    suggestions
}

/// `SuggestedPackagesReporter::getFilteredSuggestions`: without the suggestions some other package of `repository`
/// already is, provides or replaces (by the target as written), and, with `dependents_of` (the root), those of
/// packages it doesn't require (by the source's pretty name, as Composer compares them).
fn filtered(
    arena: &[Package],
    repository: &[usize],
    suggestions: &[Suggestion],
    dependents_of: Option<usize>,
) -> Vec<Suggestion> {
    let sources: Option<Vec<&str>> = dependents_of.map(|root| {
        (arena[root].requires.iter().chain(&arena[root].dev_requires))
            .map(|l| l.target.as_str())
            .chain([arena[root].name.as_str()])
            .collect()
    });
    (suggestions.iter())
        .filter(|s| {
            let others = repository.iter().any(|&id| {
                arena[id].names(true).contains(&s.target.as_str())
                    && arena[id].name != s.source.to_ascii_lowercase()
            });
            !others
                && sources
                    .as_ref()
                    .is_none_or(|sources| sources.contains(&s.source.as_str()))
        })
        .cloned()
        .collect()
}

/// `text` in a Symfony console style (`32` info, `33` comment), or as it is without `color`.
fn paint(code: &str, text: &str, color: bool) -> String {
    if color {
        format!("\x1b[{code}m{text}\x1b[39m")
    } else {
        text.to_string()
    }
}

/// A reason as the reporter shows it: newlines as spaces, without control characters; a falsy one ("" or "0") not
/// at all.
fn reason(reason: &str, color: bool, name: &str) -> String {
    let named = paint("32", name, color);
    if reason.is_empty() || reason == "0" {
        return format!(" - {named}\n");
    }
    let clean: String = (reason.replace('\n', " ").chars())
        .filter(|c| !matches!(*c as u32, 0..=0x1f | 0x7f))
        .collect();
    format!(" - {named}: {clean}\n")
}

impl Suggests {
    /// `SuggestedPackagesReporter::output`: a list of the suggested names, or by package, by suggestion, or both
    /// (by package first), each sorted by name (`ksort`); then how many more `--all` shows.
    pub fn text(&self, color: bool) -> String {
        // Each package with what it suggests, and each suggestion with who suggests it, a later reason winning.
        let mut by_source: Vec<(String, Vec<(String, String)>)> = vec![];
        let mut by_target: Vec<(String, Vec<(String, String)>)> = vec![];
        for s in &self.shown {
            add(&mut by_source, &s.source, &s.target, &s.reason);
            add(&mut by_target, &s.target, &s.source, &s.reason);
        }
        by_source.sort_by(|a, b| a.0.cmp(&b.0));
        by_target.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out = String::new();
        if self.options.list {
            for (name, _) in &by_target {
                out += &paint("32", name, color);
                out.push('\n');
            }
            return out;
        }
        let by_package = self.options.by_package || !self.options.by_suggestion;
        if by_package {
            for (source, targets) in &by_source {
                let _ = writeln!(out, "{} suggests:", paint("33", source, color));
                for (target, why) in targets {
                    out += &reason(why, color, target);
                }
                out.push('\n');
            }
        }
        if self.options.by_suggestion {
            if by_package {
                out += &"-".repeat(78);
                out.push('\n');
            }
            for (target, sources) in &by_target {
                let _ = writeln!(out, "{} is suggested by:", paint("33", target, color));
                for (source, why) in sources {
                    out += &reason(why, color, source);
                }
                out.push('\n');
            }
        }
        if let Some(more) = self.more.filter(|&more| more > 0) {
            let _ = writeln!(
                out,
                "{} by transitive dependencies can be shown with {}",
                paint("32", &format!("{more} additional suggestions"), color),
                paint("32", "--all", color)
            );
        }
        out
    }
}

/// `$grouped[$key][$item] = $reason`: the item under its key, in the order each first came, a later reason winning.
fn add(grouped: &mut Vec<(String, Vec<(String, String)>)>, key: &str, item: &str, why: &str) {
    let at = grouped
        .iter()
        .position(|(k, _)| k == key)
        .unwrap_or_else(|| {
            grouped.push((key.to_string(), vec![]));
            grouped.len() - 1
        });
    let items = &mut grouped[at].1;
    match items.iter_mut().find(|(i, _)| i == item) {
        Some((_, reason)) => *reason = why.to_string(),
        None => items.push((item.to_string(), why.to_string())),
    }
}
