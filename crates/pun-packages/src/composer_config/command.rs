//! `ConfigCommand` (with `BaseConfigCommand`): reading a setting, listing them all, and writing or removing one.

use super::source::{Source, read, write_new};
use super::{
    Config, Env, OBJECT_SETTINGS, boolean, encode, entries, is_int_key, isset, php_string, string,
    truthy,
};
use crate::error::{Result, bail};
use crate::manifest_edit::{Php, is_numeric};
use pcre2::bytes::Regex;
use serde_json::Value;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// `config`'s options.
#[derive(Debug, Clone, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one bool per command-line flag"
)]
pub struct Options {
    pub global: bool,
    pub editor: bool,
    pub file: Option<String>,
    pub unset: bool,
    pub list: bool,
    pub absolute: bool,
    pub json: bool,
    pub merge: bool,
    pub append: bool,
    pub source: bool,
}

/// Where Composer's files are: `COMPOSER_HOME`, its cache dir and its data dir.
pub struct Dirs {
    pub home: String,
    pub cache: String,
    pub data: String,
}

/// What a run prints.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub stdout: String,
    pub stderr: String,
}

/// A command's view of the settings: the merged config, and the files it reads and writes.
struct Command {
    config: Config,
    /// composer.json (or `--file`'s, or config.json with `--global`) as Composer names it, and on disk.
    file: String,
    file_path: PathBuf,
    config_source: Source,
    auth_source: Source,
    options: Options,
    cwd: PathBuf,
    out: Outcome,
}

/// The file `shown` (relative to `cwd`) on disk.
fn on_disk(cwd: &Path, shown: &str) -> PathBuf {
    if Path::new(shown).is_absolute() {
        PathBuf::from(shown)
    } else {
        cwd.join(shown)
    }
}

/// `dirname`.
fn dirname(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => "/".into(),
        Some(at) => path[..at].into(),
        None => ".".into(),
    }
}

/// `Factory::createConfig`: the defaults, the dirs, then the global config.json and auth.json, then `COMPOSER_AUTH`.
fn create_config(cwd: &Path, dirs: &Dirs, env: &Env) -> Result<Config> {
    let mut config = Config::new(Some(cwd.to_string_lossy().into_owned()), Rc::clone(env));
    config.merge(
        &Php::Array(vec![(
            "config".into(),
            Php::Array(vec![
                ("home".into(), string(&dirs.home)),
                ("cache-dir".into(), string(&dirs.cache)),
                ("data-dir".into(), string(&dirs.data)),
            ]),
        )]),
        "default",
    );
    let global = format!("{}/config.json", dirs.home);
    if Path::new(&global).exists() {
        let data = read(Path::new(&global), &global)?;
        config.merge(&data, &global);
    }
    let auth = format!("{}/auth.json", dirs.home);
    if Path::new(&auth).exists() {
        let data = read(Path::new(&auth), &auth)?;
        config.merge(&Php::Array(vec![("config".into(), data)]), &auth);
    }
    if let Some(text) = env("COMPOSER_AUTH").filter(|t| !t.is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            bail!("COMPOSER_AUTH does not contain valid JSON");
        };
        config.merge(
            &Php::Array(vec![("config".into(), Php::assoc(&value))]),
            "COMPOSER_AUTH",
        );
    }
    Ok(config)
}

/// The JSON a value is given as with `--json` (`JsonFile::parseJson`); Composer follows the error with jsonlint's
/// description of it (context/PLAN.md §3).
fn parse_json(text: &str) -> Result<Php> {
    match serde_json::from_str::<Value>(text) {
        Ok(value) => Ok(Php::assoc(&value)),
        Err(_) => bail!("The input does not contain valid JSON"),
    }
}

/// A list of strings.
fn list(values: &[String]) -> Php {
    Php::Array(
        values
            .iter()
            .enumerate()
            .map(|(i, v)| (i.to_string(), string(v)))
            .collect(),
    )
}

fn bool_value(b: bool) -> Php {
    Php::Scalar(Value::Bool(b))
}

/// `array_is_list`.
fn is_list(v: &Php) -> bool {
    entries(v)
        .iter()
        .enumerate()
        .all(|(i, (k, _))| *k == i.to_string())
}

/// `$value + $current`: the keys of `value`, then those of `current` it lacks.
fn union(value: &Php, current: &Php) -> Php {
    let mut out = entries(value).to_vec();
    for (k, v) in entries(current) {
        if !out.iter().any(|(o, _)| o == k) {
            out.push((k.clone(), v.clone()));
        }
    }
    Php::Array(out)
}

/// `VersionParser::normalizeStability`.
fn stability(value: &str) -> Result<String> {
    let lower = value.to_lowercase();
    match lower.as_str() {
        "rc" => Ok("RC".into()),
        "stable" | "beta" | "alpha" | "dev" => Ok(lower),
        _ => bail!(
            "Invalid stability string \"{lower}\", expected one of stable, RC, beta, alpha or dev"
        ),
    }
}

