//! `JsonConfigSource`: a setting written into a JSON file with `JsonManipulator`, keeping its layout, else (when the
//! manipulator can't) into the decoded file, which is then written again whole.

use super::{encode_pretty, entries};
use crate::error::{Context, Result, bail};
use crate::manifest_edit::{Manipulator, Php};
use pcre2::bytes::Regex;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The settings `addConfigSetting` keeps per host (and `platform`, per package).
const PER_HOST: &str = r"^(bitbucket-oauth|github-oauth|gitlab-oauth|gitlab-token|bearer|http-basic|custom-headers|forgejo-token|platform)\.";

/// A JSON file settings are written to: composer.json or config.json, or an auth file (`auth`).
pub(super) struct Source {
    pub(super) path: PathBuf,
    pub(super) auth: bool,
}

/// `$node[$key]`, made an array when it isn't one.
fn child<'a>(node: &'a mut Php, key: &str) -> &'a mut Php {
    if !matches!(node, Php::Array(_)) {
        *node = Php::Array(vec![]);
    }
    let Php::Array(e) = node else { return node };
    let at = e.iter().position(|(k, _)| k == key).unwrap_or_else(|| {
        e.push((key.to_string(), Php::Array(vec![])));
        e.len() - 1
    });
    if !matches!(e[at].1, Php::Array(_)) {
        e[at].1 = Php::Array(vec![]);
    }
    &mut e[at].1
}

/// `unset($node[$key])`.
fn unset(node: &mut Php, key: &str) {
    if let Php::Array(e) | Php::Object(e) = node {
        e.retain(|(k, _)| k != key);
    }
}

/// `$node[$a][$b]…`, when every step is an array.
fn existing<'a>(node: &'a mut Php, path: &[&str]) -> Option<&'a mut Php> {
    let mut node = node;
    for key in path {
        let Php::Array(e) = node else { return None };
        node = &mut e.iter_mut().find(|(k, _)| k == key)?.1;
    }
    matches!(node, Php::Array(_)).then_some(node)
}

/// `JsonConfigSource::restoreEmptyObjects`: the empty arrays Composer writes as `{}`.
fn restore_empty_objects(config: &mut Php) {
    let empty = |v: &Php| matches!(v, Php::Array(e) if e.is_empty());
    let object = || Php::Object(vec![]);
    if let Some(policy) = existing(config, &["config", "policy"]) {
        if let Php::Array(lists) = policy {
            for (_, list) in lists.iter_mut() {
                if empty(list) {
                    *list = object();
                }
            }
        }
        if empty(policy) {
            *policy = object();
        }
    }
    if let Some(settings) = existing(config, &["config"]) {
        for key in [
            "platform",
            "http-basic",
            "bearer",
            "gitlab-token",
            "gitlab-oauth",
            "github-oauth",
            "custom-headers",
            "forgejo-token",
            "preferred-install",
        ] {
            if settings.get(key).is_some_and(empty) {
                settings.set(key, object());
            }
        }
    }
    for main in ["autoload", "autoload-dev"] {
        if let Some(autoload) = existing(config, &[main]) {
            for key in ["psr-0", "psr-4"] {
                if autoload.get(key).is_some_and(empty) {
                    autoload.set(key, object());
                }
            }
        }
    }
    for key in [
        "require",
        "require-dev",
        "conflict",
        "provide",
        "replace",
        "suggest",
        "config",
        "autoload",
        "autoload-dev",
        "scripts",
        "scripts-descriptions",
        "scripts-aliases",
        "support",
    ] {
        if config.get(key).is_some_and(empty) {
            config.set(key, object());
        }
    }
}

/// `splitPropertyKey`: `extra.a.b` as `extra`, `a`, `b`; `suggest.a.b` as `suggest`, `a.b`.
fn property_path(key: &str) -> Vec<String> {
    let (main, name) = key.split_once('.').expect("a dotted key");
    if matches!(main, "extra" | "scripts") {
        let mut bits = vec![main.to_string()];
        bits.extend(name.splitn(2, '.').map(String::from));
        return bits;
    }
    vec![main.to_string(), name.to_string()]
}

