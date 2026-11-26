//! phpat/phpat 0.12.5, the architecture-test PHPStan extension, ported natively. Its test classes are read without
//! running PHP (reader.rs): each test method's `PHPat::rule()` builder chain becomes the `Statement`s phpat's
//! `StatementBuilder` builds. `rules::phpat` checks them against the analysed classes as phpat's rules do.

mod reader;

use pcre2::bytes::Regex;

/// `parameters.phpat` and the classes phpstan.neon tags `phpat.test` (phpat's `Configuration` and `TestExtractor`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub tests: Vec<String>,
    pub ignore_doc_comments: bool,
    pub ignore_built_in_classes: bool,
    pub show_rule_names: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Constraint {
    Should,
    ShouldNot,
    CanOnly,
}

/// A rule's `assertionType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Depend,
    Extend,
    Implement,
    Include,
    Construct,
    ApplyAttribute,
    BeAbstract,
    BeFinal,
    BeReadonly,
    BeEnum,
    BeInvokable,
    BeInterface,
    BeNamed,
    HaveOnlyOnePublicMethod,
    HaveOnlyOnePublicMethodNamed,
    Exist,
}

impl Kind {
    /// `RuleValidator::RELATION_TYPES`.
    fn is_relation(self) -> bool {
        matches!(
            self,
            Kind::Depend
                | Kind::Extend
                | Kind::Implement
                | Kind::Include
                | Kind::Construct
                | Kind::ApplyAttribute
        )
    }
}

/// A string or regex selector argument: the text as written, and the compiled pattern when `$regex` is true.
#[derive(Clone, Debug)]
pub(crate) struct Pattern {
    pub text: String,
    pub regex: Option<Regex>,
}

impl Pattern {
    fn new(text: String, regex: bool) -> Result<Pattern, String> {
        let regex = regex.then(|| pun_phpstan::php::preg(&text)).transpose()?;
        Ok(Pattern { text, regex })
    }

    pub(crate) fn is_match(&self, subject: &str) -> bool {
        self.regex
            .as_ref()
            .is_some_and(|r| pun_phpstan::php::is_match(r, subject))
    }
}

/// phpat's selectors (`Selector::*()`), each with its `getName()`.
#[derive(Clone, Debug)]
pub(crate) enum Selector {
    All,
    IsException,
    IsThrowable,
    IsError,
    IsInterface,
    IsAbstract,
    IsFinal,
    IsTrait,
    IsReadonly,
    IsEnum,
    IsAttribute,
    IsStandardClass,
    Namespace(Pattern),
    Classname(Pattern),
    Filepath(Pattern),
    Implements(Pattern),
    Extends(Pattern),
    AppliesAttribute(Pattern),
    Includes(Pattern),
    AllOf(Vec<Selector>),
    AnyOf(Vec<Selector>),
    NoneOf(Vec<Selector>),
    AtLeastCountOf(i64, Vec<Selector>),
    AtMostCountOf(i64, Vec<Selector>),
    OneOf(Vec<Selector>),
}

impl Selector {
    pub(crate) fn name(&self) -> String {
        let and = |s: &[Selector]| {
            s.iter()
                .map(Selector::name)
                .collect::<Vec<_>>()
                .join(" and ")
        };
        match self {
            Selector::All => "-all classes-".into(),
            Selector::IsException => "-all exceptions-".into(),
            Selector::IsThrowable => "-all throwables-".into(),
            Selector::IsError => "-all errors-".into(),
            Selector::IsInterface => "-all interfaces-".into(),
            Selector::IsAbstract => "-abstract classes-".into(),
            Selector::IsFinal => "-all final classes-".into(),
            Selector::IsTrait => "-all traits-".into(),
            Selector::IsReadonly => "-all readonly classes-".into(),
            Selector::IsEnum => "-all enums-".into(),
            Selector::IsAttribute => "-attribute classes-".into(),
            Selector::IsStandardClass => "-standard classes-".into(),
            Selector::Namespace(p)
            | Selector::Classname(p)
            | Selector::Filepath(p)
            | Selector::Implements(p)
            | Selector::Extends(p)
            | Selector::AppliesAttribute(p)
            | Selector::Includes(p) => p.text.clone(),
            Selector::AllOf(s) => and(s),
            Selector::AnyOf(s) => (s.iter().map(Selector::name))
                .collect::<Vec<_>>()
                .join(" or "),
            Selector::NoneOf(s) => format!("none of: {}", and(s)),
            Selector::AtLeastCountOf(n, s) => format!("at least {n} of {}", and(s)),
            Selector::AtMostCountOf(n, s) => format!("at most {n} of {}", and(s)),
            Selector::OneOf(s) => format!("one of: [ {} ]", and(s)),
        }
    }
}

