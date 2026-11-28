use crate::error::{Context, Result, bail};
use crate::version::is_platform;
use pcre2::bytes::Regex;
use pun_tokens::php_compat::strnatcmp;
use serde_json::{Map, Value};
use std::cmp::Ordering;
use std::fmt::Write as _;
use std::sync::LazyLock;

pub fn sort_cmp(a: &str, b: &str) -> Ordering {
    let prefix = |name: &str| -> String {
        if !is_platform(name) {
            return format!("5-{name}");
        }
        for (start, group) in [("php", '0'), ("hhvm", '1'), ("ext", '2'), ("lib", '3')] {
            if name.starts_with(start) {
                return format!("{group}-{name}");
            }
        }
        format!("4-{name}")
    };
    strnatcmp(prefix(a).as_bytes(), prefix(b).as_bytes()).cmp(&0)
}

fn parse_json(text: &str) -> Result<Map<String, Value>> {
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str(text).context("composer.json is not valid JSON")? {
        Value::Object(m) => Ok(m),
        _ => bail!("composer.json must hold an object"),
    }
}

/// Whether PHP decodes the object into a list, which `json_encode` writes as an array.
fn is_list(m: &Map<String, Value>) -> bool {
    m.keys().enumerate().all(|(i, k)| *k == i.to_string())
}

/// `JsonConfigSource::restoreEmptyObjects`: the empty values Composer writes as `{}`, not `[]`.
fn restored(path: &[String]) -> bool {
    const ROOT: [&str; 13] = [
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
    ];
    const CONFIG: [&str; 10] = [
        "policy",
        "platform",
        "http-basic",
        "bearer",
        "gitlab-token",
        "gitlab-oauth",
        "github-oauth",
        "custom-headers",
        "forgejo-token",
        "preferred-install",
    ];
    let path: Vec<&str> = path.iter().map(String::as_str).collect();
    match path[..] {
        [key] => ROOT.contains(&key),
        ["config", key] => CONFIG.contains(&key),
        ["config", "policy", _] | ["autoload" | "autoload-dev", "psr-0" | "psr-4"] => true,
        _ => false,
    }
}

/// What Composer writes when it can't edit in place: the file as `JsonFile::read` decodes it (objects become PHP
/// arrays), after `restoreEmptyObjects`, through `JsonFile::write` with the file's indent, then a newline.
pub(crate) fn write_json(m: Map<String, Value>, indent: &str) -> String {
    fn put(out: &mut String, v: &Value, path: &mut Vec<String>, indent: &str) {
        let items: Vec<(Option<&String>, &Value)> = match v {
            Value::Array(items) => items.iter().map(|v| (None, v)).collect(),
            Value::Object(m) if is_list(m) => m.values().map(|v| (None, v)).collect(),
            Value::Object(m) => m.iter().map(|(k, v)| (Some(k), v)).collect(),
            scalar => return out.push_str(&crate::php::json_pretty(scalar)),
        };
        let object = matches!(v, Value::Object(m) if !is_list(m));
        if items.is_empty() {
            out.push_str(if restored(path) { "{}" } else { "[]" });
            return;
        }
        out.push(if object { '{' } else { '[' });
        for (i, (key, item)) in items.into_iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push('\n');
            out.push_str(&indent.repeat(path.len() + 1));
            if let Some(key) = key {
                let _ = write!(out, "{}: ", encode(key));
            }
            path.push(key.map_or_else(|| i.to_string(), String::clone));
            put(out, item, path, indent);
            path.pop();
        }
        out.push('\n');
        out.push_str(&indent.repeat(path.len()));
        out.push(if object { '}' } else { ']' });
    }
    let mut out = String::new();
    put(&mut out, &Value::Object(m), &mut vec![], indent);
    out.push('\n');
    out
}

/// The JSON grammar Composer's `JsonManipulator` matches values with.
const DEFINES: &str = r#"(?(DEFINE)
   (?<number>    -? (?= [1-9]|0(?!\d) ) \d++ (?:\.\d++)? (?:[eE] [+-]?+ \d++)? )
   (?<boolean>   true | false | null )
   (?<string>    " (?:[^"\\]*+ | \\ ["\\bfnrt\/] | \\ u [0-9A-Fa-f]{4} )* " )
   (?<array>     \[  (?:  (?&json) \s*+ (?: , (?&json) \s*+ )*+  )?+  \s*+ \] )
   (?<pair>      \s*+ (?&string) \s*+ : (?&json) \s*+ )
   (?<object>    \{  (?:  (?&pair)  (?: , (?&pair)  )*+  )?+  \s*+ \} )
   (?<json>      \s*+ (?: (?&number) | (?&boolean) | (?&string) | (?&array) | (?&object) ) )
)"#;

/// The members before the one named, as Composer's patterns skip them.
const BEFORE: &str = r"\s*\{\s*(?:(?&string)\s*:\s*(?&json)\s*,\s*)*?";

/// A pattern in `x` mode after [`DEFINES`]; `i` makes it caseless.
fn pattern(body: &str, dotall: bool, caseless: bool) -> Regex {
    pcre2::bytes::RegexBuilder::new()
        .extended(true)
        .dotall(dotall)
        .caseless(caseless)
        .build(&format!("{DEFINES}{body}"))
        .expect("valid pattern")
}

/// `preg_quote`, plus spaces, which `x` mode would drop.
fn quote(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if r".\+*?[^]$(){}=!<>|:-#".contains(c) || c == ' ' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A package name in a pattern, with `/` also matching JSON's `\/`.
fn quote_name(s: &str) -> String {
    quote(s).replace('/', r"\\?/")
}

fn encode(s: &str) -> String {
    crate::php::json_pretty(&Value::String(s.to_string()))
}

fn group<'t>(c: &pcre2::bytes::Captures<'t>, name: &str) -> &'t str {
    c.name(name).map_or("", |m| {
        std::str::from_utf8(m.as_bytes()).unwrap_or_default()
    })
}

fn is_match(re: &str, s: &str) -> bool {
    Regex::new(re).is_ok_and(|r| r.is_match(s.as_bytes()).unwrap_or(false))
}

/// A value as Composer's PHP holds it: a scalar, an array (an ordered map, a list when its keys are 0, 1, … in
/// order), or a `stdClass` object. `json_decode($json, true)` gives arrays, `json_decode($json)` objects.
#[derive(Clone, Debug, PartialEq)]
pub enum Php {
    Scalar(Value),
    Array(Vec<(String, Php)>),
    Object(Vec<(String, Php)>),
}

impl Php {
    /// `json_decode($json, true)`: JSON objects as arrays.
    pub fn assoc(v: &Value) -> Php {
        match v {
            Value::Array(items) => Php::Array(
                (items.iter().enumerate())
                    .map(|(i, v)| (i.to_string(), Php::assoc(v)))
                    .collect(),
            ),
            Value::Object(m) => {
                Php::Array(m.iter().map(|(k, v)| (k.clone(), Php::assoc(v))).collect())
            }
            scalar => Php::Scalar(scalar.clone()),
        }
    }

