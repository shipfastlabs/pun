//! `composer validate` (Composer 2.11.2): [`validate`] checks one composer.json as `ConfigValidator::validate` does,
//! and [`lock_errors`] checks composer.lock against it as `ValidateCommand` does through Composer's `Locker`.

use crate::error::{Context, Result, bail};
use crate::lock::{Lock, content_hash};
use crate::verify::missing_requirements;

mod jsonlint;
mod loader;
mod schema;
mod spdx;
use pcre2::bytes::Regex;
use serde_json::{Map, Value};
use std::path::Path;
use std::sync::LazyLock;

/// What `ConfigValidator::validate` finds in a composer.json, in Composer's order within each list.
#[derive(Debug, Default, PartialEq)]
pub struct Findings {
    pub errors: Vec<String>,
    /// What only the strict schema refuses: the file works with Composer but can't be published.
    pub publish_errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// The checks `validate`'s flags leave on.
#[derive(Clone, Copy)]
pub struct Checks {
    /// `ValidatingArrayLoader::CHECK_ALL` (off with `--no-check-all`): the unbound and exact constraint warnings.
    pub constraints: bool,
    /// `ConfigValidator::CHECK_VERSION` (off with `--no-check-version`): the warning for a `version` field.
    pub version: bool,
}

/// `ConfigValidator::validate`: a file that doesn't read as a composer.json is one error and nothing else.
pub fn validate(file: &Path, display: &str, checks: Checks) -> Findings {
    let mut findings = Findings::default();
    let (data, text) = match read_text(file, display) {
        Ok(read) => read,
        Err(e) => {
            findings.errors.push(format!("{e:#}"));
            return findings;
        }
    };
    // The schema as shipped, then the strict one: what only the strict one refuses can't be published.
    let lax = schema::errors(&data, false);
    if lax.is_empty() {
        findings.publish_errors = schema::errors(&data, true);
    } else {
        findings.errors = lax;
    }
    let manifest = data.as_object().cloned().unwrap_or_default();
    let warnings = &mut findings.warnings;
    // `is_array($manifest)`: a list as well as an object.
    if (data.is_object() || data.is_array())
        && let Some((key, line)) = first_duplicate_key(&text)
    {
        warnings.push(format!(
            "Key {key} is a duplicate in {display} at line {line}"
        ));
    }
    licenses(&manifest, warnings);
    if checks.version && isset(manifest.get("version")) {
        warnings.push("The version field is present, it is recommended to leave it out if the package is published on Packagist.".into());
    }
    if let Some(name) = manifest
        .get("name")
        .filter(|n| !empty(Some(n)))
        .and_then(Value::as_str)
        && name.bytes().any(|b| b.is_ascii_uppercase())
    {
        findings.publish_errors.push(format!(
            "Name \"{name}\" does not match the best practice (e.g. lower-cased/with-dashes). We suggest using \"{}\" instead. As such you will not be able to submit it to Packagist.",
            suggested_name(name)
        ));
    }
    if manifest.get("type").and_then(Value::as_str) == Some("composer-installer") {
        warnings.push("The package type 'composer-installer' is deprecated. Please distribute your custom installers as plugins from now on. See https://getcomposer.org/doc/articles/plugins.md for plugin documentation.".into());
    }
    links(&manifest, warnings);
    scripts(&manifest, warnings);
    let autoload = manifest.get("autoload");
    for kind in ["psr-0", "psr-4"] {
        if isset(autoload.and_then(|a| a.get(kind)).and_then(|p| p.get(""))) {
            warnings.push(format!(
                "Defining autoload.{kind} with an empty namespace prefix is a bad idea for performance"
            ));
        }
    }
    // `ValidatingArrayLoader`, on the manifest with a version and a name when it has none.
    let mut config = manifest;
    for (key, default) in [("version", "1.0.0"), ("name", "dummy/dummy")] {
        if !isset(config.get(key)) {
            config.insert(key.into(), Value::String(default.into()));
        }
    }
    let loaded = loader::load(config, checks.constraints);
    findings.errors.extend(loaded.errors);
    findings.warnings.extend(loaded.warnings);
    findings
}

/// PHP's `isset()` on a decoded value: present and not null.
fn isset(value: Option<&Value>) -> bool {
    value.is_some_and(|v| !v.is_null())
}

/// PHP's `empty()` on a decoded value: absent, null, false, 0, "", "0", or an empty array or object.
fn empty(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => !b,
        Some(Value::Number(n)) => n.as_f64() == Some(0.0),
        Some(Value::String(s)) => s.is_empty() || s == "0",
        Some(Value::Array(a)) => a.is_empty(),
        Some(Value::Object(o)) => o.is_empty(),
    }
}

