//! Composer 2.11.2's `ValidatingArrayLoader::load`, as `ConfigValidator` runs it: each check in its order, on a copy of
//! the config that a failed check removes its value from, as the PHP `unset()`s it.

use crate::constraint::{self, Constraint, Op, matches_nothing};
use crate::php::{is_email, json_compact, parse_url};
use crate::version::{is_platform, normalize, normalize_branch};
use pcre2::bytes::{Regex, RegexBuilder};
use serde_json::{Map, Value};
use std::sync::LazyLock;

/// The errors and warnings of one load.
#[derive(Default)]
pub(super) struct Loaded {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// `ValidatingArrayLoader::load` with `CHECK_ALL` when `constraints`, else no constraint checks.
pub(super) fn load(config: Map<String, Value>, constraints: bool) -> Loaded {
    let mut l = Loader {
        config,
        out: Loaded::default(),
    };
    l.run(constraints);
    l.out
}

struct Loader {
    config: Map<String, Value>,
    out: Loaded,
}

/// PHP's `isset()` on a decoded value.
fn isset(v: Option<&Value>) -> bool {
    v.is_some_and(|v| !v.is_null())
}

/// PHP's `empty()` on a decoded value.
fn empty(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => !b,
        Some(Value::Number(n)) => n.as_f64() == Some(0.0),
        Some(Value::String(s)) => s.is_empty() || s == "0",
        Some(Value::Array(a)) => a.is_empty(),
        Some(Value::Object(o)) => o.is_empty(),
    }
}

/// `get_debug_type()` of a decoded value; a JSON object decodes to an array.
fn debug_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_i64() || n.is_u64() => "int",
        Value::Number(_) => "float",
        Value::String(_) => "string",
        Value::Array(_) | Value::Object(_) => "array",
    }
}

/// A value as PHP's string interpolation shows it: a number's PHP form, a bool as "1" or "".
fn php_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "1".into(),
        Value::Bool(false) | Value::Null => String::new(),
        Value::Number(n) => n.to_string(),
        Value::Array(_) | Value::Object(_) => "Array".into(),
    }
}

/// `var_export()` of a decoded value, as `config.platform`'s message shows it: an array one entry a line, its keys as
/// PHP keeps them (a numeric string becomes an integer key).
fn var_export(v: &Value) -> String {
    let mut out = String::new();
    export(&mut out, v, 0);
    out
}

fn export(out: &mut String, v: &Value, level: usize) {
    match v {
        Value::Null => out.push_str("NULL"),
        Value::Bool(b) => out.push_str(&b.to_string()),
        Value::String(s) => out.push_str(&crate::php::export_str(s)),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::Array(_) | Value::Object(_) => {
            out.push_str("array (\n");
            let pad = "  ".repeat(level + 1);
            for (key, item) in entries(v) {
                out.push_str(&pad);
                match key.parse::<i64>() {
                    Ok(i) if i.to_string() == key => out.push_str(&key),
                    _ => out.push_str(&crate::php::export_str(&key)),
                }
                out.push_str(" => ");
                if is_array(&item) {
                    out.push('\n');
                    out.push_str(&pad);
                }
                export(out, &item, level + 1);
                out.push_str(",\n");
            }
            out.push_str(&"  ".repeat(level));
            out.push(')');
        }
    }
}

/// PHP's `trim()`.
fn trim(s: &str) -> &str {
    s.trim_matches([' ', '\t', '\n', '\r', '\0', '\x0B'])
}

/// The entries of a decoded array, by PHP key: a list's index, an object's key.
fn entries(v: &Value) -> Vec<(String, Value)> {
    match v {
        Value::Array(items) => (items.iter().enumerate())
            .map(|(i, v)| (i.to_string(), v.clone()))
            .collect(),
        Value::Object(map) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => vec![],
    }
}

/// A PHP array as a map by key; `None` for a value that isn't one.
fn as_map(v: &Value) -> Option<Map<String, Value>> {
    is_array(v).then(|| entries(v).into_iter().collect())
}

fn is_array(v: &Value) -> bool {
    matches!(v, Value::Array(_) | Value::Object(_))
}

fn count(v: &Value) -> usize {
    match v {
        Value::Array(a) => a.len(),
        Value::Object(o) => o.len(),
        _ => 1,
    }
}

/// A `Preg` pattern: `{^...$}` with the `u` flag where Composer gives it.
fn pattern(regex: &str, utf: bool) -> Regex {
    RegexBuilder::new()
        .utf(utf)
        .ucp(utf)
        .build(&format!("^(?:{regex})$"))
        .expect("Composer's pattern")
}

fn is_match(re: &Regex, s: &str) -> bool {
    re.is_match(s.as_bytes()).unwrap_or(false)
}

