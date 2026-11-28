//! `composer licenses` (Composer 2.11.2's `LicensesCommand`): the licenses of the root and of each installed (or
//! locked) package, as Symfony's compact table, JSON, or a summary table.

use crate::error::{Result, bail};
use crate::php::{json_pretty, mb_strwidth};
use crate::platform_reqs::{Root, installed, locked, plugins};
use crate::resolver::package::{Origin, Package, load};
use serde_json::{Map, Value, json};
use std::fmt::Write as _;
use std::path::Path;

/// `--format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
    Summary,
}

impl Format {
    /// The format `name` names, else Composer's error.
    pub fn parse(name: &str) -> Result<Format> {
        Ok(match name {
            "text" => Format::Text,
            "json" => Format::Json,
            "summary" => Format::Summary,
            _ => bail!("Unsupported format \"{name}\".  See help for supported formats."),
        })
    }
}

/// `licenses`'s options.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// `--no-dev`.
    pub no_dev: bool,
    /// `--locked`.
    pub locked: bool,
    /// `--no-plugins`.
    pub no_plugins: bool,
}

/// A package as the report shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// Its pretty name.
    pub name: String,
    /// `getFullPrettyVersion`.
    pub version: String,
    pub licenses: Vec<String>,
    /// `PackageInfo::getViewSourceOrHomepageUrl`, the link of its name in a terminal.
    pub link: Option<String>,
}

pub struct Report {
    pub root: Shown,
    /// The packages, by name (`PackageSorter::sortPackagesAlphabetically`), a branch alias beside its package.
    pub packages: Vec<Shown>,
    /// Composer's warnings about the plugins it didn't load.
    pub warnings: Vec<String>,
}

/// What `licenses` reports for the root package `root`, its lock at `lock_file` (named `lock_display` in messages)
/// and its vendor dir `vendor`.
pub fn report(
    lock_file: &Path,
    lock_display: &str,
    root: &Root,
    vendor: &Path,
    options: Options,
) -> Result<Report> {
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
    let ids = if options.locked {
        // `Locker::isLocked`: a lock with `packages`. Composer follows a JSON error with jsonlint's description of
        // it (context/PLAN.md §3).
        let lock = match std::fs::read(lock_file) {
            Ok(text) => match serde_json::from_slice::<Value>(&text) {
                Ok(lock) => Some(lock),
                Err(_) => bail!("\"{lock_display}\" does not contain valid JSON"),
            },
            Err(_) => None,
        };
        if lock.is_none_or(|lock| lock.get("packages").is_none_or(Value::is_null)) {
            bail!(
                "Valid composer.json and composer.lock files are required to run this command with --locked"
            );
        }
        locked(lock_file, lock_display, !options.no_dev, &mut arena)?
    } else if options.no_dev {
        plugins::required(&arena, &local.ids, root_id, false)
    } else {
        local.ids
    };
    let mut packages: Vec<(String, Shown)> = (ids.iter())
        .map(|&id| (arena[id].name.clone(), shown(&arena, id)))
        .collect();
    // `usort` on the name, which keeps the order of equal ones.
    packages.sort_by(|a, b| a.0.cmp(&b.0));
    let root_package = &arena[root_id];
    let mut root_version = root.pretty_version.clone();
    if let Some(commit) = root.commit.as_deref().filter(|_| root_package.is_dev()) {
        // The source and dist reference `RootPackageLoader` gives a version from git.
        root_version = format!("{root_version} {}", truncated(commit, ""));
    }
    let root_shown = Shown {
        name: root_package.pretty_name.clone(),
        version: root_version,
        licenses: licenses(&root.manifest),
        link: None,
    };
    Ok(Report {
        root: root_shown,
        packages: packages.into_iter().map(|(_, s)| s).collect(),
        warnings: local.warnings,
    })
}

/// The package `id` as the report shows it (a branch alias holds its package's data).
fn shown(arena: &[Package], id: usize) -> Shown {
    let package = &arena[id];
    let data = package.data();
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(String::from);
    let source_type = text(data.get("source").and_then(|s| s.get("type"))).unwrap_or_default();
    let source_reference = text(data.get("source").and_then(|s| s.get("reference")));
    let dist_reference = text(data.get("dist").and_then(|d| d.get("reference")));
    // `getFullPrettyVersion` with `DISPLAY_SOURCE_REF_IF_DEV`.
    let plain = !package.is_dev()
        || (!matches!(source_type.as_str(), "git" | "hg")
            && (!source_type.is_empty()
                || dist_reference.as_deref().unwrap_or_default().is_empty()));
    let reference = source_reference
        .filter(|r| !r.is_empty())
        .or(dist_reference);
    let version = match reference {
        Some(reference) if !plain => format!(
            "{} {}",
            package.pretty_version,
            truncated(&reference, &source_type)
        ),
        _ => package.pretty_version.clone(),
    };
    // `PackageInfo::getViewSourceOrHomepageUrl`: `support.source`, else the source's URL, else the homepage.
    let support_source =
        text(data.get("support").and_then(|s| s.get("source"))).filter(|s| !s.is_empty());
    let link = support_source
        .or_else(|| text(data.get("source").and_then(|s| s.get("url"))))
        .or_else(|| text(data.get("homepage")))
        .filter(|url| !url.is_empty());
    Shown {
        name: package.pretty_name.clone(),
        version,
        licenses: licenses(&data),
        link,
    }
}

