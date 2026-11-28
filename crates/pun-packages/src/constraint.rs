use crate::error::{Result, bail};
use crate::version::{ALIAS, MODIFIER, compare, group, normalize, re, stability_of};
use pcre2::bytes::{Captures, Regex};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
    Ne,
}

impl Op {
    fn is_strict(self) -> bool {
        matches!(self, Op::Lt | Op::Gt)
    }

    fn parse(op: &str) -> Op {
        match op {
            "<" => Op::Lt,
            "<=" => Op::Le,
            ">" => Op::Gt,
            ">=" => Op::Ge,
            "!=" | "<>" => Op::Ne,
            _ => Op::Eq,
        }
    }

    fn strict(self) -> &'static str {
        match self {
            Op::Lt | Op::Le => "<",
            Op::Gt | Op::Ge => ">",
            Op::Eq | Op::Ne => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Constraint {
    Any,
    Single(Op, String),
    Multi(Vec<Constraint>, bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound<'a> {
    pub version: Cow<'a, str>,
    pub inclusive: bool,
}

const INFINITY: &str = "9223372036854775807.0.0.0";

impl<'a> Bound<'a> {
    pub fn zero() -> Bound<'a> {
        Bound {
            version: Cow::Borrowed("0.0.0.0-dev"),
            inclusive: true,
        }
    }

    pub fn infinity() -> Bound<'a> {
        Bound {
            version: Cow::Borrowed(INFINITY),
            inclusive: false,
        }
    }

    pub fn is_zero(&self) -> bool {
        self.inclusive && self.version == "0.0.0.0-dev"
    }

    pub fn into_owned(self) -> Bound<'static> {
        Bound {
            version: Cow::Owned(self.version.into_owned()),
            inclusive: self.inclusive,
        }
    }

    pub fn beyond(&self, other: &Bound, above: bool) -> bool {
        if self == other {
            return false;
        }
        match compare(&self.version, &other.version) {
            0 => other.inclusive == above,
            c => c == if above { 1 } else { -1 },
        }
    }

    pub fn above(&self, other: &Bound) -> bool {
        self.beyond(other, true)
    }
}

fn version_compare(a: &str, b: &str, op: Op) -> bool {
    let c = compare(a, b);
    match op {
        Op::Eq => c == 0,
        Op::Ne => c != 0,
        Op::Lt => c < 0,
        Op::Le => c <= 0,
        Op::Gt => c > 0,
        Op::Ge => c >= 0,
    }
}

fn compare_versions(a: &str, b: &str, op: Op, branches: bool) -> bool {
    let (a_branch, b_branch) = (a.starts_with("dev-"), b.starts_with("dev-"));
    if op == Op::Ne && (a_branch || b_branch) {
        return a != b;
    }
    if a_branch && b_branch {
        return op == Op::Eq && a == b;
    }
    if !branches && (a_branch || b_branch) {
        return false;
    }
    version_compare(a, b, op)
}

pub fn match_specific_branches(op: Op, version: &str, p_op: Op, p_version: &str) -> bool {
    match_specific(op, version, p_op, p_version, true)
}

fn match_specific(op: Op, version: &str, p_op: Op, p_version: &str, branches: bool) -> bool {
    let (is_eq, is_ne) = (op == Op::Eq, op == Op::Ne);
    let (p_eq, p_ne) = (p_op == Op::Eq, p_op == Op::Ne);
    if is_ne || p_ne {
        if is_ne && !p_ne && !p_eq && p_version.starts_with("dev-") {
            return false;
        }
        if p_ne && !is_ne && !is_eq && version.starts_with("dev-") {
            return false;
        }
        if !is_eq && !p_eq {
            return true;
        }
        return compare_versions(p_version, version, Op::Ne, branches);
    }
    if !is_eq && op.strict() == p_op.strict() {
        return !(version.starts_with("dev-") || p_version.starts_with("dev-"));
    }
    let (v1, v2, cmp) = if is_eq {
        (version, p_version, p_op)
    } else {
        (p_version, version, op)
    };
    if compare_versions(v1, v2, cmp, branches) {
        return !(p_op.is_strict()
            && !op.is_strict()
            && version_compare(p_version, version, Op::Eq));
    }
    false
}