/// `ValidatingArrayLoader::hasPackageNamingError`.
pub(super) fn package_naming_error(name: &str, is_link: bool) -> Option<String> {
    static NAME: LazyLock<Regex> = LazyLock::new(|| {
        RegexBuilder::new()
            .caseless(true)
            .build(r"^[a-z0-9](?:[_.-]?[a-z0-9]++)*+/[a-z0-9](?:(?:[_.]|-{1,2})?[a-z0-9]++)*+\z")
            .expect("valid")
    });
    const RESERVED: [&str; 22] = [
        "nul", "con", "prn", "aux", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    if is_platform(name) {
        return None;
    }
    if !NAME.is_match(name.as_bytes()).unwrap_or(false) {
        return Some(format!(
            "{name} is invalid, it should have a vendor name, a forward slash, and a package name. The vendor and package name can be words separated by -, . or _. The complete name should match \"^[a-z0-9]([_.-]?[a-z0-9]+)*/[a-z0-9](([_.]?|-{{0,2}})[a-z0-9]+)*$\"."
        ));
    }
    let lower = name.to_ascii_lowercase();
    let mut bits = lower.split('/');
    let (vendor, package) = (bits.next().unwrap_or(""), bits.next().unwrap_or(""));
    if RESERVED.contains(&vendor) || RESERVED.contains(&package) {
        return Some(format!(
            "{name} is reserved, package and vendor names can not match any of: {}.",
            RESERVED.join(", ")
        ));
    }
    #[expect(
        clippy::case_sensitive_file_extension_comparisons,
        reason = "Composer's pattern, {\\.json$}, has no i flag"
    )]
    let json = name.ends_with(".json");
    if json {
        return Some(format!(
            "{name} is invalid, package names can not end in .json, consider renaming it or perhaps using a -json suffix instead."
        ));
    }
    if name.bytes().any(|b| b.is_ascii_uppercase()) {
        if is_link {
            return Some(format!(
                "{name} is invalid, it should not contain uppercase characters. Please use {lower} instead."
            ));
        }
        return Some(format!(
            "{name} is invalid, it should not contain uppercase characters. We suggest using {} instead.",
            super::suggested_name(name)
        ));
    }
    None
}

/// `ValidatingArrayLoader::findBinError`.
fn bin_error(bin: &str) -> Option<&'static str> {
    static PARENT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?:^|[\\/])\.\.(?:[\\/]|$)").expect("valid"));
    static UNSAFE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"[*$`"&^|<>()%!;\x00-\x1f\x7f]"#).expect("valid"));
    if PARENT.is_match(bin.as_bytes()).unwrap_or(false) {
        return Some("must not contain a \"..\" path component");
    }
    if UNSAFE.is_match(bin.as_bytes()).unwrap_or(false) {
        return Some(
            "must not contain any of the characters *$`\"&^|<>()%!; nor control characters",
        );
    }
    None
}

/// `Perforce::isValidPort`.
fn perforce_port(url: &str) -> bool {
    static REMOTE_SHELL: LazyLock<Regex> = LazyLock::new(|| {
        RegexBuilder::new()
            .caseless(true)
            .build(r"^\s*+(?:rsh|jsh)\s*+:")
            .expect("valid")
    });
    static PORT: LazyLock<Regex> = LazyLock::new(|| {
        RegexBuilder::new()
            .caseless(true)
            .build(r"^(?:(?:tcp|ssl)(?:4|6|46|64)?:)?(?:\[[0-9a-f:.]++\]|[a-z0-9._][a-z0-9._-]*+)(?::[a-z0-9._][a-z0-9._-]*+)?\z")
            .expect("valid")
    });
    !REMOTE_SHELL.is_match(url.as_bytes()).unwrap_or(false)
        && PORT.is_match(url.as_bytes()).unwrap_or(false)
}

/// `ValidatingArrayLoader::filterUrl`.
fn filter_url(value: &str, schemes: &[&str]) -> bool {
    if value.is_empty() {
        return true;
    }
    let Some(bits) = parse_url(value) else {
        return false;
    };
    let text = |s: Option<String>| s.map(Value::String);
    let (scheme, host) = (text(bits.scheme), text(bits.host));
    if empty(scheme.as_ref()) || empty(host.as_ref()) {
        return false;
    }
    scheme
        .and_then(|s| s.as_str().map(String::from))
        .is_some_and(|s| schemes.contains(&s.as_str()))
}

const HTTP: [&str; 2] = ["http", "https"];

impl Loader {
    fn get(&self, key: &str) -> Option<&Value> {
        self.config.get(key)
    }

    fn unset(&mut self, key: &str) {
        self.config.shift_remove(key);
    }

    fn error(&mut self, message: String) {
        self.out.errors.push(message);
    }

    fn warning(&mut self, message: String) {
        self.out.warnings.push(message);
    }

    /// `validateString`.
    fn validate_string(&mut self, property: &str, mandatory: bool) -> bool {
        if let Some(v) = self
            .get(property)
            .filter(|v| !v.is_null() && !v.is_string())
        {
            let message = format!("{property} : should be a string, {} given", debug_type(v));
            self.error(message);
            self.unset(property);
            return false;
        }
        if self
            .get(property)
            .and_then(Value::as_str)
            .is_none_or(|s| trim(s).is_empty())
        {
            if mandatory {
                self.error(format!("{property} : must be present"));
            }
            self.unset(property);
            return false;
        }
        true
    }

    /// `validateArray`.
    fn validate_array(&mut self, property: &str, mandatory: bool) -> bool {
        if let Some(v) = self.get(property).filter(|v| !v.is_null() && !is_array(v)) {
            let message = format!("{property} : should be an array, {} given", debug_type(v));
            self.error(message);
            self.unset(property);
            return false;
        }
        if self
            .get(property)
            .is_none_or(|v| v.is_null() || count(v) == 0)
        {
            if mandatory {
                self.error(format!(
                    "{property} : must be present and contain at least one element"
                ));
            }
            self.unset(property);
            return false;
        }
        true
    }