/// The SPDX licenses Composer knows (`assets/spdx.json`, from `scripts/export-spdx.php`): lower-cased identifier to
/// whether it is deprecated.
fn spdx_licenses() -> &'static std::collections::HashMap<String, bool> {
    static LICENSES: LazyLock<std::collections::HashMap<String, bool>> = LazyLock::new(|| {
        let data: Value =
            serde_json::from_str(include_str!("../../assets/spdx.json")).expect("valid spdx.json");
        (data["licenses"].as_object().expect("licenses").iter())
            .map(|(id, deprecated)| (id.to_ascii_lowercase(), deprecated.as_bool() == Some(true)))
            .collect()
    });
    &LICENSES
}

/// The license warnings: none given, or a deprecated SPDX identifier.
fn licenses(manifest: &Map<String, Value>, warnings: &mut Vec<String>) {
    static PLUS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^[AL]?GPL-[123](\.[01])?\+$").expect("valid"));
    static PLAIN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^[AL]?GPL-[123](\.[01])?$").expect("valid"));
    let license = manifest.get("license");
    if empty(license) {
        warnings.push("No license specified, it is recommended to do so. For closed-source software you may use \"proprietary\" as license.".into());
        return;
    }
    // `(array) $manifest['license']`, without "proprietary".
    let all: Vec<&Value> = match license {
        Some(Value::Array(items)) => items.iter().collect(),
        Some(Value::Object(items)) => items.values().collect(),
        Some(other) => vec![other],
        None => vec![],
    };
    for license in all
        .into_iter()
        .filter_map(Value::as_str)
        .filter(|l| *l != "proprietary")
    {
        if spdx_licenses().get(&license.to_ascii_lowercase()) != Some(&true) {
            continue;
        }
        let matches = |re: &Regex| re.is_match(license.as_bytes()).unwrap_or(false);
        warnings.push(if matches(&PLUS) {
            format!(
                "License \"{license}\" is a deprecated SPDX license identifier, use \"{}-or-later\" instead",
                license.replace('+', "")
            )
        } else if matches(&PLAIN) {
            format!(
                "License \"{license}\" is a deprecated SPDX license identifier, use \"{license}-only\" or \"{license}-or-later\" instead"
            )
        } else {
            format!("License \"{license}\" is a deprecated SPDX license identifier, see https://spdx.org/licenses/")
        });
    }
}

/// `ConfigValidator`'s suggestion for a name with capitals: a dash between a lower-case letter and a capital, and
/// before a capital that starts a capitalised word after another capital, then all lower-case.
pub(super) fn suggested_name(name: &str) -> String {
    static WORDS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?:([a-z])([A-Z])|([A-Z])([A-Z][a-z]))").expect("valid"));
    let bytes = name.as_bytes();
    let (mut out, mut at) = (Vec::with_capacity(bytes.len() + 4), 0);
    for caps in WORDS
        .captures_iter(bytes)
        .filter_map(std::result::Result::ok)
    {
        let whole = caps.get(0).expect("the match");
        out.extend_from_slice(&bytes[at..whole.start()]);
        let group = |i| caps.get(i).map_or(&[][..], |m| m.as_bytes());
        out.extend_from_slice(group(1));
        out.extend_from_slice(group(3));
        out.push(b'-');
        out.extend_from_slice(group(2));
        out.extend_from_slice(group(4));
        at = whole.end();
    }
    out.extend_from_slice(&bytes[at..]);
    String::from_utf8_lossy(&out).to_ascii_lowercase()
}