impl Constraint {
    pub fn exact(version: &str) -> Constraint {
        Constraint::Single(Op::Eq, version.to_string())
    }

    pub fn matches(&self, provider: &Constraint) -> bool {
        match self {
            Constraint::Any => true,
            Constraint::Single(op, version) => match provider {
                Constraint::Single(p_op, p_version) => {
                    match_specific(*op, version, *p_op, p_version, false)
                }
                _ => provider.matches(self),
            },
            Constraint::Multi(items, false) => items.iter().any(|c| provider.matches(c)),
            Constraint::Multi(items, true) => {
                if matches!(provider, Constraint::Multi(_, false)) {
                    return provider.matches(self);
                }
                items.iter().all(|c| provider.matches(c))
            }
        }
    }

    pub fn lower_bound(&self) -> Bound<'_> {
        self.bounds().0
    }

    pub fn upper_bound(&self) -> Bound<'_> {
        self.bounds().1
    }

    pub fn bounds(&self) -> (Bound<'_>, Bound<'_>) {
        match self {
            Constraint::Any => (Bound::zero(), Bound::infinity()),
            Constraint::Single(_, version) if version.starts_with("dev-") => {
                (Bound::zero(), Bound::infinity())
            }
            Constraint::Single(op, version) => {
                let at = |inclusive| Bound {
                    version: Cow::Borrowed(version.as_str()),
                    inclusive,
                };
                match op {
                    Op::Eq => (at(true), at(true)),
                    Op::Lt => (Bound::zero(), at(false)),
                    Op::Le => (Bound::zero(), at(true)),
                    Op::Gt => (at(false), Bound::infinity()),
                    Op::Ge => (at(true), Bound::infinity()),
                    Op::Ne => (Bound::zero(), Bound::infinity()),
                }
            }
            Constraint::Multi(items, conjunctive) => {
                let mut bounds: Option<(Bound, Bound)> = None;
                for c in items {
                    let (low, high) = c.bounds();
                    let Some((lower, upper)) = bounds.as_mut() else {
                        bounds = Some((low, high));
                        continue;
                    };
                    if low.beyond(lower, *conjunctive) {
                        *lower = low;
                    }
                    match compare(&high.version, &upper.version) {
                        0 if high.inclusive != *conjunctive => *upper = high,
                        c if c == if *conjunctive { -1 } else { 1 } => *upper = high,
                        _ => {}
                    }
                }
                bounds.expect("a multi-constraint has two or more members")
            }
        }
    }
}

pub(crate) static OR: LazyLock<Regex> = LazyLock::new(|| re(r"\s*\|\|?\s*"));
pub(crate) static AND: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?<!^|as|[=>< ,]) *(?<!-)[, ](?!-) *(?!,|as|$)"));
static STABILITY_FLAG: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^([^,\s]*?)@(stable|RC|beta|alpha|dev)$"));
static REFERENCE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^(dev-[^,\s@]+?|[^,\s@]+?\.x-dev)#.+$"));
static ANY: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^(v)?[xX*](\.[xX*])*$"));
static TILDE: LazyLock<Regex> = LazyLock::new(|| re(&format!("(?i)^~>?{}$", version_regex())));
static CARET: LazyLock<Regex> = LazyLock::new(|| re(&format!("(?i)^\\^{}($)", version_regex())));
static WILDCARD: LazyLock<Regex> =
    LazyLock::new(|| re(r"^v?(\d++)(?:\.(\d++))?(?:\.(\d++))?(?:\.[xX*])++$"));
static HYPHEN: LazyLock<Regex> = LazyLock::new(|| {
    let v = version_regex();
    re(&format!("(?i)^(?P<from>{v}) +- +(?P<to>{v})($)"))
});
static OPERATOR: LazyLock<Regex> = LazyLock::new(|| re(r"^(<>|!=|>=?|<=?|==?)?\s*(.*)"));
static MODIFIER_END: LazyLock<Regex> = LazyLock::new(|| re(&format!("(?i)-{MODIFIER}$")));
static DEV_NAME: LazyLock<Regex> = LazyLock::new(|| re(r"^[0-9a-zA-Z-./]+$"));

