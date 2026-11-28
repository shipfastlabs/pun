//! Which installed plugins Composer 2.11.2 loads before `check-platform-reqs` runs (`PluginManager`'s
//! `loadInstalledPlugins`, `registerPackage` and `isPluginAllowed`, without interaction), and what it says about the
//! others. Pun runs no plugin: a loaded plugin only means that the install paths installed.json records count, as an
//! installer plugin can move its packages out of the vendor dir. Composer's global plugins (`COMPOSER_HOME`) are left
//! out.

use crate::constraint::Constraint;
use crate::error::{Result, bail};
use crate::resolver::package::Package;
use crate::resolver::platform::PLUGIN_API;
use crate::version::{compare, name_matches};
use serde_json::{Map, Value};
use std::path::Path;

/// `allow-plugins` as `PluginManager::parseAllowedPlugins` reads it: each pattern and whether it allows, in order;
/// `None` when it is empty and the lock is from before allow-plugins (a `plugin-api-version` below 2.2.0, or none),
/// which stops Composer at the first plugin it has to ask about.
pub(super) type Rules = Option<Vec<(String, bool)>>;

pub(super) fn rules(
    config: &Map<String, Value>,
    lock_file: &Path,
    lock_display: &str,
) -> Result<Rules> {
    let rules: Vec<(String, bool)> = match config.get("allow-plugins") {
        Some(Value::Bool(allow)) => return Ok(Some(vec![("*".into(), *allow)])),
        Some(Value::Object(rules)) => (rules.iter())
            .map(|(pattern, allow)| (pattern.clone(), *allow == Value::Bool(true)))
            .collect(),
        _ => vec![],
    };
    if !rules.is_empty() {
        return Ok(Some(rules));
    }
    // `Locker::isLocked` and `getPluginApi`: a lock with `packages`, and its `plugin-api-version`.
    let Ok(text) = std::fs::read(lock_file) else {
        return Ok(Some(rules));
    };
    // Composer follows this with jsonlint's description of the error (context/PLAN.md §3).
    let Ok(lock) = serde_json::from_slice::<Value>(&text) else {
        bail!("\"{lock_display}\" does not contain valid JSON");
    };
    let api = lock
        .get("plugin-api-version")
        .and_then(Value::as_str)
        .unwrap_or("1.1.0");
    let old = lock.get("packages").is_some_and(|p| !p.is_null()) && compare(api, "2.2.0") < 0;
    Ok((!old).then_some(rules))
}

/// The plugins that load, of the local repository's packages `local` (bases and aliases), for the root package `root`:
/// whether any does, and Composer's warnings about those that don't.
pub(super) fn load(
    arena: &[Package],
    local: &[usize],
    root: usize,
    rules: &Rules,
) -> Result<(bool, Vec<String>)> {
    let required = required(arena, local, root, true);
    let mut any = false;
    let mut warnings = vec![];
    // Composer goes through them in `PackageSorter`'s order, which only the order of several warnings shows.
    for &id in local {
        let package = &arena[id];
        if package.alias_of.is_some()
            || !matches!(
                package.kind.as_str(),
                "composer-plugin" | "composer-installer"
            )
        {
            continue;
        }
        if !required.contains(&id) && !allowed(&package.name, rules, true)? {
            warnings.push(format!(
                "The \"{}\" plugin was not loaded as it is not listed in allow-plugins and is not required by the root package anymore.",
                package.name
            ));
            continue;
        }
        if package.kind == "composer-plugin" {
            let Some(api) = package
                .requires
                .iter()
                .find(|l| l.target == "composer-plugin-api")
            else {
                bail!(
                    "Plugin {} is missing a require statement for a version of the composer-plugin-api package.",
                    package.name
                );
            };
            if api.pretty == PLUGIN_API {
                warnings.push(format!(
                    "The \"{}\" plugin requires composer-plugin-api {PLUGIN_API}, this *WILL* break in the future and it should be fixed ASAP (require ^{PLUGIN_API} instead for example).",
                    package.name
                ));
            } else if !api
                .constraint
                .matches(&Constraint::exact(&format!("{PLUGIN_API}.0")))
            {
                warnings.push(format!(
                    "The \"{}\" plugin was skipped because it requires a Plugin API version (\"{}\") that does not match your Composer installation (\"{PLUGIN_API}\"). You may need to run composer update with the \"--no-plugins\" option.",
                    package.name, api.pretty
                ));
                continue;
            }
        }
        let optional = package
            .data()
            .get("extra")
            .and_then(|e| e.get("plugin-optional"))
            == Some(&Value::Bool(true));
        any |= allowed(&package.name, rules, optional)?;
    }
    Ok((any, warnings))
}

/// `isPluginAllowed` without interaction: the first rule that matches `name`, else no for an `optional` plugin, else
/// Composer stops.
fn allowed(name: &str, rules: &Rules, optional: bool) -> Result<bool> {
    let Some(rules) = rules else {
        bail!(
            "Your composer.lock was generated before the allow-plugins security feature was introduced and your composer.json does not define allow-plugins. Run \"composer update --lock\" locally and commit the updated composer.lock, then add an explicit allow-plugins section to composer.json. See https://getcomposer.org/allow-plugins"
        );
    };
    if let Some((_, allow)) = rules
        .iter()
        .find(|(pattern, _)| name_matches(pattern, name))
    {
        return Ok(*allow);
    }
    if name == "composer/package-versions-deprecated" || optional {
        return Ok(false);
    }
    bail!(
        "{name} contains a Composer plugin which is blocked by your allow-plugins config. You may add it to the list if you consider it safe.\nYou can run \"composer config --no-plugins allow-plugins.{name} [true|false]\" to enable it (true) or disable it explicitly and suppress this exception (false)\nSee https://getcomposer.org/allow-plugins"
    )
}

/// `RepositoryUtils::filterRequiredPackages`: the packages of `local` the root requires (with require-dev when
/// `dev`), and those they require, by any of their names.
pub(crate) fn required(arena: &[Package], local: &[usize], root: usize, dev: bool) -> Vec<usize> {
    let mut found: Vec<usize> = vec![];
    let dev_requires = if dev {
        &arena[root].dev_requires[..]
    } else {
        &[]
    };
    let mut targets: Vec<&str> = (arena[root].requires.iter().chain(dev_requires))
        .map(|l| l.target.as_str())
        .collect();
    while let Some(target) = targets.pop() {
        for &id in local {
            if !found.contains(&id) && arena[id].names(true).contains(&target) {
                found.push(id);
                targets.extend(arena[id].requires.iter().map(|l| l.target.as_str()));
            }
        }
    }
    found
}
