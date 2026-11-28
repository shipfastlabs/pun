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

/// PHP's `mb_strwidth` for UTF-8: each character counts 1, but the wide ones (East Asian and emoji), which count 2.
pub fn mb_strwidth(s: &str) -> usize {
    s.chars()
        .map(|c| {
            let c = u32::from(c);
            let wide = WIDE
                .binary_search_by(|&(from, to)| {
                    if to < c {
                        std::cmp::Ordering::Less
                    } else if from > c {
                        std::cmp::Ordering::Greater
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .is_ok();
            if wide { 2 } else { 1 }
        })
        .sum()
}

/// The characters `mb_strwidth` counts 2 wide on PHP 8.5.4 (`scripts/export-mb-strwidth.php`).
const WIDE: [(u32, u32); 123] = [
    (0x1100, 0x115F),
    (0x231A, 0x231B),
    (0x2329, 0x232A),
    (0x23E9, 0x23EC),
    (0x23F0, 0x23F0),
    (0x23F3, 0x23F3),
    (0x25FD, 0x25FE),
    (0x2614, 0x2615),
    (0x2630, 0x2637),
    (0x2648, 0x2653),
    (0x267F, 0x267F),
    (0x268A, 0x268F),
    (0x2693, 0x2693),
    (0x26A1, 0x26A1),
    (0x26AA, 0x26AB),
    (0x26BD, 0x26BE),
    (0x26C4, 0x26C5),
    (0x26CE, 0x26CE),
    (0x26D4, 0x26D4),
    (0x26EA, 0x26EA),
    (0x26F2, 0x26F3),
    (0x26F5, 0x26F5),
    (0x26FA, 0x26FA),
    (0x26FD, 0x26FD),
    (0x2705, 0x2705),
    (0x270A, 0x270B),
    (0x2728, 0x2728),
    (0x274C, 0x274C),
    (0x274E, 0x274E),
    (0x2753, 0x2755),
    (0x2757, 0x2757),
    (0x2795, 0x2797),
    (0x27B0, 0x27B0),
    (0x27BF, 0x27BF),
    (0x2B1B, 0x2B1C),
    (0x2B50, 0x2B50),
    (0x2B55, 0x2B55),
    (0x2E80, 0x2E99),
    (0x2E9B, 0x2EF3),
    (0x2F00, 0x2FD5),
    (0x2FF0, 0x303E),
    (0x3041, 0x3096),
    (0x3099, 0x30FF),
    (0x3105, 0x312F),
    (0x3131, 0x318E),
    (0x3190, 0x31E5),
    (0x31EF, 0x321E),
    (0x3220, 0x3247),
    (0x3250, 0xA48C),
    (0xA490, 0xA4C6),
    (0xA960, 0xA97C),
    (0xAC00, 0xD7A3),
    (0xF900, 0xFAFF),
    (0xFE10, 0xFE19),
    (0xFE30, 0xFE52),
    (0xFE54, 0xFE66),
    (0xFE68, 0xFE6B),
    (0xFF01, 0xFF60),
    (0xFFE0, 0xFFE6),
    (0x16FE0, 0x16FE4),
    (0x16FF0, 0x16FF6),
    (0x17000, 0x18CD5),
    (0x18CFF, 0x18D1E),
    (0x18D80, 0x18DF2),
    (0x1AFF0, 0x1AFF3),
    (0x1AFF5, 0x1AFFB),
    (0x1AFFD, 0x1AFFE),
    (0x1B000, 0x1B122),
    (0x1B132, 0x1B132),
    (0x1B150, 0x1B152),
    (0x1B155, 0x1B155),
    (0x1B164, 0x1B167),
    (0x1B170, 0x1B2FB),
    (0x1D300, 0x1D356),
    (0x1D360, 0x1D376),
    (0x1F004, 0x1F004),
    (0x1F0CF, 0x1F0CF),
    (0x1F18E, 0x1F18E),
    (0x1F191, 0x1F19A),
    (0x1F200, 0x1F202),
    (0x1F210, 0x1F23B),
    (0x1F240, 0x1F248),
    (0x1F250, 0x1F251),
    (0x1F260, 0x1F265),
    (0x1F300, 0x1F320),
    (0x1F32D, 0x1F335),
    (0x1F337, 0x1F37C),
    (0x1F37E, 0x1F393),
    (0x1F3A0, 0x1F3CA),
    (0x1F3CF, 0x1F3D3),
    (0x1F3E0, 0x1F3F0),
    (0x1F3F4, 0x1F3F4),
    (0x1F3F8, 0x1F43E),
    (0x1F440, 0x1F440),
    (0x1F442, 0x1F4FC),
    (0x1F4FF, 0x1F53D),
    (0x1F54B, 0x1F54E),
    (0x1F550, 0x1F567),
    (0x1F57A, 0x1F57A),
    (0x1F595, 0x1F596),
    (0x1F5A4, 0x1F5A4),
    (0x1F5FB, 0x1F64F),
    (0x1F680, 0x1F6C5),
    (0x1F6CC, 0x1F6CC),
    (0x1F6D0, 0x1F6D2),
    (0x1F6D5, 0x1F6D8),
    (0x1F6DC, 0x1F6DF),
    (0x1F6EB, 0x1F6EC),
    (0x1F6F4, 0x1F6FC),
    (0x1F7E0, 0x1F7EB),
    (0x1F7F0, 0x1F7F0),
    (0x1F90C, 0x1F93A),
    (0x1F93C, 0x1F945),
    (0x1F947, 0x1F9FF),
    (0x1FA70, 0x1FA7C),
    (0x1FA80, 0x1FA8A),
    (0x1FA8E, 0x1FAC6),
    (0x1FAC8, 0x1FAC8),
    (0x1FACD, 0x1FADC),
    (0x1FADF, 0x1FAEA),
    (0x1FAEF, 0x1FAF8),
    (0x20000, 0x2FFFD),
    (0x30000, 0x3FFFD),
];

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

    /// Widths from PHP 8.5's `mb_strwidth`: a CJK character and an emoji count 2, an accented letter, a combining
    /// accent and a halfwidth katakana 1.
    #[test]
    fn mb_strwidth_matches_php() {
        for (text, width) in [
            ("日本", 4),
            ("é", 1),
            ("🙂", 2),
            ("a\u{301}", 2),
            ("ｶ", 1),
            ("Licence-日本", 12),
            ("", 0),
        ] {
            assert_eq!(mb_strwidth(text), width, "{text}");
        }
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