fn version_regex() -> String {
    format!(
        r"v?(\d++)(?:\.(\d++))?(?:\.(\d++))?(?:\.(\d++))?(?:{MODIFIER}|\.([xX*][.-]?dev))(?:\+[^\s]+)?"
    )
}

pub(crate) fn split<'a>(re: &Regex, s: &'a str) -> Vec<&'a str> {
    let mut out = vec![];
    let mut start = 0;
    for m in re.find_iter(s.as_bytes()).flatten() {
        out.push(&s[start..m.start()]);
        start = m.end();
    }
    out.push(&s[start..]);
    out
}

pub fn parse(constraints: &str) -> Result<Constraint> {
    thread_local! {
        static PARSED: RefCell<HashMap<String, Constraint>> = RefCell::new(HashMap::new());
    }
    if let Some(hit) = PARSED.with_borrow(|p| p.get(constraints).cloned()) {
        return Ok(hit);
    }
    let parsed = parse_uncached(constraints)?;
    PARSED.with_borrow_mut(|p| p.insert(constraints.to_string(), parsed.clone()));
    Ok(parsed)
}

fn parse_uncached(constraints: &str) -> Result<Constraint> {
    let mut groups = vec![];
    for alternative in split(&OR, constraints.trim()) {
        let mut items = vec![];
        for part in split(&AND, alternative) {
            items.extend(parse_one(part)?);
        }
        groups.push(match items.len() {
            1 => items.pop().expect("one item"),
            _ => Constraint::Multi(items, true),
        });
    }
    Ok(match groups.len() {
        1 => groups.pop().expect("one group"),
        _ => Constraint::Multi(groups, false),
    })
}

pub fn lower_bound(constraints: &str) -> Result<Bound<'static>> {
    Ok(parse(constraints)?.lower_bound().into_owned())
}

fn bump(parts: [&str; 4], position: usize, increment: u128) -> String {
    let mut out: Vec<String> = parts.iter().map(std::string::ToString::to_string).collect();
    for i in (1..=4).rev() {
        if i > position {
            out[i - 1] = "0".into();
        } else if i == position && increment > 0 {
            let n = out[i - 1].parse::<u128>().unwrap_or(0);
            out[i - 1] = (n + increment).to_string();
        }
    }
    out.join(".")
}

fn digits<'a>(m: &'a Captures, first: usize) -> [&'a str; 4] {
    [
        group(m, first),
        group(m, first + 1),
        group(m, first + 2),
        group(m, first + 3),
    ]
}

fn empty(s: &str) -> bool {
    s.is_empty() || s == "0"
}