/// How a single setting is checked and made into what is written.
#[derive(Clone, Copy)]
enum Single {
    /// `is_numeric`, `intval`.
    Integer,
    /// Any string, kept.
    Text,
    /// Any string, `"null"` as null.
    Nullable,
    /// A boolean.
    Boolean,
    /// One of these words, kept.
    OneOf(&'static [&'static str]),
    /// One of these words kept, else a boolean.
    WordOrBoolean(&'static [&'static str]),
    /// A size such as `300MiB`.
    Size,
    /// A readable file, or a directory, `"null"` as null.
    File,
    Dir,
    /// A duration (`policy.cooldown.period`), `"null"` or `""` as null, a number as an integer.
    Duration,
    /// A stability, normalized.
    Stability,
}

const BOOLEAN: &[&str] = &["true", "false", "1", "0"];
const AUDITS: &[&str] = &["ignore", "report", "fail"];
const BLOCK_SCOPES: &[&str] = &["update", "install", "all"];

/// `$uniqueConfigValues` of `ConfigCommand`.
fn unique_setting(key: &str) -> Option<Single> {
    Some(match key {
        "process-timeout" | "cache-ttl" | "cache-files-ttl" => Single::Integer,
        "use-include-path"
        | "use-github-api"
        | "notify-on-install"
        | "sort-packages"
        | "optimize-autoloader"
        | "strict-psr-autoloader"
        | "classmap-authoritative"
        | "apcu-autoloader"
        | "prepend-autoloader"
        | "update-with-minimal-changes"
        | "disable-tls"
        | "secure-http"
        | "source-fallback"
        | "github-expose-hostname"
        | "htaccess-protect"
        | "lock"
        | "allow-plugins"
        | "audit.ignore-unreachable"
        | "audit.block-insecure"
        | "audit.block-abandoned"
        | "policy.advisories.block"
        | "policy.malware.block"
        | "policy.abandoned.block"
        | "policy.cooldown.block"
        | "policy.ignore-unreachable" => Single::Boolean,
        "preferred-install" => Single::OneOf(&["auto", "source", "dist"]),
        "gitlab-protocol" => Single::OneOf(&["git", "http", "https"]),
        "bin-compat" => Single::OneOf(&["auto", "full", "proxy", "symlink"]),
        "audit.abandoned"
        | "policy.advisories.audit"
        | "policy.malware.audit"
        | "policy.abandoned.audit"
        | "policy.cooldown.audit" => Single::OneOf(AUDITS),
        "policy.malware.block-scope" => Single::OneOf(BLOCK_SCOPES),
        "store-auths" | "use-parent-dir" => Single::WordOrBoolean(&["prompt", "true", "false"]),
        "discard-changes" => Single::WordOrBoolean(&["stash", "true", "false", "1", "0"]),
        "platform-check" => Single::WordOrBoolean(&["php-only", "true", "false", "1", "0"]),
        "bump-after-update" => Single::WordOrBoolean(&["dev", "no-dev", "true", "false", "1", "0"]),
        "vendor-dir" | "bin-dir" | "archive-dir" | "archive-format" | "data-dir" | "cache-dir"
        | "cache-files-dir" | "cache-repo-dir" | "cache-vcs-dir" => Single::Text,
        "autoloader-suffix" => Single::Nullable,
        "cache-files-maxsize" => Single::Size,
        "cafile" => Single::File,
        "capath" => Single::Dir,
        "policy.cooldown.period" => Single::Duration,
        _ => return None,
    })
}

/// `$uniqueProps`.
fn unique_property(key: &str) -> Option<Single> {
    Some(match key {
        "name" | "type" | "description" | "homepage" | "version" => Single::Text,
        "minimum-stability" => Single::Stability,
        "prefer-stable" => Single::Boolean,
        _ => return None,
    })
}

/// How a list setting is checked: its error, if any.
#[derive(Clone, Copy)]
enum Multi {
    Any,
    Protocols,
    Severities,
}

/// `$multiConfigValues`.
fn multi_setting(key: &str) -> Option<Multi> {
    Some(match key {
        "github-protocols" => Multi::Protocols,
        "github-domains" | "gitlab-domains" | "policy.malware.ignore-source" => Multi::Any,
        "audit.ignore-severity" | "policy.advisories.ignore-severity" => Multi::Severities,
        _ => return None,
    })
}

/// `$multiProps`.
fn multi_property(key: &str) -> Option<Multi> {
    matches!(key, "keywords" | "license").then_some(Multi::Any)
}

