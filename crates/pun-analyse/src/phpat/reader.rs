//! Reads phpat test classes without running PHP: the test methods `TestParser` calls (public, with `#[TestRule]` or a
//! name starting with `test`), and the `PHPat::rule()` builder chains they return or yield. Anything else, such as a
//! value only PHP could compute or a builder method phpat 0.12.5 doesn't have, is an error naming the code.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mago_allocator::LocalArena;
use mago_database::file::{File, FileType};
use mago_names::ResolvedNames;
use mago_span::{HasSpan, Span};
use mago_syntax::cst::*;

use super::{Constraint, Kind, Pattern, Rule, Selector};
use crate::Project;

const PHPAT: &str = "phpat\\test\\phpat";
const SELECTOR: [&str; 2] = [
    "phpat\\selector\\selector",
    "phpat\\selector\\selectorprimitive",
];
const TEST_RULE: &str = "phpat\\test\\attributes\\testrule";

/// Where a builder is in phpat's step classes (`PHPat\Test\Builder\*Step`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Subject,
    SubjectExcludeOrConstraint,
    Constraint,
    Should,
    ShouldNot,
    CanOnly,
    Target,
    TargetExcludeOrTipOrBuild,
    TipOrBuild,
    Build,
}

impl Step {
    /// The step class and the classes it extends, whose methods it has.
    fn with_parents(self) -> &'static [Step] {
        match self {
            Step::Subject => &[Step::Subject],
            Step::SubjectExcludeOrConstraint => {
                &[Step::SubjectExcludeOrConstraint, Step::Constraint]
            }
            Step::Constraint => &[Step::Constraint],
            Step::Should => &[Step::Should],
            Step::ShouldNot => &[Step::ShouldNot],
            Step::CanOnly => &[Step::CanOnly],
            Step::Target => &[Step::Target],
            Step::TargetExcludeOrTipOrBuild => &[
                Step::TargetExcludeOrTipOrBuild,
                Step::TipOrBuild,
                Step::Build,
            ],
            Step::TipOrBuild => &[Step::TipOrBuild, Step::Build],
            Step::Build => &[Step::Build],
        }
    }
}

/// `ConstraintStep`'s deprecated aliases: the constraint method and the method it then calls.
const ALIASES: [(&str, &str, &str); 25] = [
    ("shouldbenamed", "should", "benamed"),
    ("shouldbeabstract", "should", "beabstract"),
    ("shouldnotbeabstract", "shouldnot", "beabstract"),
    ("shouldbereadonly", "should", "bereadonly"),
    ("shouldnotbereadonly", "shouldnot", "bereadonly"),
    ("shouldbefinal", "should", "befinal"),
    ("shouldnotbefinal", "shouldnot", "befinal"),
    ("shouldbeenum", "should", "beenum"),
    ("shouldnotbeenum", "shouldnot", "beenum"),
    ("shouldbeinvokable", "should", "beinvokable"),
    ("shouldnotbeinvokable", "shouldnot", "beinvokable"),
    ("shouldnotdependon", "shouldnot", "dependon"),
    ("canonlydependon", "canonly", "dependon"),
    ("shouldnotconstruct", "shouldnot", "construct"),
    ("shouldnotextend", "shouldnot", "extend"),
    ("shouldnotimplement", "shouldnot", "implement"),
    ("shouldimplement", "should", "implement"),
    ("shouldnotinclude", "shouldnot", "include"),
    ("shouldinclude", "should", "include"),
    ("shouldextend", "should", "extend"),
    (
        "shouldhaveonlyonepublicmethod",
        "should",
        "haveonlyonepublicmethod",
    ),
    (
        "shouldhaveonlyonepublicmethodnamed",
        "should",
        "haveonlyonepublicmethodnamed",
    ),
    ("shouldapplyattribute", "should", "applyattribute"),
    ("shouldbeinterface", "should", "beinterface"),
    ("shouldnotexist", "shouldnot", "exist"),
];

#[derive(Clone, Debug)]
enum Key {
    Int(i64),
    Str(String),
}

impl Key {
    fn text(&self) -> String {
        match self {
            Key::Int(i) => i.to_string(),
            Key::Str(s) => s.clone(),
        }
    }
}

#[derive(Clone, Debug)]
enum V {
    Str(String),
    Int(i64),
    Bool(bool),
    Null,
    Array(Vec<(Key, V)>),
    Selector(Selector),
    Builder(Rc<RefCell<Rule>>, Step),
}

impl V {
    fn kind(&self) -> &'static str {
        match self {
            V::Str(_) => "a string",
            V::Int(_) => "an int",
            V::Bool(_) => "a bool",
            V::Null => "null",
            V::Array(_) => "an array",
            V::Selector(_) => "a selector",
            V::Builder(..) => "a rule builder",
        }
    }
}

/// The next automatic key of a PHP array or generator: one above the largest int key so far.
fn next_key(entries: &[(Key, V)]) -> i64 {
    (entries.iter())
        .filter_map(|(k, _)| match k {
            Key::Int(i) => Some(i + 1),
            Key::Str(_) => None,
        })
        .max()
        .unwrap_or(0)
}