#[expect(
    clippy::too_many_lines,
    reason = "long; to be split (code audit: functions over 100 lines)"
)]
fn parse_one(constraint: &str) -> Result<Vec<Constraint>> {
    let ge = |v: String| Constraint::Single(Op::Ge, v);
    let lt = |v: String| Constraint::Single(Op::Lt, v);
    let mut c = constraint.to_string();
    if let Some(m) = ALIAS.captures(c.as_bytes())? {
        c = group(&m, 1).to_string();
    }
    let mut flag = String::new();
    if let Some(m) = STABILITY_FLAG.captures(c.as_bytes())? {
        if group(&m, 2) != "stable" {
            flag = group(&m, 2).to_string();
        }
        c = match group(&m, 1) {
            "" => "*".to_string(),
            v => v.to_string(),
        };
    }
    if let Some(m) = REFERENCE.captures(c.as_bytes())? {
        c = group(&m, 1).to_string();
    }
    if let Some(m) = ANY.captures(c.as_bytes())? {
        if !group(&m, 1).is_empty() || !group(&m, 2).is_empty() {
            return Ok(vec![ge("0.0.0.0-dev".into())]);
        }
        return Ok(vec![Constraint::Any]);
    }
    let dev_suffix = |m: &Captures, a: usize, b: usize, d: usize| {
        if empty(group(m, a)) && empty(group(m, b)) && empty(group(m, d)) {
            "-dev"
        } else {
            ""
        }
    };
    if let Some(m) = TILDE.captures(c.as_bytes())? {
        if c.starts_with("~>") {
            bail!(
                "Could not parse version constraint {c}: Invalid operator \"~>\", you probably meant to use the \"~\" operator"
            );
        }
        let parts = digits(&m, 1);
        let mut position = parts
            .iter()
            .rposition(|p| !p.is_empty())
            .map_or(1, |p| p + 1);
        if !empty(group(&m, 8)) {
            position += 1;
        }
        let low = normalize(&format!("{}{}", &c[1..], dev_suffix(&m, 5, 7, 8)))?;
        let high = bump(parts, (position - 1).max(1), 1);
        return Ok(vec![ge(low), lt(format!("{high}-dev"))]);
    }
    if let Some(m) = CARET.captures(c.as_bytes())? {
        let parts = digits(&m, 1);
        let position = if parts[0] != "0" || parts[1].is_empty() {
            1
        } else if parts[1] != "0" || parts[2].is_empty() {
            2
        } else {
            3
        };
        let low = normalize(&format!("{}{}", &c[1..], dev_suffix(&m, 5, 7, 8)))?;
        let high = bump(parts, position, 1);
        return Ok(vec![ge(low), lt(format!("{high}-dev"))]);
    }
    if let Some(m) = WILDCARD.captures(c.as_bytes())? {
        let parts = [group(&m, 1), group(&m, 2), group(&m, 3), ""];
        let position = parts
            .iter()
            .rposition(|p| !p.is_empty())
            .map_or(1, |p| p + 1);
        let low = format!("{}-dev", bump(parts, position, 0));
        let high = format!("{}-dev", bump(parts, position, 1));
        if low == "0.0.0.0-dev" {
            return Ok(vec![lt(high)]);
        }
        return Ok(vec![ge(low), lt(high)]);
    }
    if let Some(m) = HYPHEN.captures(c.as_bytes())? {
        let from = group(&m, 1);
        let to = group(&m, 10);
        let low = format!("{}{}", normalize(from)?, dev_suffix(&m, 6, 8, 9));
        let upper = if (!group(&m, 12).is_empty() && !group(&m, 13).is_empty())
            || !empty(group(&m, 15))
            || !empty(group(&m, 17))
            || !empty(group(&m, 18))
        {
            Constraint::Single(Op::Le, normalize(to)?)
        } else {
            normalize(to)?;
            let parts = digits(&m, 11);
            let position = if parts[1].is_empty() { 1 } else { 2 };
            lt(format!("{}-dev", bump(parts, position, 1)))
        };
        return Ok(vec![ge(low), upper]);
    }
    let m = OPERATOR
        .captures(c.as_bytes())?
        .expect("matches any string");
    let (op, rest) = (group(&m, 1), group(&m, 2));
    let mut version = match normalize(rest) {
        Ok(v) => v,
        Err(_) if rest.ends_with("-dev") && DEV_NAME.is_match(rest.as_bytes())? => {
            normalize(&format!("dev-{}", &rest[..rest.len() - 4]))?
        }
        Err(e) => bail!("Could not parse version constraint {c}: {e}"),
    };
    let op = Op::parse(op);
    if op != Op::Eq && !flag.is_empty() && stability_of(&version) == "stable" {
        version = format!("{version}-{flag}");
    } else if matches!(op, Op::Lt | Op::Ge)
        && !MODIFIER_END.is_match(rest.as_bytes())?
        && !rest.starts_with("dev-")
    {
        version.push_str("-dev");
    }
    Ok(vec![Constraint::Single(op, version)])
}

/// `(string) $constraint` of what composer/semver's `VersionParser::parseConstraints` builds from the text `constraint`
/// was parsed from: `>= 1.0.0.0-dev` for one, `[a b]` for all of several, `[a || b]` for any of them, `*` for any version.
/// Its alternatives are joined as `MultiConstraint::create` joins them: a range that ends where the next starts becomes
/// one (`^1.0 || ^2.0` is `[>= 1.0.0.0-dev < 3.0.0.0-dev]`).
pub fn composer_string(constraint: &Constraint) -> String {
    match constraint {
        Constraint::Multi(alternatives, false) => {
            let mut joined = join_ranges(alternatives);
            if joined.len() == 1 {
                joined.pop().expect("one")
            } else {
                format!("[{}]", joined.join(" || "))
            }
        }
        other => text(other),
    }
}