impl Single {
    /// The validator and the normalizer: the value to write, else Composer's error.
    fn check(self, value: &str, cwd: &Path) -> Result<Php> {
        let invalid = || -> Result<Php> { bail!("\"{value}\" is an invalid value") };
        let readable = |p: &Path| std::fs::metadata(cwd.join(p)).is_ok();
        Ok(match self {
            Single::Integer if is_numeric(value) => {
                Php::Scalar(serde_json::json!(super::intval(value)))
            }
            Single::Text => string(value),
            Single::Nullable | Single::File | Single::Dir => {
                let path = cwd.join(value);
                let ok = match self {
                    Single::File => path.is_file() && readable(&path),
                    Single::Dir => path.is_dir() && readable(&path),
                    _ => true,
                };
                if !ok {
                    return invalid();
                }
                if value == "null" {
                    Php::Scalar(Value::Null)
                } else {
                    string(value)
                }
            }
            Single::Boolean if BOOLEAN.contains(&value) => bool_value(boolean(&string(value))),
            Single::OneOf(words) if words.contains(&value) => string(value),
            Single::WordOrBoolean(words) if words.contains(&value) => {
                if BOOLEAN.contains(&value) {
                    bool_value(boolean(&string(value)))
                } else {
                    string(value)
                }
            }
            Single::Size => {
                let size = Regex::new(r"(?i)^\s*([0-9.]+)\s*(?:([kmg])(?:i?b)?)?\s*$")
                    .expect("valid pattern");
                if !size.is_match(value.as_bytes()).unwrap_or(false) {
                    return invalid();
                }
                string(value)
            }
            Single::Duration => {
                if value == "null" || value.is_empty() {
                    return Ok(Php::Scalar(Value::Null));
                }
                if crate::resolver::filters::duration(Some(&Value::String(value.into()))).is_err() {
                    return invalid();
                }
                if is_numeric(value) {
                    Php::Scalar(serde_json::json!(super::intval(value)))
                } else {
                    string(value)
                }
            }
            Single::Stability => string(&stability(value)?),
            _ => return invalid(),
        })
    }
}

impl Multi {
    /// The validator's error for `values`, if any.
    fn error(self, values: &[String]) -> Option<&'static str> {
        match self {
            Multi::Any => None,
            Multi::Protocols => values
                .iter()
                .any(|v| !["git", "https", "ssh"].contains(&v.as_str()))
                .then_some("valid protocols include: git, https, ssh"),
            Multi::Severities => values
                .iter()
                .any(|v| !["low", "medium", "high", "critical"].contains(&v.as_str()))
                .then_some("valid severities include: low, medium, high, critical"),
        }
    }
}

/// `config`: run with `key` and `values` from the directory `cwd`, Composer's files in `dirs`, the composer file
/// `COMPOSER` names (else composer.json), and the environment `env`.
pub fn run(
    cwd: &Path,
    dirs: &Dirs,
    env: &Env,
    options: Options,
    key: Option<&str>,
    values: &[String],
) -> Result<Outcome> {
    if options.global && options.file.is_some() {
        bail!("--file and --global can not be combined");
    }
    let mut config = create_config(cwd, dirs, env)?;
    if options.global {
        config.set_base_dir(Some(dirs.home.clone()));
    }
    let composer_file = env("COMPOSER")
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| "./composer.json".into());
    let file = if options.global {
        format!("{}/config.json", dirs.home)
    } else {
        options.file.clone().unwrap_or(composer_file)
    };
    let file_path = on_disk(cwd, &file);
    if options.global && !file_path.exists() {
        std::fs::create_dir_all(file_path.parent().expect("a directory"))?;
        let skeleton = Php::Array(vec![("config".into(), Php::Object(vec![]))]);
        write_new(&file_path, &super::encode_pretty(&skeleton, "    "))?;
    }
    if !file_path.exists() {
        bail!("File \"{file}\" cannot be found in the current directory");
    }
    let auth_file = if options.global {
        format!("{}/auth.json", dirs.home)
    } else {
        format!("{}/auth.json", dirname(&file))
    };
    let auth_path = on_disk(cwd, &auth_file);
    if options.global && !auth_path.exists() {
        let empty = || Php::Object(vec![]);
        let skeleton = Php::Array(
            [
                "bitbucket-oauth",
                "github-oauth",
                "gitlab-oauth",
                "gitlab-token",
                "http-basic",
                "bearer",
                "forgejo-token",
            ]
            .iter()
            .map(|k| ((*k).to_string(), empty()))
            .collect(),
        );
        write_new(&auth_path, &super::encode_pretty(&skeleton, "    "))?;
    }
    if options.editor {
        bail!("`pun composer config` doesn't open an editor (--editor): edit {file} yourself");
    }
    if !options.global {
        let data = read(&file_path, &file)?;
        config.merge(&data, &file);
        let auth = if auth_path.exists() {
            read(&auth_path, &auth_file)?
        } else {
            Php::Array(vec![])
        };
        config.merge(&Php::Array(vec![("config".into(), auth)]), &auth_file);
    }
    load_auth_domains(&mut config)?;
    let mut command = Command {
        config,
        file,
        config_source: Source {
            path: file_path.clone(),
            auth: false,
        },
        auth_source: Source {
            path: auth_path,
            auth: true,
        },
        file_path,
        options,
        cwd: cwd.to_path_buf(),
        out: Outcome::default(),
    };
    command.execute(key, values)?;
    Ok(command.out)
}