/// phpat's `Statement`: one subject of one rule.
#[derive(Clone, Debug)]
pub(crate) struct Statement {
    pub rule_name: String,
    pub kind: Kind,
    pub constraint: Constraint,
    pub subject: Selector,
    pub subject_excludes: Vec<Selector>,
    pub targets: Vec<Selector>,
    pub target_excludes: Vec<Selector>,
    pub tips: Vec<String>,
    /// `beNamed()`'s classname and `haveOnlyOnePublicMethodNamed()`'s name.
    pub name: Option<Pattern>,
    pub non_ignorable: bool,
}

/// The statements of every test, and the options they run with.
#[derive(Debug, Default)]
pub struct Rules {
    pub(crate) statements: Vec<Statement>,
    pub(crate) ignore_doc_comments: bool,
    pub(crate) ignore_built_in_classes: bool,
    pub(crate) show_rule_names: bool,
}

impl Rules {
    pub(crate) fn of(&self, kind: Kind) -> impl Iterator<Item = &Statement> {
        self.statements.iter().filter(move |s| s.kind == kind)
    }

    /// phpat's `prepareMessage()`.
    pub(crate) fn message(&self, rule_name: &str, message: String) -> String {
        if self.show_rule_names {
            format!("{rule_name}: {message}")
        } else {
            message
        }
    }
}

/// A rule as its builder leaves it (`RelationRule`).
#[derive(Clone, Debug, Default)]
pub(crate) struct Rule {
    pub name: String,
    pub subjects: Vec<Selector>,
    pub subject_excludes: Vec<Selector>,
    pub targets: Vec<Selector>,
    pub target_excludes: Vec<Selector>,
    pub constraint: Option<Constraint>,
    pub kind: Option<Kind>,
    pub tips: Vec<String>,
    pub name_param: Option<Pattern>,
    pub non_ignorable: bool,
}

/// `StatementBuilder::extractRuleName()`: the part after the class name, with `_x` camel-cased and anything but
/// letters, digits and dots dropped.
pub(crate) fn rule_name(full: &str) -> String {
    let name = full.split_once(':').map_or(full, |(_, n)| n);
    let mut camel = String::new();
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '_'
            && let Some(&n) = chars.peek()
            && n.is_ascii_alphabetic()
        {
            camel.push(n.to_ascii_uppercase());
            chars.next();
        } else {
            camel.push(c);
        }
    }
    let kept: String = camel
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.')
        .collect();
    let mut collapsed = String::new();
    for c in kept.chars() {
        if !(c == '.' && collapsed.ends_with('.')) {
            collapsed.push(c);
        }
    }
    collapsed.trim_matches('.').to_string()
}

impl Rules {
    /// Reads every test class in `config.tests` (phpat's `TestParser`), validates each rule (`RuleValidator`), and
    /// builds the statements. A test Pun can't read without running PHP, or a rule phpat would reject, is an error
    /// that names it.
    pub fn load(project: &crate::Project, config: &Config) -> Result<Rules, String> {
        let mut rules = Vec::new();
        for class in &config.tests {
            rules.extend(reader::read(project, class)?);
        }
        let mut statements = Vec::new();
        for rule in rules {
            if rule.subjects.is_empty() {
                return Err(format!("The PHPat rule {} has no subjects", rule.name));
            }
            // The builder picks the constraint before the assertion, so RuleValidator's "has no constraint" can't
            // happen.
            let (Some(kind), Some(constraint)) = (rule.kind, rule.constraint) else {
                return Err(format!("The PHPat rule {} has no assertion", rule.name));
            };
            if kind.is_relation() && rule.targets.is_empty() {
                return Err(format!("The PHPat rule {} has no targets", rule.name));
            }
            let rule_name = rule_name(&rule.name);
            for subject in rule.subjects {
                statements.push(Statement {
                    rule_name: rule_name.clone(),
                    kind,
                    constraint,
                    subject,
                    subject_excludes: rule.subject_excludes.clone(),
                    targets: rule.targets.clone(),
                    target_excludes: rule.target_excludes.clone(),
                    tips: rule.tips.clone(),
                    name: rule.name_param.clone(),
                    non_ignorable: rule.non_ignorable,
                });
            }
        }
        Ok(Rules {
            statements,
            ignore_doc_comments: config.ignore_doc_comments,
            ignore_built_in_classes: config.ignore_built_in_classes,
            show_rule_names: config.show_rule_names,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rule_names_follow_extract_rule_name() {
        // Values worked out by hand from StatementBuilder::extractRuleName().
        assert_eq!(super::rule_name("App\\T:testDomain"), "testDomain");
        assert_eq!(
            super::rule_name("App\\T:test_domain_rule"),
            "testDomainRule"
        );
        assert_eq!(
            super::rule_name("App\\T:rules:my rule.v2"),
            "rulesmyrule.v2"
        );
        assert_eq!(super::rule_name("App\\T:rules:0"), "rules0");
        assert_eq!(super::rule_name("App\\T:rules:..a..b."), "rules.a.b");
    }
}
