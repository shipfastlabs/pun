use pun_tokens::php_compat::var_export_string;
use serde_json::Value;
use std::fmt::Write;

pub fn md5(bytes: &[u8]) -> String {
    use md5::Digest;
    md5::Md5::digest(bytes)
        .iter()
        .fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

pub fn sha1(bytes: &[u8]) -> String {
    use sha1::Digest;
    sha1::Sha1::digest(bytes)
        .iter()
        .fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

pub fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

pub fn json_compact(v: &Value) -> String {
    let mut out = String::new();
    json(&mut out, v, None, true);
    out
}

pub fn json_pretty_escaped(v: &Value) -> String {
    let mut out = String::new();
    json(&mut out, v, Some(0), true);
    out
}

pub fn json_pretty(v: &Value) -> String {
    let mut out = String::new();
    json(&mut out, v, Some(0), false);
    out
}

fn is_list(m: &serde_json::Map<String, Value>) -> bool {
    m.keys().enumerate().all(|(i, k)| *k == i.to_string())
}

fn json(out: &mut String, v: &Value, indent: Option<usize>, escape: bool) {
    let newline = |out: &mut String, level: usize| {
        if indent.is_some() {
            out.push('\n');
            out.push_str(&"    ".repeat(level));
        }
    };
    let level = indent.unwrap_or(0);
    let deeper = indent.map(|l| l + 1);
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => write!(out, "{b}").unwrap(),
        Value::Number(n) => match n.as_i64().map(|_| n.to_string()) {
            Some(i) => out.push_str(&i),
            None => out.push_str(&float(n.as_f64().unwrap_or_default())),
        },
        Value::String(s) => string(out, s, escape),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, level + 1);
                json(out, item, deeper, escape);
            }
            newline(out, level);
            out.push(']');
        }
        Value::Object(m) if is_list(m) => {
            json(
                out,
                &Value::Array(m.values().cloned().collect()),
                indent,
                escape,
            );
        }
        Value::Object(m) => {
            out.push('{');
            for (i, (k, item)) in m.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, level + 1);
                string(out, k, escape);
                out.push_str(if indent.is_some() { ": " } else { ":" });
                json(out, item, deeper, escape);
            }
            newline(out, level);
            out.push('}');
        }
    }
}

fn float(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{f:.0}")
    } else {
        format!("{f}")
    }
}

fn string(out: &mut String, s: &str, escape: bool) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '/' if escape => out.push_str("\\/"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32).unwrap(),
            '\u{2028}' | '\u{2029}' => write!(out, "\\u{:04x}", c as u32).unwrap(),
            c if escape && !c.is_ascii() => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    write!(out, "\\u{u:04x}").unwrap();
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

pub fn serialize(v: &Value) -> String {
    let mut out = String::new();
    serialize_into(&mut out, v);
    out
}

