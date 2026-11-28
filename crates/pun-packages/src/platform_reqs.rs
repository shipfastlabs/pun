//! `composer check-platform-reqs` (Composer 2.11.2's `CheckPlatformReqsCommand`): each platform requirement of the
//! installed (or locked) packages and of the root, checked against the real platform, `config.platform` aside.

use crate::constraint::{Constraint, composer_string};
use crate::error::{Result, bail};
use crate::resolver::package::{Link, Origin, Package, alias_package, load};
use crate::resolver::platform::from_probe;
use crate::version::is_platform;
use serde_json::{Map, Value, json};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub(crate) mod plugins;

/// Where the packages came from, which the first line names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `--lock`.
    Lock,
    /// The packages installed in the vendor dir.
    Vendor,
    /// The lock, as the vendor dir has nothing installed.
    NoVendor,
}

/// A requirement that failed: the link and the package it is from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub source: String,
    /// `Link::getDescription`: "requires", or "requires (for development)" for the root's require-dev.
    pub description: String,
    pub target: String,
    /// The constraint as composer/semver writes it (`Link::__toString`).
    pub constraint: String,
    /// The constraint as written in composer.json.
    pub pretty: String,
}

/// One line of the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub version: String,
    pub failed: Option<Failed>,
    /// "success", "failed" or "missing".
    pub status: &'static str,
    /// The package that provides or replaces the platform package, when not the platform itself.
    pub provider: Option<String>,
}

pub struct Report {
    pub source: Source,
    /// Composer's warnings about the plugins it didn't load, before the first line.
    pub warnings: Vec<String>,
    pub rows: Vec<Row>,
    /// 0, else 1 for a requirement the platform doesn't meet, else 2 for one it lacks.
    pub exit: u8,
}

/// `RootPackageLoader::load`'s root version, for the root package `manifest` in `dir`: its `version`, else
/// `COMPOSER_ROOT_VERSION` from `env`, else the version git gives, else 1.0.0, which Composer warns about; the
/// manifest with that version, and the warning.
pub fn root_versioned(
    manifest: &Map<String, Value>,
    dir: &Path,
    env: crate::Env,
) -> (Map<String, Value>, Option<String>) {
    let root = root_package(manifest, dir, env);
    (root.manifest, root.warning)
}

/// The root package as `RootPackageLoader::load` makes it: see [`root_package`].
pub struct Root {
    /// The manifest with its version (the one its links were read with).
    pub manifest: Map<String, Value>,
    /// Composer's warning about a version it couldn't find.
    pub warning: Option<String>,
    /// The version Composer shows: `1.0.0+no-version-set` for one it couldn't find (`replaceVersion`).
    pub pretty_version: String,
    /// The commit git gave the version from, which Composer records as its source and dist reference.
    pub commit: Option<String>,
}