/// `BaseIO::loadConfiguration`: a host with GitHub, GitLab or Forgejo credentials joins that service's domains.
fn load_auth_domains(config: &mut Config) -> Result<()> {
    for (auth, domains, default) in [
        ("github-oauth", "github-domains", Some("github.com")),
        ("gitlab-oauth", "gitlab-domains", Some("gitlab.com")),
        ("gitlab-token", "gitlab-domains", Some("gitlab.com")),
        ("forgejo-token", "forgejo-domains", None),
    ] {
        let hosts: Vec<String> = entries(&config.get(auth, false)?)
            .iter()
            .map(|(k, _)| k.clone())
            .collect();
        for host in hosts {
            let known = config.get(domains, false)?;
            if Some(host.as_str()) == default
                || entries(&known).iter().any(|(_, v)| *v == string(&host))
            {
                continue;
            }
            let added = super::array_merge(&known, &Php::Array(vec![("0".into(), string(&host))]));
            config.merge(
                &Php::Array(vec![(
                    "config".into(),
                    Php::Array(vec![(domains.into(), added)]),
                )]),
                "implicit-due-to-auth",
            );
        }
    }
    Ok(())
}

impl Command {
    fn execute(&mut self, key: Option<&str>, values: &[String]) -> Result<()> {
        // `Factory::createHttpDownloader`'s warning, which `config disable-tls` itself goes without.
        if key != Some("disable-tls") && self.config.get("disable-tls", false)? == bool_value(true)
        {
            self.out
                .stderr
                .push_str("You are running Composer with SSL/TLS protection disabled.\n");
        }
        if self.options.list {
            let all = self.config.all(false)?;
            let raw = self.config.raw_all();
            return self.list(&all, Some(&raw), None);
        }
        let Some(key) = key else {
            return Ok(());
        };
        if !values.is_empty() && self.options.unset {
            bail!("You can not combine a setting value with --unset");
        }
        if values.is_empty() && !self.options.unset {
            return self.show(key);
        }
        self.set(key, values)
    }

    /// Reading a setting: a repository, a dotted key, a config setting, a property of composer.json or its default.
    fn show(&mut self, key: &str) -> Result<()> {
        let raw_data = read(&self.file_path, &self.file)?;
        let data = self.config.all(false)?;
        let mut source = self.config.source_of(key)?;
        let repositories = Regex::new(r"^repos?(?:itories)?(?:\.(.+))?").expect("valid pattern");
        let value = if let Ok(Some(c)) = repositories.captures(key.as_bytes()) {
            let all = data
                .get("repositories")
                .cloned()
                .unwrap_or(Php::Array(vec![]));
            match c.get(1) {
                None => all,
                Some(name) => {
                    let name = String::from_utf8_lossy(name.as_bytes()).into_owned();
                    match all.get(&name) {
                        Some(repository) => repository.clone(),
                        None => bail!("There is no {name} repository defined"),
                    }
                }
            }
        } else if key.find('.').is_some_and(|at| at > 0) {
            let bits: Vec<&str> = key.split('.').collect();
            let mut node = if matches!(bits[0], "extra" | "suggest") {
                raw_data
            } else {
                data.get("config").cloned().unwrap_or(Php::Array(vec![]))
            };
            let mut pending: Option<String> = None;
            let mut found = false;
            for bit in bits {
                let part = pending
                    .take()
                    .map_or_else(|| bit.to_string(), |p| format!("{p}.{bit}"));
                found = false;
                match node.get(&part).filter(|v| isset(Some(v))).cloned() {
                    Some(next) => {
                        node = next;
                        found = true;
                    }
                    None => pending = Some(part),
                }
            }
            if !found {
                bail!("{key} is not defined.");
            }
            node
        } else if isset(data.get("config").and_then(|c| c.get(key))) {
            let value = self.config.get(key, !self.options.absolute)?;
            if value == Php::Array(vec![]) && OBJECT_SETTINGS.contains(&key) {
                Php::Object(vec![])
            } else {
                value
            }
        } else if isset(raw_data.get(key))
            && [
                "name",
                "type",
                "description",
                "homepage",
                "version",
                "minimum-stability",
                "prefer-stable",
                "keywords",
                "license",
                "repositories",
                "suggest",
                "extra",
            ]
            .contains(&key)
        {
            source.clone_from(&self.file);
            raw_data.get(key).expect("set").clone()
        } else {
            let default = match key {
                "type" => string("library"),
                "description" | "homepage" => string(""),
                "minimum-stability" => string("stable"),
                "prefer-stable" => bool_value(false),
                "keywords" | "license" | "suggest" | "extra" => Php::Array(vec![]),
                _ => bail!("{key} is not defined"),
            };
            source = "defaults".into();
            default
        };
        let shown = match value {
            Php::Array(_) | Php::Object(_) | Php::Scalar(Value::Bool(_)) => encode(&value, false),
            other @ Php::Scalar(_) => php_string(&other),
        };
        let source = if self.options.source {
            format!(" ({source})")
        } else {
            String::new()
        };
        let _ = writeln!(self.out.stdout, "{shown}{source}");
        Ok(())
    }