/// A reference as `getFullPrettyVersion` shows it: a 40-character one cut to 7, but svn's.
fn truncated(reference: &str, source_type: &str) -> String {
    if reference.len() == 40 && source_type != "svn" {
        reference[..7].to_string()
    } else {
        reference.to_string()
    }
}

/// `ArrayLoader`'s `license`: a list, or one license; nothing for an empty one (PHP's `empty`, so "0" too).
fn licenses(data: &Map<String, Value>) -> Vec<String> {
    match data.get("license") {
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect(),
        Some(Value::String(one)) if !one.is_empty() && one != "0" => vec![one.clone()],
        _ => vec![],
    }
}

/// `text` in a Symfony console style (`32` info, `33` comment), or as it is without `color`.
fn paint(code: &str, text: &str, color: bool) -> String {
    if color {
        format!("\x1b[{code}m{text}\x1b[39m")
    } else {
        text.to_string()
    }
}

/// The licenses as one cell: joined by ", ", else "none".
fn joined(licenses: &[String]) -> String {
    if licenses.is_empty() {
        "none".to_string()
    } else {
        licenses.join(", ")
    }
}

impl Report {
    /// The text format: the root, then Symfony's compact table (each cell padded to its column, by `mb_strwidth`, and
    /// followed by a space); with `color`, Composer's styles and each name a link.
    pub fn text(&self, color: bool) -> String {
        let mut out = format!(
            "Name: {}\nVersion: {}\nLicenses: {}\nDependencies:\n\n",
            paint("33", &self.root.name, color),
            paint("33", &self.root.version, color),
            paint("33", &joined(&self.root.licenses), color)
        );
        let rows: Vec<[String; 3]> = (self.packages.iter())
            .map(|p| [p.name.clone(), p.version.clone(), joined(&p.licenses)])
            .collect();
        let header = ["Name", "Version", "Licenses"].map(String::from);
        let widths: Vec<usize> = (0..3)
            .map(|i| {
                (std::iter::once(&header).chain(&rows))
                    .map(|row| mb_strwidth(&row[i]))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let pad = |text: &str, width: usize| " ".repeat(width - mb_strwidth(text) + 1);
        for (i, cell) in header.iter().enumerate() {
            out += &paint("32", &format!("{cell}{}", pad(cell, widths[i])), color);
        }
        out.push('\n');
        for (package, row) in self.packages.iter().zip(&rows) {
            for (i, cell) in row.iter().enumerate() {
                match &package.link {
                    // Symfony's `<href=...>`: an OSC 8 link in a terminal, the text alone elsewhere.
                    Some(link) if i == 0 && color => {
                        let _ = write!(out, "\x1b]8;;{link}\x1b\\{cell}\x1b]8;;\x1b\\");
                    }
                    _ => out += cell,
                }
                out += &pad(cell, widths[i]);
            }
            out.push('\n');
        }
        out
    }

    /// The JSON format (`JsonFile::encode`): the root, and each package by its name, the last of a name winning.
    pub fn json(&self) -> String {
        let mut dependencies = Map::new();
        for package in &self.packages {
            dependencies.insert(
                package.name.clone(),
                json!({"version": package.version, "license": package.licenses}),
            );
        }
        json_pretty(&json!({
            "name": self.root.name,
            "version": self.root.version,
            "license": self.root.licenses,
            "dependencies": dependencies,
        }))
    }

    /// The summary format: each license and how many packages have it ("none" for those with none), the most used
    /// first (`arsort`, which keeps the order of equal ones), as `SymfonyStyle`'s table; with `color`, its header in
    /// Composer's info style.
    pub fn summary(&self, color: bool) -> String {
        let mut counts: Vec<(String, usize)> = vec![];
        for package in &self.packages {
            let none = ["none".to_string()];
            let licenses = if package.licenses.is_empty() {
                &none[..]
            } else {
                &package.licenses[..]
            };
            for license in licenses {
                match counts.iter_mut().find(|(l, _)| l == license) {
                    Some((_, count)) => *count += 1,
                    None => counts.push((license.clone(), 1)),
                }
            }
        }
        counts.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        let rows: Vec<[String; 2]> = (counts.into_iter())
            .map(|(license, count)| [license, count.to_string()])
            .collect();
        let header = ["License", "Number of dependencies"].map(String::from);
        let widths: Vec<usize> = (0..2)
            .map(|i| {
                (std::iter::once(&header).chain(&rows))
                    .map(|row| mb_strwidth(&row[i]))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let border = format!(
            " {} {} \n",
            "-".repeat(widths[0] + 2),
            "-".repeat(widths[1] + 2)
        );
        let line = |row: &[String; 2], style: bool| {
            let cells: Vec<String> = (row.iter().enumerate())
                .map(|(i, cell)| {
                    let padded = format!(" {cell}{} ", " ".repeat(widths[i] - mb_strwidth(cell)));
                    paint("32", &padded, style)
                })
                .collect();
            format!(" {} \n", cells.join(" "))
        };
        let mut out = border.clone() + &line(&header, color) + &border;
        if !rows.is_empty() {
            for row in &rows {
                out += &line(row, false);
            }
            out += &border;
        }
        out + "\n"
    }
}