struct Arg {
    name: Option<String>,
    value: V,
}

/// The file of `class`: its Composer autoload entry, else an analysed or scanned file that declares it.
fn find_file(project: &Project, class: &str) -> Option<PathBuf> {
    let lower = class.trim_start_matches('\\').to_ascii_lowercase();
    if let Some(a) = (project.autoloaded.iter()).find(|a| a.class == lower.as_bytes()) {
        return Some(a.path.clone());
    }
    let short = lower
        .rsplit('\\')
        .next()
        .unwrap_or(&lower)
        .as_bytes()
        .to_vec();
    (project.analysed.iter().chain(&project.scanned))
        .find(|p| {
            std::fs::read(p).is_ok_and(|b| {
                b.to_ascii_lowercase()
                    .windows(short.len())
                    .any(|w| w == short)
                    && declares(p, &b, &lower)
            })
        })
        .cloned()
}

fn declares(path: &Path, contents: &[u8], lower: &str) -> bool {
    let file = file(path, contents.to_vec());
    let arena = LocalArena::new();
    let (program, names) = crate::codebase::parse(&arena, &file);
    class_likes(program, &names)
        .iter()
        .any(|c| c.name.eq_ignore_ascii_case(lower))
}

fn file(path: &Path, contents: Vec<u8>) -> File {
    File::new(
        std::borrow::Cow::Owned(path.to_string_lossy().into_owned().into_bytes()),
        FileType::Host,
        Some(path.to_path_buf()),
        std::borrow::Cow::Owned(contents),
    )
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// A class or trait: its resolved name, its members and the class it extends.
struct Decl<'a, 'arena> {
    name: String,
    members: &'a Sequence<'arena, ClassLikeMember<'arena>>,
    extends: Option<&'a Extends<'arena>>,
}

/// The top-level classes and traits of a file, in namespaces too.
fn class_likes<'a, 'arena>(
    program: &'a Program<'arena>,
    names: &ResolvedNames<'arena>,
) -> Vec<Decl<'a, 'arena>> {
    fn of<'a, 'arena>(
        statements: &'a [Statement<'arena>],
        names: &ResolvedNames<'arena>,
        out: &mut Vec<Decl<'a, 'arena>>,
    ) {
        for s in statements {
            match s {
                Statement::Class(c) => out.push(Decl {
                    name: text(names.get(&c.name.span)),
                    members: &c.members,
                    extends: c.extends.as_ref(),
                }),
                Statement::Trait(t) => out.push(Decl {
                    name: text(names.get(&t.name.span)),
                    members: &t.members,
                    extends: None,
                }),
                Statement::Namespace(n) => of(n.statements().as_slice(), names, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    of(program.statements.as_slice(), names, &mut out);
    out
}

/// What `read` has gathered so far for one test class.
struct Gather<'p> {
    project: &'p Project,
    test_name: String,
    /// The lowercase names of the methods read or overridden, which a trait or parent method of the same name loses to.
    seen: HashSet<String>,
    /// The lowercase names of the traits already read.
    traits: HashSet<String>,
    rules: Vec<Rule>,
}

/// Parses the file that declares the class or trait `name` and calls `f` with it; `what` names it in errors.
fn with_decl<T>(
    project: &Project,
    name: &str,
    what: &str,
    f: impl for<'b, 'arena> FnOnce(
        &Path,
        &File,
        &ResolvedNames<'arena>,
        &Decl<'b, 'arena>,
    ) -> Result<T, String>,
) -> Result<T, String> {
    let path = find_file(project, name).ok_or_else(|| {
        format!(
            "{what} was not found: Pun looks for it in Composer's autoload (autoload-dev included) and in the analysed and scanned files"
        )
    })?;
    let contents = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file = file(&path, contents);
    let arena = LocalArena::new();
    let (program, names) = crate::codebase::parse(&arena, &file);
    let Some(decl) = class_likes(program, &names)
        .into_iter()
        .find(|c| c.name.eq_ignore_ascii_case(name))
    else {
        return Err(format!("{}: {what} is not declared there", path.display()));
    };
    f(&path, &file, &names, &decl)
}

/// Reads the rules of test class `class`, the test methods of its traits and parents included, as
/// `ReflectionClass::getMethods()` lists them: a class's own method wins over a trait's, and both over a parent's.
pub(super) fn read(project: &Project, class: &str) -> Result<Vec<Rule>, String> {
    let mut gather = Gather {
        project,
        test_name: String::new(),
        seen: HashSet::new(),
        traits: HashSet::new(),
        rules: Vec::new(),
    };
    let mut current = Some(class.trim_start_matches('\\').to_string());
    for _ in 0..64 {
        let Some(name) = current.take() else { break };
        current = with_decl(
            project,
            &name,
            &format!("phpat test class {name}"),
            |path, file, names, decl| {
                if gather.test_name.is_empty() {
                    gather.test_name.clone_from(&decl.name);
                }
                gather.members(path, file, names, decl, &decl.name)?;
                Ok((decl.extends)
                    .and_then(|e| e.types.first())
                    .map(|parent| text(names.get(&parent.span()))))
            },
        )?;
    }
    Ok(gather.rules)
}

impl Gather<'_> {
    /// The test methods of `decl`, then those of the traits it uses. `declared` is the class `self` names: the test
    /// class or parent whose `use` brought a trait in.
    fn members<'arena>(
        &mut self,
        path: &Path,
        file: &File,
        names: &ResolvedNames<'arena>,
        decl: &Decl<'_, 'arena>,
        declared: &str,
    ) -> Result<(), String> {
        let reader = Reader {
            file,
            names,
            members: decl.members,
            declared: declared.to_string(),
            vars: RefCell::new(HashMap::new()),
            depth: RefCell::new(0),
        };
        for member in decl.members {
            let ClassLikeMember::Method(m) = member else {
                continue;
            };
            let MethodBody::Concrete(body) = &m.body else {
                continue;
            };
            let name = text(m.name.value);
            if !self.seen.insert(name.to_ascii_lowercase()) || !reader.is_test(m) {
                continue;
            }
            reader.vars.borrow_mut().clear();
            let at = |e: String| {
                format!(
                    "{}:{}: phpat test {declared}::{name}(): {e}",
                    path.display(),
                    file.line_number(m.span().start.offset) + 1,
                )
            };
            let prefix = format!("{}:{name}", self.test_name);
            match reader.body(&body.statements).map_err(at)? {
                Out::Single(rule) => self.rules.push(Rule {
                    name: prefix,
                    ..*rule
                }),
                Out::Many(list) => {
                    for (key, rule) in list {
                        self.rules.push(Rule {
                            name: format!("{prefix}:{}", key.text()),
                            ..rule
                        });
                    }
                }
            }
        }
        for member in decl.members {
            let ClassLikeMember::TraitUse(u) = member else {
                continue;
            };
            if let TraitUseSpecification::Concrete(s) = &u.specification
                && !s.adaptations.is_empty()
            {
                return Err(format!(
                    "{}:{}: phpat test class {declared}: Pun reads phpat tests without running PHP and can't read the trait adaptations in `{}` yet",
                    path.display(),
                    file.line_number(u.span().start.offset) + 1,
                    reader.source(u.span())
                ));
            }
            for t in &u.trait_names {
                let name = text(names.get(&t.span()));
                if !self.traits.insert(name.to_ascii_lowercase()) {
                    continue;
                }
                let what = format!("trait {name} of phpat test class {declared}");
                with_decl(self.project, &name, &what, |path, file, names, decl| {
                    self.members(path, file, names, decl, declared)
                })?;
            }
        }
        Ok(())
    }
}

enum Out {
    Single(Box<Rule>),
    Many(Vec<(Key, Rule)>),
}

struct Reader<'a, 'arena> {
    file: &'a File,
    names: &'a ResolvedNames<'arena>,
    /// The members of the class or trait whose method is read, where `self::X` looks up a constant.
    members: &'a Sequence<'arena, ClassLikeMember<'arena>>,
    declared: String,
    vars: RefCell<HashMap<Vec<u8>, V>>,
    depth: RefCell<u32>,
}

fn rule_of(v: V) -> Result<Rule, String> {
    match v {
        V::Builder(rule, _) => Ok(rule.borrow().clone()),
        v => Err(format!(
            "a test method returns a rule builder or an iterable of them, not {}",
            v.kind()
        )),
    }
}

impl<'arena> Reader<'_, 'arena> {
    fn source(&self, span: Span) -> String {
        let s = text(&self.file.contents[span.to_range_usize()]);
        let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if s.chars().count() > 80 {
            format!("{}...", s.chars().take(77).collect::<String>())
        } else {
            s
        }
    }

    fn unreadable(&self, span: Span) -> String {
        format!(
            "Pun reads phpat tests without running PHP and can't read `{}` (line {})",
            self.source(span),
            self.file.line_number(span.start.offset) + 1
        )
    }

    /// `TestParser::parse()`'s test method filter.
    fn is_test(&self, m: &Method<'arena>) -> bool {
        let public = !m
            .modifiers
            .iter()
            .any(|m| matches!(m, Modifier::Private(_) | Modifier::Protected(_)));
        let attribute = (m.attribute_lists.iter())
            .flat_map(|l| l.attributes.iter())
            .any(|a| text(self.names.get(&a.name.span())).eq_ignore_ascii_case(TEST_RULE));
        public && (attribute || m.name.value.starts_with(b"test"))
    }

    fn body(&self, statements: &Sequence<'arena, Statement<'arena>>) -> Result<Out, String> {
        let mut yielded: Option<Vec<(Key, V)>> = None;
        for s in statements {
            match s {
                Statement::Expression(e) => match e.expression {
                    Expression::Assignment(a) if a.operator.is_assign() => self.assign(a)?,
                    Expression::Yield(Yield::Value(y)) => {
                        let list = yielded.get_or_insert_with(Vec::new);
                        let key = Key::Int(next_key(list));
                        let value = y.value.map_or(Ok(V::Null), |v| self.eval(v))?;
                        list.push((key, value));
                    }
                    Expression::Yield(Yield::Pair(y)) => {
                        let key = self.key(y.key)?;
                        let value = self.eval(y.value)?;
                        yielded.get_or_insert_with(Vec::new).push((key, value));
                    }
                    _ => return Err(self.unreadable(e.span())),
                },
                Statement::Return(r) => {
                    if yielded.is_some() {
                        break;
                    }
                    let Some(value) = r.value else {
                        return Err(self.unreadable(r.span()));
                    };
                    return match self.eval(value)? {
                        V::Array(list) => Ok(Out::Many(
                            (list.into_iter())
                                .map(|(k, v)| Ok((k, rule_of(v)?)))
                                .collect::<Result<_, String>>()?,
                        )),
                        v => Ok(Out::Single(Box::new(rule_of(v)?))),
                    };
                }
                Statement::Foreach(f) => self.foreach(f)?,
                Statement::Noop(_) => {}
                s => return Err(self.unreadable(s.span())),
            }
        }
        match yielded {
            Some(list) => Ok(Out::Many(
                (list.into_iter())
                    .map(|(k, v)| Ok((k, rule_of(v)?)))
                    .collect::<Result<_, String>>()?,
            )),
            None => Err("a test method returns a rule builder or an iterable of them".into()),
        }
    }

    /// `$x = ...;` or `$x[] = ...;`.
    fn assign(&self, a: &Assignment<'arena>) -> Result<(), String> {
        let (name, append) = match a.lhs {
            Expression::Variable(Variable::Direct(v)) => (v.name, false),
            Expression::ArrayAppend(ArrayAppend {
                array: Expression::Variable(Variable::Direct(v)),
                ..
            }) => (v.name, true),
            _ => return Err(self.unreadable(a.lhs.span())),
        };
        let value = self.eval(a.rhs)?;
        if !append {
            self.vars.borrow_mut().insert(name.to_vec(), value);
            return Ok(());
        }
        let mut vars = self.vars.borrow_mut();
        // PHP creates the array when `$x` is undefined; `$x` holding anything but an array is beyond Pun.
        match vars
            .entry(name.to_vec())
            .or_insert_with(|| V::Array(Vec::new()))
        {
            V::Array(list) => {
                let key = Key::Int(next_key(list));
                list.push((key, value));
                Ok(())
            }
            _ => Err(self.unreadable(a.span())),
        }
    }

    /// A `foreach` over a statically known array whose body only assigns local variables, such as
    /// `$selectors[] = Selector::classname($class);`.
    fn foreach(&self, f: &Foreach<'arena>) -> Result<(), String> {
        let V::Array(list) = self.eval(f.expression)? else {
            return Err(self.unreadable(f.expression.span()));
        };
        let var = |e: &Expression<'arena>| match e {
            Expression::Variable(Variable::Direct(v)) => Ok(v.name.to_vec()),
            e => Err(self.unreadable(e.span())),
        };
        let (key, value) = match &f.target {
            ForeachTarget::Value(t) => (None, var(t.value)?),
            ForeachTarget::KeyValue(t) => (Some(var(t.key)?), var(t.value)?),
        };
        for (k, v) in list {
            if let Some(key) = &key {
                let k = match k {
                    Key::Int(i) => V::Int(i),
                    Key::Str(s) => V::Str(s),
                };
                self.vars.borrow_mut().insert(key.clone(), k);
            }
            self.vars.borrow_mut().insert(value.clone(), v);
            for s in f.body.statements() {
                self.loop_statement(s)?;
            }
        }
        Ok(())
    }

    fn loop_statement(&self, s: &Statement<'arena>) -> Result<(), String> {
        match s {
            Statement::Block(b) => (b.statements.iter()).try_for_each(|s| self.loop_statement(s)),
            Statement::Expression(e) => match e.expression {
                Expression::Assignment(a) if a.operator.is_assign() => self.assign(a),
                _ => Err(self.unreadable(e.span())),
            },
            Statement::Noop(_) => Ok(()),
            s => Err(self.unreadable(s.span())),
        }
    }

    /// `array_map($callback, $array)` with one array: the callback applied to each value, keys kept.
    fn array_map(&self, call: &FunctionCall<'arena>) -> Result<V, String> {
        let unreadable = || Err(self.unreadable(call.span()));
        let [Argument::Positional(callback), Argument::Positional(array)] =
            call.argument_list.arguments.as_slice()
        else {
            return unreadable();
        };
        if callback.ellipsis.is_some() || array.ellipsis.is_some() {
            return unreadable();
        }
        let V::Array(list) = self.eval(array.value)? else {
            return unreadable();
        };
        (list.into_iter())
            .map(|(k, v)| Ok((k, self.call(callback.value, v)?)))
            .collect::<Result<_, String>>()
            .map(V::Array)
    }

    /// Calls `callback` with one argument: a Selector factory as a first-class callable or a string, or a closure
    /// or arrow function of one parameter whose body Pun can read.
    fn call(&self, callback: &Expression<'arena>, value: V) -> Result<V, String> {
        let unreadable = || Err(self.unreadable(callback.span()));
        let selector = |class: &str, method: &str| {
            if !SELECTOR.contains(&class.to_ascii_lowercase().as_str()) {
                return unreadable();
            }
            let args = vec![Arg {
                name: None,
                value: value.clone(),
            }];
            (self.selector(&method.to_ascii_lowercase(), args))
                .map(V::Selector)
                .map_err(|why| format!("{}: {why}", self.unreadable(callback.span())))
        };
        let parameter = |list: &FunctionLikeParameterList<'arena>| match list.parameters.as_slice()
        {
            [p] if p.ampersand.is_none() && p.ellipsis.is_none() => Some(p.variable.name.to_vec()),
            _ => None,
        };
        match callback {
            Expression::PartialApplication(PartialApplication::StaticMethod(p))
                if p.argument_list.is_first_class_callable() =>
            {
                let (Some(class), ClassLikeMemberSelector::Identifier(m)) =
                    (self.class_name(p.class), &p.method)
                else {
                    return unreadable();
                };
                selector(&class, &text(m.value))
            }
            Expression::Literal(Literal::String(s)) => {
                let Some((class, method)) = s.value.map(text).and_then(|s| {
                    let (c, m) = s.split_once("::")?;
                    Some((c.trim_start_matches('\\').to_string(), m.to_string()))
                }) else {
                    return unreadable();
                };
                selector(&class, &method)
            }
            Expression::ArrowFunction(f) if f.ampersand.is_none() => {
                let Some(name) = parameter(&f.parameter_list) else {
                    return unreadable();
                };
                // An arrow function sees the enclosing variables; its parameter shadows one of them.
                let saved = self.vars.borrow_mut().insert(name.clone(), value);
                let out = self.eval(f.expression);
                let mut vars = self.vars.borrow_mut();
                match saved {
                    Some(v) => vars.insert(name, v),
                    None => vars.remove(&name),
                };
                out
            }
            Expression::Closure(c) if c.ampersand.is_none() && c.use_clause.is_none() => {
                let (Some(name), [Statement::Return(Return { value: Some(e), .. })]) =
                    (parameter(&c.parameter_list), c.body.statements.as_slice())
                else {
                    return unreadable();
                };
                // A closure without `use` sees only its parameter.
                let outer = self.vars.replace(HashMap::from([(name, value)]));
                let out = self.eval(e);
                self.vars.replace(outer);
                out
            }
            _ => unreadable(),
        }
    }

    fn key(&self, e: &Expression<'arena>) -> Result<Key, String> {
        match self.eval(e)? {
            V::Int(i) => Ok(Key::Int(i)),
            V::Str(s) => Ok(match s.parse::<i64>() {
                Ok(i) if i.to_string() == s => Key::Int(i),
                _ => Key::Str(s),
            }),
            _ => Err(self.unreadable(e.span())),
        }
    }

    fn eval(&self, e: &Expression<'arena>) -> Result<V, String> {
        let depth = *self.depth.borrow();
        if depth > 64 {
            return Err(self.unreadable(e.span()));
        }
        *self.depth.borrow_mut() += 1;
        let v = self.eval_inner(e);
        *self.depth.borrow_mut() -= 1;
        v
    }

    fn class_name(&self, e: &Expression<'arena>) -> Option<String> {
        match e {
            Expression::Identifier(id) => Some(text(self.names.get(&id.span()))),
            Expression::Self_(_) | Expression::Static(_) => Some(self.declared.clone()),
            _ => None,
        }
    }

    fn eval_inner(&self, e: &Expression<'arena>) -> Result<V, String> {
        let unreadable = || Err(self.unreadable(e.span()));
        match e {
            Expression::Parenthesized(p) => self.eval(p.expression),
            Expression::Literal(Literal::String(s)) => match s.value {
                Some(v) => Ok(V::Str(text(v))),
                None => unreadable(),
            },
            // PHP reads a literal past PHP_INT_MAX as a float, which Pun doesn't read.
            Expression::Literal(Literal::Integer(i)) => {
                (i.value.and_then(|v| i64::try_from(v).ok()))
                    .map_or_else(unreadable, |i| Ok(V::Int(i)))
            }
            Expression::Literal(Literal::True(_)) => Ok(V::Bool(true)),
            Expression::Literal(Literal::False(_)) => Ok(V::Bool(false)),
            Expression::Literal(Literal::Null(_)) => Ok(V::Null),
            Expression::Binary(b) if matches!(b.operator, BinaryOperator::StringConcat(_)) => {
                let piece = |v: V| match v {
                    V::Str(s) => Some(s),
                    V::Int(i) => Some(i.to_string()),
                    _ => None,
                };
                match (piece(self.eval(b.lhs)?), piece(self.eval(b.rhs)?)) {
                    (Some(l), Some(r)) => Ok(V::Str(l + &r)),
                    _ => unreadable(),
                }
            }
            Expression::Array(a) => self.array(a.elements.iter()),
            Expression::LegacyArray(a) => self.array(a.elements.iter()),
            Expression::Variable(Variable::Direct(v)) => match self.vars.borrow().get(v.name) {
                Some(v) => Ok(v.clone()),
                None => unreadable(),
            },
            Expression::Access(Access::ClassConstant(c)) => {
                let ClassLikeConstantSelector::Identifier(constant) = &c.constant else {
                    return unreadable();
                };
                let Some(class) = self.class_name(c.class) else {
                    return unreadable();
                };
                if constant.value.eq_ignore_ascii_case(b"class") {
                    return Ok(V::Str(class));
                }
                if !class.eq_ignore_ascii_case(&self.declared) {
                    return unreadable();
                }
                let value = (self.members.iter()).find_map(|m| match m {
                    ClassLikeMember::Constant(k) => {
                        k.items.iter().find(|i| i.name.value == constant.value)
                    }
                    _ => None,
                });
                match value {
                    Some(item) => self.eval(item.value),
                    None => unreadable(),
                }
            }
            Expression::Call(Call::StaticMethod(call)) => {
                let (Some(class), ClassLikeMemberSelector::Identifier(m)) =
                    (self.class_name(call.class), &call.method)
                else {
                    return unreadable();
                };
                let class = class.to_ascii_lowercase();
                let method = text(m.value).to_ascii_lowercase();
                let args = self.args(&call.argument_list)?;
                if class == PHPAT && method == "rule" && args.is_empty() {
                    return Ok(V::Builder(
                        Rc::new(RefCell::new(Rule::default())),
                        Step::Subject,
                    ));
                }
                if SELECTOR.contains(&class.as_str()) {
                    return (self.selector(&method, args))
                        .map(V::Selector)
                        .map_err(|why| format!("{}: {why}", self.unreadable(e.span())));
                }
                unreadable()
            }
            Expression::Call(Call::Function(call))
                if matches!(call.function, Expression::Identifier(id)
                    if self.names.get(&id.span()).rsplit(|b| *b == b'\\').next()
                        .is_some_and(|n| n.eq_ignore_ascii_case(b"array_map"))) =>
            {
                self.array_map(call)
            }
            Expression::Call(Call::Method(call)) => {
                let ClassLikeMemberSelector::Identifier(m) = &call.method else {
                    return unreadable();
                };
                let V::Builder(rule, step) = self.eval(call.object)? else {
                    return unreadable();
                };
                let args = self.args(&call.argument_list)?;
                let method = text(m.value).to_ascii_lowercase();
                (apply(&rule, step, &method, args))
                    .map(|step| V::Builder(rule, step))
                    .map_err(|why| format!("{}: {why}", self.unreadable(e.span())))
            }
            _ => unreadable(),
        }
    }

    fn array<'b>(
        &self,
        elements: impl Iterator<Item = &'b ArrayElement<'arena>>,
    ) -> Result<V, String>
    where
        'arena: 'b,
    {
        let mut out = Vec::new();
        for element in elements {
            match element {
                ArrayElement::KeyValue(kv) => {
                    let key = self.key(kv.key)?;
                    let value = self.eval(kv.value)?;
                    out.push((key, value));
                }
                ArrayElement::Value(v) => {
                    let key = Key::Int(next_key(&out));
                    let value = self.eval(v.value)?;
                    out.push((key, value));
                }
                // PHP rejects an empty element outside list(); Pun doesn't read `...` in arrays yet.
                ArrayElement::Missing(_) | ArrayElement::Variadic(_) => {
                    return Err(self.unreadable(element.span()));
                }
            }
        }
        // A later entry with the same key replaces the earlier one, keeping its position.
        let mut deduped: Vec<(Key, V)> = Vec::new();
        for (k, v) in out {
            match deduped.iter_mut().find(|(d, _)| d.text() == k.text()) {
                Some(entry) => entry.1 = v,
                None => deduped.push((k, v)),
            }
        }
        Ok(V::Array(deduped))
    }

    fn args(&self, list: &ArgumentList<'arena>) -> Result<Vec<Arg>, String> {
        let mut out = Vec::new();
        for a in &list.arguments {
            match a {
                Argument::Positional(p) if p.ellipsis.is_none() => out.push(Arg {
                    name: None,
                    value: self.eval(p.value)?,
                }),
                // `...$array` passes int keys as positional arguments and string keys as named ones.
                Argument::Positional(p) => {
                    let V::Array(list) = self.eval(p.value)? else {
                        return Err(self.unreadable(a.span()));
                    };
                    out.extend(list.into_iter().map(|(k, value)| Arg {
                        name: match k {
                            Key::Int(_) => None,
                            Key::Str(s) => Some(s),
                        },
                        value,
                    }));
                }
                Argument::Named(n) => out.push(Arg {
                    name: Some(text(n.name.value)),
                    value: self.eval(n.value)?,
                }),
            }
        }
        Ok(out)
    }

    fn selector(&self, method: &str, args: Vec<Arg>) -> Result<Selector, String> {
        let flag = |args: Vec<Arg>, name: &str| -> Result<(String, bool), String> {
            let [s, regex] = bind(args, &[name, "regex"], &[None, Some(V::Bool(false))])?;
            Ok((string(s)?, boolean(regex)?))
        };
        let pattern = |args: Vec<Arg>, name: &str| -> Result<Pattern, String> {
            let (s, regex) = flag(args, name)?;
            Pattern::new(s, regex)
        };
        let none = |args: &[Arg], s: Selector| {
            if args.is_empty() {
                Ok(s)
            } else {
                Err(format!("{method}() takes no arguments"))
            }
        };
        Ok(match method {
            "all" => none(&args, Selector::All)?,
            "isexception" => none(&args, Selector::IsException)?,
            "isthrowable" => none(&args, Selector::IsThrowable)?,
            "iserror" => none(&args, Selector::IsError)?,
            "isinterface" => none(&args, Selector::IsInterface)?,
            "isabstract" => none(&args, Selector::IsAbstract)?,
            "isfinal" => none(&args, Selector::IsFinal)?,
            "istrait" => none(&args, Selector::IsTrait)?,
            "isreadonly" => none(&args, Selector::IsReadonly)?,
            "isenum" => none(&args, Selector::IsEnum)?,
            "isattribute" => none(&args, Selector::IsAttribute)?,
            "isstandardclass" => none(&args, Selector::IsStandardClass)?,
            "innamespace" => Selector::Namespace(pattern(args, "namespace")?),
            "classname" => Selector::Classname(pattern(args, "fqcn")?),
            "withfilepath" => Selector::Filepath(pattern(args, "filename")?),
            "implements" => Selector::Implements(pattern(args, "fqcn")?),
            "extends" => Selector::Extends(pattern(args, "fqcn")?),
            "includes" => Selector::Includes(pattern(args, "fqcn")?),
            "appliesattribute" => {
                let [s, regex, arguments] = bind(
                    args,
                    &["fqcn", "regex", "arguments"],
                    &[None, Some(V::Bool(false)), Some(V::Array(Vec::new()))],
                )?;
                match arguments {
                    V::Array(a) if a.is_empty() => {}
                    _ => {
                        return Err(
                            "Pun does not support appliesAttribute()'s $arguments yet".into()
                        );
                    }
                }
                Selector::AppliesAttribute(Pattern::new(string(s)?, boolean(regex)?)?)
            }
            "not" => {
                let [s] = bind(args, &["selector"], &[None])?;
                Selector::NoneOf(vec![selector(s)?])
            }
            "allof" => Selector::AllOf(selectors(args)?),
            "anyof" => Selector::AnyOf(selectors(args)?),
            "noneof" => Selector::NoneOf(selectors(args)?),
            "oneof" => Selector::OneOf(selectors(args)?),
            "atleastcountof" | "atmostcountof" => {
                let mut args = args.into_iter();
                let count = match args.next() {
                    Some(Arg {
                        name: None,
                        value: V::Int(i),
                    }) => i,
                    _ => return Err(format!("{method}() takes an int count first")),
                };
                let rest = selectors(args.collect())?;
                if method == "atleastcountof" {
                    Selector::AtLeastCountOf(count, rest)
                } else {
                    Selector::AtMostCountOf(count, rest)
                }
            }
            _ => return Err(format!("phpat 0.12.5 has no selector {method}()")),
        })
    }
}

