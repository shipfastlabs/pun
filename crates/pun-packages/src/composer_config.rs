//! `composer config` (Composer 2.11.2's `ConfigCommand`), with what it stands on: `Config` (the defaults, the merging
//! of the global config.json, composer.json and the auth files, and where each value comes from) and
//! `JsonConfigSource` (writing a setting with `JsonManipulator`, else rewriting the file).

use crate::error::{Result, bail};
use crate::manifest_edit::Php;
use pcre2::bytes::Regex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::rc::Rc;

/// How a setting's environment variable is read: its value, if it is set.
pub type Env = Rc<dyn Fn(&str) -> Option<String>>;

mod command;
mod source;

pub use command::{Dirs, Options, Outcome, run};

/// `Config::$defaultConfig`, in its order.
const DEFAULTS: &str = r#"{
        "process-timeout": 300, "use-include-path": false, "allow-plugins": [], "use-parent-dir": "prompt",
        "preferred-install": "dist", "audit": {"ignore": [], "abandoned": "fail"}, "policy": true,
        "notify-on-install": true, "github-protocols": ["https", "ssh", "git"], "gitlab-protocol": null,
        "vendor-dir": "vendor", "bin-dir": "{$vendor-dir}/bin", "cache-dir": "{$home}/cache", "data-dir": "{$home}",
        "cache-files-dir": "{$cache-dir}/files", "cache-repo-dir": "{$cache-dir}/repo",
        "cache-vcs-dir": "{$cache-dir}/vcs", "cache-ttl": 15552000, "cache-files-ttl": null,
        "cache-files-maxsize": "300MiB", "cache-read-only": false, "bin-compat": "auto", "discard-changes": false,
        "autoloader-suffix": null, "sort-packages": false, "optimize-autoloader": false,
        "strict-psr-autoloader": false, "classmap-authoritative": false, "apcu-autoloader": false,
        "prepend-autoloader": true, "update-with-minimal-changes": false, "github-domains": ["github.com"],
        "bitbucket-expose-hostname": true, "disable-tls": false, "secure-http": true, "secure-svn-domains": [],
        "cafile": null, "capath": null, "github-expose-hostname": true, "gitlab-domains": ["gitlab.com"],
        "store-auths": "prompt", "platform": [], "archive-format": "tar", "archive-dir": ".",
        "htaccess-protect": true, "use-github-api": true, "lock": true, "platform-check": "php-only",
        "bitbucket-oauth": [], "github-oauth": [], "gitlab-oauth": [], "gitlab-token": [], "http-basic": [],
        "bearer": [], "custom-headers": [], "bump-after-update": false, "allow-missing-requirements": false,
        "client-certificate": [], "forgejo-domains": ["codeberg.org"], "forgejo-token": [],
        "source-fallback": false
    }"#;

fn defaults() -> Php {
    Php::assoc(&serde_json::from_str(DEFAULTS).expect("valid defaults"))
}

/// The config settings whose schema allows an object: an empty one shows as `{}`.
const OBJECT_SETTINGS: [&str; 14] = [
    "platform",
    "allow-plugins",
    "preferred-install",
    "audit",
    "policy",
    "github-oauth",
    "gitlab-oauth",
    "gitlab-token",
    "bearer",
    "custom-headers",
    "forgejo-token",
    "http-basic",
    "client-certificate",
    "bitbucket-oauth",
];

/// The settings that merge into what is already there: auth and certificates.
const MERGED: [&str; 9] = [
    "bitbucket-oauth",
    "github-oauth",
    "gitlab-oauth",
    "gitlab-token",
    "http-basic",
    "bearer",
    "custom-headers",
    "client-certificate",
    "forgejo-token",
];

/// PHP's `(int)` of an array key that is an integer.
fn is_int_key(key: &str) -> bool {
    key == "0"
        || (key.starts_with(|c: char| c.is_ascii_digit() && c != '0')
            && key.bytes().all(|b| b.is_ascii_digit()))
        || (key.starts_with('-') && key.len() > 1 && is_int_key(&key[1..]) && key != "-0")
}