    /// `json_decode($json)`: JSON objects as `stdClass`.
    pub fn object(v: &Value) -> Php {
        match v {
            Value::Array(items) => Php::Array(
                (items.iter().enumerate())
                    .map(|(i, v)| (i.to_string(), Php::object(v)))
                    .collect(),
            ),
            Value::Object(m) => {
                Php::Object(m.iter().map(|(k, v)| (k.clone(), Php::object(v))).collect())
            }
            scalar => Php::Scalar(scalar.clone()),
        }
    }

    fn entries(&self) -> Option<&Vec<(String, Php)>> {
        match self {
            Php::Array(e) | Php::Object(e) => Some(e),
            Php::Scalar(_) => None,
        }
    }

    /// `array_key_exists` (or a property of an object).
    pub fn get(&self, key: &str) -> Option<&Php> {
        self.entries()?
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }

    /// `$x[$key] = $value` (or `$x->{$key}`): in place when it is there, else last.
    pub fn set(&mut self, key: &str, value: Php) {
        if let Php::Array(e) | Php::Object(e) = self {
            match e.iter_mut().find(|(k, _)| k == key) {
                Some((_, v)) => *v = value,
                None => e.push((key.to_string(), value)),
            }
        }
    }

    /// `array_is_list`.
    fn is_list(entries: &[(String, Php)]) -> bool {
        entries
            .iter()
            .enumerate()
            .all(|(i, (k, _))| *k == i.to_string())
    }

    /// The value as JSON: a list as an array, any other array or object as an object.
    pub fn to_value(&self) -> Value {
        match self {
            Php::Scalar(v) => v.clone(),
            Php::Array(e) if Php::is_list(e) => {
                Value::Array(e.iter().map(|(_, v)| v.to_value()).collect())
            }
            Php::Array(e) | Php::Object(e) => {
                Value::Object(e.iter().map(|(k, v)| (k.clone(), v.to_value())).collect())
            }
        }
    }
}

/// PHP's `is_numeric` for an array key.
pub(crate) fn is_numeric(s: &str) -> bool {
    is_match(r"^\s*[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?\s*$", s)
}

/// Composer's `JsonManipulator`: edits a JSON file's text in place, keeping its layout.
pub struct Manipulator {
    contents: String,
    newline: &'static str,
    indent: String,
}