fn string(v: V) -> Result<String, String> {
    match v {
        V::Str(s) => Ok(s),
        v => Err(format!("expected a string, got {}", v.kind())),
    }
}

fn boolean(v: V) -> Result<bool, String> {
    match v {
        V::Bool(b) => Ok(b),
        v => Err(format!("expected a bool, got {}", v.kind())),
    }
}

fn selector(v: V) -> Result<Selector, String> {
    match v {
        V::Selector(s) => Ok(s),
        v => Err(format!("expected a selector, got {}", v.kind())),
    }
}

/// The selectors of a variadic `SelectorInterface ...$selectors` parameter.
fn selectors(args: Vec<Arg>) -> Result<Vec<Selector>, String> {
    (args.into_iter())
        .map(|a| match a.name {
            Some(n) => Err(format!("unknown named argument ${n}")),
            None => selector(a.value),
        })
        .collect()
}

/// Binds arguments to a method's parameters as PHP does: positional ones in order (extra ones are ignored), then
/// named ones by name; a parameter left without a value takes its default.
fn bind<const N: usize>(
    args: Vec<Arg>,
    params: &[&str; N],
    defaults: &[Option<V>; N],
) -> Result<[V; N], String> {
    let mut out: [Option<V>; N] = std::array::from_fn(|_| None);
    let mut position = 0;
    for a in args {
        match a.name {
            None => {
                if position < N {
                    out[position] = Some(a.value);
                }
                position += 1;
            }
            Some(n) => {
                let i = (params.iter().position(|p| *p == n))
                    .ok_or_else(|| format!("unknown named argument ${n}"))?;
                if out[i].is_some() {
                    return Err(format!("argument ${n} is given twice"));
                }
                out[i] = Some(a.value);
            }
        }
    }
    let mut values = Vec::with_capacity(N);
    for (i, v) in out.into_iter().enumerate() {
        match v.or_else(|| defaults[i].clone()) {
            Some(v) => values.push(v),
            None => return Err(format!("argument ${} is missing", params[i])),
        }
    }
    Ok(values.try_into().unwrap_or_else(|_| unreachable!()))
}