    /// `listConfiguration`: each setting as `[key] value`, the raw value first where it differs, with its source.
    fn list(&mut self, contents: &Php, raw: Option<&Php>, prefix: Option<&str>) -> Result<()> {
        for (key, value) in entries(contents) {
            if prefix.is_none() && !matches!(key.as_str(), "config" | "repositories") {
                continue;
            }
            let raw_value = raw.and_then(|r| r.get(key));
            let first_numeric = entries(value).first().is_some_and(|(k, _)| is_int_key(k));
            if matches!(value, Php::Array(_))
                && (!first_numeric || (key == "repositories" && prefix.is_none()))
            {
                let next = format!(
                    "{}{}",
                    prefix.unwrap_or_default(),
                    format!("{key}.").trim_start_matches("config.")
                );
                self.list(value, raw_value, Some(&next))?;
                continue;
            }
            let shown = match value {
                Php::Array(items) => {
                    let items: Vec<String> = (items.iter())
                        .map(|(_, v)| {
                            if matches!(v, Php::Array(_) | Php::Object(_)) {
                                encode(v, true)
                            } else {
                                php_string(v)
                            }
                        })
                        .collect();
                    format!("[{}]", items.join(", "))
                }
                Php::Scalar(Value::Bool(b)) => b.to_string(),
                other => php_string(other),
            };
            let path = format!("{}{key}", prefix.unwrap_or_default());
            let source = if self.options.source {
                format!(" ({})", self.config.source_of(&path)?)
            } else {
                String::new()
            };
            let differs = match raw_value {
                Some(Php::Scalar(Value::String(raw))) => {
                    !matches!(value, Php::Scalar(Value::String(v)) if v == raw)
                }
                _ => false,
            };
            if differs {
                let raw = php_string(raw_value.expect("a string"));
                let _ = writeln!(self.out.stdout, "[{path}] {raw} ({shown}){source}");
            } else {
                let _ = writeln!(self.out.stdout, "[{path}] {shown}{source}");
            }
        }
        Ok(())
    }
}

/// The policy lists Composer has, and the policy keys that are no list.
const POLICY_LISTS: [&str; 4] = ["advisories", "abandoned", "malware", "cooldown"];

/// `PolicyConfig::getFutureReservedListNameError`.
fn reserved(list: &str) -> Option<String> {
    if list.starts_with("ignore") {
        return Some(format!(
            "\"{list}\" starts with reserved prefix \"ignore\"."
        ));
    }
    let names = [
        "package",
        "packages",
        "license",
        "licence",
        "licenses",
        "licences",
        "support",
        "maintenance",
        "security",
        "minimum-release-age",
    ];
    names
        .contains(&list)
        .then(|| format!("\"{list}\" is reserved for future use."))
}

/// The capture groups of `pattern` in `key`, when it matches.
fn groups(pattern: &str, key: &str) -> Option<Vec<String>> {
    let c = Regex::new(pattern)
        .expect("valid pattern")
        .captures(key.as_bytes())
        .ok()??;
    Some(
        (1..c.len())
            .map(|i| {
                c.get(i).map_or_else(String::new, |m| {
                    String::from_utf8_lossy(m.as_bytes()).into_owned()
                })
            })
            .collect(),
    )
}