/// `Constraint::$transOpInt`: an operator as composer/semver writes it.
fn op_str(op: Op) -> &'static str {
    match op {
        Op::Eq => "==",
        Op::Lt => "<",
        Op::Le => "<=",
        Op::Gt => ">",
        Op::Ge => ">=",
        Op::Ne => "!=",
    }
}

/// `__toString` of a constraint as it stands.
fn text(constraint: &Constraint) -> String {
    match constraint {
        Constraint::Any => "*".into(),
        Constraint::Single(op, version) => format!("{} {version}", op_str(*op)),
        Constraint::Multi(items, conjunctive) => {
            let parts: Vec<String> = items.iter().map(text).collect();
            format!("[{}]", parts.join(if *conjunctive { " " } else { " || " }))
        }
    }
}

/// `MultiConstraint::optimizeConstraints` for alternatives, on their strings: `[>= a < b]` then `[>= b < c]` is
/// `[>= a < c]`. The alternatives come back as text.
fn join_ranges(alternatives: &[Constraint]) -> Vec<String> {
    // A two-sided range: its lower and upper constraint as text.
    let range = |c: &Constraint| match c {
        Constraint::Multi(items, true) if items.len() == 2 => {
            let (low, high) = (text(&items[0]), text(&items[1]));
            (low.starts_with(">=") && high.starts_with('<')).then_some((low, high))
        }
        _ => None,
    };
    let mut out = vec![];
    let mut left: Option<(String, String)> = None;
    let mut left_text = text(&alternatives[0]);
    if let Some(r) = range(&alternatives[0]) {
        left = Some(r);
    }
    for right in &alternatives[1..] {
        let right_range = range(right);
        match (&left, &right_range) {
            (Some((low, high)), Some((right_low, right_high)))
                if high.get(2..) == right_low.get(3..) =>
            {
                let merged = (low.clone(), right_high.clone());
                left_text = format!("[{} {}]", merged.0, merged.1);
                left = Some(merged);
            }
            _ => {
                out.push(std::mem::replace(&mut left_text, text(right)));
                left = right_range;
            }
        }
    }
    out.push(left_text);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERSIONS: [&str; 20] = [
        "0.0.1.0",
        "0.1.0.0",
        "0.1.5.0",
        "0.2.0.0",
        "1.0.0.0-dev",
        "1.0.0.0-alpha1",
        "1.0.0.0-beta2",
        "1.0.0.0-RC1",
        "1.0.0.0",
        "1.0.1.0",
        "1.2.0.0",
        "1.2.3.0",
        "1.9999999.9999999.9999999-dev",
        "2.0.0.0-dev",
        "2.0.0.0",
        "2.5.0.0",
        "3.0.0.0",
        "9999999-dev",
        "dev-main",
        "dev-feature/x",
    ];

    #[test]
    fn matching_agrees_with_composer() {
        for (c, want) in [
            ("^1.0", "00001111111110000000"),
            ("^0.1", "01100000000000000000"),
            ("^0.0.1", "10000000000000000000"),
            ("~1.2", "00000000001110000000"),
            ("~1.2.0", "00000000001100000000"),
            ("~1", "00001111111110000000"),
            ("1.0.*", "00001111110000000000"),
            ("1.*", "00001111111110000000"),
            ("0.*", "11110000000000000000"),
            ("*", "11111111111111111111"),
            ("v*", "11111111111111111100"),
            ("1.0 - 2.0", "00001111111111100000"),
            ("1.0 - 2", "00001111111111110000"),
            ("1.0 - 2.0.0", "00001111111111100000"),
            ("1.2 - 2.5-beta", "00000000001111100000"),
            (">=1.0 <2.0", "00001111111110000000"),
            (">=1.0,<2.0", "00001111111110000000"),
            (">1.0", "00000000011111111100"),
            (">=1.0", "00001111111111111100"),
            ("<1.0", "11110000000000000000"),
            ("<=1.0", "11111111100000000000"),
            ("!=1.0", "11111111011111111111"),
            ("<>1.0.1", "11111111101111111111"),
            ("1.0", "00000000100000000000"),
            ("==1.2.3", "00000000000100000000"),
            ("^1.0 || ^3.0", "00001111111110001000"),
            ("^1.0 | ~2.5", "00001111111110010000"),
            ("dev-main", "00000000000000000010"),
            ("dev-main as 1.0", "00000000000000000010"),
            ("1.x-dev", "00000000000010000000"),
            ("1.0.x-dev#abc", "00000000000000000000"),
            ("dev-main#abc", "00000000000000000010"),
            ("feature/x-dev", "00000000000000000001"),
            ("@dev", "11111111111111111111"),
            ("^1.0@beta", "00001111111110000000"),
            (">=1.0@beta", "00000011111111111100"),
            ("<2.0@RC", "11111111111111000000"),
            (">=1.0@stable", "00001111111111111100"),
            ("1.0-beta2", "00000010000000000000"),
            (">=1.0-beta2", "00000011111111111100"),
            ("~1.0-beta", "00000011111110000000"),
            ("^1.2.3-alpha", "00000000000110000000"),
            ("~1.x-dev", "00000000000010000000"),
            ("2.x-dev", "00000000000000000000"),
            (">=1.0 <1.1 || >=1.1 <2.0", "00001111111110000000"),
            ("!=dev-main", "11111111111111111101"),
        ] {
            let parsed = parse(c).unwrap();
            let got: String = VERSIONS
                .iter()
                .map(|v| {
                    if parsed.matches(&Constraint::exact(v)) {
                        '1'
                    } else {
                        '0'
                    }
                })
                .collect();
            assert_eq!(got, want, "{c}");
        }
    }

    #[test]
    fn constraints_intersect_as_composer() {
        for (a, b, want) in [
            ("^1.0", "^1.5", true),
            ("^1.0", "^2.0", false),
            ("<1.0", "<2.0", true),
            (">1.0", ">=2.0", true),
            ("<=1.0", ">1.0", false),
            ("<=1.0", ">=1.0", true),
            ("<1.0", ">=1.0", false),
            ("!=1.0", "1.0", false),
            ("!=1.0", "!=2.0", true),
            ("!=1.0", ">1.0", true),
            ("!=dev-main", "<1.0", true),
            ("!=dev-main", "dev-main", false),
            ("<1.0", "!=dev-main", true),
            ("dev-main", "<1.0", false),
            ("dev-a", "dev-b", false),
            ("<dev-a", "<dev-b", false),
            ("!=1.0", ">dev-a", false),
            (">dev-a", "!=1.0", false),
            ("^1.0 || ^3.0", "^3.1", true),
            ("^3.1", "^1.0 || ^2.0", false),
            ("^1.0", ">=1.5 <1.6 || >=2", true),
            ("*", "dev-main", true),
            ("dev-main", "*", true),
        ] {
            let (pa, pb) = (parse(a).unwrap(), parse(b).unwrap());
            assert_eq!(pa.matches(&pb), want, "{a} / {b}");
        }
    }

    #[test]
    fn bounds_match_composer() {
        let b = |v: &'static str, inclusive| Bound {
            version: v.into(),
            inclusive,
        };
        for (c, low, high) in [
            ("^8.4", b("8.4.0.0-dev", true), b("9.0.0.0-dev", false)),
            ("^8.4.1", b("8.4.1.0-dev", true), b("9.0.0.0-dev", false)),
            ("~8.1.2", b("8.1.2.0-dev", true), b("8.2.0.0-dev", false)),
            (">=8.1", b("8.1.0.0-dev", true), b(INFINITY, false)),
            (">=8.1-beta1", b("8.1.0.0-beta1", true), b(INFINITY, false)),
            (">8.1", b("8.1.0.0", false), b(INFINITY, false)),
            ("8.2.*", b("8.2.0.0-dev", true), b("8.3.0.0-dev", false)),
            ("8.2.1", b("8.2.1.0", true), b("8.2.1.0", true)),
            ("<=8.2", b("0.0.0.0-dev", true), b("8.2.0.0", true)),
            ("!=8.2", b("0.0.0.0-dev", true), b(INFINITY, false)),
            (
                "^7.2 || ^8.0",
                b("7.2.0.0-dev", true),
                b("9.0.0.0-dev", false),
            ),
            (
                "^8.0 || ^7.2",
                b("7.2.0.0-dev", true),
                b("9.0.0.0-dev", false),
            ),
            ("<=8.0 || <8.0", b("0.0.0.0-dev", true), b("8.0.0.0", true)),
            ("<8.0 || <=8.0", b("0.0.0.0-dev", true), b("8.0.0.0", true)),
            (
                ">=8 <=9, <9",
                b("8.0.0.0-dev", true),
                b("9.0.0.0-dev", false),
            ),
            (
                ">=7.4 <9 , >=8.1",
                b("8.1.0.0-dev", true),
                b("9.0.0.0-dev", false),
            ),
            (">8.1 >=8.1", b("8.1.0.0", false), b(INFINITY, false)),
            ("8.1 - 8.3", b("8.1.0.0-dev", true), b("8.4.0.0-dev", false)),
            ("^8.1@beta", b("8.1.0.0-dev", true), b("9.0.0.0-dev", false)),
            (">8.1@beta", b("8.1.0.0-beta", false), b(INFINITY, false)),
            ("*", b("0.0.0.0-dev", true), b(INFINITY, false)),
            ("<9", b("0.0.0.0-dev", true), b("9.0.0.0-dev", false)),
            ("dev-main", b("0.0.0.0-dev", true), b(INFINITY, false)),
            (
                "5.x-dev",
                b("5.9999999.9999999.9999999-dev", true),
                b("5.9999999.9999999.9999999-dev", true),
            ),
            ("1.0 as 2.0", b("1.0.0.0", true), b("1.0.0.0", true)),
            (
                "~1.2-beta",
                b("1.2.0.0-beta", true),
                b("2.0.0.0-dev", false),
            ),
            (
                "1.2 - 1.3-beta",
                b("1.2.0.0-dev", true),
                b("1.3.0.0-beta", true),
            ),
            (
                ">=feature/x-dev",
                b("0.0.0.0-dev", true),
                b(INFINITY, false),
            ),
            (
                ">=my.feature-dev",
                b("0.0.0.0-dev", true),
                b(INFINITY, false),
            ),
            ("0.*", b("0.0.0.0-dev", true), b("1.0.0.0-dev", false)),
            ("~1.2.3.4", b("1.2.3.4-dev", true), b("1.2.4.0-dev", false)),
        ] {
            let parsed = parse(c).unwrap();
            assert_eq!(
                (parsed.lower_bound(), parsed.upper_bound()),
                (low, high),
                "{c}"
            );
        }
        assert_eq!(lower_bound("^8.4").unwrap(), b("8.4.0.0-dev", true));
        assert!(Bound::zero().is_zero());
        assert!(!b("1.0", false).above(&b("1.0", false)));
        assert!(b("1.0", false).above(&b("1.0", true)));
    }

    #[test]
    fn errors_match_composer() {
        for (c, message) in [
            (
                "~>1.2",
                "Could not parse version constraint ~>1.2: Invalid operator \"~>\", you probably meant to use the \"~\" operator",
            ),
            (
                ">=foo",
                "Could not parse version constraint >=foo: Invalid version string \"foo\"",
            ),
            (
                "1.0 - foo",
                "Could not parse version constraint 1.0 - foo: Invalid version string \"1.0 - foo\"",
            ),
        ] {
            assert_eq!(parse(c).unwrap_err().to_string(), message, "{c}");
        }
    }

    /// composer/semver's own `(string) parseConstraints(...)` (tests/fixtures/constraint-strings.json, from
    /// scripts/export-constraint-strings.php).
    #[test]
    fn strings_as_composer_writes_them() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/constraint-strings.json"))
                .unwrap();
        let wrong: Vec<String> = (fixture.as_object().unwrap().iter())
            .filter_map(|(given, want)| {
                let got = composer_string(&parse(given).unwrap());
                (got != want.as_str().unwrap())
                    .then(|| format!("{given:?}: Composer {want}, Pun {got}"))
            })
            .collect();
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