/// The requirement warnings: a package in both `require` and `require-dev`, one that `provide` or `replace` already
/// satisfies, and a constraint pointing at a commit.
fn links(manifest: &Map<String, Value>, warnings: &mut Vec<String>) {
    let object = |key: &str| manifest.get(key).and_then(Value::as_object);
    if let (Some(require), Some(require_dev)) = (object("require"), object("require-dev")) {
        let both: Vec<&str> = (require.keys())
            .filter(|k| require_dev.contains_key(*k))
            .map(String::as_str)
            .collect();
        if !both.is_empty() {
            let verb = if both.len() > 1 { "are" } else { "is" };
            warnings.push(format!(
                "{} {verb} required both in require and require-dev, this can lead to unexpected behavior",
                both.join(", ")
            ));
        }
    }
    for link in ["provide", "replace"] {
        let Some(provided) = object(link) else {
            continue;
        };
        for require in ["require", "require-dev"] {
            let Some(required) = object(require) else {
                continue;
            };
            for name in provided.keys().filter(|n| isset(required.get(*n))) {
                warnings.push(format!(
                    "The package {name} in {require} is also listed in {link} which satisfies the requirement. Remove it from {link} if you wish to install it."
                ));
            }
        }
    }
    // `array_merge($require, $requireDev)`: require-dev's constraint for a package in both, at require's place.
    let mut merged: Map<String, Value> = object("require").cloned().unwrap_or_default();
    for (name, constraint) in object("require-dev").into_iter().flatten() {
        merged.insert(name.clone(), constraint.clone());
    }
    for (name, constraint) in &merged {
        if constraint.as_str().is_some_and(|c| c.contains('#')) {
            warnings.push(format!(
                "The package \"{name}\" is pointing to a commit-ref, this is bad practice and can cause unforeseen issues."
            ));
        }
    }
}

/// Descriptions and aliases for scripts that don't exist.
fn scripts(manifest: &Map<String, Value>, warnings: &mut Vec<String>) {
    let scripts = manifest.get("scripts").and_then(Value::as_object);
    let defined = |name: &str| scripts.is_some_and(|s| s.contains_key(name));
    for (key, what) in [
        ("scripts-descriptions", "Description"),
        ("scripts-aliases", "Aliases"),
    ] {
        for name in manifest
            .get(key)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|e| e.0)
        {
            if !defined(name) {
                warnings.push(format!(
                    "{what} for non-existent script \"{name}\" found in \"{key}\""
                ));
            }
        }
    }
}

/// The first duplicate key jsonlint's `DETECT_KEY_CONFLICTS` finds in `text` (valid JSON): the first whose value ends
/// first, as its parser reduces each member then, with the line that value ends on. A key whose earlier value is null
/// is no conflict to it, as it checks with `isset()`.
fn first_duplicate_key(text: &str) -> Option<(String, usize)> {
    let mut scan = Scan {
        text: text.as_bytes(),
        at: 0,
        line: 1,
        end_line: 1,
    };
    scan.value().err()
}

/// A walk over valid JSON that keeps the line each value ends on.
struct Scan<'a> {
    text: &'a [u8],
    at: usize,
    line: usize,
    end_line: usize,
}

impl Scan<'_> {
    fn space(&mut self) {
        while let Some(&b) = self.text.get(self.at).filter(|b| b.is_ascii_whitespace()) {
            self.line += usize::from(b == b'\n');
            self.at += 1;
        }
    }

    /// One value; whether it is null, or the duplicate found inside it.
    fn value(&mut self) -> std::result::Result<bool, (String, usize)> {
        self.space();
        let null = match self.text.get(self.at) {
            Some(b'{') => {
                self.object()?;
                false
            }
            Some(b'[') => {
                self.at += 1;
                self.space();
                while self.text.get(self.at) != Some(&b']') {
                    self.value()?;
                    self.space();
                    self.at += usize::from(self.text.get(self.at) == Some(&b','));
                    self.space();
                }
                self.at += 1;
                false
            }
            Some(b'"') => {
                self.string();
                false
            }
            _ => {
                let start = self.at;
                while self
                    .text
                    .get(self.at)
                    .is_some_and(|b| !b"\t\n\r ,]}".contains(b))
                {
                    self.at += 1;
                }
                self.end_line = self.line;
                return Ok(&self.text[start..self.at] == b"null");
            }
        };
        self.end_line = self.line;
        Ok(null)
    }

    /// An object's members, each checked against the earlier ones once its value ends.
    fn object(&mut self) -> std::result::Result<(), (String, usize)> {
        let mut seen: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
        self.at += 1;
        self.space();
        while self.text.get(self.at) != Some(&b'}') {
            let key = self.string();
            self.space();
            self.at += 1; // the colon
            let null = self.value()?;
            if seen.get(&key) == Some(&false) {
                return Err((key, self.end_line));
            }
            seen.insert(key, null);
            self.space();
            self.at += usize::from(self.text.get(self.at) == Some(&b','));
            self.space();
        }
        self.at += 1;
        Ok(())
    }

    /// A string, decoded.
    fn string(&mut self) -> String {
        let start = self.at;
        self.at += 1;
        while let Some(&b) = self.text.get(self.at) {
            self.at += 1;
            match b {
                b'\\' => self.at += 1,
                b'"' => break,
                _ => {}
            }
        }
        serde_json::from_slice(&self.text[start..self.at]).unwrap_or_default()
    }
}