impl Command {
    /// Writing (or with `--unset` removing) a setting, in `ConfigCommand`'s order of cases.
    fn set(&mut self, key: &str, values: &[String]) -> Result<()> {
        let unset = self.options.unset;
        let first = values.first().map_or("", String::as_str);
        if unset && (key == "audit" || key == "policy" || key.starts_with("policy.")) {
            return self.config_source.remove_config_setting(key);
        }
        if self.set_policy(key, values)? {
            return Ok(());
        }
        let config_single = unique_setting(key);
        let config_multi = multi_setting(key);
        if unset && (config_single.is_some() || config_multi.is_some()) {
            if key == "disable-tls" && self.config.get("disable-tls", false)? == bool_value(true) {
                self.out
                    .stderr
                    .push_str("You are now running Composer with SSL/TLS protection enabled.\n");
            }
            return self.config_source.remove_config_setting(key);
        }
        if let Some(single) = config_single {
            let value = self.single(key, single, values)?;
            return self.config_source.add_config_setting(key, &value);
        }
        if let Some(multi) = config_multi {
            let value = Self::multi(multi, values)?;
            return self.config_source.add_config_setting(key, &value);
        }
        if groups(r"^preferred-install\.(.+)", key).is_some() {
            if unset {
                return self.config_source.remove_config_setting(key);
            }
            if !["auto", "source", "dist"].contains(&first) {
                bail!("Invalid value for {key}. Should be one of: auto, source, or dist");
            }
            return self.config_source.add_config_setting(key, &string(first));
        }
        if groups(r"^allow-plugins\.([a-zA-Z0-9/*-]+)", key).is_some() {
            if unset {
                return self.config_source.remove_config_setting(key);
            }
            if !BOOLEAN.contains(&first) {
                bail!("\"{first}\" is an invalid value");
            }
            return self
                .config_source
                .add_config_setting(key, &bool_value(boolean(&string(first))));
        }
        self.set_property(key, values)
    }

    /// The `policy.*` cases before the config settings; whether one of them was it.
    fn set_policy(&mut self, key: &str, values: &[String]) -> Result<bool> {
        let first = values.first().map_or("", String::as_str);
        let custom_ignore = groups(
            r"^policy\.(?!(?:advisories|abandoned|malware|cooldown|ignore-unreachable)$)([^.]+)\.ignore$",
            key,
        );
        let merge_keys = [
            "policy.advisories.ignore-id",
            "policy.advisories.ignore",
            "policy.abandoned.ignore",
            "policy.malware.ignore",
            "policy.cooldown.ignore",
        ];
        if merge_keys.contains(&key) || custom_ignore.is_some() {
            if let Some(error) = custom_ignore.and_then(|g| reserved(&g[0])) {
                bail!("Invalid dependency policy name: {error}");
            }
            let value = self.json_or_list(key, values)?;
            let value = self.merged_setting(key, value, &key.split('.').collect::<Vec<_>>())?;
            self.config_source.add_config_setting(key, &value)?;
            return Ok(true);
        }
        if key == "policy.ignore-unreachable" {
            let scopes = ["audit", "install", "update"];
            let invalid = || format!("valid values for {key} include: audit, install, update");
            if self.options.json {
                let value = parse_json(first)?;
                if !matches!(value, Php::Array(_)) {
                    bail!("Expected a boolean or array for {key}");
                }
                if entries(&value)
                    .iter()
                    .any(|(_, v)| !scopes.iter().any(|s| *v == string(s)))
                {
                    bail!("{}", invalid());
                }
                self.config_source.add_config_setting(key, &value)?;
                return Ok(true);
            }
            if scopes.contains(&first) {
                if values.iter().any(|v| !scopes.contains(&v.as_str())) {
                    bail!("{}", invalid());
                }
                self.config_source.add_config_setting(key, &list(values))?;
                return Ok(true);
            }
        }
        if let Some(g) = groups(r"^policy\.([^.]+)$", key).filter(|g| g[0] != "ignore-unreachable")
        {
            if let Some(error) = reserved(&g[0]) {
                bail!("Invalid dependency policy name: {error}");
            }
            if !BOOLEAN.contains(&first) {
                bail!("\"{first}\" is an invalid value for {key}, expected a boolean");
            }
            self.config_source
                .add_config_setting(key, &bool_value(boolean(&string(first))))?;
            return Ok(true);
        }
        if let Some(g) = groups(r"^policy\.([^.]+)\.(block|block-scope|audit)$", key)
            .filter(|g| !POLICY_LISTS.contains(&g[0].as_str()) && g[0] != "ignore-unreachable")
        {
            if let Some(error) = reserved(&g[0]) {
                bail!("Invalid dependency policy name: {error}");
            }
            let value = match g[1].as_str() {
                "block" if BOOLEAN.contains(&first) => bool_value(boolean(&string(first))),
                "block" => bail!("\"{first}\" is an invalid value for {key}, expected a boolean"),
                "block-scope" if BLOCK_SCOPES.contains(&first) => string(first),
                "block-scope" => bail!(
                    "\"{first}\" is an invalid value for {key}, must be one of: all, update, install"
                ),
                _ if AUDITS.contains(&first) => string(first),
                _ => bail!(
                    "\"{first}\" is an invalid value for {key}, must be one of: ignore, report, fail"
                ),
            };
            self.config_source.add_config_setting(key, &value)?;
            return Ok(true);
        }
        if let Some(g) = groups(r"^policy\.([^.]+)\.sources$", key) {
            bail!(
                "Setting dependency policy sources is not supported by `composer config`. Use `composer policy add-source {} url <https-url>` instead.",
                g[0]
            );
        }
        Ok(false)
    }

