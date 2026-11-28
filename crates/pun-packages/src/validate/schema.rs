//! Composer 2.11.2's JSON schema check (`JsonFile::validateJsonSchema`) with justinrainbow/json-schema 6.14's draft-04
//! constraints, as far as `assets/composer-schema.json` uses them: `$ref`, `type`, `required`, `not`, `allOf`, `anyOf`,
//! `oneOf`, `properties`, `patternProperties`, `additionalProperties`, `minProperties`, `maxProperties`, `items` (one
//! schema for every item), `minItems`, `pattern`, `minimum`, `maximum`, `enum` (of strings and booleans), and the `uri`
//! and `email` formats. The order of the errors is the library's, merges and all. The constraints the schema doesn't
//! use (string lengths, `maxItems`, tuples, `number`, `const`, `dependencies`) are left out.

use crate::php::{is_email_unicode, json_compact};
use pcre2::bytes::{Regex, RegexBuilder};
use serde_json::{Map, Value};
use std::sync::LazyLock;

static SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../assets/composer-schema.json"))
        .expect("valid composer-schema.json")
});

/// The schema's messages for `data`, as `"<property> : <message>"` or the message alone at the root: with the schema as
/// shipped (`LAX_SCHEMA`), or with `strict` (`STRICT_SCHEMA`) no property beyond the schema's and a required name and
/// description.
pub(super) fn errors(data: &Value, strict: bool) -> Vec<String> {
    let strict_root;
    let root = if strict {
        let mut schema = SCHEMA.clone();
        if let Value::Object(root) = &mut schema {
            root.insert("additionalProperties".into(), Value::Bool(false));
            root.insert(
                "required".into(),
                serde_json::json!(["name", "description"]),
            );
        }
        strict_root = schema;
        &strict_root
    } else {
        &*SCHEMA
    };
    let mut checker = Checker { errors: vec![] };
    checker.undefined(Some(data), root, &[], None);
    // `Validator::validate`'s `array_unique`: the first of each.
    let mut seen: Vec<&Error> = vec![];
    for error in &checker.errors {
        if !seen.contains(&error) {
            seen.push(error);
        }
    }
    (seen.into_iter())
        .map(|e| {
            if e.property.is_empty() {
                e.message.clone()
            } else {
                format!("{} : {}", e.property, e.message)
            }
        })
        .collect()
}

#[derive(Clone, PartialEq)]
struct Error {
    property: String,
    message: String,
}

struct Checker {
    errors: Vec<Error>,
}

/// PHP's `is_numeric` on a string: optional whitespace, a sign, digits with a point, an exponent, trailing whitespace.
fn numeric_string(s: &str) -> bool {
    static NUMERIC: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^[ \t\n\r\x0B\x0C]*[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?[ \t\n\r\x0B\x0C]*$",
        )
        .expect("valid")
    });
    NUMERIC.is_match(s.as_bytes()).unwrap_or(false)
}

fn is_numeric(v: &Value) -> bool {
    match v {
        Value::Number(_) => true,
        Value::String(s) => numeric_string(s),
        _ => false,
    }
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) if numeric_string(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// A PHP integer: a JSON number without a fraction or exponent that fits in 64 bits.
fn is_int(v: &Value) -> bool {
    matches!(v, Value::Number(n) if n.is_i64())
}

/// `gettype()` of a value `json_decode` gives.
fn gettype(v: &Value) -> &'static str {
    match v {
        Value::Null => "NULL",
        Value::Bool(_) => "boolean",
        Value::Number(_) if is_int(v) => "integer",
        Value::Number(_) => "double",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// A message parameter as `BaseConstraint::addError` formats it: a number as PHP prints it, a list as JSON.
fn param(v: &Value) -> String {
    match v {
        Value::Number(n) => n.to_string(),
        _ => json_compact(v),
    }
}

/// `ucfirst`.
fn ucfirst(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |c| {
        c.to_ascii_uppercase().to_string() + chars.as_str()
    })
}

/// A JSON pointer's segments as `convertJsonPointerIntoPropertyPath` writes them: `[n]` for a numeric one, `.name`
/// otherwise, without the leading dot.
#[expect(
    clippy::cast_possible_truncation,
    reason = "sprintf('%d') of a numeric string takes its integer part, as the cast does"
)]
fn property_path(path: &[String]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for segment in path {
        if numeric_string(segment) {
            let n = segment
                .trim()
                .parse::<f64>()
                .map_or(0, |f| f.trunc() as i64);
            let _ = write!(out, "[{n}]");
        } else {
            out.push('.');
            out.push_str(segment);
        }
    }
    out.trim_matches('.').to_string()
}