fn strings(args: Vec<Arg>) -> Result<Vec<String>, String> {
    (args.into_iter())
        .map(|a| match a.name {
            Some(n) => Err(format!("unknown named argument ${n}")),
            None => string(a.value),
        })
        .collect()
}

/// Calls `method` on a builder at `step`, as phpat's step classes do, and returns the step it moves to.
fn apply(
    rule: &Rc<RefCell<Rule>>,
    step: Step,
    method: &str,
    args: Vec<Arg>,
) -> Result<Step, String> {
    let has = |s: Step| step.with_parents().contains(&s);
    if has(Step::Constraint)
        && let Some((_, constraint, then)) = ALIASES.iter().find(|(m, ..)| *m == method)
    {
        let next = apply(rule, step, constraint, Vec::new())?;
        return apply(rule, next, then, args);
    }
    let none = |args: &[Arg]| {
        if args.is_empty() {
            Ok(())
        } else {
            Err(format!("{method}() takes no arguments"))
        }
    };
    let mut r = rule.borrow_mut();
    let kind = |r: &mut Rule, kind: Kind, next: Step| {
        r.kind = Some(kind);
        Ok(next)
    };
    match method {
        "classes" if has(Step::Subject) => {
            r.subjects = selectors(args)?;
            Ok(Step::SubjectExcludeOrConstraint)
        }
        "classes" if has(Step::Target) => {
            r.targets = selectors(args)?;
            Ok(Step::TargetExcludeOrTipOrBuild)
        }
        "excluding" if has(Step::SubjectExcludeOrConstraint) => {
            r.subject_excludes = selectors(args)?;
            Ok(Step::Constraint)
        }
        "excluding" if has(Step::TargetExcludeOrTipOrBuild) => {
            r.target_excludes = selectors(args)?;
            Ok(Step::TipOrBuild)
        }
        "should" | "shouldnot" | "canonly" if has(Step::Constraint) => {
            none(&args)?;
            let (constraint, next) = match method {
                "should" => (Constraint::Should, Step::Should),
                "shouldnot" => (Constraint::ShouldNot, Step::ShouldNot),
                _ => (Constraint::CanOnly, Step::CanOnly),
            };
            r.constraint = Some(constraint);
            Ok(next)
        }
        "because" if has(Step::TipOrBuild) => {
            r.tips = strings(args)?;
            Ok(Step::Build)
        }
        "nonignorable" if has(Step::Build) => {
            none(&args)?;
            r.non_ignorable = true;
            Ok(step)
        }
        "benamed" | "haveonlyonepublicmethodnamed" if has(Step::Should) => {
            let (names, k) = if method == "benamed" {
                (["classname", "regex"], Kind::BeNamed)
            } else {
                (["name", "isRegex"], Kind::HaveOnlyOnePublicMethodNamed)
            };
            let [name, regex] = bind(args, &names, &[None, Some(V::Bool(false))])?;
            r.name_param = Some(Pattern::new(string(name)?, boolean(regex)?)?);
            kind(&mut r, k, Step::TipOrBuild)
        }
        _ => {
            let target = |k| (k, Step::Target);
            let tip = |k| (k, Step::TipOrBuild);
            let found = match (step, method) {
                (Step::Should | Step::ShouldNot | Step::CanOnly, "dependon") => {
                    Some(target(Kind::Depend))
                }
                (Step::ShouldNot | Step::CanOnly, "construct") => Some(target(Kind::Construct)),
                (Step::Should | Step::ShouldNot, m) => match m {
                    "extend" => Some(target(Kind::Extend)),
                    "implement" => Some(target(Kind::Implement)),
                    "include" => Some(target(Kind::Include)),
                    "beabstract" => Some(tip(Kind::BeAbstract)),
                    "befinal" => Some(tip(Kind::BeFinal)),
                    "bereadonly" => Some(tip(Kind::BeReadonly)),
                    "beenum" => Some(tip(Kind::BeEnum)),
                    "beinvokable" => Some(tip(Kind::BeInvokable)),
                    "applyattribute" if step == Step::Should => Some(target(Kind::ApplyAttribute)),
                    "beinterface" if step == Step::Should => Some(tip(Kind::BeInterface)),
                    "haveonlyonepublicmethod" if step == Step::Should => {
                        Some(tip(Kind::HaveOnlyOnePublicMethod))
                    }
                    "exist" if step == Step::ShouldNot => Some(tip(Kind::Exist)),
                    _ => None,
                },
                _ => None,
            };
            let Some((k, next)) = found else {
                return Err(format!(
                    "phpat 0.12.5 has no {method}() at this point of the rule builder"
                ));
            };
            none(&args)?;
            kind(&mut r, k, next)
        }
    }
}