impl Manipulator {
    /// For text that parses as an object, so it is `{...}` once trimmed, as Composer's constructor requires.
    pub fn new(text: &str) -> Manipulator {
        let contents = text.trim();
        let contents = if contents.is_empty() { "{}" } else { contents };
        let newline = if contents.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let indent = Regex::new(r#"(?m)^([ \t]+)""#)
            .expect("valid pattern")
            .captures(contents.as_bytes())
            .ok()
            .flatten()
            .map_or("    ".to_string(), |c| {
                String::from_utf8_lossy(&c[1]).into_owned()
            });
        let contents = if contents == "{}" {
            format!("{{{newline}}}")
        } else {
            contents.to_string()
        };
        Manipulator {
            contents,
            newline,
            indent,
        }
    }

    fn decoded(&self) -> Map<String, Value> {
        parse_json(&self.contents).unwrap_or_default()
    }

    /// `getContents`: the text, with a newline.
    pub fn text(&self) -> String {
        format!("{}{}", self.contents, self.newline)
    }

    /// `format` of a decoded array: laid out at `depth`, the way Composer writes a node it adds.
    fn format(&self, v: &Value, depth: usize) -> String {
        self.format_php(&Php::assoc(v), depth, false)
    }

    /// `format`: an array or object laid out at `depth`; an empty object (or array, when `was_object`) as `{` and
    /// `}` on lines of their own, an empty array as `[]`, a list on one line.
    fn format_php(&self, v: &Php, depth: usize, was_object: bool) -> String {
        let (entries, was_object) = match v {
            Php::Object(e) => (e, true),
            Php::Array(e) => (e, was_object),
            Php::Scalar(s) => return crate::php::json_pretty(s),
        };
        let nl = self.newline;
        if entries.is_empty() {
            return if was_object {
                format!("{{{nl}{}}}", self.indent.repeat(depth + 1))
            } else {
                "[]".to_string()
            };
        }
        if !was_object && Php::is_list(entries) {
            let items: Vec<String> = (entries.iter())
                .map(|(_, v)| self.format_php(v, depth + 1, false))
                .collect();
            return format!("[{}]", items.join(", "));
        }
        let pad = self.indent.repeat(depth + 2);
        let elems: Vec<String> = (entries.iter())
            .map(|(k, v)| {
                format!(
                    "{pad}{}: {}",
                    encode(k),
                    self.format_php(v, depth + 1, false)
                )
            })
            .collect();
        format!(
            "{{{nl}{}{nl}{}}}",
            elems.join(&format!(",{nl}")),
            self.indent.repeat(depth + 1)
        )
    }

    /// `addLink`.
    fn add_link(&mut self, kind: &str, package: &str, constraint: &str, sort: bool) -> bool {
        if self.decoded().get(kind).is_none_or(Value::is_null) {
            let mut content = Map::new();
            content.insert(package.into(), constraint.into());
            return self.add_main_key(kind, &Php::assoc(&Value::Object(content)));
        }
        let re = pattern(
            &format!(
                r"^(?P<start>{BEFORE})(?P<property>{}\s*:\s*)(?P<value>(?&json))(?P<end>.*)",
                quote(&encode(kind))
            ),
            true,
            false,
        );
        let Ok(Some(m)) = re.captures(self.contents.as_bytes()) else {
            return false;
        };
        let (start, property, end) = (group(&m, "start"), group(&m, "property"), group(&m, "end"));
        let mut links = group(&m, "value").to_string();
        let existing = pattern(
            &format!(r#""(?P<package>{})"(\s*:\s*)(?&json)"#, quote_name(package)),
            false,
            true,
        );
        if let Ok(Some(found)) = existing.captures(links.as_bytes()) {
            let name = group(&found, "package").to_string();
            let each = pattern(
                &format!(r#""{}"(?P<separator>\s*:\s*)(?&json)"#, quote_name(&name)),
                false,
                true,
            );
            let mut out = String::new();
            let mut last = 0;
            for c in each.captures_iter(links.as_bytes()).flatten() {
                let whole = c.get(0).expect("the match");
                out.push_str(&links[last..whole.start()]);
                let _ = write!(
                    out,
                    "{}{}\"{constraint}\"",
                    encode(&name.replace(r"\/", "/")),
                    group(&c, "separator")
                );
                last = whole.end();
            }
            out.push_str(&links[last..]);
            links = out;
        } else {
            let tail = Regex::new(r"(?s)^\s*\{\s*\S+.*?(\s*\}\s*)$")
                .expect("valid pattern")
                .captures(links.as_bytes())
                .ok()
                .flatten()
                .map(|c| String::from_utf8_lossy(&c[1]).into_owned());
            let entry = format!("{}: {}", encode(package), encode(constraint));
            let (nl, pad) = (self.newline, self.indent.repeat(2));
            links = match tail {
                Some(tail) => format!(
                    "{},{nl}{pad}{entry}{tail}",
                    &links[..links.len() - tail.len()]
                ),
                None => format!("{{{nl}{pad}{entry}{nl}{}}}", self.indent),
            };
        }
        if sort {
            // Composer fails with a TypeError where the links no longer decode, as when the constraint holds a `"`.
            let requirements = match serde_json::from_str::<Value>(&links) {
                Ok(Value::Object(m)) => {
                    let mut entries: Vec<(String, Value)> = m.into_iter().collect();
                    entries.sort_by(|a, b| sort_cmp(&a.0, &b.0));
                    Value::Object(entries.into_iter().collect())
                }
                Ok(list) => list,
                Err(_) => return false,
            };
            links = self.format(&requirements, 0);
        }
        self.contents = format!("{start}{property}{links}{end}");
        true
    }

    /// `removeLink`: the entry, then the section when it is left empty.
    fn remove_link(&mut self, kind: &str, package: &str) -> bool {
        let before = self.contents.clone();
        self.remove_sub_node(kind, package)
            && (before == self.contents || self.remove_main_key_if_empty(kind))
    }

    /// The node `main` of the text, an object: what comes before its value, the value, and what comes after.
    fn node(&self, main: &str, kind: &str) -> Option<(String, String, String)> {
        let re = pattern(
            &format!(
                r"^(?P<start> \s* \{{ \s* (?: (?&string) \s* : (?&json) \s* , \s* )*?{}\s*:\s*)(?P<content>(?&{kind}))(?P<end>.*)",
                quote(&encode(main))
            ),
            true,
            false,
        );
        let m = re.captures(self.contents.as_bytes()).ok().flatten()?;
        Some((
            group(&m, "start").into(),
            group(&m, "content").into(),
            group(&m, "end").into(),
        ))
    }

    /// `matchChild`: the member `name` of the object `children`, as (the text before its value, the members before
    /// it, its value, the comma after it, the length of the whole match).
    fn match_child(name: &str, children: &str) -> Option<(String, String, String, String, usize)> {
        let key = quote_name(&encode(name));
        let child = pattern(
            &format!(
                r"^(?P<start>(?P<prefix>\{{ \s* (?: (?!{key}\s*:) (?&string) \s* : (?&json) \s* , \s* )*+) {key}\s*:\s*)(?P<content>(?&json))(?P<end>,?)"
            ),
            false,
            false,
        );
        let c = child.captures(children.as_bytes()).ok().flatten()?;
        Some((
            group(&c, "start").into(),
            group(&c, "prefix").into(),
            group(&c, "content").into(),
            group(&c, "end").into(),
            c.get(0).expect("the match").end(),
        ))
    }

    /// The main node `main` and `name` in it, split at its first dot for `config`, `extra` and `scripts`.
    fn sub_name(main: &str, name: &str) -> (String, Option<String>) {
        match name.split_once('.') {
            Some((name, sub)) if matches!(main, "config" | "extra" | "scripts") => {
                (name.into(), Some(sub.into()))
            }
            _ => (name.into(), None),
        }
    }

    /// `addSubNode`.
    pub fn add_sub_node(&mut self, main: &str, name: &str, value: &Php, append: bool) -> bool {
        let decoded = Php::assoc(&Value::Object(self.decoded()));
        let (name, sub) = Self::sub_name(main, name);
        let wrap = |value: &Php| match &sub {
            Some(sub) => Php::Array(vec![(sub.clone(), value.clone())]),
            None => value.clone(),
        };
        if decoded
            .get(main)
            .is_none_or(|v| *v == Php::Scalar(Value::Null))
        {
            return self.add_main_key(main, &Php::Array(vec![(name, wrap(value))]));
        }
        let Some((start, mut children, end)) = self.node(main, "object") else {
            return false;
        };
        let nl = self.newline;
        let pad = self.indent.repeat(2);
        if decoded.get(main).and_then(|m| m.get(&name)).is_some() {
            let Some((child_start, _, content, child_end, whole)) =
                Self::match_child(&name, &children)
            else {
                return false;
            };
            let value = match &sub {
                Some(sub) => {
                    let current: Value = serde_json::from_str(&content).unwrap_or(Value::Null);
                    let mut current = match Php::object(&current) {
                        object @ Php::Object(_) => object,
                        array @ Php::Array(_) => array,
                        Php::Scalar(_) => Php::Array(vec![]),
                    };
                    current.set(sub, value.clone());
                    current
                }
                None => value.clone(),
            };
            children = format!(
                "{child_start}{}{child_end}{}",
                self.format_php(&value, 1, false),
                &children[whole..]
            );
        } else {
            let re = Regex::new(
                r"(?s)^\{(?P<leadingspace>\s*?)(?P<content>\S+.*?)?(?P<trailingspace>\s*)\}$",
            )
            .expect("valid pattern");
            let Ok(Some(c)) = re.captures(children.as_bytes()) else {
                return false;
            };
            let (leading, trailing) = (group(&c, "leadingspace"), group(&c, "trailingspace"));
            let entry = format!(
                "{}: {}",
                encode(&name),
                self.format_php(&wrap(value), 1, false)
            );
            children = if c.name("content").is_some() {
                if append {
                    format!(
                        "{},{nl}{pad}{entry}{trailing}}}",
                        &children[..children.len() - trailing.len() - 1]
                    )
                } else {
                    format!(
                        "{{{leading}{entry},{nl}{pad}{}",
                        &children[1 + leading.len()..]
                    )
                }
            } else {
                format!("{{{nl}{pad}{entry}{trailing}}}")
            };
        }
        self.contents = format!("{start}{children}{end}");
        true
    }

    /// `removeSubNode`.
    pub fn remove_sub_node(&mut self, main: &str, name: &str) -> bool {
        let decoded = Php::assoc(&Value::Object(self.decoded()));
        // PHP's `empty()`.
        let empty = match decoded.get(main) {
            None => true,
            Some(Php::Scalar(v)) => match v {
                Value::Null => true,
                Value::Bool(b) => !b,
                Value::Number(n) => n.as_f64() == Some(0.0),
                Value::String(s) => s.is_empty() || s == "0",
                _ => false,
            },
            Some(Php::Array(e) | Php::Object(e)) => e.is_empty(),
        };
        if empty {
            return true;
        }
        let Some((start, children, end)) = self.node(main, "object") else {
            return false;
        };
        let (name, sub) = Self::sub_name(main, name);
        let node = decoded.get(main).expect("not empty");
        let present = matches!(node, Php::Array(_))
            && match node.get(&name) {
                None => false,
                Some(value) => match &sub {
                    Some(sub) => matches!(value, Php::Array(_)) && value.get(sub).is_some(),
                    None => true,
                },
            };
        if !present {
            return true;
        }
        let Some((_, prefix, _, tail, whole)) = Self::match_child(&name, &children) else {
            return false;
        };
        let rest = &children[whole - tail.len()..];
        let clean = if is_match(r",\s*$", &prefix) {
            let cut = Regex::new(r",\s*$")
                .expect("valid pattern")
                .find(prefix.as_bytes())
                .ok()
                .flatten();
            format!(
                "{}{rest}",
                &prefix[..cut.map_or(prefix.len(), |m| m.start())]
            )
        } else {
            let cut = Regex::new(r"^\s*,?\s*")
                .expect("valid pattern")
                .find(rest.as_bytes())
                .ok()
                .flatten();
            format!("{prefix}{}", &rest[cut.map_or(0, |m| m.end())..])
        };
        // `removeKey`: an object without the member, an array without the item (re-indexed).
        let without = |value: &Php, key: &str| match value {
            Php::Object(e) => Php::Object(e.iter().filter(|(k, _)| k != key).cloned().collect()),
            Php::Array(e) => Php::Array(
                (e.iter().filter(|(k, _)| k != key).enumerate())
                    .map(|(i, (_, v))| (i.to_string(), v.clone()))
                    .collect(),
            ),
            scalar @ Php::Scalar(_) => scalar.clone(),
        };
        let current =
            || Php::object(&serde_json::from_str::<Value>(&children).unwrap_or(Value::Null));
        let nothing_left = Regex::new(r"(?s)^\{\s*?(?P<content>\S+.*?)?(?P<trailingspace>\s*)\}$")
            .expect("valid pattern")
            .captures(clean.as_bytes())
            .ok()
            .flatten()
            .is_some_and(|c| c.name("content").is_none());
        if nothing_left {
            self.contents = format!("{start}{{{}{}}}{end}", self.newline, self.indent);
            if let Some(sub) = &sub {
                let value = without(current().get(&name).expect("the member"), sub);
                self.add_sub_node(main, &name, &value, true);
            }
            return true;
        }
        let children = match &sub {
            Some(sub) => {
                let mut whole = current();
                let value = without(whole.get(&name).expect("the member"), sub);
                whole.set(&name, value);
                self.format_php(&whole, 0, false)
            }
            None => clean,
        };
        self.contents = format!("{start}{children}{end}");
        true
    }

    /// `addListItem`.
    pub fn add_list_item(&mut self, main: &str, value: &Php, append: bool) -> bool {
        if self.decoded().get(main).is_none_or(Value::is_null)
            && !self.add_main_key(main, &Php::Array(vec![]))
        {
            return false;
        }
        let Some((start, children, end)) = self.node(main, "array") else {
            return false;
        };
        let re = Regex::new(
            r"(?s)^\[(?P<leadingspace>\s*?)(?P<content>\S+.*?)?(?P<trailingspace>\s*)\]$",
        )
        .expect("valid pattern");
        let Ok(Some(c)) = re.captures(children.as_bytes()) else {
            return false;
        };
        let (leading, trailing) = (group(&c, "leadingspace"), group(&c, "trailingspace"));
        let nl = self.newline;
        let (lead_item, trail_item, depth) = if trailing.contains(nl) {
            (
                format!("{nl}{}", self.indent.repeat(2)),
                trailing.to_string(),
                1,
            )
        } else {
            (leading.to_string(), leading.to_string(), 0)
        };
        let item = self.format_php(value, depth, false);
        let children = if c.name("content").is_some() {
            if append {
                format!(
                    "{},{lead_item}{item}{trail_item}]",
                    &children[..children.len() - trailing.len() - 1]
                )
            } else {
                format!(
                    "[{leading}{item},{lead_item}{}",
                    &children[1 + leading.len()..]
                )
            }
        } else {
            format!("[{lead_item}{item}{trail_item}]")
        };
        self.contents = format!("{start}{children}{end}");
        true
    }

    /// `removeListItem`.
    fn remove_list_item(&mut self, main: &str, index: usize) -> bool {
        let decoded = Php::assoc(&Value::Object(self.decoded()));
        if matches!(decoded.get(main), Some(Php::Array(e)) if e.is_empty()) {
            return true;
        }
        let Some((start, children, end)) = self.node(main, "array") else {
            return false;
        };
        if decoded
            .get(main)
            .and_then(|m| m.get(&index.to_string()))
            .is_none()
        {
            return true;
        }
        let (before, content, after) = match index {
            0 => (
                r"\s*+",
                r"(?&json)\s*+,?\s*+",
                r"(?:((?&json)\s*+,\s*+)*(?:\s*+(?&json))?)\s*+",
            ),
            1 => (
                r"(?&json)\s*+",
                r"\s*+,?\s*+(?&json)",
                r"(?:(\s*+,\s*+(?&json))*(?:\s*+(?&json))?)\s*+",
            ),
            _ => (
                "",
                r"\s*+,?\s*+(?&json)",
                r"(?:(\s*+,\s*+(?&json))*(?:\s*+(?&json))?)\s*+",
            ),
        };
        let before = if index > 1 {
            format!(r"(?&json)\s*+(?:,(?&json)\s*+){{{}}}", index - 1)
        } else {
            before.to_string()
        };
        let re = pattern(
            &format!(r"(?P<start>\[{before})(?P<content>{content})(?P<end>{after}\])"),
            true,
            false,
        );
        let Ok(Some(c)) = re.captures(children.as_bytes()) else {
            return false;
        };
        self.contents = format!("{start}{}{}{end}", group(&c, "start"), group(&c, "end"));
        true
    }

    /// `changeEmptyMainKeyFromAssocToList`.
    fn change_empty_main_key_to_list(&mut self, key: &str) -> bool {
        if !self.decoded().contains_key(key) {
            return true;
        }
        let re = pattern(
            &format!(
                r"^(?P<start>\s*\{{\s*(?:(?&string)\s*:\s*(?&json)\s*,\s*)*?{}\s*:\s*)(?P<removal>\{{(?P<removal_space>\s*+)\}})(?P<end>\s*,?\s*.*)",
                quote(&encode(key))
            ),
            true,
            false,
        );
        let Ok(Some(m)) = re.captures(self.contents.as_bytes()) else {
            return false;
        };
        self.contents = format!(
            "{}[{}]{}",
            group(&m, "start"),
            group(&m, "removal_space"),
            group(&m, "end")
        );
        true
    }

    /// `addMainKey`.
    fn add_main_key(&mut self, key: &str, content: &Php) -> bool {
        let content = self.format_php(content, 0, false);
        let re = pattern(
            &format!(
                r"^(?P<start>{BEFORE})(?P<key>{}\s*:\s*(?&json))(?P<end>.*)",
                quote(&encode(key))
            ),
            true,
            false,
        );
        if self.decoded().contains_key(key)
            && let Ok(Some(m)) = re.captures(self.contents.as_bytes())
        {
            self.contents = format!(
                "{}{}: {content}{}",
                group(&m, "start"),
                encode(key),
                group(&m, "end")
            );
            return true;
        }
        let (nl, indent) = (self.newline, &self.indent);
        let entry = format!("{indent}{}: {content}{nl}}}", encode(key));
        let tail = Regex::new(r"[^{\s](\s*)\}$")
            .expect("valid pattern")
            .captures(self.contents.as_bytes())
            .ok()
            .flatten()
            .map(|c| c[1].len());
        self.contents = match tail {
            Some(n) => format!(
                "{},{nl}{entry}",
                &self.contents[..self.contents.len() - n - 1]
            ),
            None => format!("{}{entry}", &self.contents[..self.contents.len() - 1]),
        };
        true
    }

    /// `removeMainKeyIfEmpty`.
    fn remove_main_key_if_empty(&mut self, key: &str) -> bool {
        match self.decoded().get(key) {
            Some(Value::Object(o)) if o.is_empty() => self.remove_main_key(key),
            Some(Value::Array(a)) if a.is_empty() => self.remove_main_key(key),
            _ => true,
        }
    }

    /// `removeMainKey`.
    pub fn remove_main_key(&mut self, key: &str) -> bool {
        if !self.decoded().contains_key(key) {
            return true;
        }
        let re = pattern(
            &format!(
                r"^(?P<start>{BEFORE})(?P<removal>{}\s*:\s*(?&json))\s*,?\s*(?P<end>.*)",
                quote(&encode(key))
            ),
            true,
            false,
        );
        let Ok(Some(m)) = re.captures(self.contents.as_bytes()) else {
            return false;
        };
        let (mut start, end) = (group(&m, "start").to_string(), group(&m, "end"));
        if is_match(r",\s*$", &start) && end == "}" {
            let comma = start.rfind(',').expect("a comma");
            start = format!("{}{}", &start[..comma], &start[comma + 1..]);
            let chars: Vec<char> = self.indent.chars().collect();
            start = start.trim_end_matches(chars.as_slice()).to_string();
        }
        self.contents = format!("{start}{end}");
        if is_match(r"^\{\s*\}\s*$", &self.contents) {
            self.contents = "{\n}".into();
        }
        true
    }

    /// `addProperty`: a member of `suggest`, `extra` or `scripts`, else a main key.
    pub fn add_property(&mut self, name: &str, value: &Php) -> bool {
        for main in ["suggest", "extra", "scripts"] {
            if let Some(rest) = name.strip_prefix(&format!("{main}.")) {
                return self.add_sub_node(main, rest, value, true);
            }
        }
        self.add_main_key(name, value)
    }

    /// `removeProperty`: a member of `suggest`, `extra`, `scripts`, `autoload` or `autoload-dev`, else a main key.
    pub fn remove_property(&mut self, name: &str) -> bool {
        for main in ["suggest", "extra", "scripts", "autoload", "autoload-dev"] {
            if let Some(rest) = name.strip_prefix(&format!("{main}.")) {
                return self.remove_sub_node(main, rest);
            }
        }
        self.remove_main_key(name)
    }

    /// The list `policy.<list>` of `config`, as an array (empty when it isn't one).
    fn policy_list(&self, list: &str) -> Php {
        let decoded = Php::assoc(&Value::Object(self.decoded()));
        match decoded
            .get("config")
            .and_then(|c| c.get("policy"))
            .and_then(|p| p.get(list))
        {
            Some(array @ Php::Array(_)) => array.clone(),
            _ => Php::Array(vec![]),
        }
    }

    /// `addConfigSetting`: a `config` member; a field of a `policy` list replaces that list.
    pub fn add_config_setting(&mut self, name: &str, value: &Php) -> bool {
        if let Some(rest) = name.strip_prefix("policy.")
            && let Some((list, field)) = rest.split_once('.')
        {
            if field.contains('.') {
                return false;
            }
            let mut block = self.policy_list(list);
            block.set(field, value.clone());
            return self.add_sub_node("config", &format!("policy.{list}"), &block, true);
        }
        self.add_sub_node("config", name, value, true)
    }

    /// `removeConfigSetting`.
    pub fn remove_config_setting(&mut self, name: &str) -> bool {
        if let Some(rest) = name.strip_prefix("policy.")
            && let Some((list, field)) = rest.split_once('.')
        {
            if field.contains('.') {
                return false;
            }
            let block = self.policy_list(list);
            let Some(entries) = block
                .entries()
                .filter(|e| e.iter().any(|(k, _)| k == field))
            else {
                return true;
            };
            let left: Vec<(String, Php)> = entries
                .iter()
                .filter(|(k, _)| k != field)
                .cloned()
                .collect();
            if left.is_empty() {
                return self.remove_config_setting(&format!("policy.{list}"));
            }
            return self.add_sub_node("config", &format!("policy.{list}"), &Php::Array(left), true);
        }
        self.remove_sub_node("config", name)
    }

    /// `doConvertRepositoriesFromAssocToList`: `repositories` as an object becomes a list, a named entry with its
    /// name as `name`.
    fn repositories_to_list(&mut self) -> bool {
        let decoded = Php::object(&Value::Object(self.decoded()));
        let Some(Php::Object(repositories)) = decoded.get("repositories").cloned() else {
            return true;
        };
        for (key, _) in repositories.iter().rev() {
            if !self.remove_sub_node("repositories", key) {
                return false;
            }
        }
        self.change_empty_main_key_to_list("repositories");
        for (name, repository) in repositories {
            let item = match repository {
                Php::Object(fields) if !is_numeric(&name) => {
                    let mut item = vec![("name".to_string(), Php::Scalar(name.into()))];
                    item.extend(fields.into_iter().filter(|(k, _)| k != "name"));
                    Php::Array(item)
                }
                object @ Php::Object(_) => object,
                other => Php::Array(vec![(name, other)]),
            };
            if !self.add_list_item("repositories", &item, true) {
                return false;
            }
        }
        true
    }

    /// `doRemoveRepository`.
    fn remove_repository_entry(&mut self, name: &str) -> bool {
        let decoded = Php::object(&Value::Object(self.decoded()));
        let (assoc, repositories) = match decoded.get("repositories") {
            Some(Php::Object(e)) => (true, e.clone()),
            Some(Php::Array(e)) => (false, e.clone()),
            _ => (false, vec![]),
        };
        for (i, (key, repository)) in repositories.iter().enumerate() {
            if assoc && key == name {
                return self.remove_sub_node("repositories", key);
            }
            if repository.get("name") == Some(&Php::Scalar(name.into())) {
                return if assoc {
                    self.remove_sub_node("repositories", key)
                } else {
                    self.remove_list_item("repositories", i)
                };
            }
            if !assoc
                && let Php::Object(fields) = repository
                && fields.len() == 1
                && fields[0] == (name.to_string(), Php::Scalar(Value::Bool(false)))
            {
                return self.remove_list_item("repositories", i);
            }
        }
        true
    }

    /// `addRepository`: without an entry of that name, as a list, with the repository (named, or `{name: false}`
    /// for one turned off) first, or last when `append`.
    pub fn add_repository(&mut self, name: &str, config: &Php, append: bool) -> bool {
        if !name.is_empty() && !self.remove_repository_entry(name) {
            return false;
        }
        if !self.repositories_to_list() {
            return false;
        }
        let item = match config {
            Php::Array(fields) | Php::Object(fields) if !is_numeric(name) && !name.is_empty() => {
                let mut item = vec![("name".to_string(), Php::Scalar(name.into()))];
                item.extend(fields.iter().filter(|(k, _)| k != "name").cloned());
                Php::Array(item)
            }
            Php::Scalar(Value::Bool(false)) => Php::Array(vec![(name.to_string(), config.clone())]),
            other => other.clone(),
        };
        self.add_list_item("repositories", &item, append)
    }

    /// `removeRepository`.
    pub fn remove_repository(&mut self, name: &str) -> bool {
        let before = self.contents.clone();
        self.remove_repository_entry(name)
            && (before == self.contents || self.remove_main_key_if_empty("repositories"))
    }
}

/// composer.json's text with `name` set to `constraint` in `section` and taken out of `other`, as `composer
/// require` edits it: in place, keeping the file's layout, or rewritten when the file is laid out in a way
/// Composer's patterns don't follow.
pub fn json_set(
    text: &str,
    section: &str,
    other: &str,
    name: &str,
    constraint: &str,
    sort: bool,
) -> Result<String> {
    let mut m = parse_json(text)?;
    let mut edit = Manipulator::new(text);
    if edit.add_link(section, name, constraint, sort) && edit.remove_link(other, name) {
        return Ok(edit.text());
    }
    let links = m
        .entry(section)
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(links) = links else {
        bail!("composer.json: {section} must be an object");
    };
    links.insert(name.to_string(), constraint.into());
    if let Some(Value::Object(o)) = m.get_mut(other) {
        o.shift_remove(name);
        if o.is_empty() {
            m.shift_remove(other);
        }
    }
    Ok(write_json(m, &edit.indent))
}

/// composer.json's text without `name` in `section` (and without `section` when that empties it), as
/// `composer remove` edits it.
pub fn json_remove(text: &str, section: &str, name: &str) -> Result<String> {
    let mut m = parse_json(text)?;
    let mut edit = Manipulator::new(text);
    if edit.remove_link(section, name) {
        return Ok(edit.text());
    }
    if let Some(Value::Object(o)) = m.get_mut(section) {
        o.shift_remove(name);
        if o.is_empty() {
            m.shift_remove(section);
        }
    }
    Ok(write_json(m, &edit.indent))
}

pub fn name_version_pairs(args: &[String]) -> Vec<(String, Option<String>)> {
    static SEPARATOR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([^=: ]+)[=: ](.*)$").expect("valid pattern"));
    static PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?<=[a-z0-9_/-])\*|\*(?=[a-z0-9_/-])").expect("valid pattern")
    });
    let mut out = vec![];
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].trim();
        let mut pair = match SEPARATOR.captures(arg.as_bytes()).ok().flatten() {
            Some(c) => format!(
                "{} {}",
                String::from_utf8_lossy(&c[1]),
                String::from_utf8_lossy(&c[2])
            ),
            None => arg.to_string(),
        };
        if !pair.contains(' ')
            && let Some(next) = args.get(i + 1)
            && !next.contains('/')
            && !PATTERN.is_match(next.as_bytes()).unwrap_or(false)
            && !is_platform(next)
        {
            pair = format!("{pair} {next}");
            i += 1;
        }
        match pair.split_once(' ') {
            Some((name, version)) if !name.is_empty() => {
                out.push((name.to_string(), Some(version.to_string())));
            }
            _ => out.push((pair, None)),
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_as_composer() {
        let mut names = vec![
            "laravel/framework",
            "ext-mbstring",
            "lib-icu",
            "composer-plugin-api",
            "php",
            "acme/pkg10",
            "acme/pkg9",
            "ext-intl",
            "hhvm",
        ];
        names.sort_by(|a, b| sort_cmp(a, b));
        assert_eq!(
            names,
            [
                "php",
                "hhvm",
                "ext-intl",
                "ext-mbstring",
                "lib-icu",
                "composer-plugin-api",
                "acme/pkg9",
                "acme/pkg10",
                "laravel/framework"
            ]
        );
    }

    #[test]
    fn pairs_names_and_versions_as_composer() {
        let args: Vec<String> = [
            "a/b:^1.0", "c/d", "^2", "e/f", "g/h", "ext-intl", "i/j=1.0", "k/l 2.0", "m/n", "o/*",
            "p/q", "*", "r/s",
        ]
        .map(String::from)
        .to_vec();
        let v = |s: &str| Some(s.to_string());
        assert_eq!(
            name_version_pairs(&args),
            [
                ("a/b".into(), v("^1.0")),
                ("c/d".into(), v("^2")),
                ("e/f".into(), None),
                ("g/h".into(), None),
                ("ext-intl".into(), None),
                ("i/j".into(), v("1.0")),
                ("k/l".into(), v("2.0")),
                ("m/n".into(), None),
                ("o/*".into(), None),
                ("p/q".into(), v("*")),
                ("r/s".into(), None),
            ]
        );
    }

    /// `JsonManipulator` cases: `linkProvider`, `providerAddLinkAndSortPackages` and `provideRemoveLink` from
    /// Composer 2.11.2's `JsonManipulatorTest`, then what Composer 2.11.2's manipulator returns for the rest.
    #[test]
    fn manipulates_as_composer() {
        type Edit = fn(&mut Manipulator) -> bool;
        let unsorted = "{\n    \"require\": {\n        \"vendor/baz\": \"qux\"\n    }\n}";
        let cases: [(&str, Edit, bool, &str); 11] = [
            (
                "{\n    \"require\": {\n    }\n}",
                |m| m.add_link("require", "vendor/baz", "qux", false),
                true,
                "{\n    \"require\": {\n        \"vendor/baz\": \"qux\"\n    }\n}\n",
            ),
            (
                unsorted,
                |m| m.add_link("require", "foo", "bar", true),
                true,
                "{\n    \"require\": {\n        \"foo\": \"bar\",\n        \"vendor/baz\": \"qux\"\n    }\n}\n",
            ),
            (
                unsorted,
                |m| m.add_link("require", "foo", "bar", false),
                true,
                "{\n    \"require\": {\n        \"vendor/baz\": \"qux\",\n        \"foo\": \"bar\"\n    }\n}\n",
            ),
            (
                "{\n    \"require\": {\n        \"foo\": \"baz\",\n        \"ext-10gd\": \"*\",\n        \"ext-2mcrypt\": \"*\",\n        \"lib-foo\": \"*\",\n        \"hhvm\": \"*\",\n        \"php\": \">=5.5\"\n    }\n}",
                |m| m.add_link("require", "igorw/retry", "*", true),
                true,
                "{\n    \"require\": {\n        \"php\": \">=5.5\",\n        \"hhvm\": \"*\",\n        \"ext-2mcrypt\": \"*\",\n        \"ext-10gd\": \"*\",\n        \"lib-foo\": \"*\",\n        \"foo\": \"baz\",\n        \"igorw/retry\": \"*\"\n    }\n}\n",
            ),
            (
                "{\n    \"require\": {\n        \"foo/bar\": \"^1\"\n    },\n    \"require-dev\": {\n        \"baz/qux\": \"^1\"\n    }\n}",
                |m| m.remove_link("require-dev", "baz/qux"),
                true,
                "{\n    \"require\": {\n        \"foo/bar\": \"^1\"\n    }\n}\n",
            ),
            (
                "{\n    \"require-dev\": {\n        \"baz/qux\": \"^1\",\n        \"foo/bar\": \"^1\"\n    }\n}",
                |m| m.remove_link("require-dev", "baz/qux"),
                true,
                "{\n    \"require-dev\": {\n        \"foo/bar\": \"^1\"\n    }\n}\n",
            ),
            (
                "{\n    \"require\": {\n        \"foo/bar\": \"^1\"\n    },\n    \"require-dev\": {\n    }\n}",
                |m| m.remove_link("require-dev", "baz/qux"),
                true,
                "{\n    \"require\": {\n        \"foo/bar\": \"^1\"\n    },\n    \"require-dev\": {\n    }\n}\n",
            ),
            // A null section is `isset`-absent, so Composer sets it as a new key, in its own layout.
            (
                "{\n    \"name\": \"foo/bar\",\n    \"require\" :  null\n}",
                |m| m.add_link("require", "foo/baz", "^1", true),
                true,
                "{\n    \"name\": \"foo/bar\",\n    \"require\": {\n        \"foo/baz\": \"^1\"\n    }\n}\n",
            ),
            // Sorting re-lays the links out with `format`, which writes every decoded array inline.
            (
                "{\n    \"require\": {\n        \"b/b\": [],\n        \"a/a\": [1, {\"0\": 2}, {\"k\": [3]}],\n        \"c/c\": {\"x\": {}}\n    }\n}",
                |m| m.add_link("require", "php", "^8.4", true),
                true,
                "{\n    \"require\": {\n        \"php\": \"^8.4\",\n        \"a/a\": [1, [2], {\n                \"k\": [3]\n            }],\n        \"b/b\": [],\n        \"c/c\": {\n            \"x\": []\n        }\n    }\n}\n",
            ),
            (
                "{\n    \"require\": [{\"a/b\": \"1\"}]\n}",
                |m| m.add_link("require", "a/b", "2", true),
                true,
                "{\n    \"require\": [{\n            \"a/b\": \"2\"\n        }]\n}\n",
            ),
            // The constraint goes in unescaped, so the links no longer decode: Composer throws a TypeError.
            (
                "{\n    \"require\": {\n        \"a/b\": \"1\"\n    }\n}",
                |m| m.add_link("require", "a/b", "2\"", true),
                false,
                "",
            ),
        ];
        for (text, edit, ok, expected) in cases {
            let mut m = Manipulator::new(text);
            assert_eq!(edit(&mut m), ok, "{text}");
            if ok {
                assert_eq!(m.text(), expected, "{text}");
            }
        }
        // `removeSubNode` on a section PHP's `empty()` holds empty leaves the file as it is and succeeds;
        // any other value that isn't an object can't be matched.
        for (value, ok) in [
            ("[]", true),
            ("\"0\"", true),
            ("\"\"", true),
            ("0", true),
            ("0.0", true),
            ("false", true),
            ("null", true),
            ("[1]", false),
            ("\"x\"", false),
            ("true", false),
            ("1", false),
        ] {
            let text = format!("{{\n    \"require-dev\": {value}\n}}");
            let mut m = Manipulator::new(&text);
            assert_eq!(m.remove_sub_node("require-dev", "a/b"), ok, "{value}");
            assert_eq!(m.text(), format!("{text}\n"));
        }
    }

    /// What Composer 2.11.2's manipulator leaves of a section emptied or already empty: `removeLink` on an empty list
    /// changes nothing, so the section stays; removing the last repository leaves an empty list, which
    /// `removeMainKeyIfEmpty` takes out with its key.
    #[test]
    fn removes_emptied_sections_as_composer() {
        type Edit = fn(&mut Manipulator) -> bool;
        let cases: [(&str, Edit, &str); 2] = [
            (
                "{\n    \"name\": \"a/b\",\n    \"require\": [],\n    \"require-dev\": {\n        \"x/y\": \"*\"\n    }\n}\n",
                |m| m.remove_link("require", "acme/x"),
                "{\n    \"name\": \"a/b\",\n    \"require\": [],\n    \"require-dev\": {\n        \"x/y\": \"*\"\n    }\n}\n",
            ),
            (
                "{\n    \"name\": \"a/b\",\n    \"repositories\": [\n        {\n            \"name\": \"foo\",\n            \"type\": \"vcs\",\n            \"url\": \"https://example.org/foo\"\n        }\n    ]\n}\n",
                |m| m.remove_repository("foo"),
                "{\n    \"name\": \"a/b\"\n}\n",
            ),
        ];
        for (text, edit, expected) in cases {
            let mut m = Manipulator::new(text);
            assert!(edit(&mut m), "{text}");
            assert_eq!(m.text(), expected, "{text}");
        }
    }

    /// The text Composer 2.11.2's `require -n --no-update` and `remove -n --no-update` write for the same
    /// composer.json.
    #[test]
    fn edits_composer_json() {
        let text = "{\n    \"name\": \"acme/app\",\n    \"require\": {\n        \"php\": \"^8.4\",\n        \"acme/b\": \"1.0\"\n    },\n    \"require-dev\": {\n        \"acme/c\": \"^2.0\"\n    },\n    \"extra\": {}\n}\n";
        let moved = json_set(text, "require", "require-dev", "acme/c", "^2.1", true).unwrap();
        assert_eq!(
            json_set(&moved, "require", "require-dev", "ext-intl", "*", true).unwrap(),
            "{\n    \"name\": \"acme/app\",\n    \"require\": {\n        \"php\": \"^8.4\",\n        \"ext-intl\": \"*\",\n        \"acme/b\": \"1.0\",\n        \"acme/c\": \"^2.1\"\n    },\n    \"extra\": {}\n}\n"
        );
        assert_eq!(
            json_set(text, "require", "require-dev", "acme/b", "2.0", false).unwrap(),
            text.replace("\"acme/b\": \"1.0\"", "\"acme/b\": \"2.0\"")
        );
        assert_eq!(
            json_remove(text, "require", "acme/b").unwrap(),
            "{\n    \"name\": \"acme/app\",\n    \"require\": {\n        \"php\": \"^8.4\"\n    },\n    \"require-dev\": {\n        \"acme/c\": \"^2.0\"\n    },\n    \"extra\": {}\n}\n"
        );
        assert_eq!(
            json_set("", "require", "require-dev", "a/b", "*", false).unwrap(),
            "{\n    \"require\": {\n        \"a/b\": \"*\"\n    }\n}\n"
        );
        // composer remove --dev on the only require-dev entry.
        assert_eq!(
            json_remove(text, "require-dev", "acme/c").unwrap(),
            text.replace(
                "    \"require-dev\": {\n        \"acme/c\": \"^2.0\"\n    },\n",
                ""
            )
        );
        assert!(json_set("[]", "require", "require-dev", "a/b", "*", false).is_err());
        assert!(json_remove("{", "require", "a/b").is_err());
        // Composer rejects both values against its schema; the rewrite can't put a link into a string.
        let strings = "{\"require\": \"x\", \"require-dev\": \"y\"}";
        assert!(json_set(strings, "require", "require-dev", "a/b", "*", false).is_err());
    }

    /// `composer require [--dev] name:constraint` and `composer remove [--dev] name`, run with Composer 2.11.2.
    #[test]
    fn edits_other_layouts_as_composer() {
        // From the fourth case on, most can't be edited in place: a key written with an escape, or a section
        // given twice, which Composer decodes to the last one. Composer rewrites those files with their indent,
        // PHP's empty arrays, lists and floats, and `{}` only where `restoreEmptyObjects` puts it back.
        let dups = "{\n\t\"name\": \"acme/app\",\n\t\"require\": {\n\t\t\"acme/b\": \"1.0\"\n\t},\n\t\"require-dev\": {\n\t\t\"acme/c\": \"^2.0\"\n\t},\n\t\"require-dev\": {\n\t\t\"acme/d\": \"^3.0\"\n\t},\n\t\"extra\": {\"a\": {}, \"b\": {\"0\": \"x\"}, \"c\": 1.0, \"d\": \"\\u00e9\\/\"},\n\t\"autoload\": {\"psr-4\": {}},\n\t\"config\": {\"platform\": {}, \"policy\": {\"advisories\": {}}},\n\t\"support\": {}\n}\n";
        let rewritten = "{\n\t\"name\": \"acme/app\",\n\t\"require\": {\n\t\t\"acme/b\": \"1.0\",\n\t\t\"acme/d\": \"^3.1\"\n\t},\n\t\"extra\": {\n\t\t\"a\": [],\n\t\t\"b\": [\n\t\t\t\"x\"\n\t\t],\n\t\t\"c\": 1,\n\t\t\"d\": \"é/\"\n\t},\n\t\"autoload\": {\n\t\t\"psr-4\": {}\n\t},\n\t\"config\": {\n\t\t\"platform\": {},\n\t\t\"policy\": {\n\t\t\t\"advisories\": {}\n\t\t}\n\t},\n\t\"support\": {}\n}\n";
        let removed = rewritten.replace(",\n\t\t\"acme/d\": \"^3.1\"", "");
        let cases: [(&str, &str, &str, Option<&str>, &str); 10] = [
            (
                "{\r\n    \"name\": \"acme/app\",\r\n    \"require\": {\r\n        \"acme/b\": \"1.0\"\r\n    }\r\n}\r\n",
                "require",
                "acme/c",
                Some("^2.0"),
                "{\r\n    \"name\": \"acme/app\",\r\n    \"require\": {\r\n        \"acme/b\": \"1.0\",\r\n        \"acme/c\": \"^2.0\"\r\n    }\r\n}\r\n",
            ),
            (
                "{\n    \"name\": \"acme/app\"\n}\n",
                "require",
                "acme/b",
                Some("^1.0"),
                "{\n    \"name\": \"acme/app\",\n    \"require\": {\n        \"acme/b\": \"^1.0\"\n    }\n}\n",
            ),
            (
                "{\n    \"require\": {\n        \"acme/b\": \"^1.0\"\n    }\n}\n",
                "require",
                "acme/b",
                None,
                "{\n}\n",
            ),
            (
                "{\n  \"name\": \"acme/app\",\n  \"requir\\u0065\": {\n    \"acme/b\": \"1.0\"\n  },\n  \"extra\": {\"a\": {}, \"b\": {\"0\": \"x\"}, \"c\": 1.0},\n  \"autoload\": {\"psr-4\": {}}\n}\n",
                "require",
                "acme/c",
                Some("^2.1"),
                "{\n  \"name\": \"acme/app\",\n  \"require\": {\n    \"acme/b\": \"1.0\",\n    \"acme/c\": \"^2.1\"\n  },\n  \"extra\": {\n    \"a\": [],\n    \"b\": [\n      \"x\"\n    ],\n    \"c\": 1\n  },\n  \"autoload\": {\n    \"psr-4\": {}\n  }\n}\n",
            ),
            (
                "{\n    \"name\": \"acme/app\",\n    \"require-d\\u0065v\": {\n        \"acme/c\": \"^2.0\",\n        \"acme/d\": \"^3.0\"\n    }\n}\n",
                "require-dev",
                "acme/c",
                None,
                "{\n    \"name\": \"acme/app\",\n    \"require-dev\": {\n        \"acme/d\": \"^3.0\"\n    }\n}\n",
            ),
            (dups, "require", "acme/d", Some("^3.1"), rewritten),
            (dups, "require-dev", "acme/d", None, &removed),
            // `--dev`: the first `require` holds the name, so Composer edits it in place, and keeps it as the
            // section Composer decodes is the second, still not empty.
            (
                "{\n    \"keywords\": [\"a\"],\n    \"require\": {\n        \"acme/d\": \"^3.0\"\n    },\n    \"require\": {\n        \"acme/d\": \"^3.0\",\n        \"acme/e\": \"^1.0\"\n    }\n}\n",
                "require-dev",
                "acme/d",
                Some("^3.1"),
                "{\n    \"keywords\": [\"a\"],\n    \"require\": {\n    },\n    \"require\": {\n        \"acme/d\": \"^3.0\",\n        \"acme/e\": \"^1.0\"\n    },\n    \"require-dev\": {\n        \"acme/d\": \"^3.1\"\n    }\n}\n",
            ),
            // It doesn't, so Composer rewrites the file.
            (
                "{\n    \"keywords\": [\"a\", \"b\"],\n    \"require\": {\n        \"acme/c\": \"^2.0\"\n    },\n    \"require\": {\n        \"acme/d\": \"^3.0\",\n        \"acme/e\": \"^1.0\"\n    }\n}\n",
                "require-dev",
                "acme/d",
                Some("^3.1"),
                "{\n    \"keywords\": [\n        \"a\",\n        \"b\"\n    ],\n    \"require\": {\n        \"acme/e\": \"^1.0\"\n    },\n    \"require-dev\": {\n        \"acme/d\": \"^3.1\"\n    }\n}\n",
            ),
            // Composer's schema rejects this file on load; `JsonConfigSource::removeLink`, run on it, rewrites it.
            (
                "{\n  \"require\": [1, {\"x\": {}}]\n}\n",
                "require",
                "a/b",
                None,
                "{\n  \"require\": [\n    1,\n    {\n      \"x\": []\n    }\n  ]\n}\n",
            ),
        ];
        for (text, section, name, constraint, expected) in cases {
            let edited = match constraint {
                Some(c) => {
                    let other = if section == "require" {
                        "require-dev"
                    } else {
                        "require"
                    };
                    json_set(text, section, other, name, c, false)
                }
                None => json_remove(text, section, name),
            };
            assert_eq!(edited.unwrap(), expected, "{text}");
        }
    }
}