    /// `validateRegex`, whose pattern has the `u` flag.
    fn validate_regex(&mut self, property: &str, regex: &str, mandatory: bool) -> bool {
        if !self.validate_string(property, mandatory) {
            return false;
        }
        let value = self
            .get(property)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !is_match(&pattern(regex, true), &value) {
            let message = format!("{property} : invalid value ({value}), must match {regex}");
            if mandatory {
                self.error(message);
            } else {
                self.warning(message);
            }
            self.unset(property);
            return false;
        }
        true
    }

    /// `validateFlatArray`.
    fn validate_flat_array(
        &mut self,
        property: &str,
        regex: Option<&str>,
        mandatory: bool,
    ) -> bool {
        if !self.validate_array(property, mandatory) {
            return false;
        }
        let mut pass = true;
        let mut removed = vec![];
        for (key, value) in entries(&self.config[property]) {
            if !value.is_string() && !value.is_number() {
                self.error(format!(
                    "{property}.{key} : must be a string or int, {} given",
                    debug_type(&value)
                ));
                removed.push(key);
                pass = false;
                continue;
            }
            if let Some(regex) = regex
                && !is_match(&pattern(regex, true), &php_string(&value))
            {
                self.warning(format!(
                    "{property}.{key} : invalid value ({}), must match {regex}",
                    php_string(&value)
                ));
                removed.push(key);
                pass = false;
            }
        }
        self.remove_entries(property, &removed);
        pass
    }

    /// Removes entries of the array at `property` by key, as `unset($this->config[$property][$key])` does.
    fn remove_entries(&mut self, property: &str, keys: &[String]) {
        if keys.is_empty() {
            return;
        }
        let Some(value) = self.config.get_mut(property) else {
            return;
        };
        *value = match std::mem::take(value) {
            Value::Array(items) => Value::Object(
                (items.into_iter().enumerate())
                    .filter(|(i, _)| !keys.contains(&i.to_string()))
                    .map(|(i, v)| (i.to_string(), v))
                    .collect(),
            ),
            Value::Object(mut map) => {
                for key in keys {
                    map.shift_remove(key);
                }
                Value::Object(map)
            }
            other => other,
        };
    }

    /// `validateUrl`.
    fn validate_url(&mut self, property: &str) -> bool {
        if !self.validate_string(property, false) {
            return false;
        }
        let value = self
            .get(property)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !filter_url(&value, &HTTP) {
            self.warning(format!(
                "{property} : invalid value ({value}), must be an http/https URL"
            ));
            self.unset(property);
            return false;
        }
        true
    }

    fn run(&mut self, constraints: bool) {
        // The naming check reads the config as given (`$config`), not the copy `validateString` may have unset it from.
        let name = self.get("name").and_then(Value::as_str).map(String::from);
        self.validate_string("name", true);
        if let Some(name) = name
            && let Some(error) = package_naming_error(&name, false)
        {
            self.error(format!("name : {error}"));
        }
        self.version();
        self.platform();
        self.validate_regex("type", "[A-Za-z0-9-]+", false);
        self.validate_string("target-dir", false);
        self.validate_array("extra", false);
        self.bin();
        self.validate_array("scripts", false);
        self.validate_string("description", false);
        self.validate_url("homepage");
        self.validate_flat_array("keywords", Some(r"[\p{N}\p{L} ._-]+"), false);
        let release = self.time();
        self.license(release);
        self.authors();
        self.support();
        self.funding();
        self.php_ext();
        self.links(constraints);
        self.suggest();
        self.minimum_stability();
        self.autoload();
        for kind in ["source", "dist"] {
            self.source(kind);
        }
        self.validate_flat_array("include-path", None, false);
        self.validate_array("transport-options", false);
        self.branch_alias();
    }

    fn version(&mut self) {
        let Some(version) = self.get("version").filter(|v| !v.is_null()).cloned() else {
            return;
        };
        if is_array(&version) {
            self.validate_string("version", false);
            return;
        }
        let version = php_string(&version);
        if let Err(e) = normalize(&version) {
            self.error(format!("version : invalid value ({version}): {e}"));
            self.unset("version");
        }
    }

    fn platform(&mut self) {
        let Some(platform) = self
            .get("config")
            .and_then(|c| c.get("platform"))
            .filter(|p| !p.is_null())
        else {
            return;
        };
        let pairs = if is_array(platform) {
            entries(platform)
        } else {
            vec![("0".into(), platform.clone())]
        };
        for (key, value) in pairs {
            match &value {
                Value::Bool(false) => {}
                Value::String(version) => {
                    if let Err(e) = normalize(version) {
                        self.error(format!(
                            "config.platform.{key} : invalid value ({version}): {e}"
                        ));
                    }
                }
                other => self.error(format!(
                    "config.platform.{key} : invalid value ({} {}): expected string or false",
                    debug_type(other),
                    var_export(other)
                )),
            }
        }
    }

    fn bin(&mut self) {
        if !isset(self.get("bin")) {
            return;
        }
        if self.get("bin").is_some_and(Value::is_string) {
            self.validate_string("bin", false);
        } else {
            self.validate_flat_array("bin", None, false);
        }
        match self.get("bin").cloned() {
            Some(Value::String(bin)) => {
                if let Some(error) = bin_error(&bin) {
                    self.error(format!("bin : invalid value ({bin}), {error}"));
                    self.unset("bin");
                }
            }
            Some(bins) if is_array(&bins) => {
                let mut removed = vec![];
                for (key, bin) in entries(&bins) {
                    if let Some(bin) = bin.as_str()
                        && let Some(error) = bin_error(bin)
                    {
                        self.error(format!("bin.{key} : invalid value ({bin}), {error}"));
                        removed.push(key);
                    }
                }
                self.remove_entries("bin", &removed);
            }
            _ => {}
        }
    }