/// What `RootPackageLoader::load` refuses when Composer loads the root package, before `validate` reports: an invalid
/// name, a link constraint that doesn't parse, a package requiring itself, an invalid link name. Each stops the command.
pub fn load_root(manifest: &Map<String, Value>) -> Result<()> {
    let name = manifest.get("name").filter(|n| !n.is_null());
    if let Some(name) = name.and_then(Value::as_str)
        && let Some(error) = loader::package_naming_error(name, false)
    {
        bail!("Your package name {error}");
    }
    let source = name.and_then(Value::as_str).unwrap_or("__root__");
    // `ArrayLoader::parseLinks`, in `BasePackage::$supportedLinkTypes`' order, with each type's `Link` method.
    let links = [
        ("require", "requires"),
        ("conflict", "conflicts"),
        ("provide", "provides"),
        ("replace", "replaces"),
        ("require-dev", "devRequires"),
    ];
    for (link, method) in links {
        for (target, constraint) in manifest
            .get(link)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let Some(constraint) = constraint.as_str().filter(|c| *c != "self.version") else {
                continue;
            };
            if let Err(e) = crate::constraint::parse(constraint) {
                return Err(e).with_context(|| {
                    format!(
                        "Link constraint in {source} {method} > {} should be a valid version constraint, got \"{constraint}\"",
                        target.to_lowercase()
                    )
                });
            }
        }
    }
    for link in ["require", "require-dev"] {
        let targets = manifest.get(link).and_then(Value::as_object);
        for (target, constraint) in targets.into_iter().flatten() {
            if let Some(constraint) = constraint.as_str() {
                aliases(&target.to_lowercase(), constraint)?;
            }
        }
        if name.is_some() && targets.is_some_and(|t| t.keys().any(|k| k.to_lowercase() == source)) {
            bail!(
                "Root package '{source}' cannot require itself in its composer.json\nDid you accidentally name your root package after an external package?"
            );
        }
    }
    for (link, _) in links {
        for target in manifest
            .get(link)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|e| e.0)
        {
            if let Some(error) = loader::package_naming_error(target, true) {
                bail!("{link}.{error}");
            }
        }
    }
    Ok(())
}

/// `RootPackageLoader::extractAliases` for one requirement: both sides of an `X as Y` must be exact versions.
fn aliases(target: &str, constraint: &str) -> Result<()> {
    static ALIAS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?:^|\| *|, *)([^,\s#|]+)(?:#[^ ]+)? +as +([^,\s|]+)(?:$| *\|| *,)")
            .expect("valid")
    });
    match ALIAS.captures(constraint.as_bytes())? {
        Some(caps) => {
            for i in [1, 2] {
                let version =
                    String::from_utf8_lossy(caps.get(i).expect("a side").as_bytes()).into_owned();
                normalize_in(&version, constraint)?;
            }
            Ok(())
        }
        None if constraint.contains(" as ") => bail!(
            "Invalid alias definition in \"{target}\": \"{constraint}\". Aliases should be in the form \"exact-version as other-exact-version\"."
        ),
        None => Ok(()),
    }
}

/// `VersionParser::normalize($version, $fullVersion)`: its error names the alias side that isn't an exact version.
fn normalize_in(version: &str, full: &str) -> Result<String> {
    crate::version::normalize(version).or_else(|_| {
        let version = version.trim();
        let quoted = crate::php::preg_quote(version);
        let stability = "(?:@(?:stable|RC|beta|alpha|dev))?";
        let matches = |pattern: String| Regex::new(&pattern).is_ok_and(|re| re.is_match(full.as_bytes()).unwrap_or(false));
        let extra = if matches(format!(" +as +{quoted}{stability}$")) {
            format!(" in \"{full}\", the alias must be an exact version")
        } else if matches(format!("^{quoted}{stability} +as +")) {
            format!(
                " in \"{full}\", the alias source must be an exact version, if it is a branch name you should prefix it with dev-"
            )
        } else {
            String::new()
        };
        bail!("Invalid version string \"{version}\"{extra}")
    })
}