    /// The values as a list, or with `--json` the array the first one is.
    fn json_or_list(&self, key: &str, values: &[String]) -> Result<Php> {
        if !self.options.json {
            return Ok(list(values));
        }
        let value = parse_json(values.first().map_or("", String::as_str))?;
        if !matches!(value, Php::Array(_)) {
            bail!("Expected an array or object for {key}");
        }
        Ok(value)
    }

    /// With `--merge`, `value` merged with what the file has at `config` and `path`: lists one after the other, maps
    /// with the new keys first.
    fn merged_setting(&self, key: &str, value: Php, path: &[&str]) -> Result<Php> {
        if !self.options.merge {
            return Ok(value);
        }
        let file = read(&self.file_path, &self.file)?;
        let mut current = file.get("config").cloned();
        for bit in path {
            current = current
                .as_ref()
                .and_then(|c| c.get(bit))
                .filter(|v| isset(Some(v)))
                .cloned();
        }
        Ok(match current {
            Some(current @ Php::Array(_)) if matches!(value, Php::Array(_)) => {
                match (is_list(&current), is_list(&value)) {
                    (true, true) => super::array_merge(&current, &value),
                    (false, false) => union(&value, &current),
                    _ => bail!("Cannot merge array and object for {key}"),
                }
            }
            _ => value,
        })
    }

    /// `handleSingleValue`.
    fn single(&mut self, key: &str, single: Single, values: &[String]) -> Result<Php> {
        if values.len() != 1 {
            bail!(
                "You can only pass one value. Example: php composer.phar config process-timeout 300"
            );
        }
        let value = single.check(&values[0], &self.cwd)?;
        if key == "disable-tls" {
            let now = self.config.get("disable-tls", false)? == bool_value(true);
            if !truthy(&value) && now {
                self.out
                    .stderr
                    .push_str("You are now running Composer with SSL/TLS protection enabled.\n");
            } else if truthy(&value) && !now {
                self.out
                    .stderr
                    .push_str("You are now running Composer with SSL/TLS protection disabled.\n");
            }
        }
        Ok(value)
    }

    /// `handleMultiValue`.
    fn multi(multi: Multi, values: &[String]) -> Result<Php> {
        if let Some(error) = multi.error(values) {
            bail!(
                "{} is an invalid value ({error})",
                encode(&list(values), true)
            );
        }
        Ok(list(values))
    }

    /// The root properties, repositories, `extra`, `suggest`, `platform`, `audit`, auth and `scripts`.
    fn set_property(&mut self, key: &str, values: &[String]) -> Result<()> {
        let unset = self.options.unset;
        let first = values.first().map_or("", String::as_str);
        let (single, multi) = (unique_property(key), multi_property(key));
        if self.options.global && (single.is_some() || multi.is_some() || key.starts_with("extra."))
        {
            bail!(
                "The {key} property can not be set in the global config.json file. Use `composer global config` to apply changes to the global composer.json"
            );
        }
        if unset && (single.is_some() || multi.is_some()) {
            return self.config_source.remove_property(key);
        }
        if let Some(single) = single {
            let value = self.single(key, single, values)?;
            return self.config_source.add_property(key, &value);
        }
        if let Some(multi) = multi {
            let value = Self::multi(multi, values)?;
            return self.config_source.add_property(key, &value);
        }
        if let Some(g) = groups(r"^repos?(?:itories)?\.(.+)", key) {
            return self.set_repository(&g[0], values);
        }
        if key.starts_with("extra.") {
            return self.set_extra(key, first);
        }
        if key.starts_with("suggest.") {
            if unset {
                return self.config_source.remove_property(key);
            }
            return self
                .config_source
                .add_property(key, &string(&values.join(" ")));
        }
        if matches!(key, "suggest" | "extra") && unset {
            return self.config_source.remove_property(key);
        }
        if key.starts_with("platform.") {
            if unset {
                return self.config_source.remove_config_setting(key);
            }
            let value = if first == "false" {
                bool_value(false)
            } else {
                string(first)
            };
            return self.config_source.add_config_setting(key, &value);
        }
        if key == "platform" && unset {
            return self.config_source.remove_config_setting(key);
        }
        if matches!(key, "audit.ignore" | "audit.ignore-abandoned") {
            if unset {
                return self.config_source.remove_config_setting(key);
            }
            let value = self.json_or_list(key, values)?;
            let value =
                self.merged_setting(key, value, &["audit", key.trim_start_matches("audit.")])?;
            return self.config_source.add_config_setting(key, &value);
        }
        if let Some(g) = groups(
            r"^(bitbucket-oauth|github-oauth|gitlab-oauth|gitlab-token|http-basic|custom-headers|bearer|forgejo-token)\.(.+)",
            key,
        ) {
            return self.set_auth(&g[0], &g[1], values);
        }
        if key.starts_with("scripts.") {
            if unset {
                return self.config_source.remove_property(key);
            }
            let value = if values.len() > 1 {
                list(values)
            } else {
                string(first)
            };
            return self.config_source.add_property(key, &value);
        }
        if unset {
            return self.config_source.remove_property(key);
        }
        bail!("Setting {key} does not exist or is not supported by this command")
    }