/// [`root_versioned`]'s root package, with the version Composer shows and the commit of a version from git.
pub fn root_package(manifest: &Map<String, Value>, dir: &Path, env: crate::Env) -> Root {
    let mut manifest = manifest.clone();
    let root = |manifest: Map<String, Value>, commit: Option<String>| {
        let pretty_version = (manifest.get("version").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();
        Root {
            manifest,
            warning: None,
            pretty_version,
            commit,
        }
    };
    if manifest.contains_key("version") {
        return root(manifest, None);
    }
    if let Some(version) = crate::installed::root_version(env) {
        manifest.insert("version".into(), version.into());
        return root(manifest, None);
    }
    if let Some(git) = crate::installed::git_version(&manifest, dir) {
        manifest.insert("version".into(), git.pretty.into());
        manifest.insert("version_normalized".into(), git.version.into());
        return root(manifest, git.commit);
    }
    manifest.insert("version".into(), "1.0.0".into());
    let name = manifest
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("__root__");
    let project = manifest.get("type").and_then(Value::as_str) == Some("project");
    let warning = (name != "__root__" && !project).then(|| {
        format!(
            "Composer could not detect the root package ({name}) version, defaulting to '1.0.0'. See https://getcomposer.org/root-version"
        )
    });
    Root {
        manifest,
        warning,
        pretty_version: "1.0.0+no-version-set".into(),
        commit: None,
    }
}

/// The local repository as `Factory::createComposer` leaves it: the packages installed in the vendor dir (each and
/// its branch alias), their `dev-package-names`, and Composer's warnings about the plugins it didn't load.
pub(crate) struct Installed {
    pub(crate) ids: Vec<usize>,
    pub(crate) dev_names: Vec<String>,
    pub(crate) warnings: Vec<String>,
}

/// [`Installed`] for the root package `root` (in `arena`), its lock at `lock_file` (named `lock_display` in messages)
/// and its vendor dir `vendor`: the plugin manager (which reads allow-plugins, and the lock with it), the installed
/// plugins (none with `no_plugins`), then the local repository without the packages that aren't installed.
pub(crate) fn installed(
    arena: &mut Vec<Package>,
    root: usize,
    lock_file: &Path,
    lock_display: &str,
    vendor: &Path,
    no_plugins: bool,
) -> Result<Installed> {
    let config = (arena[root].data().get("config").and_then(Value::as_object))
        .cloned()
        .unwrap_or_default();
    let rules = plugins::rules(&config, lock_file, lock_display)?;
    let (entries, dev_names) = local(vendor, arena)?;
    let all: Vec<usize> = entries.iter().flat_map(|e| e.ids.clone()).collect();
    let (any_plugin, warnings) = if no_plugins {
        (false, vec![])
    } else {
        plugins::load(arena, &all, root, &rules)?
    };
    let ids = (entries.iter())
        .filter(|e| e.installed(any_plugin))
        .flat_map(|e| e.ids.clone())
        .collect();
    Ok(Installed {
        ids,
        dev_names,
        warnings,
    })
}

/// A requirement's links: the package each is from and its description.
type Links = Vec<(usize, &'static str, Link)>;

/// `check-platform-reqs`'s options.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// `--no-dev`.
    pub no_dev: bool,
    /// `--lock`.
    pub lock: bool,
    /// `--no-plugins`.
    pub no_plugins: bool,
}

/// What `check-platform-reqs` finds for the root package `manifest` (with its version: `root_versioned`), its lock at
/// `lock_file` (named `lock_display` in messages) and its vendor dir `vendor`, on the platform `probe` describes.
pub fn check(
    lock_file: &Path,
    lock_display: &str,
    manifest: &Map<String, Value>,
    vendor: &Path,
    probe: &Value,
    options: Options,
) -> Result<Report> {
    let mut arena: Vec<Package> = vec![];
    // As `Factory::createComposer` goes: the root, the plugin manager (which reads allow-plugins, and the lock with
    // it), the installed plugins, then the local repository without the packages that aren't installed. The command
    // then checks those, or the lock's.
    let mut root_data = manifest.clone();
    root_data.entry("name").or_insert_with(|| "__root__".into());
    let root_ids = load(root_data, Origin::Root, &mut arena)?;
    let root = *root_ids.last().expect("the root");
    let Installed {
        ids: local,
        mut dev_names,
        warnings,
    } = installed(
        &mut arena,
        root,
        lock_file,
        lock_display,
        vendor,
        options.no_plugins,
    )?;
    let dev = !options.no_dev;
    let (source, installed) = if options.lock {
        (
            Source::Lock,
            locked(lock_file, lock_display, dev, &mut arena)?,
        )
    } else if local.is_empty() {
        // The lock leaves its dev packages out itself; the vendor dir's dev package names don't apply.
        dev_names.clear();
        (
            Source::NoVendor,
            locked(lock_file, lock_display, dev, &mut arena)?,
        )
    } else {
        if dev {
            dev_names.clear();
        }
        (Source::Vendor, local)
    };
    if options.lock {
        dev_names.clear();
    }

    // The root package: `RootPackageRepository`, after the installed or locked packages.
    let repository: Vec<usize> = installed.iter().chain(&root_ids).copied().collect();
    let requires = requirements(&arena, &repository, root, &dev_names, options.no_dev);

    // The platform, after the installed packages and the root.
    let mut candidates_from = repository;
    for data in from_probe(probe, None)? {
        candidates_from.extend(load(data, Origin::Platform, &mut arena)?);
    }

    let mut rows = vec![];
    let mut exit = 0;
    for (target, links) in requires.iter().filter(|(t, _)| is_platform(t)) {
        exit = exit.max(requirement(
            &arena,
            &candidates_from,
            target,
            links,
            &mut rows,
        ));
    }
    Ok(Report {
        source,
        warnings,
        rows,
        exit,
    })
}

/// Each requirement and its links, by name (`ksort`): the root's require-dev first, unless `no_dev`, then each
/// package's requirements but the dev packages', the root's last.
fn requirements(
    arena: &[Package],
    repository: &[usize],
    root: usize,
    dev_names: &[String],
    no_dev: bool,
) -> Vec<(String, Links)> {
    let mut requires: Vec<(String, Links)> = vec![];
    let mut add = |link: &Link, from: (usize, &'static str), replace: bool| {
        let entry = (from.0, from.1, link.clone());
        match requires.iter_mut().find(|(t, _)| *t == link.target) {
            Some((_, links)) if replace => *links = vec![entry],
            Some((_, links)) => links.push(entry),
            None => requires.push((link.target.clone(), vec![entry])),
        }
    };
    if !no_dev {
        for link in &arena[root].dev_requires {
            add(link, (root, "requires (for development)"), true);
        }
    }
    for &id in repository
        .iter()
        .filter(|&&id| !dev_names.contains(&arena[id].name))
    {
        for link in &arena[id].requires {
            add(link, (id, "requires"), false);
        }
    }
    requires.sort_by(|a, b| a.0.cmp(&b.0));
    requires
}

/// One platform requirement's rows, and its exit code: the first candidate (named so, or providing or replacing it)
/// that meets every link is a success; else each candidate's first failed link, or "missing" with no candidate.
fn requirement(
    arena: &[Package],
    from: &[usize],
    target: &str,
    links: &Links,
    rows: &mut Vec<Row>,
) -> u8 {
    let candidates = providers(arena, from, target);
    if candidates.is_empty() {
        let (source, description, link) = &links[0];
        rows.push(Row {
            name: target.to_string(),
            version: "n/a".into(),
            failed: Some(failed(&arena[*source], description, link)),
            status: "missing",
            provider: None,
        });
        return 2;
    }
    let mut failures = vec![];
    for &candidate in &candidates {
        let package = &arena[candidate];
        let own = package.name == target;
        let (constraint, shown) = if own {
            (
                Constraint::exact(&package.version),
                package.pretty_version.clone(),
            )
        } else {
            let link = (package.provides.iter().chain(&package.replaces))
                .find(|l| l.target == target)
                .expect("a candidate not named so provides or replaces it");
            // `self.version` was parsed from the version of the package that wrote it: an alias keeps its package's
            // link first, so that version shows.
            let shown = if link.pretty == "self.version" {
                package
                    .alias_of
                    .map_or(package, |of| &arena[of])
                    .pretty_version
                    .clone()
            } else {
                link.pretty.clone()
            };
            (link.constraint.clone(), shown)
        };
        // `Constraint::getPrettyString`: a pretty string PHP takes for false ("" or "0", the version Composer gives an
        // extension whose own isn't one) shows the constraint as composer/semver writes it.
        let shown = if shown.is_empty() || shown == "0" {
            composer_string(&constraint)
        } else {
            shown
        };
        let name = if own {
            package.pretty_name.clone()
        } else {
            target.to_string()
        };
        let provider = (!own).then(|| format!("provided by {}", package.pretty_name));
        let Some((source, description, link)) = links
            .iter()
            .find(|(_, _, l)| !l.constraint.matches(&constraint))
        else {
            rows.push(Row {
                name,
                version: shown,
                failed: None,
                status: "success",
                provider,
            });
            return 0;
        };
        failures.push(Row {
            name,
            version: shown,
            failed: Some(failed(&arena[*source], description, link)),
            status: "failed",
            provider,
        });
    }
    rows.extend(failures);
    1
}

fn failed(from: &Package, description: &str, link: &Link) -> Failed {
    Failed {
        source: from.name.clone(),
        description: description.to_string(),
        target: link.target.clone(),
        constraint: composer_string(&link.constraint),
        pretty: link.pretty.clone(),
    }
}

/// `InstalledRepository::findPackagesWithReplacersAndProviders`: each package named `target`, or providing or replacing
/// it, in the repositories' order.
fn providers(arena: &[Package], from: &[usize], target: &str) -> Vec<usize> {
    (from.iter().copied())
        .filter(|&p| {
            let package = &arena[p];
            package.name == target
                || package
                    .provides
                    .iter()
                    .chain(&package.replaces)
                    .any(|l| l.target == target)
        })
        .collect()
}

/// A package of the local repository: its id and its branch alias's, and where it is installed.
struct Entry {
    ids: Vec<usize>,
    /// Where its installer put it, as installed.json records it (Composer 1's list has none).
    install_path: Option<PathBuf>,
    /// Where Composer's own installer puts it: vendor/<name>.
    vendor_path: PathBuf,
    metapackage: bool,
}

impl Entry {
    /// `Factory::purgePackages`: whether its install path is there (a metapackage has none to lose). An installer
    /// plugin, when `plugins` load, can have put it elsewhere, as installed.json records.
    fn installed(&self, plugins: bool) -> bool {
        let path = match &self.install_path {
            Some(path) if plugins => path,
            _ => &self.vendor_path,
        };
        self.metapackage || path.exists()
    }
}

/// The local repository (vendor/composer/installed.json): each package, and its `dev-package-names`.
fn local(vendor: &Path, arena: &mut Vec<Package>) -> Result<(Vec<Entry>, Vec<String>)> {
    let path = vendor.join("composer/installed.json");
    let Ok(text) = std::fs::read(&path) else {
        return Ok((vec![], vec![]));
    };
    // `FilesystemRepository::initialize`'s error, around the reason. Composer follows a JSON error with jsonlint's
    // description of it (context/PLAN.md §3).
    let invalid = |why: &str| {
        format!(
            "Invalid repository data in {}, packages could not be loaded: {why}",
            path.display()
        )
    };
    let Ok(data) = serde_json::from_slice::<Value>(&text) else {
        bail!(
            "{}",
            invalid(&format!(
                "[Seld\\JsonLint\\ParsingException] \"{}\" does not contain valid JSON",
                path.display()
            ))
        );
    };
    // Composer 2's `{"packages": [...]}`, or Composer 1's list.
    let packages = match data.get("packages").filter(|p| !p.is_null()) {
        Some(packages) => packages.clone(),
        None => data.clone(),
    };
    let packages: Vec<Value> = match packages {
        Value::Array(list) => list,
        Value::Object(map) => map.into_iter().map(|(_, p)| p).collect(),
        _ => bail!(
            "{}",
            invalid("[UnexpectedValueException] Could not parse package list from the repository")
        ),
    };
    let dev_names = (data
        .get("dev-package-names")
        .and_then(Value::as_array)
        .into_iter()
        .flatten())
    .filter_map(Value::as_str)
    .map(str::to_ascii_lowercase)
    .collect();
    let mut entries = vec![];
    for package in packages.iter().filter_map(Value::as_object) {
        let install_path = (package.get("install-path").and_then(Value::as_str))
            .map(|relative| vendor.join("composer").join(relative));
        let ids = load(package.clone(), Origin::Locked, arena)?;
        let base = &arena[*ids.last().expect("the package")];
        entries.push(Entry {
            vendor_path: vendor.join(&base.pretty_name),
            metapackage: base.kind == "metapackage",
            install_path,
            ids,
        });
    }
    Ok((entries, dev_names))
}

/// `Locker::getLockedRepository`: the lock's packages, with `packages-dev` when `dev`, each branch alias before its
/// package, then the root's inline aliases the lock records.
pub(crate) fn locked(
    lock_file: &Path,
    display: &str,
    dev: bool,
    arena: &mut Vec<Package>,
) -> Result<Vec<usize>> {
    let Ok(text) = std::fs::read(lock_file) else {
        bail!("No lockfile found. Unable to read locked packages");
    };
    // Composer follows this with jsonlint's description of the error (context/PLAN.md §3). A lock that isn't an object
    // has no `packages`, which PHP warns about; without that warning it has no `packages-dev` either.
    let Ok(data) = serde_json::from_slice::<Value>(&text) else {
        bail!("\"{display}\" does not contain valid JSON");
    };
    let list = |key: &str| {
        data.get(key)
            .filter(|v| !v.is_null())
            .and_then(Value::as_array)
            .cloned()
    };
    if dev && list("packages-dev").is_none() {
        bail!(
            "The lock file does not contain require-dev information, run install with the --no-dev option or delete it and run composer update to generate a new lock file."
        );
    }
    let packages = list("packages").unwrap_or_default();
    let dev_packages = if dev {
        list("packages-dev").unwrap_or_default()
    } else {
        vec![]
    };
    let mut ids = vec![];
    for package in packages
        .iter()
        .chain(&dev_packages)
        .filter_map(Value::as_object)
    {
        ids.extend(load(package.clone(), Origin::Locked, arena)?);
    }
    for alias in data
        .get("aliases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let field = |k: &str| {
            alias
                .get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let of = (ids.iter().copied())
            .find(|&p| arena[p].alias_of.is_none() && arena[p].name == field("package"));
        if let Some(of) = of {
            ids.push(alias_package(
                arena,
                of,
                field("alias_normalized"),
                field("alias"),
                true,
            ));
        }
    }
    Ok(ids)
}

/// Composer's output styles, as Symfony's console writes them with colour.
#[derive(Debug, Clone, Copy)]
pub enum Style {
    Info,
    Comment,
    Warning,
    Error,
}

impl Style {
    /// `text` in this style, or as it is without `color`.
    pub fn paint(self, text: &str, color: bool) -> String {
        if !color {
            return text.to_string();
        }
        let (on, off) = match self {
            Style::Info => ("32", "39"),
            Style::Comment => ("33", "39"),
            Style::Warning => ("30;43", "39;49"),
            Style::Error => ("37;41", "39;49"),
        };
        format!("\x1b[{on}m{text}\x1b[{off}m")
    }
}

impl Report {
    /// The table `renderTable` prints: name, version, the failed link (as `Link::__toString` and with its constraint
    /// as written), and the status and its provider, each cell padded to its column (by the text it shows) and
    /// followed by a space; with `color`, the status and provider in Composer's styles.
    pub fn text(&self, color: bool) -> String {
        let cells: Vec<[(String, String); 5]> = (self.rows.iter())
            .map(|row| {
                let (link, written) = row.failed.as_ref().map_or_else(Default::default, |f| {
                    (
                        format!(
                            "{} {} {} ({})",
                            f.source, f.description, f.target, f.constraint
                        ),
                        format!("{} {} {} ({})", f.source, f.description, f.target, f.pretty),
                    )
                });
                let style = if row.status == "success" {
                    Style::Info
                } else {
                    Style::Error
                };
                let mut status = (row.status.to_string(), style.paint(row.status, color));
                if let Some(provider) = &row.provider {
                    status.0 = format!("{} {provider}", status.0);
                    status.1 = format!("{} {}", status.1, Style::Comment.paint(provider, color));
                }
                let plain = |text: &String| (text.clone(), text.clone());
                [
                    plain(&row.name),
                    plain(&row.version),
                    plain(&link),
                    plain(&written),
                    status,
                ]
            })
            .collect();
        let width = |i: usize| {
            cells
                .iter()
                .map(|c| c[i].0.chars().count())
                .max()
                .unwrap_or(0)
        };
        let widths = [width(0), width(1), width(2), width(3), width(4)];
        let mut out = String::new();
        for row in &cells {
            for ((shown, cell), width) in row.iter().zip(widths) {
                let pad = width - shown.chars().count();
                let _ = write!(out, "{cell}{:pad$} ", "");
            }
            out.push('\n');
        }
        out
    }

    /// The JSON `--format=json` prints: one object for each row.
    pub fn json(&self) -> String {
        let rows: Vec<Value> = (self.rows.iter())
            .map(|row| {
                json!({
                    "name": row.name,
                    "version": row.version,
                    "status": row.status,
                    "failed_requirement": row.failed.as_ref().map(|f| json!({
                        "source": f.source,
                        "type": f.description,
                        "target": f.target,
                        "constraint": f.pretty,
                    })),
                    "provider": row.provider,
                })
            })
            .collect();
        crate::php::json_pretty(&Value::Array(rows))
    }
}
