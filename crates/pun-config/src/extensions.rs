use anyhow::{Result, bail};
use serde::{Serialize, Serializer};
use std::collections::BTreeMap;

/// What Pun knows about an extension before it runs: its name, the rule groups it owns and the locked packages
/// `pun migrate` and `pun init` look for to turn it on.
#[derive(Debug, PartialEq, Eq)]
pub struct ExtensionMeta {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// The rule groups it owns. Each also owns its version groups, such as `laravel-12`.
    pub groups: &'static [&'static str],
    /// The locked packages that show a project uses it, the first of them also gating its version groups.
    pub packages: &'static [&'static str],
}

impl Serialize for ExtensionMeta {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.id)
    }
}

pub static LARAVEL: ExtensionMeta = ExtensionMeta {
    id: "laravel",
    name: "Laravel",
    description: "Laravel's rules and Larastan's analysis",
    groups: &["laravel"],
    packages: &["laravel/framework"],
};

pub static PEST: ExtensionMeta = ExtensionMeta {
    id: "pest",
    name: "Pest",
    description: "Rules for Pest tests",
    groups: &["pest"],
    packages: &["pestphp/pest"],
};

pub static PHPUNIT: ExtensionMeta = ExtensionMeta {
    id: "phpunit",
    name: "PHPUnit",
    description: "Rules for PHPUnit tests",
    groups: &["phpunit"],
    packages: &["phpunit/phpunit"],
};

pub static CARBON: ExtensionMeta = ExtensionMeta {
    id: "carbon",
    name: "Carbon",
    description: "Carbon's macros and DatePeriodBase in analyse",
    groups: &[],
    packages: &["nesbot/carbon"],
};

pub static MOCKERY: ExtensionMeta = ExtensionMeta {
    id: "mockery",
    name: "Mockery",
    description: "phpstan-mockery's mock types in analyse",
    groups: &[],
    packages: &["mockery/mockery", "phpstan/phpstan-mockery"],
};

pub static DISALLOWED_CALLS: ExtensionMeta = ExtensionMeta {
    id: "disallowed-calls",
    name: "Disallowed calls",
    description: "The calls and uses your config forbids, in analyse",
    groups: &["disallowed"],
    packages: &["spaze/phpstan-disallowed-calls"],
};

pub static TYPE_COVERAGE: ExtensionMeta = ExtensionMeta {
    id: "type-coverage",
    name: "Type coverage",
    description: "Minimum shares of declared types, in analyse",
    groups: &["type-coverage"],
    packages: &["tomasvotruba/type-coverage"],
};

pub static PHPAT: ExtensionMeta = ExtensionMeta {
    id: "phpat",
    name: "phpat",
    description: "phpat's architecture tests in analyse",
    groups: &["phpat"],
    packages: &["phpat/phpat"],
};

pub static ALL: [&ExtensionMeta; 8] = [
    &LARAVEL,
    &PEST,
    &PHPUNIT,
    &CARBON,
    &MOCKERY,
    &DISALLOWED_CALLS,
    &TYPE_COVERAGE,
    &PHPAT,
];

/// `[extensions] phpat`'s settings: the test classes phpstan.neon tags `phpat.test`, and `parameters.phpat`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Phpat {
    #[serde(default)]
    pub tests: Vec<String>,
    #[serde(default)]
    pub ignore_doc_comments: bool,
    #[serde(default)]
    pub ignore_built_in_classes: bool,
    #[serde(default)]
    pub show_rule_names: bool,
}

/// `[extensions] phpat` as a table of settings (which turns phpat on), or `None` for a plain switch.
pub(crate) fn phpat(at: &str, t: &toml::Table) -> Result<Option<Phpat>> {
    match t.get(PHPAT.id) {
        Some(toml::Value::Table(settings)) => settings
            .clone()
            .try_into()
            .map(Some)
            .map_err(|e| anyhow::anyhow!("{at}: [extensions] phpat: {}", e.message())),
        _ => Ok(None),
    }
}

/// The extension that owns `group`, and the version in the group's name when it is a version group.
pub fn owner(group: &str) -> Option<(&'static ExtensionMeta, Option<&str>)> {
    ALL.into_iter().find_map(|e| {
        e.groups.iter().find_map(|g| {
            let rest = group.strip_prefix(g)?;
            if rest.is_empty() {
                return Some((e, None));
            }
            Some((e, Some(rest.strip_prefix('-')?)))
        })
    })
}

/// Every package some extension looks for.
pub(crate) fn packages() -> impl Iterator<Item = &'static str> {
    ALL.into_iter().flat_map(|e| e.packages.iter().copied())
}

/// The extensions whose package `composer` locks: the ones `pun migrate` and `pun init` turn on.
pub fn detected(composer: &crate::composer::Composer) -> Vec<&'static ExtensionMeta> {
    (ALL.into_iter())
        .filter(|e| e.packages.iter().any(|p| composer.locked(p).is_some()))
        .collect()
}

/// An `[extensions]` value. An extension pun.toml leaves out is off: nothing turns one on but pun.toml.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    On,
    Off,
}

impl Switch {
    pub fn name(self) -> &'static str {
        match self {
            Switch::On => "on",
            Switch::Off => "off",
        }
    }
}

/// The `[extensions]` table, keyed by extension ID.
pub(crate) fn switches(at: &str, t: toml::Table) -> Result<BTreeMap<&'static str, Switch>> {
    (t.into_iter())
        .map(|(name, v)| {
            let Some(e) = ALL.into_iter().find(|e| e.id == name) else {
                let ids: Vec<String> = ALL.iter().map(|e| format!("`{}`", e.id)).collect();
                bail!(
                    "{at}: [extensions] `{name}` is not an extension: expected {}",
                    ids.join(", ")
                );
            };
            let s = match v.as_str() {
                Some("on") => Switch::On,
                None if e.id == PHPAT.id && v.is_table() => Switch::On,
                Some("off") => Switch::Off,
                Some("auto") => bail!(
                    "{at}: [extensions] `{name}` = \"auto\" was removed: an extension is on only when pun.toml says so; use \"on\" or \"off\" (`pun migrate` writes the ones composer.lock shows)"
                ),
                _ if e.id == PHPAT.id => bail!(
                    "{at}: [extensions] `{name}` = {v}: expected \"on\", \"off\" or a table of phpat settings"
                ),
                _ => bail!("{at}: [extensions] `{name}` = {v}: expected \"on\" or \"off\""),
            };
            Ok((e.id, s))
        })
        .collect()
}