fn null() -> Php {
    Php::Scalar(Value::Null)
}

fn string(s: &str) -> Php {
    Php::Scalar(Value::String(s.into()))
}

/// The entries of an array or object; none for a scalar.
fn entries(v: &Php) -> &[(String, Php)] {
    match v {
        Php::Array(e) | Php::Object(e) => e,
        Php::Scalar(_) => &[],
    }
}

/// PHP's `isset`: there and not null.
fn isset(v: Option<&Php>) -> bool {
    v.is_some_and(|v| *v != null())
}

/// `array_merge`: integer keys of both renumbered after each other, string keys of the second replacing the first's.
fn array_merge(a: &Php, b: &Php) -> Php {
    let mut out: Vec<(String, Php)> = vec![];
    let mut next = 0;
    for (k, v) in entries(a).iter().chain(entries(b)) {
        if is_int_key(k) {
            out.push((next.to_string(), v.clone()));
            next += 1;
        } else {
            match out.iter_mut().find(|(o, _)| o == k) {
                Some((_, o)) => *o = v.clone(),
                None => out.push((k.clone(), v.clone())),
            }
        }
    }
    Php::Array(out)
}

/// PHP's string conversion of a scalar: true as "1", false and null as "".
fn php_string(v: &Php) -> String {
    match v {
        Php::Scalar(Value::String(s)) => s.clone(),
        Php::Scalar(Value::Bool(true)) => "1".into(),
        Php::Scalar(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// PHP's truthiness.
fn truthy(v: &Php) -> bool {
    match v {
        Php::Scalar(Value::Null | Value::Bool(false)) => false,
        Php::Scalar(Value::Number(n)) => n.as_f64() != Some(0.0),
        Php::Scalar(Value::String(s)) => !(s.is_empty() || s == "0"),
        Php::Array(e) => !e.is_empty(),
        _ => true,
    }
}

/// `$value !== 'false' && (bool) $value`.
fn boolean(v: &Php) -> bool {
    *v != string("false") && truthy(v)
}

/// `JsonFile::encode` without pretty printing: `escape` escapes slashes and non-ASCII as `json_encode` does by
/// default.
fn encode(v: &Php, escape: bool) -> String {
    match v {
        Php::Scalar(s) if escape => crate::php::json_compact(s),
        Php::Scalar(s) => crate::php::json_pretty(s),
        Php::Array(e) if e.iter().enumerate().all(|(i, (k, _))| *k == i.to_string()) => {
            let items: Vec<String> = e.iter().map(|(_, v)| encode(v, escape)).collect();
            format!("[{}]", items.join(","))
        }
        Php::Array(e) | Php::Object(e) => {
            let items: Vec<String> = (e.iter())
                .map(|(k, v)| format!("{}:{}", encode(&string(k), escape), encode(v, escape)))
                .collect();
            format!("{{{}}}", items.join(","))
        }
    }
}

/// `JsonFile::encode` pretty printed with `indent`, as `JsonFile::write` writes a file (with a newline).
fn encode_pretty(v: &Php, indent: &str) -> String {
    fn put(out: &mut String, v: &Php, depth: usize, indent: &str) {
        let (items, object) = match v {
            Php::Scalar(s) => return out.push_str(&crate::php::json_pretty(s)),
            Php::Array(e) if e.iter().enumerate().all(|(i, (k, _))| *k == i.to_string()) => {
                (e, false)
            }
            Php::Array(e) | Php::Object(e) => (e, true),
        };
        if items.is_empty() {
            out.push_str(if matches!(v, Php::Object(_)) {
                "{}"
            } else {
                "[]"
            });
            return;
        }
        out.push(if object { '{' } else { '[' });
        for (i, (k, item)) in items.iter().enumerate() {
            out.push_str(if i > 0 { ",\n" } else { "\n" });
            out.push_str(&indent.repeat(depth + 1));
            if object {
                let _ = write!(
                    out,
                    "{}: ",
                    crate::php::json_pretty(&Value::String(k.clone()))
                );
            }
            put(out, item, depth + 1, indent);
        }
        out.push('\n');
        out.push_str(&indent.repeat(depth));
        out.push(if object { '}' } else { ']' });
    }
    let mut out = String::new();
    put(&mut out, v, 0, indent);
    out.push('\n');
    out
}

/// Composer's `Config`: the settings with their defaults, what merged into them, and where each value came from.
pub struct Config {
    settings: Php,
    repositories: Php,
    sources: HashMap<String, String>,
    base_dir: Option<String>,
    env: Env,
}

impl Config {
    /// The defaults, relative paths resolved from `base_dir`, the environment read with `env`.
    pub fn new(base_dir: Option<String>, env: Env) -> Config {
        let mut config = Config {
            settings: defaults(),
            repositories: Php::assoc(
                &json!({"packagist.org": {"type": "composer", "url": "https://repo.packagist.org"}}),
            ),
            sources: HashMap::new(),
            base_dir,
            env,
        };
        for (key, value) in entries(&config.settings.clone()) {
            config.set_source(value, key, "default");
        }
        for (key, value) in entries(&config.repositories.clone()) {
            config.set_source(value, &format!("repositories.{key}"), "default");
        }
        config
    }

    pub fn set_base_dir(&mut self, base_dir: Option<String>) {
        self.base_dir = base_dir;
    }

    fn set_source(&mut self, value: &Php, path: &str, source: &str) {
        self.sources.insert(path.into(), source.into());
        if matches!(value, Php::Array(_)) {
            for (key, item) in entries(value) {
                self.set_source(item, &format!("{path}.{key}"), source);
            }
        }
    }

    /// `merge`: the `config` and `repositories` of `data` (a decoded file) over what is there, from `source`.
    pub fn merge(&mut self, data: &Php, source: &str) {
        if let Some(config @ Php::Array(_)) = data.get("config").filter(|c| truthy(c)) {
            for (key, value) in entries(config) {
                self.merge_setting(key, value, source);
            }
        }
        if let Some(repositories @ Php::Array(_)) = data.get("repositories").filter(|r| truthy(r)) {
            self.merge_repositories(repositories, source);
        }
    }

    fn merge_setting(&mut self, key: &str, value: &Php, source: &str) {
        let current = self.settings.get(key).cloned();
        let merged = if MERGED.contains(&key) && isset(current.as_ref()) {
            array_merge(current.as_ref().expect("set"), value)
        } else if key == "allow-plugins"
            && matches!(current, Some(Php::Array(_)))
            && matches!(value, Php::Array(_))
        {
            array_merge(&array_merge(value, current.as_ref().expect("set")), value)
        } else if matches!(key, "gitlab-domains" | "github-domains") && isset(current.as_ref()) {
            let all = array_merge(current.as_ref().expect("set"), value);
            let mut unique: Vec<(String, Php)> = vec![];
            for (k, v) in entries(&all) {
                if !unique.iter().any(|(_, u)| u == v) {
                    unique.push((k.clone(), v.clone()));
                }
            }
            Php::Array(unique)
        } else if key == "preferred-install" && isset(current.as_ref()) {
            self.merge_preferred_install(current.expect("set"), value.clone(), source)
        } else if key == "audit" {
            let ignores = self
                .settings
                .get("audit")
                .and_then(|a| a.get("ignore"))
                .cloned()
                .unwrap_or(Php::Array(vec![]));
            let mut audit = array_merge(
                self.settings.get("audit").unwrap_or(&Php::Array(vec![])),
                value,
            );
            audit.set(
                "ignore",
                array_merge(&ignores, value.get("ignore").unwrap_or(&Php::Array(vec![]))),
            );
            audit
        } else if key == "policy" {
            Self::merge_policy(current, value.clone())
        } else {
            value.clone()
        };
        self.settings.set(key, merged);
        self.set_source(value, key, source);
    }

    fn merge_preferred_install(&mut self, current: Php, value: Php, source: &str) -> Php {
        if !matches!(value, Php::Array(_)) && !matches!(current, Php::Array(_)) {
            return value;
        }
        let wrap = |v: Php| match v {
            Php::Scalar(Value::String(_)) => Php::Array(vec![("*".into(), v)]),
            other => other,
        };
        if matches!(current, Php::Scalar(Value::String(_))) {
            self.sources
                .insert("preferred-install*".into(), source.into());
        }
        let mut merged = array_merge(&wrap(current), &wrap(value));
        if let Php::Array(e) = &mut merged
            && let Some(at) = e.iter().position(|(k, _)| k == "*")
        {
            let wildcard = e.remove(at);
            e.push(wildcard);
        }
        merged
    }

    fn merge_policy(current: Option<Php>, value: Php) -> Php {
        let value = if value == Php::Scalar(Value::Bool(true)) {
            Php::Array(vec![])
        } else {
            value
        };
        if value == Php::Scalar(Value::Bool(false)) {
            return value;
        }
        let Php::Array(lists) = value else {
            return current.unwrap_or(Php::Scalar(Value::Bool(true)));
        };
        let mut policy = match current {
            Some(array @ Php::Array(_)) => array,
            _ => Php::Array(vec![]),
        };
        for (name, list) in lists {
            if name == "ignore-unreachable" {
                policy.set(&name, list);
                continue;
            }
            let list = if list == Php::Scalar(Value::Bool(true)) {
                Php::Array(vec![])
            } else {
                list
            };
            let existing = match policy.get(&name).cloned() {
                Some(Php::Scalar(Value::Bool(true))) => Some(Php::Array(vec![])),
                other => other,
            };
            let merged = match (&existing, &list) {
                (Some(existing @ Php::Array(_)), Php::Array(_)) => {
                    let mut merged = array_merge(existing, &list);
                    for inner in ["ignore", "ignore-id", "ignore-severity", "ignore-source"] {
                        if let (Some(a @ Php::Array(_)), Some(b @ Php::Array(_))) =
                            (existing.get(inner), list.get(inner))
                        {
                            merged.set(inner, array_merge(a, b));
                        }
                    }
                    merged
                }
                _ => list.clone(),
            };
            policy.set(&name, merged);
        }
        policy
    }

    /// The `repositories` of a file over the defaults: in reverse, a `false` one turning one off, a composer
    /// repository on packagist.org turning the default one off.
    fn merge_repositories(&mut self, repositories: &Php, source: &str) {
        let mut current: Vec<(String, Php)> =
            entries(&self.repositories).iter().rev().cloned().collect();
        let disable = |current: &mut Vec<(String, Php)>, name: &str| {
            if let Some(at) = current.iter().position(|(k, _)| k == name) {
                current.remove(at);
            } else if name == "packagist" {
                current.retain(|(k, _)| k != "packagist.org");
            }
        };
        let packagist =
            Regex::new(r"^https?://(?:[a-z0-9-.]+\.)?packagist.org(/|$)").expect("valid pattern");
        for (name, repository) in entries(repositories).iter().rev() {
            if *repository == Php::Scalar(Value::Bool(false)) {
                disable(&mut current, name);
                continue;
            }
            if let [(key, Php::Scalar(Value::Bool(false)))] = entries(repository) {
                disable(&mut current, key);
                continue;
            }
            if repository.get("type") == Some(&string("composer"))
                && let Some(Php::Scalar(Value::String(url))) = repository.get("url")
                && packagist.is_match(url.as_bytes()).unwrap_or(false)
            {
                disable(&mut current, "packagist.org");
            }
            let key = if is_int_key(name) {
                if current.iter().any(|(k, _)| k == name) {
                    let next = (current.iter())
                        .filter_map(|(k, _)| k.parse::<i64>().ok().filter(|_| is_int_key(k)))
                        .max()
                        .map_or(0, |m| m + 1);
                    next.to_string()
                } else {
                    name.clone()
                }
            } else if name == "packagist" {
                "packagist.org".to_string()
            } else {
                name.clone()
            };
            match current.iter_mut().find(|(k, _)| *k == key) {
                Some((_, v)) => *v = repository.clone(),
                None => current.push((key.clone(), repository.clone())),
            }
            self.set_source(repository, &format!("repositories.{key}"), source);
        }
        current.reverse();
        self.repositories = Php::Array(current);
    }

    pub fn repositories(&self) -> &Php {
        &self.repositories
    }

    /// `process`: each `{$name}` replaced by that setting.
    fn process(&mut self, value: &Php, relative: bool) -> Result<Php> {
        let Php::Scalar(Value::String(text)) = value else {
            return Ok(value.clone());
        };
        let re = Regex::new(r"\{\$(.+)\}").expect("valid pattern");
        let Ok(Some(c)) = re.captures(text.as_bytes()) else {
            return Ok(value.clone());
        };
        let whole = c.get(0).expect("the match");
        let name = String::from_utf8_lossy(&c[1]).into_owned();
        let replacement = php_string(&self.get(&name, relative)?);
        Ok(string(&format!(
            "{}{replacement}{}",
            &text[..whole.start()],
            &text[whole.end()..]
        )))
    }

    /// `Platform::expandPath`: `~/` as the user's home, a leading `$VAR` as that variable.
    fn expand_path(&self, path: &str) -> String {
        if let Some(rest) = path.strip_prefix('~').filter(|r| r.starts_with('/')) {
            return format!("{}{rest}", (self.env)("HOME").unwrap_or_default());
        }
        if let Some(rest) = path.strip_prefix('$') {
            let end = rest
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            if end > 0
                && let Some(value) = (self.env)(&rest[..end])
            {
                return format!("{value}{}", &rest[end..]);
            }
        }
        path.to_string()
    }

    fn realpath(&self, path: &str) -> String {
        let absolute = Regex::new(r"(?i)^(?:/|[a-z]:|[a-z0-9.]+://|\\\\)").expect("valid pattern");
        match &self.base_dir {
            Some(base) if !absolute.is_match(path.as_bytes()).unwrap_or(false) => {
                format!("{base}/{path}")
            }
            _ => path.to_string(),
        }
    }

    fn raw(&self, key: &str) -> Php {
        self.settings.get(key).cloned().unwrap_or_else(null)
    }

    /// `get`: a setting as Composer uses it, `*-dir` ones absolute unless `relative`; Composer's error for a value
    /// it can't use.
    pub fn get(&mut self, key: &str, relative: bool) -> Result<Php> {
        Ok(match key {
            "vendor-dir" | "bin-dir" | "process-timeout" | "data-dir" | "cache-dir"
            | "cache-files-dir" | "cache-repo-dir" | "cache-vcs-dir" | "cafile" | "capath" => {
                self.get_path(key, relative)?
            }
            "cache-read-only" | "htaccess-protect" => {
                let env = format!("COMPOSER_{}", key.to_uppercase().replace('-', "_"));
                let value = match (self.env)(&env) {
                    Some(v) => {
                        self.sources.insert(key.into(), env);
                        string(&v)
                    }
                    None => self.raw(key),
                };
                Php::Scalar(Value::Bool(boolean(&value)))
            }
            "disable-tls"
            | "secure-http"
            | "use-github-api"
            | "lock"
            | "source-fallback"
            | "strict-psr-autoloader" => {
                if key == "secure-http"
                    && self.get("disable-tls", false)? == Php::Scalar(Value::Bool(true))
                {
                    return Ok(Php::Scalar(Value::Bool(false)));
                }
                Php::Scalar(Value::Bool(boolean(&self.raw(key))))
            }
            "cache-ttl" => Php::Scalar(json!(intval(&php_string(&self.raw(key))).max(0))),
            "cache-files-maxsize" => self.get_maxsize()?,
            "cache-files-ttl" => match self.raw(key) {
                Php::Scalar(Value::Null) => self.get("cache-ttl", relative)?,
                value => Php::Scalar(json!(intval(&php_string(&value)).max(0))),
            },
            "home" => {
                let home = self.expand_path(&php_string(&self.raw(key)));
                string(
                    php_string(&self.process(&string(&home), relative)?)
                        .trim_end_matches(['/', '\\']),
                )
            }
            "bin-compat" => {
                let value = (self.env)("COMPOSER_BIN_COMPAT")
                    .filter(|v| !v.is_empty() && v != "0")
                    .map_or_else(|| self.raw(key), |v| string(&v));
                if !["auto", "full", "proxy", "symlink"]
                    .iter()
                    .any(|w| value == string(w))
                {
                    bail!(
                        "Invalid value for 'bin-compat': {}. Expected auto, full or proxy",
                        php_string(&value)
                    );
                }
                value
            }
            "discard-changes" => self.get_discard_changes()?,
            "github-protocols" => {
                let mut protocols = self.raw(key);
                if boolean(&self.raw("secure-http"))
                    && let Php::Array(e) = &mut protocols
                    && let Some(at) = e.iter().position(|(_, v)| *v == string("git"))
                {
                    e.remove(at);
                }
                if entries(&protocols)
                    .first()
                    .is_some_and(|(_, v)| *v == string("http"))
                {
                    bail!(
                        "The http protocol for github is not available anymore, update your config's github-protocols to use \"https\", \"git\" or \"ssh\""
                    );
                }
                protocols
            }
            "autoloader-suffix" if self.raw(key) == string("") => null(),
            "audit" => self.get_audit()?,
            "policy" => match self.bool_env("COMPOSER_POLICY")? {
                Some(false) => Php::Scalar(Value::Bool(false)),
                Some(true) if self.raw(key) == Php::Scalar(Value::Bool(false)) => {
                    Php::Scalar(Value::Bool(true))
                }
                _ => self.raw(key),
            },
            _ => {
                let value = self.raw(key);
                if value == null() {
                    return Ok(value);
                }
                self.process(&value, relative)?
            }
        })
    }

    /// `Platform::getBoolEnv`: unset or empty as `None`, else one of Composer's words for a boolean.
    fn bool_env(&self, name: &str) -> Result<Option<bool>> {
        match (self.env)(name).filter(|v| !v.is_empty()) {
            None => Ok(None),
            Some(v) if ["1", "true", "on"].contains(&v.as_str()) => Ok(Some(true)),
            Some(v) if ["0", "false", "off"].contains(&v.as_str()) => Ok(Some(false)),
            Some(v) => {
                bail!("Invalid value for {name}: {v}. Expected 0, 1, false, true, off, or on.")
            }
        }
    }

    fn get_discard_changes(&self) -> Result<Php> {
        if let Some(env) = (self.env)("COMPOSER_DISCARD_CHANGES") {
            if !["stash", "true", "false", "1", "0"].contains(&env.as_str()) {
                bail!(
                    "Invalid value for COMPOSER_DISCARD_CHANGES: {env}. Expected 1, 0, true, false or stash"
                );
            }
            if env == "stash" {
                return Ok(string("stash"));
            }
            return Ok(Php::Scalar(Value::Bool(boolean(&string(&env)))));
        }
        let value = self.raw("discard-changes");
        if !matches!(value, Php::Scalar(Value::Bool(_))) && value != string("stash") {
            bail!(
                "Invalid value for 'discard-changes': {}. Expected true, false or stash",
                php_string(&value)
            );
        }
        Ok(value)
    }

    fn get_audit(&self) -> Result<Php> {
        let mut audit = self.raw("audit");
        if let Some(abandoned) = (self.env)("COMPOSER_AUDIT_ABANDONED") {
            if !["ignore", "report", "fail"].contains(&abandoned.as_str()) {
                bail!(
                    "Invalid value for COMPOSER_AUDIT_ABANDONED: {abandoned}. Expected one of ignore, report, fail."
                );
            }
            audit.set("abandoned", string(&abandoned));
        }
        if (self.env)("COMPOSER_SECURITY_BLOCKING_ABANDONED").is_some() {
            let block = self.bool_env("COMPOSER_SECURITY_BLOCKING_ABANDONED")?;
            audit.set(
                "block-abandoned",
                block.map_or_else(null, |b| Php::Scalar(Value::Bool(b))),
            );
        }
        Ok(audit)
    }

    fn get_path(&mut self, key: &str, relative: bool) -> Result<Php> {
        let env = format!("COMPOSER_{}", key.to_uppercase().replace('-', "_"));
        let from_env = (self.env)(&env);
        if from_env.is_some() {
            self.sources.insert(key.into(), env);
        }
        if key == "process-timeout" {
            let value =
                from_env.map_or_else(|| intval(&php_string(&self.raw(key))), |v| intval(&v));
            return Ok(Php::Scalar(json!(value.max(0))));
        }
        let value = from_env.map_or_else(|| self.raw(key), |v| string(&v));
        let processed = php_string(&self.process(&value, relative)?);
        let value = self.expand_path(processed.trim_end_matches(['/', '\\']));
        if !key.ends_with("-dir") || relative {
            return Ok(string(&value));
        }
        Ok(string(&self.realpath(&value)))
    }

    fn get_maxsize(&self) -> Result<Php> {
        let raw = php_string(&self.raw("cache-files-maxsize"));
        let re =
            Regex::new(r"(?i)^\s*([0-9.]+)\s*(?:([kmg])(?:i?b)?)?\s*$").expect("valid pattern");
        let Ok(Some(c)) = re.captures(raw.as_bytes()) else {
            bail!("Could not parse the value of 'cache-files-maxsize': {raw}");
        };
        let mut size: f64 = String::from_utf8_lossy(&c[1]).parse().unwrap_or(0.0);
        let times = match c.get(2).map(|u| u.as_bytes()[0].to_ascii_lowercase()) {
            Some(b'g') => 3,
            Some(b'm') => 2,
            Some(_) => 1,
            None => 0,
        };
        for _ in 0..times {
            size *= 1024.0;
        }
        #[expect(clippy::cast_possible_truncation, reason = "PHP's (int) of a float")]
        Ok(Php::Scalar(json!((size as i64).max(0))))
    }

    /// `all`: the repositories and every setting as `get` gives it.
    pub fn all(&mut self, relative: bool) -> Result<Php> {
        let keys: Vec<String> = entries(&self.settings)
            .iter()
            .map(|(k, _)| k.clone())
            .collect();
        let mut config = vec![];
        for key in keys {
            let value = self.get(&key, relative)?;
            config.push((key, value));
        }
        Ok(Php::Array(vec![
            ("repositories".into(), self.repositories.clone()),
            ("config".into(), Php::Array(config)),
        ]))
    }

    /// `raw`: the repositories and every setting as merged.
    pub fn raw_all(&self) -> Php {
        Php::Array(vec![
            ("repositories".into(), self.repositories.clone()),
            ("config".into(), self.settings.clone()),
        ])
    }

    /// `getSourceOfValue`.
    pub fn source_of(&mut self, key: &str) -> Result<String> {
        self.get(key, false)?;
        Ok(self
            .sources
            .get(key)
            .cloned()
            .unwrap_or_else(|| "unknown".into()))
    }
}

/// PHP's `(int)` of a string: its leading number, cut to an integer.
fn intval(s: &str) -> i64 {
    let s = s.trim_start();
    let end = s
        .char_indices()
        .take_while(|&(i, c)| {
            c.is_ascii_digit()
                || (i == 0 && (c == '-' || c == '+'))
                || c == '.'
                || c == 'e'
                || c == 'E'
        })
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    let number = &s[..end];
    if number.contains(['.', 'e', 'E']) {
        #[expect(clippy::cast_possible_truncation, reason = "PHP's (int) of a float")]
        return number.parse::<f64>().map_or(0, |f| f as i64);
    }
    number.parse().unwrap_or(0)
}