/// The composer.json at `file` (`display` as the user named it) as Composer loads the root package: valid JSON that the
/// schema as shipped accepts (`Factory`'s `validateSchema(LAX_SCHEMA)`), so a JSON object.
pub fn read(file: &Path, display: &str) -> Result<Map<String, Value>> {
    let (data, _) = read_text(file, display)?;
    let errors = schema::errors(&data, false);
    if !errors.is_empty() {
        bail!(
            "\"{display}\" does not match the expected JSON schema:\n - {}",
            errors.join("\n - ")
        );
    }
    Ok(data.as_object().cloned().unwrap_or_default())
}

/// The JSON in `file` and its text.
fn read_text(file: &Path, display: &str) -> Result<(Value, String)> {
    let bytes = std::fs::read(file).with_context(|| format!("\"{display}\" is not readable"))?;
    let data = parse_json(&bytes, display)?;
    Ok((data, String::from_utf8_lossy(&bytes).into_owned()))
}

/// PHP's `json_decode` with its default depth: JSON nested in fewer than 512 arrays and objects.
fn json_decode(bytes: &[u8]) -> Option<Value> {
    use serde::Deserialize;
    if nesting(bytes) >= 512 {
        return None;
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    decoder.disable_recursion_limit();
    let data = Value::deserialize(&mut decoder).ok()?;
    decoder.end().ok()?;
    Some(data)
}

/// How deep arrays and objects nest in `bytes`, strings skipped. [`json_decode`] must run this check before it calls
/// `disable_recursion_limit()`: it is the only bound on the recursion after that, so a deeper document can't overflow
/// the stack.
fn nesting(bytes: &[u8]) -> usize {
    let (mut depth, mut deepest, mut string, mut escaped) = (0usize, 0, false, false);
    for &b in bytes {
        match (string, escaped, b) {
            (true, true, _) => escaped = false,
            (true, false, b'\\') => escaped = true,
            (true, false, b'"') | (false, _, b'"') => string = !string,
            (false, _, b'[' | b'{') => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            (false, _, b']' | b'}') => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

/// `JsonFile::parseJson`: the JSON, else jsonlint's first error, else (`json_decode` refusing what jsonlint takes) the
/// UTF-8 error, else null as `json_decode` gives it.
fn parse_json(bytes: &[u8], display: &str) -> Result<Value> {
    if let Some(data) = json_decode(bytes) {
        return Ok(data);
    }
    match jsonlint::lint(bytes) {
        Some(error) => bail!("\"{display}\" does not contain valid JSON\n{error}"),
        None if std::str::from_utf8(bytes).is_err() => {
            bail!("\"{display}\" is not UTF-8, could not parse as JSON")
        }
        None => Ok(Value::Null),
    }
}

/// `ValidateCommand`'s lock errors for a composer.lock that Composer reads (`Locker::isLocked`: it has `packages`): the
/// staleness line when `Locker::isFresh` fails, then `Locker::getMissingRequirementInfo` with the dev requirements.
/// `composer_json` is composer.json's own text, which an old lock's `hash` is the md5 of; `lock_display` is the lock's
/// path as Composer names it.
pub fn lock_errors(
    manifest: &Map<String, Value>,
    composer_json: &[u8],
    lock_text: &[u8],
    lock_display: &str,
) -> Result<Vec<String>> {
    // `Locker::isLocked`: `isset($data['packages'])`, so a lock that isn't an object, or whose packages are null, isn't one.
    let Value::Object(mut lock) = parse_json(lock_text, lock_display)? else {
        return Ok(vec![]);
    };
    if lock.get("packages").is_none_or(Value::is_null) {
        return Ok(vec![]);
    }
    let text = |key: &str| {
        lock.get(key)
            .and_then(Value::as_str)
            .filter(|h| !h.is_empty())
    };
    let fresh = match (text("content-hash"), text("hash")) {
        (Some(hash), _) => hash == content_hash(manifest),
        (None, Some(hash)) => hash == crate::php::md5(composer_json),
        (None, None) => false,
    };
    let mut errors = vec![];
    if !fresh {
        errors.push("- The lock file is not up to date with the latest changes in composer.json, it is recommended that you run `composer update` or `composer update <package name>`.".to_string());
    }
    if lock.get("packages-dev").is_none_or(Value::is_null) {
        bail!(
            "The lock file does not contain require-dev information, run install with the --no-dev option or delete it and run composer update to generate a new lock file."
        );
    }
    // An old lock has `hash` alone; the requirements don't need the content hash.
    lock.entry("content-hash")
        .or_insert_with(|| Value::String(String::new()));
    let lock = Lock::from_composer_lock(&Value::Object(lock).to_string())?;
    errors.extend(missing_requirements(manifest, &lock, true)?);
    Ok(errors)
}