/// `SchemaStorage::resolveRefSchema`: a schema with a `$ref` is the schema it points at, its other keywords unread.
/// Every `$ref` of Composer's schema is a pointer into it (`#/definitions/...`).
fn resolve(mut schema: &Value) -> &Value {
    while let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let pointer = reference.trim_start_matches('#');
        schema = SCHEMA
            .pointer(pointer)
            .expect("Composer's schema resolves its own pointers");
    }
    schema
}

/// `jsonPatternToPhpRegex`: the pattern as a UTF-8 PCRE; every pattern of Composer's schema compiles.
fn json_pattern(pattern: &str) -> Regex {
    RegexBuilder::new()
        .utf(true)
        .build(pattern)
        .expect("Composer's schema pattern")
}

/// PHP truthiness of the keyword values Composer's schema has: schemas (objects) and booleans.
fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Null | Value::Bool(false))
}

/// json-schema's `UriValidator::isValid`, the `uri` format.
fn is_uri(uri: &str) -> bool {
    static HIERARCHICAL: LazyLock<Regex> = LazyLock::new(|| {
        RegexBuilder::new()
            .caseless(true)
            .extended(true)
            .build(
                r"^
            ([a-z][a-z0-9+\-.]*):\/\/
            (?:([^:@\/?#]+)(?::([^@\/?#]*))?@)?
            ([a-z0-9._~-]+|\[[a-f0-9:.]+\])
            (?::(\d{1,5}))?
            (\/[a-zA-Z0-9._~!$&'()*+,;=:@\/%-]*)*
            (\?([^\#]*))?
            (\#(.*))?
        $",
            )
            .expect("valid")
    });
    static NON_HIERARCHICAL: LazyLock<Regex> = LazyLock::new(|| {
        RegexBuilder::new()
            .caseless(true)
            .extended(true)
            .build(r"^ (mailto|data|urn|news|tel|file): (.+) $")
            .expect("valid")
    });
    static NEWS: LazyLock<Regex> = LazyLock::new(|| {
        RegexBuilder::new()
            .caseless(true)
            .build(r"^[a-z0-9]+(\.[a-z0-9]+)*$")
            .expect("valid")
    });
    static TEL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\+?[0-9.\-() ]+$").expect("valid"));
    static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$").expect("valid")
    });
    static DOTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.\.").expect("valid"));
    static UNSAFE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[<>{}|\\^`]").expect("valid"));
    let bytes = uri.as_bytes();
    let group = |caps: &pcre2::bytes::Captures, i: usize| {
        caps.get(i).map_or(&[][..], |m| m.as_bytes()).to_vec()
    };
    if let Ok(Some(caps)) = HIERARCHICAL.captures(bytes) {
        let host = group(&caps, 4);
        if !host.is_empty() && DOTS.is_match(&host).unwrap_or(false) {
            return false;
        }
        let port = group(&caps, 5);
        // `!empty($matches[5])`: "0" is empty, so port 0 passes.
        if !port.is_empty() && port != b"0" {
            let port: u32 = String::from_utf8_lossy(&port).parse().unwrap_or(0);
            if !(1..=65535).contains(&port) {
                return false;
            }
        }
        let path = group(&caps, 6);
        return path.is_empty() || !UNSAFE.is_match(&path).unwrap_or(false);
    }
    if let Ok(Some(caps)) = NON_HIERARCHICAL.captures(bytes) {
        let rest = group(&caps, 2);
        return match String::from_utf8_lossy(&group(&caps, 1))
            .to_ascii_lowercase()
            .as_str()
        {
            "mailto" => EMAIL.is_match(&rest).unwrap_or(false),
            "news" => NEWS.is_match(&rest).unwrap_or(false),
            "tel" => TEL.is_match(&rest).unwrap_or(false),
            _ => true,
        };
    }
    false
}

impl Checker {
    fn error(&mut self, path: &[String], message: impl Into<String>) {
        self.errors.push(Error {
            property: property_path(path),
            message: ucfirst(&message.into()),
        });
    }

    /// `UndefinedConstraint::check`; `None` is the library's undefined value, a property the data lacks.
    fn undefined(
        &mut self,
        value: Option<&Value>,
        schema: &Value,
        path: &[String],
        i: Option<&str>,
    ) {
        let schema = resolve(schema);
        if !schema.is_object() {
            return;
        }
        let mut path = path.to_vec();
        if let Some(i) = i.filter(|i| !i.is_empty()) {
            path.push(i.to_string());
        }
        let Some(value) = value else {
            return;
        };
        // Each call is a new constraint with errors of its own, which `addErrors` then gives the caller: the snapshots
        // `not`, `anyOf` and `oneOf` take hold this call's errors only.
        let caller = std::mem::take(&mut self.errors);
        self.common(value, schema, &path);
        self.of(value, schema, &path);
        self.types(value, schema, &path);
        let own = std::mem::replace(&mut self.errors, caller);
        self.errors.extend(own);
    }

    /// `validateCommonProperties`: `required`, `type`, `not`.
    fn common(&mut self, value: &Value, schema: &Value, path: &[String]) {
        if let (Value::Object(object), Some(Value::Array(required))) =
            (value, schema.get("required"))
        {
            for name in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(name) {
                    let mut at = path.to_vec();
                    at.push(name.to_string());
                    self.error(&at, format!("The property {name} is required"));
                }
            }
        }
        self.check_type(value, schema, path);
        if let Some(not) = schema.get("not").filter(|n| !n.is_null()) {
            let before = self.errors.clone();
            self.undefined(Some(value), not, path, None);
            if self.errors.len() == before.len() {
                self.error(path, "Matched a schema which it should not");
            } else {
                self.errors = before;
            }
        }
    }

    /// `validateOfProperties`: `allOf`, `anyOf`, `oneOf`.
    fn of(&mut self, value: &Value, schema: &Value, path: &[String]) {
        if let Some(Value::Array(all)) = schema.get("allOf") {
            let mut valid = true;
            for s in all {
                let before = self.errors.len();
                self.undefined(Some(value), s, path, None);
                valid = valid && self.errors.len() == before;
            }
            if !valid {
                self.error(path, "Failed to match all schemas");
            }
        }
        if let Some(Value::Array(any)) = schema.get("anyOf") {
            let start = self.errors.clone();
            let mut valid = false;
            for s in any {
                let before = self.errors.len();
                self.undefined(Some(value), s, path, None);
                if self.errors.len() == before {
                    valid = true;
                    break;
                }
            }
            if valid {
                self.errors = start;
            } else {
                self.error(path, "Failed to match at least one schema");
            }
        }
        if let Some(Value::Array(one)) = schema.get("oneOf") {
            let start = self.errors.clone();
            let (mut all, mut matched) = (vec![], 0);
            for s in one {
                self.errors.clear();
                self.undefined(Some(value), s, path, None);
                if self.errors.is_empty() {
                    matched += 1;
                }
                all.extend(self.errors.iter().cloned());
            }
            if matched == 1 {
                self.errors = start;
            } else {
                // The last branch's errors stay, then every branch's, then those from before.
                self.errors.extend(all);
                self.errors.extend(start);
                self.error(path, "Failed to match exactly one schema");
            }
        }
    }

    /// `validateTypes`: the array, object, string, number and enum constraints.
    fn types(&mut self, value: &Value, schema: &Value, path: &[String]) {
        match value {
            Value::Array(items) => self.collection(items, schema, path),
            Value::Object(object) => self.object(object, schema, path),
            Value::String(s) => self.string(s, schema, path),
            _ => {}
        }
        if is_numeric(value) {
            self.number(value, schema, path);
        }
        if let Some(Value::Array(allowed)) = schema.get("enum") {
            self.check_enum(value, allowed, path);
        }
    }

    /// `TypeConstraint::check`, for the types Composer's schema names.
    fn check_type(&mut self, value: &Value, schema: &Value, path: &[String]) {
        /// A type's name, its wording in the message, and the check.
        type Type = (&'static str, &'static str, fn(&Value) -> bool);
        const TYPES: [Type; 6] = [
            ("integer", "an integer", is_int),
            ("boolean", "a boolean", Value::is_boolean),
            ("object", "an object", Value::is_object),
            ("array", "an array", Value::is_array),
            ("string", "a string", Value::is_string),
            ("null", "a null", Value::is_null),
        ];
        let lookup = |t: &str| {
            TYPES
                .iter()
                .find(|(name, ..)| *name == t)
                .expect("a type of Composer's schema")
        };
        let wording = |t: &str| lookup(t).1;
        let matches = |t: &str| (lookup(t).2)(value);
        let (valid, expected) = match schema.get("type") {
            Some(Value::String(t)) => (matches(t), vec![wording(t)]),
            Some(Value::Array(types)) => {
                let mut words = vec![];
                let mut valid = false;
                for t in types.iter().filter_map(Value::as_str) {
                    if valid {
                        break;
                    }
                    words.push(wording(t));
                    valid = matches(t);
                }
                (valid, words)
            }
            _ => (true, vec![]),
        };
        if valid {
            return;
        }
        // `implodeWith($wording, ', ', 'or')`.
        let expected = match expected.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
            _ => expected.join(", "),
        };
        self.error(
            path,
            format!(
                "{} value found, but {expected} is required",
                ucfirst(gettype(value))
            ),
        );
    }

    /// `CollectionConstraint::check`.
    fn collection(&mut self, items: &[Value], schema: &Value, path: &[String]) {
        let count = items.len();
        if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
            && (count as u64) < min
        {
            self.error(
                path,
                format!("There must be a minimum of {min} items in the array, {count} found"),
            );
        }
        if let Some(item) = schema.get("items") {
            for (k, v) in items.iter().enumerate() {
                self.undefined(Some(v), item, path, Some(&k.to_string()));
            }
        }
    }

    /// `ObjectConstraint::check`: `patternProperties`, `properties`, then each property against `additionalProperties`.
    fn object(&mut self, object: &Map<String, Value>, schema: &Value, path: &[String]) {
        let properties = schema.get("properties").filter(|p| truthy(p));
        let additional = schema.get("additionalProperties").filter(|a| !a.is_null());
        let mut matched: Vec<&str> = vec![];
        if let Some(Value::Object(patterns)) = schema.get("patternProperties") {
            for (pattern, sub) in patterns {
                let regex = json_pattern(pattern);
                for (name, v) in object {
                    if regex.is_match(name.as_bytes()).unwrap_or(false) {
                        matched.push(name);
                        self.undefined(Some(v), sub, path, Some(name));
                    }
                }
            }
        }
        if let Some(Value::Object(properties)) = properties {
            for (name, definition) in properties {
                if definition.is_object() {
                    self.undefined(object.get(name), definition, path, Some(name));
                }
            }
        }
        // `validateElement`: the object's own property count, then each property against `additionalProperties` and
        // its definition's property count.
        self.min_max(object, Some(schema), path);
        for (name, v) in object {
            let definition = properties.and_then(|p| p.get(name)).filter(|d| truthy(d));
            if !matched.contains(&name.as_str()) && definition.is_none() {
                match additional {
                    Some(Value::Bool(false)) if name != "$schema" => self.error(
                        path,
                        format!(
                            "The property {name} is not defined and the definition does not allow additional properties"
                        ),
                    ),
                    Some(sub) if truthy(sub) && *sub != Value::Bool(true) => {
                        self.undefined(Some(v), sub, path, Some(name));
                    }
                    _ => {}
                }
            }
            if let Value::Object(inner) = v {
                self.min_max(inner, definition, path);
            }
        }
    }

    /// `validateMinMaxConstraint`: `minProperties` and `maxProperties` of `definition`, for an object.
    fn min_max(
        &mut self,
        object: &Map<String, Value>,
        definition: Option<&Value>,
        path: &[String],
    ) {
        let count = object.len() as u64;
        let limit = |key: &str| {
            definition
                .and_then(|d| d.get(key))
                .and_then(Value::as_i64)
                .map(|n| n.max(0).unsigned_abs())
        };
        if let Some(min) = limit("minProperties")
            && count < min
        {
            self.error(path, format!("Must contain a minimum of {min} properties"));
        }
        if let Some(max) = limit("maxProperties")
            && count > max
        {
            self.error(path, format!("Must contain no more than {max} properties"));
        }
    }

    /// `StringConstraint::check`: `pattern`, `format`.
    fn string(&mut self, s: &str, schema: &Value, path: &[String]) {
        if let Some(pattern) = schema.get("pattern").and_then(Value::as_str)
            && !json_pattern(pattern)
                .is_match(s.as_bytes())
                .unwrap_or(false)
        {
            self.error(path, format!("Does not match the regex pattern {pattern}"));
        }
        match schema.get("format").and_then(Value::as_str) {
            Some("uri") if !is_uri(s) => self.error(path, "Invalid URL format"),
            Some("email") if !is_email_unicode(s) => self.error(path, "Invalid email"),
            _ => {}
        }
    }

    /// `NumberConstraint::check`: `minimum` and `maximum`, for numbers and numeric strings.
    fn number(&mut self, value: &Value, schema: &Value, path: &[String]) {
        let Some(n) = as_f64(value) else {
            return;
        };
        if let Some(min) = schema.get("minimum").filter(|m| m.is_number())
            && n < min.as_f64().unwrap_or(f64::MIN)
        {
            self.error(
                path,
                format!(
                    "Must have a minimum value greater than or equal to {}",
                    param(min)
                ),
            );
        }
        if let Some(max) = schema.get("maximum").filter(|m| m.is_number())
            && n > max.as_f64().unwrap_or(f64::MAX)
        {
            self.error(
                path,
                format!(
                    "Must have a maximum value less than or equal to {}",
                    param(max)
                ),
            );
        }
    }

    /// `EnumConstraint::check`, for enums of strings and booleans: a value of the same type and equal.
    fn check_enum(&mut self, value: &Value, allowed: &[Value], path: &[String]) {
        if allowed.contains(value) {
            return;
        }
        let list = Value::Array(allowed.to_vec());
        self.error(
            path,
            format!("Does not have a value in the enumeration {}", param(&list)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// json-schema's own `UriValidator::isValid` answers (`uri` in tests/fixtures/php-filters.json, from
    /// scripts/export-php-filters.php).
    #[test]
    fn uris_as_json_schema_validates_them() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/php-filters.json")).unwrap();
        let wrong: Vec<String> = (fixture["uri"].as_object().unwrap().iter())
            .filter(|(uri, valid)| is_uri(uri) != valid.as_bool().unwrap())
            .map(|(uri, valid)| format!("{uri:?}: json-schema says {valid}"))
            .collect();
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