    /// `extra.<key>`: removed, a string, or with `--json` JSON (with `--merge` merged with what is there).
    fn set_extra(&mut self, key: &str, first: &str) -> Result<()> {
        if self.options.unset {
            return self.config_source.remove_property(key);
        }
        let mut value = string(first);
        if self.options.json {
            value = parse_json(first)?;
            if self.options.merge {
                let mut current = Some(read(&self.file_path, &self.file)?);
                for bit in key.split('.') {
                    current = current
                        .as_ref()
                        .and_then(|c| c.get(bit))
                        .filter(|v| isset(Some(v)))
                        .cloned();
                }
                if let Some(current @ Php::Array(_)) = current
                    && matches!(value, Php::Array(_))
                {
                    value = if is_list(&current) && is_list(&value) {
                        super::array_merge(&current, &value)
                    } else {
                        union(&value, &current)
                    };
                }
            }
        }
        self.config_source.add_property(key, &value)
    }

    /// `repositories.<name>`: removed; a type and a URL; `false` (turned off); or JSON.
    fn set_repository(&mut self, name: &str, values: &[String]) -> Result<()> {
        if self.options.unset {
            return self.config_source.remove_repository(name);
        }
        let append = self.options.append;
        match values {
            [kind, url] => {
                let repository = Php::Array(vec![
                    ("type".into(), string(kind)),
                    ("url".into(), string(url)),
                ]);
                self.config_source.add_repository(name, &repository, append)
            }
            [value] => {
                let lower = value.to_lowercase();
                if BOOLEAN.contains(&lower.as_str()) {
                    if !boolean(&string(&lower)) {
                        return self
                            .config_source
                            .add_repository(name, &bool_value(false), append);
                    }
                    bail!(
                        "You must pass the type and a url. Example: php composer.phar config repositories.foo vcs https://bar.com"
                    );
                }
                let repository = parse_json(value)?;
                self.config_source.add_repository(name, &repository, append)
            }
            _ => bail!(
                "You must pass the type and a url. Example: php composer.phar config repositories.foo vcs https://bar.com"
            ),
        }
    }

    /// An auth setting: into the auth file, out of the config file.
    fn set_auth(&mut self, kind: &str, host: &str, values: &[String]) -> Result<()> {
        let key = format!("{kind}.{host}");
        if self.options.unset {
            self.auth_source.remove_config_setting(&key)?;
            return self.config_source.remove_config_setting(&key);
        }
        let pair = |a: &str, b: &str| {
            Php::Array(vec![
                (a.into(), string(&values[0])),
                (b.into(), string(&values[1])),
            ])
        };
        let value = match kind {
            "bitbucket-oauth" if values.len() == 2 => pair("consumer-key", "consumer-secret"),
            "bitbucket-oauth" => bail!(
                "Expected two arguments (consumer-key, consumer-secret), got {}",
                values.len()
            ),
            "gitlab-token" if values.len() == 2 => pair("username", "token"),
            "github-oauth" | "gitlab-oauth" | "gitlab-token" | "bearer" if values.len() == 1 => {
                string(&values[0])
            }
            "github-oauth" | "gitlab-oauth" | "gitlab-token" | "bearer" => {
                bail!("Too many arguments, expected only one token")
            }
            "http-basic" if values.len() == 2 => pair("username", "password"),
            "http-basic" => bail!(
                "Expected two arguments (username, password), got {}",
                values.len()
            ),
            "custom-headers" => {
                if values.is_empty() {
                    bail!("Expected at least one argument (header), got none");
                }
                let header = Regex::new(r"^[^:]+:\s*.+$").expect("valid pattern");
                if let Some(bad) = values
                    .iter()
                    .find(|h| !header.is_match(h.as_bytes()).unwrap_or(false))
                {
                    bail!("Header \"{bad}\" is not in \"Header-Name: Header-Value\" format");
                }
                list(values)
            }
            _ if values.len() == 2 => pair("username", "token"),
            _ => bail!(
                "Expected two arguments (username, access token), got {}",
                values.len()
            ),
        };
        self.config_source.remove_config_setting(&key)?;
        self.auth_source.add_config_setting(&key, &value)
    }
}