    /// `time`, and the release date it gives; see [`release_timestamp`] for the formats read.
    fn time(&mut self) -> Option<i64> {
        self.validate_string("time", false);
        self.get("time")
            .and_then(Value::as_str)
            .and_then(release_timestamp)
    }

    fn license(&mut self, release: Option<i64>) {
        let Some(license) = self.get("license").filter(|l| !l.is_null()).cloned() else {
            return;
        };
        if !(license.is_string() || is_array(&license)) {
            self.warning(format!(
                "License must be a string or array of strings, got {}.",
                json_compact(&license)
            ));
            self.unset("license");
            return;
        }
        let mut licenses = vec![];
        for (_, value) in if license.is_string() {
            vec![("0".into(), license)]
        } else {
            entries(&license)
        } {
            match value {
                Value::String(s) => licenses.push(s),
                other => self.warning(format!(
                    "License {} should be a string.",
                    json_compact(&other)
                )),
            }
        }
        // Licenses of a release more than eight days old aren't checked.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        if release.is_some_and(|r| r < now - 8 * 86400) {
            return;
        }
        for license in licenses.iter().filter(|l| *l != "proprietary") {
            let checked = license.replace("proprietary", "MIT");
            if super::spdx::is_valid(&checked) {
                continue;
            }
            if super::spdx::is_valid(trim(&checked)) {
                self.warning(format!(
                    "License {} must not contain extra spaces, make sure to trim it.",
                    json_compact(&Value::String(license.clone()))
                ));
            } else {
                self.warning(format!(
                    "License {} is not a valid SPDX license identifier, see https://spdx.org/licenses/ if you use an open license.\nIf the software is closed-source, you may use \"proprietary\" as license.",
                    json_compact(&Value::String(license.clone()))
                ));
            }
        }
    }

    fn authors(&mut self) {
        if !self.validate_array("authors", false) {
            return;
        }
        let mut kept = Map::new();
        for (key, author) in entries(&self.config["authors"]) {
            let Some(mut author) = as_map(&author) else {
                self.error(format!(
                    "authors.{key} : should be an array, {} given",
                    debug_type(&author)
                ));
                continue;
            };
            for field in ["homepage", "email", "name", "role"] {
                if author
                    .get(field)
                    .is_some_and(|v| !v.is_null() && !v.is_string())
                {
                    self.error(format!(
                        "authors.{key}.{field} : invalid value, must be a string"
                    ));
                    author.shift_remove(field);
                }
            }
            if let Some(homepage) = author
                .get("homepage")
                .and_then(Value::as_str)
                .map(String::from)
                && !filter_url(&homepage, &HTTP)
            {
                self.warning(format!(
                    "authors.{key}.homepage : invalid value ({homepage}), must be an http/https URL"
                ));
                author.shift_remove("homepage");
            }
            if let Some(email) = author
                .get("email")
                .and_then(Value::as_str)
                .map(String::from)
                && !is_email(&email)
            {
                self.warning(format!(
                    "authors.{key}.email : invalid value ({email}), must be a valid email address"
                ));
                author.shift_remove("email");
            }
            if !author.is_empty() {
                kept.insert(key, Value::Object(author));
            }
        }
        if kept.is_empty() {
            self.unset("authors");
        } else {
            self.config.insert("authors".into(), Value::Object(kept));
        }
    }

    fn support(&mut self) {
        if !self.validate_array("support", false) || empty(self.get("support")) {
            return;
        }
        let mut support: Map<String, Value> =
            entries(&self.config["support"]).into_iter().collect();
        for key in [
            "issues", "forum", "wiki", "source", "email", "irc", "docs", "rss", "chat", "security",
        ] {
            if support
                .get(key)
                .is_some_and(|v| !v.is_null() && !v.is_string())
            {
                self.error(format!("support.{key} : invalid value, must be a string"));
                support.shift_remove(key);
            }
        }
        if let Some(email) = support
            .get("email")
            .and_then(Value::as_str)
            .map(String::from)
            && !is_email(&email)
        {
            self.warning(format!(
                "support.email : invalid value ({email}), must be a valid email address"
            ));
            support.shift_remove("email");
        }
        if let Some(irc) = support.get("irc").and_then(Value::as_str).map(String::from)
            && !filter_url(&irc, &["irc", "ircs"])
        {
            self.warning(format!(
                "support.irc : invalid value ({irc}), must be a irc://<server>/<channel> or ircs:// URL"
            ));
            support.shift_remove("irc");
        }
        for key in [
            "issues", "forum", "wiki", "source", "docs", "chat", "security",
        ] {
            if let Some(url) = support.get(key).and_then(Value::as_str).map(String::from)
                && !filter_url(&url, &HTTP)
            {
                self.warning(format!(
                    "support.{key} : invalid value ({url}), must be an http/https URL"
                ));
                support.shift_remove(key);
            }
        }
        if support.is_empty() {
            self.unset("support");
        } else {
            self.config.insert("support".into(), Value::Object(support));
        }
    }

    fn funding(&mut self) {
        if !self.validate_array("funding", false) || empty(self.get("funding")) {
            return;
        }
        let mut kept = Map::new();
        for (key, option) in entries(&self.config["funding"]) {
            let Some(mut option) = as_map(&option) else {
                self.error(format!(
                    "funding.{key} : should be an array, {} given",
                    debug_type(&option)
                ));
                continue;
            };
            for field in ["type", "url"] {
                if option
                    .get(field)
                    .is_some_and(|v| !v.is_null() && !v.is_string())
                {
                    self.error(format!(
                        "funding.{key}.{field} : invalid value, must be a string"
                    ));
                    option.shift_remove(field);
                }
            }
            if let Some(url) = option.get("url").and_then(Value::as_str).map(String::from)
                && !filter_url(&url, &HTTP)
            {
                self.warning(format!(
                    "funding.{key}.url : invalid value ({url}), must be an http/https URL"
                ));
                option.shift_remove("url");
            }
            if !option.is_empty() {
                kept.insert(key, Value::Object(option));
            }
        }
        if kept.is_empty() {
            self.unset("funding");
        } else {
            self.config.insert("funding".into(), Value::Object(kept));
        }
    }

    fn php_ext(&mut self) {
        if !isset(self.get("php-ext")) || !self.validate_array("php-ext", false) {
            return;
        }
        let kind = self.get("type").and_then(Value::as_str).unwrap_or("");
        if !["php-ext", "php-ext-zend"].contains(&kind) {
            self.error(
                "php-ext can only be set by packages of type \"php-ext\" or \"php-ext-zend\" which must be C extensions".into(),
            );
            // The PHP goes on with `$phpExt = &$this->config['php-ext']`, a new null, so nothing below applies.
            self.unset("php-ext");
            return;
        }
        let ext: Map<String, Value> = entries(&self.config["php-ext"]).into_iter().collect();
        let mut ext = ext;
        let typed = |ext: &Map<String, Value>, key: &str, ok: fn(&Value) -> bool| {
            ext.get(key)
                .filter(|v| !v.is_null() && !ok(v))
                .map(debug_type)
        };
        if let Some(given) = typed(&ext, "extension-name", Value::is_string) {
            self.error(format!(
                "php-ext.extension-name : should be a string, {given} given"
            ));
            ext.shift_remove("extension-name");
        }
        if let Some(given) = typed(&ext, "priority", |v| v.is_i64() || v.is_u64()) {
            self.error(format!(
                "php-ext.priority : should be an integer, {given} given"
            ));
            ext.shift_remove("priority");
        }
        for key in ["support-zts", "support-nts"] {
            if let Some(given) = typed(&ext, key, Value::is_boolean) {
                self.error(format!(
                    "php-ext.{key} : should be a boolean, {given} given"
                ));
                ext.shift_remove(key);
            }
        }
        if let Some(given) = typed(&ext, "build-path", Value::is_string) {
            self.error(format!(
                "php-ext.build-path : should be a string or null, {given} given"
            ));
            ext.shift_remove("build-path");
        }
        self.download_url_method(&mut ext);
        self.os_families(&mut ext);
        self.configure_options(&mut ext);
        if ext.is_empty() {
            self.unset("php-ext");
        } else {
            self.config.insert("php-ext".into(), Value::Object(ext));
        }
    }

    fn download_url_method(&mut self, ext: &mut Map<String, Value>) {
        const VALID: [&str; 3] = [
            "composer-default",
            "pre-packaged-source",
            "pre-packaged-binary",
        ];
        let Some(method) = ext
            .get("download-url-method")
            .filter(|v| !v.is_null())
            .cloned()
        else {
            return;
        };
        if !is_array(&method) && !method.is_string() {
            self.error(format!(
                "php-ext.download-url-method : should be an array or a string, {} given",
                debug_type(&method)
            ));
            ext.shift_remove("download-url-method");
            return;
        }
        let methods = if method.is_string() {
            vec![("0".into(), method)]
        } else {
            entries(&method)
        };
        if methods.is_empty() {
            self.error("php-ext.download-url-method : must contain at least one element".into());
            ext.shift_remove("download-url-method");
            return;
        }
        for (key, method) in methods {
            match method.as_str() {
                None => {
                    self.error(format!(
                        "php-ext.download-url-method.{key} : should be a string, {} given",
                        debug_type(&method)
                    ));
                    ext.shift_remove("download-url-method");
                }
                Some(m) if !VALID.contains(&m) => {
                    self.error(format!(
                        "php-ext.download-url-method.{key} : invalid value ({m}), must be one of {}",
                        VALID.join(", ")
                    ));
                    ext.shift_remove("download-url-method");
                }
                Some(_) => {}
            }
        }
    }

    fn os_families(&mut self, ext: &mut Map<String, Value>) {
        const VALID: [&str; 6] = ["windows", "bsd", "darwin", "solaris", "linux", "unknown"];
        if isset(ext.get("os-families")) && isset(ext.get("os-families-exclude")) {
            self.error(
                "php-ext : os-families and os-families-exclude cannot both be specified".into(),
            );
            ext.shift_remove("os-families");
            ext.shift_remove("os-families-exclude");
            return;
        }
        for field in ["os-families", "os-families-exclude"] {
            let Some(families) = ext.get(field).filter(|v| !v.is_null()).cloned() else {
                continue;
            };
            if !is_array(&families) {
                self.error(format!(
                    "php-ext.{field} : should be an array, {} given",
                    debug_type(&families)
                ));
                ext.shift_remove(field);
                continue;
            }
            if count(&families) == 0 {
                self.error(format!(
                    "php-ext.{field} : must contain at least one element"
                ));
                ext.shift_remove(field);
                continue;
            }
            let mut kept = Map::new();
            for (key, family) in entries(&families) {
                match family.as_str() {
                    None => self.error(format!(
                        "php-ext.{field}.{key} : should be a string, {} given",
                        debug_type(&family)
                    )),
                    Some(f) if !VALID.contains(&f) => self.error(format!(
                        "php-ext.{field}.{key} : invalid value ({f}), must be one of {}",
                        VALID.join(", ")
                    )),
                    Some(_) => {
                        kept.insert(key, family);
                    }
                }
            }
            if kept.is_empty() {
                ext.shift_remove(field);
            } else {
                ext.insert(field.into(), Value::Object(kept));
            }
        }
    }

    fn configure_options(&mut self, ext: &mut Map<String, Value>) {
        let Some(options) = ext
            .get("configure-options")
            .filter(|v| !v.is_null())
            .cloned()
        else {
            return;
        };
        if !is_array(&options) {
            self.error(format!(
                "php-ext.configure-options : should be an array, {} given",
                debug_type(&options)
            ));
            ext.shift_remove("configure-options");
            return;
        }
        let mut kept = Map::new();
        for (key, option) in entries(&options) {
            let Some(mut option) = as_map(&option) else {
                self.error(format!(
                    "php-ext.configure-options.{key} : should be an array, {} given",
                    debug_type(&option)
                ));
                continue;
            };
            match option.get("name").filter(|v| !v.is_null()) {
                None => {
                    self.error(format!(
                        "php-ext.configure-options.{key}.name : must be present"
                    ));
                    continue;
                }
                Some(name) if !name.is_string() => {
                    self.error(format!(
                        "php-ext.configure-options.{key}.name : should be a string, {} given",
                        debug_type(name)
                    ));
                    continue;
                }
                Some(_) => {}
            }
            if let Some(v) = option
                .get("needs-value")
                .filter(|v| !v.is_null() && !v.is_boolean())
            {
                self.error(format!(
                    "php-ext.configure-options.{key}.needs-value : should be a boolean, {} given",
                    debug_type(v)
                ));
                option.shift_remove("needs-value");
            }
            if let Some(v) = option
                .get("description")
                .filter(|v| !v.is_null() && !v.is_string())
            {
                self.error(format!(
                    "php-ext.configure-options.{key}.description : should be a string, {} given",
                    debug_type(v)
                ));
                option.shift_remove("description");
            }
            kept.insert(key, Value::Object(option));
        }
        if kept.is_empty() {
            ext.shift_remove("configure-options");
        } else {
            ext.insert("configure-options".into(), Value::Object(kept));
        }
    }

    /// The links, in `BasePackage::$supportedLinkTypes`' order.
    fn links(&mut self, constraints: bool) {
        static NAME: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_./-]+$").expect("valid"));
        let unbound = Constraint::exact("10000000-dev");
        let at_least_one = Constraint::Single(Op::Ge, "1.0.0.0-dev".into());
        for link in ["require", "conflict", "provide", "replace", "require-dev"] {
            if !self.validate_array(link, false) || !isset(self.get(link)) {
                continue;
            }
            let own = self
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_ascii_lowercase);
            // PHP's foreach walks a copy, while each `unset()` changes the config the later checks read.
            for (package, constraint) in entries(&self.config[link]) {
                let package = package.as_str();
                if own.as_deref() == Some(package.to_ascii_lowercase().as_str()) {
                    self.error(format!(
                        "{link}.{package} : a package cannot set a {link} on itself"
                    ));
                    self.remove_entries(link, &[package.to_string()]);
                    continue;
                }
                if let Some(error) = package_naming_error(package, true) {
                    self.warning(format!("{link}.{error}"));
                } else if !NAME.is_match(package.as_bytes()).unwrap_or(false) {
                    self.error(format!(
                        "{link}.{package} : invalid key, package names must be strings containing only [A-Za-z0-9_./-]"
                    ));
                }
                // A constraint that isn't a string goes on to the conflict check, as in the PHP.
                if let Some(text) = constraint.as_str().filter(|t| *t != "self.version") {
                    let parsed = match constraint::parse(text) {
                        Ok(parsed) => parsed,
                        Err(e) => {
                            self.error(format!(
                                "{link}.{package} : invalid version constraint ({e:#})"
                            ));
                            self.remove_entries(link, &[package.to_string()]);
                            continue;
                        }
                    };
                    if constraints
                        && link == "require"
                        && parsed.matches(&unbound)
                        && !is_platform(package)
                    {
                        self.warning(format!(
                            "{link}.{package} : unbound version constraints ({text}) should be avoided"
                        ));
                    } else if constraints
                        && link == "require"
                        && matches!(parsed, Constraint::Single(Op::Eq, _))
                        && at_least_one.matches(&parsed)
                    {
                        self.warning(format!(
                            "{link}.{package} : exact version constraints ({text}) should be avoided if the package follows semantic versioning"
                        ));
                    }
                    if matches_nothing(&parsed) {
                        self.warning(format!(
                            "{link}.{package} : this version constraint cannot possibly match anything ({text})"
                        ));
                    }
                } else if !constraint.is_string() {
                    self.error(format!(
                        "{link}.{package} : invalid value, must be a string containing a version constraint"
                    ));
                    self.remove_entries(link, &[package.to_string()]);
                }
                if link == "conflict"
                    && let (Some(replace), Some(conflict)) = (
                        self.get("replace").and_then(Value::as_object),
                        self.get("conflict").and_then(Value::as_object),
                    )
                    && replace.keys().any(|k| conflict.contains_key(k))
                {
                    self.error(format!(
                        "{link}.{package} : you cannot conflict with a package that is also replaced, as replace already creates an implicit conflict rule"
                    ));
                    self.remove_entries(link, &[package.to_string()]);
                }
            }
        }
    }

    fn suggest(&mut self) {
        if !self.validate_array("suggest", false) || !isset(self.get("suggest")) {
            return;
        }
        let mut removed = vec![];
        for (package, description) in entries(&self.config["suggest"]) {
            if !description.is_string() {
                self.error(format!(
                    "suggest.{package} : invalid value, must be a string describing why the package is suggested"
                ));
                removed.push(package);
            }
        }
        self.remove_entries("suggest", &removed);
    }

    fn minimum_stability(&mut self) {
        if !self.validate_string("minimum-stability", false) {
            return;
        }
        let Some(stability) = self
            .get("minimum-stability")
            .and_then(Value::as_str)
            .map(String::from)
        else {
            return;
        };
        // `BasePackage::STABILITIES`' keys, looked up lower-cased, so only "RC" itself matches its key.
        let known =
            ["stable", "beta", "alpha", "dev"].contains(&stability.to_ascii_lowercase().as_str());
        if !known && stability != "RC" {
            self.error(format!(
                "minimum-stability : invalid value ({stability}), must be one of stable, RC, beta, alpha, dev"
            ));
            self.unset("minimum-stability");
        }
    }

    fn autoload(&mut self) {
        const TYPES: [&str; 5] = [
            "psr-0",
            "psr-4",
            "classmap",
            "files",
            "exclude-from-classmap",
        ];
        if self.validate_array("autoload", false) && isset(self.get("autoload")) {
            let mut removed = vec![];
            for (kind, config) in entries(&self.config["autoload"]) {
                if !TYPES.contains(&kind.as_str()) {
                    self.error(format!(
                        "autoload : invalid value ({kind}), must be one of {}",
                        TYPES.join(", ")
                    ));
                    removed.push(kind.clone());
                }
                if kind == "psr-4" {
                    for (namespace, _) in entries(&config) {
                        if !namespace.is_empty() && !namespace.ends_with('\\') {
                            self.error(format!(
                                "autoload.psr-4 : invalid value ({namespace}), namespaces must end with a namespace separator, should be {namespace}\\\\"
                            ));
                        }
                    }
                }
            }
            self.remove_entries("autoload", &removed);
        }
        let psr4 = isset(self.get("autoload").and_then(|a| a.get("psr-4")));
        if psr4 && isset(self.get("target-dir")) {
            self.error(
                "target-dir : this can not be used together with the autoload.psr-4 setting, remove target-dir to upgrade to psr-4".into(),
            );
            if let Some(Value::Object(autoload)) = self.config.get_mut("autoload") {
                autoload.shift_remove("psr-4");
            }
        }
    }

    fn source(&mut self, kind: &str) {
        static DASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*-").expect("valid"));
        if !self.validate_array(kind, false) || empty(self.get(kind)) {
            return;
        }
        let source: Map<String, Value> = entries(&self.config[kind]).into_iter().collect();
        let field = |key: &str| source.get(key).filter(|v| !v.is_null());
        if field("type").is_none() {
            self.error(format!("{kind}.type : must be present"));
        }
        if field("url").is_none() {
            self.error(format!("{kind}.url : must be present"));
        }
        if kind == "source" && field("reference").is_none() {
            self.error(format!("{kind}.reference : must be present"));
        }
        for key in ["type", "url"] {
            if let Some(v) = field(key).filter(|v| !v.is_string()) {
                self.error(format!(
                    "{kind}.{key} : should be a string, {} given",
                    debug_type(v)
                ));
            }
        }
        if let Some(v) = field("reference").filter(|v| !v.is_string() && !v.is_i64() && !v.is_u64())
        {
            self.error(format!(
                "{kind}.reference : should be a string or int, {} given",
                debug_type(v)
            ));
        }
        for key in ["reference", "url"] {
            if let Some(v) = field(key).map(php_string)
                && DASH.is_match(v.as_bytes()).unwrap_or(false)
            {
                self.error(format!(
                    "{kind}.{key} : must not start with a \"-\", \"{v}\" given"
                ));
            }
        }
        if kind == "source"
            && field("type").and_then(Value::as_str) == Some("perforce")
            && let Some(url) = field("url").and_then(Value::as_str)
            && !perforce_port(url)
        {
            self.error(format!(
                "{kind}.url : invalid Perforce port (\"{url}\"), it must be of the form [tcp|ssl:][host:]port"
            ));
        }
    }

    fn branch_alias(&mut self) {
        let Some(aliases) = self
            .get("extra")
            .and_then(|e| e.get("branch-alias"))
            .filter(|v| !v.is_null())
            .cloned()
        else {
            return;
        };
        if !is_array(&aliases) {
            self.error("extra.branch-alias : must be an array of versions => aliases".into());
            return;
        }
        for (source, target) in entries(&aliases) {
            let Some(target) = target.as_str() else {
                self.warning(format!(
                    "extra.branch-alias.{source} : the target branch ({}) must be a string, \"{}\" received.",
                    json_compact(&target),
                    debug_type(&target)
                ));
                continue;
            };
            let Some(stem) = target.strip_suffix("-dev") else {
                self.warning(format!(
                    "extra.branch-alias.{source} : the target branch ({target}) must end in -dev"
                ));
                continue;
            };
            if !normalize_branch(stem).ends_with("-dev") {
                self.warning(format!(
                    "extra.branch-alias.{source} : the target branch ({target}) must be a parseable number like 2.0-dev"
                ));
                continue;
            }
            if let (Some(source_prefix), Some(target_prefix)) =
                (numeric_alias_prefix(&source), numeric_alias_prefix(target))
                && !target_prefix
                    .to_ascii_lowercase()
                    .starts_with(&source_prefix.to_ascii_lowercase())
            {
                self.warning(format!(
                    "extra.branch-alias.{source} : the target branch ({target}) is not a valid numeric alias for this version"
                ));
            }
        }
    }
}

