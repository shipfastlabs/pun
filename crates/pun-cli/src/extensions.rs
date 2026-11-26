//! The built-in extensions, as Mago lists its built-in plugins.

use crate::Exit;
use anyhow::{Result, bail};
use pun_config::extensions::{self as meta, Switch};
use pun_extension::{Extension, ExtensionMeta, FixerRegistry, RuleRegistry};
use std::fmt::Write as _;
use std::path::PathBuf;

struct Laravel;
struct Pest;
struct Phpunit;
struct Carbon;
struct Mockery;
struct DisallowedCalls;
struct TypeCoverage;
struct Phpat;

impl Extension for Laravel {
    fn meta(&self) -> &'static ExtensionMeta {
        &meta::LARAVEL
    }

    fn format(&self, r: &mut FixerRegistry) {
        r.add(pun_fixers::laravel());
    }

    fn lint(&self, r: &mut RuleRegistry) {
        r.add(pun_lint::rules::laravel());
    }
}

impl Extension for Pest {
    fn meta(&self) -> &'static ExtensionMeta {
        &meta::PEST
    }

    fn lint(&self, r: &mut RuleRegistry) {
        r.add(pun_lint::rules::pest());
    }
}

impl Extension for Phpunit {
    fn meta(&self) -> &'static ExtensionMeta {
        &meta::PHPUNIT
    }

    fn lint(&self, r: &mut RuleRegistry) {
        r.add(pun_lint::rules::phpunit());
    }
}

impl Extension for Carbon {
    fn meta(&self) -> &'static ExtensionMeta {
        &meta::CARBON
    }
}

impl Extension for Mockery {
    fn meta(&self) -> &'static ExtensionMeta {
        &meta::MOCKERY
    }
}

impl Extension for DisallowedCalls {
    fn meta(&self) -> &'static ExtensionMeta {
        &meta::DISALLOWED_CALLS
    }
}

impl Extension for TypeCoverage {
    fn meta(&self) -> &'static ExtensionMeta {
        &meta::TYPE_COVERAGE
    }
}

impl Extension for Phpat {
    fn meta(&self) -> &'static ExtensionMeta {
        &meta::PHPAT
    }
}

pub(crate) static ALL: [&dyn Extension; 8] = [
    &Laravel,
    &Pest,
    &Phpunit,
    &Carbon,
    &Mockery,
    &DisallowedCalls,
    &TypeCoverage,
    &Phpat,
];

pub(crate) fn fixers() -> Vec<Box<dyn pun_format::Fixer>> {
    pun_extension::fixers(&ALL)
}

pub(crate) fn lint_rules() -> Vec<Box<dyn pun_lint::Rule>> {
    pun_extension::lint_rules(&ALL)
}

#[derive(clap::Subcommand)]
pub(crate) enum ExtensionCmd {
    /// List the built-in extensions, whether each is on for this project, and why.
    List(ListArgs),
}

#[derive(clap::Args)]
pub(crate) struct ListArgs {
    /// Output format: `json` prints an array.
    #[arg(long)]
    format: Option<String>,
    /// The pun.toml to read (default: the one in the current directory, if any).
    #[arg(long)]
    config: Option<PathBuf>,
}

pub(crate) fn run(cmd: ExtensionCmd) -> Result<Exit> {
    let ExtensionCmd::List(a) = cmd;
    let json = match a.format.as_deref() {
        None => false,
        Some("json") => true,
        Some(f) => bail!("--format={f}: expected `json`"),
    };
    let root = crate::root()?;
    let no_config = a.config.is_none() && !root.join("pun.toml").exists();
    let cfg = pun_config::load(
        &root,
        &pun_config::Options {
            config: a.config,
            no_config,
            ignore_php_constraint: crate::ignore_php_constraint(),
            ..Default::default()
        },
    )?;
    let composer = pun_config::composer::detect(&root)?;
    let rows = ALL.map(|e| {
        let m = e.meta();
        let switch = cfg.extension_switches.get(m.id).copied();
        let locked = m
            .packages
            .first()
            .and_then(|p| Some((p, composer.locked(p)?)));
        let why = match (switch, locked) {
            (Some(s), _) => format!("{} in pun.toml", s.name()),
            (None, Some((p, l))) => format!(
                "{p} {} is locked; turn it on with {} = \"on\" in [extensions]",
                l.version, m.id
            ),
            (None, None) => "not in pun.toml".to_string(),
        };
        (m, switch, cfg.extensions.contains(&m), why)
    });
    let out = if json {
        let items: Vec<_> = (rows.iter())
            .map(|(m, switch, on, _)| {
                serde_json::json!({
                    "id": m.id,
                    "name": m.name,
                    "description": m.description,
                    "on": on,
                    "switch": switch.map(Switch::name),
                    "packages": m.packages,
                    "groups": m.groups,
                })
            })
            .collect();
        format!("{}\n", serde_json::Value::Array(items))
    } else {
        let width = rows.iter().map(|r| r.0.id.len()).max().unwrap_or(0);
        rows.iter().fold(String::new(), |mut out, (m, _, on, why)| {
            let state = if *on { "on" } else { "off" };
            let _ = writeln!(
                out,
                "{:<width$}  {state:<3}  {}: {why}",
                m.id, m.description
            );
            out
        })
    };
    print!("{out}");
    Ok(Exit::Success)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: &str) -> &str {
        id.split('/').nth(1).unwrap()
    }

    #[test]
    fn extensions_keep_to_their_groups() {
        let metas: Vec<_> = ALL.iter().map(|e| e.meta()).collect();
        assert_eq!(metas, meta::ALL);
        for rule in pun_lint::rules::core() {
            assert_eq!(meta::owner(group(rule.id())), None, "{}", rule.id());
        }
        for e in ALL {
            let mut r = RuleRegistry::default();
            e.lint(&mut r);
            for rule in r.into_rules() {
                let owner = meta::owner(group(rule.id())).map(|o| o.0);
                assert_eq!(owner, Some(e.meta()), "{}", rule.id());
            }
        }
        let ids = |rules: Vec<Box<dyn pun_lint::Rule>>| -> Vec<_> {
            rules.iter().map(|r| r.id()).collect()
        };
        assert_eq!(ids(lint_rules()), ids(pun_lint::rules::all()));
    }

    #[test]
    fn extension_fixers_keep_to_their_groups() {
        for e in ALL {
            let mut r = FixerRegistry::default();
            e.format(&mut r);
            for f in r.into_fixers() {
                let id = pun_config::rule_of(f.name()).unwrap();
                let owner = meta::owner(group(id)).map(|o| o.0);
                assert_eq!(owner, Some(e.meta()), "{id}");
            }
        }
        let names = |fixers: Vec<Box<dyn pun_format::Fixer>>| -> Vec<_> {
            fixers.iter().map(|f| f.name()).collect()
        };
        assert_eq!(names(fixers()), names(pun_fixers::all()));
    }
}