fn serialize_into(out: &mut String, v: &Value) {
    match v {
        Value::Null => out.push_str("N;"),
        Value::Bool(b) => write!(out, "b:{};", u8::from(*b)).unwrap(),
        Value::Number(n) => match n.as_i64() {
            Some(i) => write!(out, "i:{i};").unwrap(),
            None => write!(out, "d:{};", float(n.as_f64().unwrap_or_default())).unwrap(),
        },
        Value::String(s) => write!(out, "s:{}:\"{s}\";", s.len()).unwrap(),
        Value::Array(items) => {
            write!(out, "a:{}:{{", items.len()).unwrap();
            for (i, item) in items.iter().enumerate() {
                write!(out, "i:{i};").unwrap();
                serialize_into(out, item);
            }
            out.push('}');
        }
        Value::Object(m) => {
            write!(out, "a:{}:{{", m.len()).unwrap();
            for (k, item) in m {
                match Key::of(k) {
                    Key::Int(i) => write!(out, "i:{i};").unwrap(),
                    Key::Str(s) => write!(out, "s:{}:\"{s}\";", s.len()).unwrap(),
                }
                serialize_into(out, item);
            }
            out.push('}');
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Php {
    Str(String),
    Int(i64),
    Array(Vec<(Key, Php)>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    Str(String),
    Int(i64),
}

impl Key {
    pub fn of(k: &str) -> Key {
        match k.parse::<i64>() {
            Ok(i) if i.to_string() == k => Key::Int(i),
            _ => Key::Str(k.to_string()),
        }
    }
}

pub fn var_export(v: &Php) -> String {
    let mut out = String::new();
    export(&mut out, v, 0);
    out
}

pub fn export_str(s: &str) -> String {
    String::from_utf8(var_export_string(s.as_bytes())).expect("var_export keeps UTF-8 valid")
}

fn export(out: &mut String, v: &Php, level: usize) {
    match v {
        Php::Str(s) => out.push_str(&export_str(s)),
        Php::Int(i) => write!(out, "{i}").unwrap(),
        Php::Array(pairs) => {
            out.push_str("array (\n");
            let pad = "  ".repeat(level + 1);
            for (k, item) in pairs {
                out.push_str(&pad);
                match k {
                    Key::Str(s) => out.push_str(&export_str(s)),
                    Key::Int(i) => write!(out, "{i}").unwrap(),
                }
                out.push_str(" => ");
                if matches!(item, Php::Array(_)) {
                    out.push('\n');
                    out.push_str(&pad);
                }
                export(out, item, level + 1);
                out.push_str(",\n");
            }
            out.push_str(&"  ".repeat(level));
            out.push(')');
        }
    }
}

pub fn strtr(s: &str, pairs: &[(&str, &str)]) -> String {
    let mut pairs: Vec<&(&str, &str)> = pairs.iter().filter(|p| !p.0.is_empty()).collect();
    pairs.sort_by_key(|p| std::cmp::Reverse(p.0.len()));
    let firsts: Vec<u8> = pairs.iter().map(|p| p.0.as_bytes()[0]).collect();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    'outer: while i < s.len() {
        let next = (bytes[i..].iter())
            .position(|b| firsts.contains(b))
            .map_or(s.len(), |n| i + n);
        out.push_str(&s[i..next]);
        i = next;
        if i == s.len() {
            break;
        }
        for (from, to) in &pairs {
            if s[i..].starts_with(from) {
                out.push_str(to);
                i += from.len();
                continue 'outer;
            }
        }
        let c = s[i..].chars().next().expect("a character at a first byte");
        out.push(c);
        i += c.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_matches_php() {
        let v: Value = serde_json::from_str(
            r#"{"a/b":"é/𝄞","l":{"0":"x","1":"y"},"e":{},"n":[1,1.5,2.0,true,null],"c":"\u0001\b\u2028\"\f\n\r\t"}"#,
        )
        .unwrap();
        assert_eq!(
            json_compact(&v),
            r#"{"a\/b":"\u00e9\/\ud834\udd1e","l":["x","y"],"e":[],"n":[1,1.5,2,true,null],"c":"\u0001\b\u2028\"\f\n\r\t"}"#
        );
        assert_eq!(
            json_pretty(&v),
            "{\n    \"a/b\": \"é/𝄞\",\n    \"l\": [\n        \"x\",\n        \"y\"\n    ],\n    \"e\": [],\n    \"n\": [\n        1,\n        1.5,\n        2,\n        true,\n        null\n    ],\n    \"c\": \"\\u0001\\b\\u2028\\\"\\f\\n\\r\\t\"\n}"
        );
        assert_eq!(
            json_pretty(&json!({"k": {"x": 1}})),
            "{\n    \"k\": {\n        \"x\": 1\n    }\n}"
        );
        assert_eq!(
            json_pretty_escaped(&json!(["A\\B", "a/é"])),
            "[\n    \"A\\\\B\",\n    \"a\\/\\u00e9\"\n]"
        );
        assert_eq!(md5(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(sha1(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn serialize_matches_php() {
        let v: Value = serde_json::from_str(
            r#"{"symlink":false,"versions":{"a/b":"1.0"},"7":[1,2.5,null,"é"],"07":true}"#,
        )
        .unwrap();
        assert_eq!(
            serialize(&v),
            r#"a:4:{s:7:"symlink";b:0;s:8:"versions";a:1:{s:3:"a/b";s:3:"1.0";}i:7;a:4:{i:0;i:1;i:1;d:2.5;i:2;N;i:3;s:2:"é";}s:2:"07";b:1;}"#
        );
    }

    #[test]
    fn var_export_matches_php() {
        let v = Php::Array(vec![
            (
                Key::of("v"),
                Php::Array(vec![(Key::Str("voku\\".into()), Php::Int(5))]),
            ),
            (Key::of("0"), Php::Str("a'b".into())),
            (Key::of("07"), Php::Array(vec![])),
        ]);
        assert_eq!(
            var_export(&v),
            "array (\n  'v' => \n  array (\n    'voku\\\\' => 5,\n  ),\n  0 => 'a\\'b',\n  '07' => \n  array (\n  ),\n)"
        );
    }

    #[test]
    fn strtr_matches_php() {
        assert_eq!(
            strtr(
                "/a/b/c",
                &[("/a", "X"), ("/a/b", "Y"), ("Y", "Z"), ("", "E")]
            ),
            "Y/c"
        );
        assert_eq!(strtr("é/a", &[("/a", "!")]), "é!");
    }
}

/// `filter_var($value, FILTER_VALIDATE_EMAIL)`: PHP's own pattern (`regexp0` of `php_filter_validate_email` in
/// `ext/filter/logical_filters.c`), for addresses of at most 320 bytes.
pub fn is_email(value: &str) -> bool {
    static EMAIL: std::sync::LazyLock<pcre2::bytes::Regex> =
        std::sync::LazyLock::new(|| email_pattern(false));
    value.len() <= 320 && EMAIL.is_match(value.as_bytes()).unwrap_or(false)
}

/// The same with `FILTER_FLAG_EMAIL_UNICODE` (`regexp1`): letters and digits of any script in the local part, matched
/// as UTF-8.
pub fn is_email_unicode(value: &str) -> bool {
    static EMAIL: std::sync::LazyLock<pcre2::bytes::Regex> =
        std::sync::LazyLock::new(|| email_pattern(true));
    value.len() <= 320 && EMAIL.is_match(value.as_bytes()).unwrap_or(false)
}

/// PHP's email pattern; `unicode` adds `\pL\pN` to the local part's characters and the `u` flag.
fn email_pattern(unicode: bool) -> pcre2::bytes::Regex {
    let local = if unicode { r"\pL\pN" } else { "" };
    let pattern = [
        r"^(?!(?:(?:\x22?\x5C[\x00-\x7E]\x22?)|(?:\x22?[^\x5C\x22]\x22?)){255,})",
        r"(?!(?:(?:\x22?\x5C[\x00-\x7E]\x22?)|(?:\x22?[^\x5C\x22]\x22?)){65,}@)",
        &format!(r"(?:(?:[\x21\x23-\x27\x2A\x2B\x2D\x2F-\x39\x3D\x3F\x5E-\x7E{local}]+)|"),
        &format!(r"(?:\x22(?:[\x01-\x08\x0B\x0C\x0E-\x1F\x21\x23-\x5B\x5D-\x7F{local}]|(?:\x5C[\x00-\x7F]))*\x22))"),
        &format!(r"(?:\.(?:(?:[\x21\x23-\x27\x2A\x2B\x2D\x2F-\x39\x3D\x3F\x5E-\x7E{local}]+)|"),
        &format!(r"(?:\x22(?:[\x01-\x08\x0B\x0C\x0E-\x1F\x21\x23-\x5B\x5D-\x7F{local}]|(?:\x5C[\x00-\x7F]))*\x22)))*"),
        r"@(?:(?:(?!.*[^.]{64,})(?:(?:(?:xn--)?[a-z0-9]+(?:-+[a-z0-9]+)*\.){1,126}){1,}",
        r"(?:(?:[a-z][a-z0-9]*)|(?:(?:xn--)[a-z0-9]+))(?:-+[a-z0-9]+)*)|",
        r"(?:\[(?:(?:IPv6:(?:(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){7})|",
        r"(?:(?!(?:.*[a-f0-9][:\]]){7,})(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){0,5})?::",
        r"(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){0,5})?)))|",
        r"(?:(?:IPv6:(?:(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){5}:)|",
        r"(?:(?!(?:.*[a-f0-9]:){5,})(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){0,3})?::",
        r"(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){0,3}:)?)))?",
        r"(?:(?:25[0-5])|(?:2[0-4][0-9])|(?:1[0-9]{2})|(?:[1-9]?[0-9]))",
        r"(?:\.(?:(?:25[0-5])|(?:2[0-4][0-9])|(?:1[0-9]{2})|(?:[1-9]?[0-9]))){3}))\]))\z",
    ]
    .concat();
    pcre2::bytes::RegexBuilder::new()
        .caseless(true)
        .utf(unicode)
        .ucp(unicode)
        .build(&pattern)
        .expect("PHP's email pattern")
}

/// The scheme and host `parse_url` finds, as far as `php_url_parse_ex2` (ext/standard/url.c) reads them; `None` where
/// it returns false. Control characters in either become `_`, as `php_replace_controlchars_ex` makes them.
#[derive(Debug, PartialEq, Eq)]
pub struct UrlParts {
    pub scheme: Option<String>,
    pub host: Option<String>,
}

#[expect(
    clippy::too_many_lines,
    reason = "a port of php_url_parse_ex2, kept as one function as upstream has it"
)]
pub fn parse_url(url: &str) -> Option<UrlParts> {
    let b = url.as_bytes();
    let ue = b.len();
    let text = |from: usize, to: usize| {
        let mut s = b[from..to].to_vec();
        for c in &mut s {
            if c.is_ascii_control() {
                *c = b'_';
            }
        }
        Some(String::from_utf8_lossy(&s).into_owned())
    };
    let find =
        |from: usize, to: usize, c: u8| b[from..to].iter().position(|&x| x == c).map(|i| from + i);
    let rfind =
        |from: usize, to: usize, c: u8| b[from..to].iter().rposition(|&x| x == c).map(|i| from + i);
    let relative = |s: usize| s + 1 < ue && b[s] == b'/' && b[s + 1] == b'/';
    let mut parts = UrlParts {
        scheme: None,
        host: None,
    };
    // `ret->port`: 0 until a port is read.
    let mut port = 0i64;
    let mut s = 0;

    // The port after a colon with no scheme before it ("a.com:80", ":80"): `parse_port`.
    let port_after = |e: usize, s: &mut usize, port: &mut i64| -> Option<bool> {
        let p = e + 1;
        let mut pp = p;
        while pp < ue && pp - p < 6 && b[pp].is_ascii_digit() {
            pp += 1;
        }
        if pp > p && pp - p < 6 && (pp == ue || b[pp] == b'/') {
            let value: i64 = std::str::from_utf8(&b[p..pp]).ok()?.parse().ok()?;
            if value > 65535 {
                return None;
            }
            *port = value;
            if relative(*s) {
                *s += 2;
            }
            Some(true)
        } else if p == pp && pp == ue {
            None
        } else if relative(*s) {
            *s += 2;
            Some(true)
        } else {
            Some(false)
        }
    };

    let host_follows = match find(0, ue, b':') {
        Some(e) if e != 0 => {
            // A scheme is letters, digits, '+', '.' and '-'.
            if (0..e).any(|i| !(b[i].is_ascii_alphanumeric() || b"+.-".contains(&b[i]))) {
                let question = find(0, ue, b'?');
                if e + 1 < ue && question.is_some_and(|q| e < q) {
                    port_after(e, &mut s, &mut port)?
                } else if relative(0) {
                    s = 2;
                    true
                } else {
                    false
                }
            } else if e + 1 == ue {
                parts.scheme = text(0, e);
                return Some(parts);
            } else if b[e + 1] != b'/' {
                let mut p = e + 1;
                while p < ue && b[p].is_ascii_digit() {
                    p += 1;
                }
                if (p == ue || b[p] == b'/') && p - e < 7 {
                    port_after(e, &mut s, &mut port)?
                } else {
                    parts.scheme = text(0, e);
                    false
                }
            } else {
                parts.scheme = text(0, e);
                if e + 2 < ue && b[e + 2] == b'/' {
                    s = e + 3;
                    let file = parts
                        .scheme
                        .as_deref()
                        .is_some_and(|sc| sc.eq_ignore_ascii_case("file"));
                    !(file && e + 3 < ue && b[e + 3] == b'/')
                } else {
                    false
                }
            }
        }
        Some(e) => port_after(e, &mut s, &mut port)?,
        None if relative(0) => {
            s = 2;
            true
        }
        None => false,
    };
    if !host_follows {
        return Some(parts);
    }

    // `parse_host`: up to the first '/', '?' or '#', after any user info, before any port.
    let e = b"/?#"
        .iter()
        .filter_map(|&c| find(s, ue, c))
        .min()
        .unwrap_or(ue);
    if let Some(at) = rfind(s, e, b'@') {
        s = at + 1;
    }
    let colon = if s < ue && b[s] == b'[' && e > s && b[e - 1] == b']' {
        None
    } else {
        rfind(s, e, b':')
    };
    let p = match colon {
        // `if (!ret->port)`: a port of 0 is read again.
        Some(colon) if port == 0 => {
            let digits = &b[colon + 1..e];
            if digits.len() > 5 {
                return None;
            }
            if !digits.is_empty() {
                // `ZEND_STRTOL`: leading whitespace and a sign, then digits; none at all is no port.
                let text = std::str::from_utf8(digits).ok()?.trim_start();
                let (sign, rest) = match text.as_bytes().first() {
                    Some(b'-') => (-1, &text[1..]),
                    Some(b'+') => (1, &text[1..]),
                    _ => (1, text),
                };
                let end = rest
                    .bytes()
                    .position(|c| !c.is_ascii_digit())
                    .unwrap_or(rest.len());
                if end == 0 {
                    return None;
                }
                let value = sign * rest[..end].parse::<i64>().ok()?;
                if !(0..=65535).contains(&value) {
                    return None;
                }
            }
            colon
        }
        Some(colon) => colon,
        None => e,
    };
    if p <= s {
        return None;
    }
    parts.host = text(s, p);
    Some(parts)
}

/// PHP's `preg_quote` with no delimiter.
pub fn preg_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if ".\\+*?[^]$(){}=!<>|:-#/".contains(c) {
            out.push('\\');
        }
        if c == '\0' {
            out.push_str("\\000");
        } else {
            out.push(c);
        }
    }
    out
}
