use crate::mapping;

const SECTIONS: [&str; 5] = [
    "phpstan",
    "larastan",
    "phpstan-strict-rules",
    "phpstan-deprecation-rules",
    "phpat",
];

pub(crate) fn entry(identifier: &str) -> Option<&'static toml::Value> {
    let sections = SECTIONS.map(|s| mapping::section(&["upstream", s]).table());
    if let Some(e) = sections.iter().find_map(|t| t.get(identifier)) {
        return Some(e);
    }
    // phpat names an error after its test method, whose name may keep dots (`phpat.rules.v2`)
    if identifier
        .strip_prefix("phpat.")
        .is_some_and(|n| !n.is_empty())
    {
        return mapping::section(&["upstream", "phpat"])
            .table()
            .get("phpat.%s");
    }
    sections.iter().find_map(|t| {
        t.iter().find_map(|(k, e)| {
            let parts: Vec<&str> = k.split("%s").collect();
            (parts.len() > 1 && glob(&parts, identifier)).then_some(e)
        })
    })
}

pub fn rule_of_identifier(identifier: &str) -> Option<&'static str> {
    entry(identifier)?.get("pun")?.as_str()
}

/// The identifiers the upstream rule classes whose Pun rule `on` accepts can report (each class's `identifiers` in
/// mapping.toml).
pub fn identifiers_of_rules(on: impl Fn(&str) -> bool) -> std::collections::HashSet<&'static str> {
    (SECTIONS.iter().chain(&["phpstan-classes"]))
        .flat_map(|s| mapping::section(&["upstream", s]).table().values())
        .filter(|e| (e.get("pun").and_then(toml::Value::as_str)).is_some_and(&on))
        .filter_map(|e| e.get("identifiers")?.as_array())
        .flatten()
        .filter_map(toml::Value::as_str)
        .collect()
}

fn glob(parts: &[&str], s: &str) -> bool {
    let Some(rest) = s.strip_prefix(parts[0]) else {
        return false;
    };
    if parts.len() == 1 {
        return rest.is_empty();
    }
    let word = |c: &u8| c.is_ascii_alphanumeric() || *c == b'_';
    let run = rest.bytes().take_while(word).count();
    (1..=run).any(|n| glob(&parts[1..], &rest[n..]))
}