impl Source {
    /// The file's text, or what Composer starts a new one with; and whether it is new.
    fn contents(&self) -> Result<(String, bool)> {
        if self.path.exists() {
            let text = std::fs::read_to_string(&self.path)
                .with_context(|| format!("can't read {}", self.path.display()))?;
            return Ok((text, false));
        }
        Ok((
            if self.auth {
                "{\n}\n"
            } else {
                "{\n    \"config\": {\n    }\n}\n"
            }
            .to_string(),
            true,
        ))
    }

    fn write(&self, text: &str, new: bool) -> Result<()> {
        if new {
            write_new(&self.path, text)
        } else {
            std::fs::write(&self.path, text).map_err(Into::into)
        }
        .with_context(|| format!("can't write {}", self.path.display()))
    }

    /// `manipulateJson`: `edit` with the manipulator, else `fallback` on the decoded file, written again whole.
    fn manipulate(
        &self,
        edit: impl FnOnce(&mut Manipulator) -> bool,
        fallback: impl FnOnce(&mut Php) -> Result<()>,
    ) -> Result<()> {
        let (text, new) = self.contents()?;
        let mut manipulator = Manipulator::new(&text);
        if edit(&mut manipulator) {
            return self.write(&manipulator.text(), new);
        }
        let decoded: Value =
            serde_json::from_str(&text).unwrap_or(Value::Object(serde_json::Map::new()));
        let mut config = Php::assoc(&decoded);
        fallback(&mut config)?;
        restore_empty_objects(&mut config);
        let indent = Regex::new(r#"(?m)^([ \t]+)""#)
            .expect("valid pattern")
            .captures(text.as_bytes())
            .ok()
            .flatten()
            .map_or("    ".to_string(), |c| {
                String::from_utf8_lossy(&c[1]).into_owned()
            });
        self.write(&encode_pretty(&config, &indent), new)
    }

    /// `addConfigSetting`: in an auth file through `addSubNode`, which manages any auth setting that is an object;
    /// for one that isn't, Composer's rewritten file fails its schema check, and Composer puts the file back.
    pub(super) fn add_config_setting(&self, name: &str, value: &Php) -> Result<()> {
        if self.auth {
            let (text, new) = self.contents()?;
            let mut manipulator = Manipulator::new(&text);
            let (main, rest) = name.split_once('.').expect("an auth setting has a host");
            if !manipulator.add_sub_node(main, rest, value, true) {
                bail!(
                    "Failed to update composer.json with a valid format, reverting to the original content. Please report an issue to us with details (command you run and a copy of your composer.json)."
                );
            }
            return self.write(&manipulator.text(), new);
        }
        let per_host = Regex::new(PER_HOST)
            .expect("valid pattern")
            .is_match(name.as_bytes())
            .unwrap_or(false);
        self.manipulate(
            |m| m.add_config_setting(name, value),
            |config| {
                if per_host {
                    let (key, host) = name.split_once('.').expect("a dotted key");
                    child(child(config, "config"), key).set(host, value.clone());
                } else if let Some(rest) = name.strip_prefix("policy.") {
                    let bits: Vec<&str> = rest.split('.').collect();
                    let mut node = child(child(config, "config"), "policy");
                    for bit in &bits[..bits.len() - 1] {
                        node = child(node, bit);
                    }
                    node.set(bits[bits.len() - 1], value.clone());
                } else {
                    child(config, "config").set(name, value.clone());
                }
                Ok(())
            },
        )
    }

    /// `removeConfigSetting`. The manipulator manages any file Composer reads: its fallback only follows a `config`
    /// (or an auth setting) that isn't an object, which Composer stops on before it gets here.
    pub(super) fn remove_config_setting(&self, name: &str) -> Result<()> {
        let (text, new) = self.contents()?;
        let mut manipulator = Manipulator::new(&text);
        match name.split_once('.').filter(|_| self.auth) {
            Some((main, rest)) => manipulator.remove_sub_node(main, rest),
            None => manipulator.remove_config_setting(name),
        };
        self.write(&manipulator.text(), new)
    }

    /// `addProperty`.
    pub(super) fn add_property(&self, name: &str, value: &Php) -> Result<()> {
        self.manipulate(
            |m| m.add_property(name, value),
            |config| {
                if ["suggest.", "extra.", "scripts."]
                    .iter()
                    .any(|p| name.starts_with(p))
                {
                    let bits = property_path(name);
                    let mut node = config;
                    for bit in &bits[..bits.len() - 1] {
                        node = child(node, bit);
                    }
                    node.set(&bits[bits.len() - 1], value.clone());
                } else {
                    config.set(name, value.clone());
                }
                Ok(())
            },
        )
    }

    /// `removeProperty`.
    pub(super) fn remove_property(&self, name: &str) -> Result<()> {
        self.manipulate(
            |m| m.remove_property(name),
            |config| {
                if [
                    "suggest.",
                    "extra.",
                    "scripts.",
                    "autoload.",
                    "autoload-dev.",
                ]
                .iter()
                .any(|p| name.starts_with(p))
                {
                    let bits = property_path(name);
                    let path: Vec<&str> =
                        bits[..bits.len() - 1].iter().map(String::as_str).collect();
                    let Some(node) = existing(config, &path) else {
                        return Ok(());
                    };
                    let was_list = entries(node)
                        .iter()
                        .enumerate()
                        .all(|(i, (k, _))| *k == i.to_string());
                    unset(node, &bits[bits.len() - 1]);
                    if was_list {
                        let renumbered = (entries(node).iter().enumerate())
                            .map(|(i, (_, v))| (i.to_string(), v.clone()))
                            .collect();
                        *node = Php::Array(renumbered);
                    } else if entries(node).is_empty() {
                        *node = Php::Object(vec![]);
                    }
                } else {
                    unset(config, name);
                }
                Ok(())
            },
        )
    }

    /// `addRepository`. The manipulator manages any file whose `repositories` is a list, an object or not there;
    /// for any other Composer falls back to rewriting the file, which PHP stops with its error.
    pub(super) fn add_repository(&self, name: &str, repository: &Php, append: bool) -> Result<()> {
        let (text, new) = self.contents()?;
        let mut manipulator = Manipulator::new(&text);
        if !manipulator.add_repository(name, repository, append) {
            let found = (serde_json::from_str::<Value>(&text).ok())
                .and_then(|v| v.get("repositories").cloned())
                .unwrap_or(Value::Null);
            let kind = match found {
                Value::Bool(b) => {
                    if b {
                        "true"
                    } else {
                        "false"
                    }
                }
                Value::Number(n) if n.is_f64() => "float",
                Value::Number(_) => "int",
                _ => "string",
            };
            bail!("array_is_list(): Argument #1 ($array) must be of type array, {kind} given");
        }
        self.write(&manipulator.text(), new)
    }

    /// `removeRepository`. The manipulator manages any file (a `repositories` it can't read has nothing to remove).
    pub(super) fn remove_repository(&self, name: &str) -> Result<()> {
        let (text, new) = self.contents()?;
        let mut manipulator = Manipulator::new(&text);
        manipulator.remove_repository(name);
        self.write(&manipulator.text(), new)
    }
}

/// Composer's error for a file it can't decode.
pub(super) fn read(path: &std::path::Path, shown: &str) -> Result<Php> {
    let text = std::fs::read(path).with_context(|| format!("can't read {}", path.display()))?;
    match serde_json::from_slice::<Value>(&text) {
        Ok(value) => Ok(Php::assoc(&value)),
        Err(_) => bail!("\"{shown}\" does not contain valid JSON"),
    }
}

/// A new file with `text`, readable by its owner only: created with mode 0600 (Composer chmods it so after writing),
/// so an auth file is never readable by others, not even for a moment.
pub(super) fn write_new(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(text.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Composer 2.11.2 leaves a config or auth file it creates at mode 0600, whatever the umask.
    #[test]
    fn new_files_are_private() {
        let dir = std::env::temp_dir().join(format!("pun-config-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("auth.json");
        write_new(&file, "{\n}\n").unwrap();
        let meta = std::fs::metadata(&file).unwrap();
        assert_eq!(
            (
                meta.permissions().mode() & 0o777,
                std::fs::read_to_string(&file).unwrap()
            ),
            (0o600, "{\n}\n".to_string())
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