/// `VersionParser::parseNumericAliasPrefix`: the numeric prefix of a branch like "1.2.x-dev" or "dev-1.2", with its
/// trailing dot; `None` for a branch that has none.
fn numeric_alias_prefix(branch: &str) -> Option<String> {
    static PREFIX: LazyLock<Regex> = LazyLock::new(|| {
        RegexBuilder::new()
            .caseless(true)
            .build(r"^(?P<version>(\d++\.)*\d++)(?:\.x)?-dev$")
            .expect("valid")
    });
    let caps = PREFIX.captures(branch.as_bytes()).ok()??;
    let version = caps.name("version")?.as_bytes();
    Some(format!("{}.", String::from_utf8_lossy(version)))
}

/// The release date of `time`, in seconds since the epoch, for the forms Composer documents ("YYYY-MM-DD" or
/// "YYYY-MM-DD HH:MM:SS") and ISO 8601's (a `T`, fractions of a second, `Z` or an offset). PHP's `DateTime` reads far
/// more; a value outside these forms counts as no date, so its licenses are checked, and gets no error.
fn release_timestamp(time: &str) -> Option<i64> {
    static TIME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(\d{4})-(\d{2})-(\d{2})(?:[ T](\d{2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?)?(Z|[+-]\d{2}:?\d{2})?$")
            .expect("valid")
    });
    let caps = TIME.captures(time.trim().as_bytes()).ok()??;
    let number = |i: usize| -> i64 {
        caps.get(i).map_or(0, |m| {
            std::str::from_utf8(m.as_bytes())
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0)
        })
    };
    let (year, month, day) = (number(1), number(2), number(3));
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days since the epoch (Howard Hinnant's days_from_civil).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let mut seconds = days * 86400 + number(4) * 3600 + number(5) * 60 + number(6);
    if let Some(offset) = caps
        .get(7)
        .map(|m| String::from_utf8_lossy(m.as_bytes()).into_owned())
        && offset != "Z"
    {
        let digits: String = offset.chars().filter(char::is_ascii_digit).collect();
        let (hours, minutes) = (
            digits[..2].parse::<i64>().unwrap_or(0),
            digits[2..].parse::<i64>().unwrap_or(0),
        );
        let sign = if offset.starts_with('-') { -1 } else { 1 };
        seconds -= sign * (hours * 3600 + minutes * 60);
    }
    Some(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Composer 2.11.2's `ValidatingArrayLoader` on configs that reach each check (tests/fixtures/loader.json, from
    /// scripts/export-loader-cases.php).
    #[test]
    fn loads_as_composer_does() {
        let cases: Vec<Value> =
            serde_json::from_str(include_str!("../../tests/fixtures/loader.json")).unwrap();
        let mut wrong = vec![];
        for case in &cases {
            let config: Map<String, Value> =
                serde_json::from_str(case["config"].as_str().unwrap()).unwrap();
            let loaded = load(config, case["constraints"].as_bool().unwrap());
            let strings = |v: &Value| -> Vec<String> {
                (v.as_array().unwrap().iter())
                    .map(|s| s.as_str().unwrap().to_string())
                    .collect()
            };
            let (errors, warnings) = (strings(&case["errors"]), strings(&case["warnings"]));
            if loaded.errors != errors || loaded.warnings != warnings {
                wrong.push(format!(
                    "=== {}\nwant errors {errors:#?}\nwant warnings {warnings:#?}\ngot errors {:#?}\ngot warnings {:#?}",
                    case["name"], loaded.errors, loaded.warnings
                ));
            }
        }
        assert!(
            wrong.is_empty(),
            "{} of {} differ:\n{}",
            wrong.len(),
            cases.len(),
            wrong.join("\n")
        );
    }

    /// Pun's choice for `time` (context/PLAN.md §3, docs/content/docs/commands/composer.mdx): a value outside the forms
    /// it reads is no error, where Composer reports what PHP's `DateTime` says, and with no release date the licenses
    /// are checked.
    #[test]
    fn time_outside_the_read_forms_is_no_error() {
        let invalid_license = "License \"Not-A-License\" is not a valid SPDX license identifier, see https://spdx.org/licenses/ if you use an open license.\nIf the software is closed-source, you may use \"proprietary\" as license.";
        for time in [
            "2001-13-01",
            "2001-01-32",
            "yesterday",
            "2001-01-01 25:00:00 nonsense",
        ] {
            let config = serde_json::json!({"name": "acme/pkg", "version": "1.0.0", "time": time, "license": "Not-A-License"});
            let loaded = load(config.as_object().unwrap().clone(), true);
            assert_eq!(
                (loaded.errors, loaded.warnings),
                (Vec::<String>::new(), vec![invalid_license.to_string()]),
                "{time}"
            );
        }
    }
}
