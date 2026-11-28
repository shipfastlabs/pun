use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use mago_allocator::LocalArena;
use mago_codex::build_synthetic_name;
use mago_codex::metadata::CodebaseMetadata;
use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::get_mixed;
use mago_codex::ttype::union::TUnion;
use mago_database::file::{File, FileType};
use mago_names::ResolvedNames;
use mago_span::HasSpan;
use mago_syntax::cst::*;
use mago_syntax::walker::MutWalker;
use mago_word::Word;

use crate::codebase::{parse, read};
use crate::larastan::app::App;

const MACROABLE: &[u8] = b"Illuminate\\Support\\Traits\\Macroable";
const BUILDER: &[u8] = b"Illuminate\\Database\\Eloquent\\Builder";
const QUERY_BUILDER: &[u8] = b"Illuminate\\Database\\Query\\Builder";

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct Macros {
    #[serde(serialize_with = "crate::fingerprint::unordered_nested")]
    macros: HashMap<String, HashMap<String, Macro>>,
    #[serde(serialize_with = "crate::fingerprint::unordered")]
    accessors: HashMap<String, String>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Macro {
    /// The return type; a `static` in it is the class the macro is called on.
    pub(crate) ty: TUnion,
    pub(crate) instance: Option<(String, String)>,
    /// A method callable (`[Foo::class, 'bar']`, `'Foo::bar'`): Larastan reflects it as the native method, not as a
    /// `Macro`, so EloquentBuilderForwardsCallsExtension types its call on an Eloquent builder as the builder.
    pub(crate) native: bool,
}

#[derive(Clone)]
pub(crate) enum Definition {
    /// A closure or arrow function: its native return type, its synthetic name in the codebase, and whether it is
    /// `static`.
    Closure(Option<TUnion>, Word, bool),
    Function(String),
    Method(String, String),
    /// `X::mixin(new Mixin)`: Macroable::mixin() registers each public and protected method's returned closure.
    Mixin(String),
    /// `X::macro('name', $this->method())`: the closure the class's method returns.
    Returned(String, String),
}

pub(crate) fn discovered_providers(root: &Path) -> Vec<PathBuf> {
    let composer = root.join("vendor/composer");
    let Some(installed) = crate::io::read(composer.join("installed.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
    else {
        return Vec::new();
    };
    let packages = installed.get("packages").unwrap_or(&installed).as_array();
    let mut files = Vec::new();
    for package in packages.into_iter().flatten() {
        let providers = package["extra"]["laravel"]["providers"].as_array();
        let (Some(path), Some(psr4)) = (
            package["install-path"].as_str(),
            package["autoload"]["psr-4"].as_object(),
        ) else {
            continue;
        };
        for provider in providers.into_iter().flatten().filter_map(|p| p.as_str()) {
            for (prefix, directories) in psr4 {
                let Some(rest) = provider
                    .trim_start_matches('\\')
                    .strip_prefix(prefix.as_str())
                else {
                    continue;
                };
                let directories = match directories {
                    serde_json::Value::String(directory) => vec![directory.as_str()],
                    other => other
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|d| d.as_str())
                        .collect(),
                };
                let file = format!("{}.php", rest.replace('\\', "/"));
                files.extend(
                    directories
                        .into_iter()
                        .map(|directory| composer.join(path).join(directory).join(&file))
                        .filter(|file| crate::io::is_file(&file)),
                );
            }
        }
    }
    files
}

impl Macros {
    /// Larastan boots the application and reads the macros its providers registered. Pun cannot run PHP, so it reads
    /// the `macro()` and `mixin()` calls of the bootstrap files, the app's providers, the framework's and the
    /// discovered packages' providers, and of every analysed file that calls `::macro(` or `::mixin(`, wherever the
    /// call sits (a provider registered in `bootstrap/providers.php`, or a helper class a provider calls).
    pub(crate) fn load(
        bootstrap: &[PathBuf],
        analysed: &[PathBuf],
        scanned: &[PathBuf],
        autoloaded: &[crate::project::Autoloaded],
        root: &Path,
        codebase: &CodebaseMetadata,
        app: &App,
    ) -> Macros {
        let registers = |path: &&PathBuf| {
            crate::io::read(path).is_ok_and(|b| {
                memchr::memmem::find_iter(&b, b"::").any(|at| {
                    b[at..].get(..7).is_some_and(|w| {
                        w.eq_ignore_ascii_case(b"::macro") || w.eq_ignore_ascii_case(b"::mixin")
                    })
                })
            })
        };
        let mut files: Vec<PathBuf> = {
            use rayon::prelude::*;
            (analysed.par_iter())
                .filter(|p| registers(p))
                .cloned()
                .collect()
        };
        files.extend_from_slice(bootstrap);
        let php = ["php".to_string()];
        files.extend(crate::php_files(&root.join("app/Providers"), &php));
        let framework = root.join("vendor/laravel/framework/src/Illuminate");
        files.extend(crate::php_files(&framework.join("Support/Facades"), &php));
        files.push(framework.join("Foundation/Providers/FoundationServiceProvider.php"));
        files.extend(discovered_providers(root));
        let mut seen = HashSet::new();
        let mut read_files = Reader::default();
        read_files.read_all(files, &mut seen);
        // A class a provider instantiates or names may register the macros (laravel/head's `(new
        // HeadRouteMacros)->register()` in its service provider, Livewire's feature classes); it sits where Composer's
        // PSR-4 autoloading finds it.
        let helpers: Vec<PathBuf> = (autoloaded.iter())
            .filter(|a| {
                !read_files
                    .classes
                    .contains(&String::from_utf8_lossy(&a.class).to_ascii_lowercase())
                    && (read_files.instantiated)
                        .contains(&String::from_utf8_lossy(&a.class).to_ascii_lowercase())
            })
            .map(|a| a.path.clone())
            .filter(|path| registers(&path))
            .collect();
        read_files.read_all(helpers, &mut seen);
        // A mixin class Pun has not read yet sits in the file named after it, as PSR-4 autoloading wants.
        let mixins: HashSet<String> = (read_files.registrations.iter())
            .filter_map(|(_, _, definition)| match definition {
                Definition::Mixin(class)
                    if !read_files.classes.contains(&class.to_ascii_lowercase()) =>
                {
                    Some(
                        class
                            .rsplit('\\')
                            .next()
                            .unwrap_or(class)
                            .to_ascii_lowercase(),
                    )
                }
                _ => None,
            })
            .collect();
        for path in analysed.iter().chain(scanned) {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_ascii_lowercase());
            if stem.is_some_and(|stem| mixins.contains(&stem)) && seen.insert(path.clone()) {
                read_files.read(path);
            }
        }
        // A package's mixin (Filament's `Testable::mixin(new TestsActions)` in its provider) sits where Composer's
        // PSR-4 autoloading finds it.
        let unread: HashSet<String> = (read_files.registrations.iter())
            .filter_map(|(_, _, definition)| match definition {
                Definition::Mixin(class)
                    if !read_files.classes.contains(&class.to_ascii_lowercase()) =>
                {
                    Some(class.trim_start_matches('\\').to_ascii_lowercase())
                }
                _ => None,
            })
            .collect();
        for a in autoloaded {
            if unread.contains(String::from_utf8_lossy(&a.class).as_ref())
                && seen.insert(a.path.clone())
            {
                read_files.read(&a.path);
            }
        }
        let mut macros = Macros {
            accessors: std::mem::take(&mut read_files.accessors),
            ..Macros::default()
        };
        let populated = |ty: Option<TUnion>| populated(codebase, ty);
        for (class, name, definition) in std::mem::take(&mut read_files.registrations) {
            let Some(home) = home(codebase, &class).or_else(|| {
                let root = macros.facade_root(app, &class)?;
                let root = if root.eq_ignore_ascii_case("Illuminate\\Cache\\CacheManager") {
                    "Illuminate\\Cache\\Repository".to_string()
                } else {
                    root
                };
                home(codebase, &root)
            }) else {
                continue;
            };
            let holder = codebase.get_class_like(home.as_bytes()).map_or_else(
                || class.clone(),
                |k| String::from_utf8_lossy(k.original_name.as_bytes()).into_owned(),
            );
            let return_type = |t: Option<&mago_codex::metadata::ttype::TypeMetadata>| {
                t.map_or_else(get_mixed, |t| t.type_union.clone())
            };
            let found = match definition {
                Definition::Closure(ty, ..) => Macro {
                    ty: populated(ty),
                    instance: None,
                    native: false,
                },
                Definition::Returned(class, method) => {
                    let key = (class.to_ascii_lowercase(), method.to_ascii_lowercase());
                    let Some(ty) = read_files.returned.get(&key) else {
                        continue;
                    };
                    Macro {
                        ty: populated(ty.clone()),
                        instance: None,
                        native: false,
                    }
                }
                Definition::Mixin(mixin) => {
                    let methods = read_files.mixin_methods.get(&mixin.to_ascii_lowercase());
                    let defined = macros.macros.entry(home).or_default();
                    for (method, returned) in methods.into_iter().flatten() {
                        let found = Macro {
                            ty: populated(returned.clone()),
                            instance: None,
                            native: false,
                        };
                        defined.insert(method.to_ascii_lowercase(), found);
                    }
                    continue;
                }
                Definition::Function(function) => {
                    let Some(f) =
                        codebase.get_function(function.trim_start_matches('\\').as_bytes())
                    else {
                        continue;
                    };
                    Macro {
                        ty: return_type(f.return_type_metadata.as_ref()),
                        instance: Some((holder, name.clone())),
                        native: false,
                    }
                }
                Definition::Method(class, method) => {
                    let local = read_files
                        .methods
                        .get(&(class.to_ascii_lowercase(), method.to_ascii_lowercase()));
                    match (
                        codebase.get_method(class.as_bytes(), method.as_bytes()),
                        local,
                    ) {
                        (Some(m), _) => {
                            let declaring = m
                                .method_metadata
                                .as_ref()
                                .filter(|mm| !mm.is_static)
                                .and_then(|_| {
                                    codebase.get_declaring_method_class(
                                        class.as_bytes(),
                                        method.as_bytes(),
                                    )
                                })
                                .and_then(|d| codebase.get_class_like(d.as_bytes()));
                            Macro {
                                ty: return_type(m.return_type_metadata.as_ref()),
                                instance: declaring.map(|d| {
                                    let name = String::from_utf8_lossy(m.original_name.as_bytes())
                                        .into_owned();
                                    (
                                        String::from_utf8_lossy(d.original_name.as_bytes())
                                            .into_owned(),
                                        name,
                                    )
                                }),
                                native: true,
                            }
                        }
                        (None, Some(local)) => Macro {
                            ty: populated(local.ty.clone()),
                            instance: local.instance.clone(),
                            native: true,
                        },
                        (None, None) => continue,
                    }
                }
            };
            macros
                .macros
                .entry(home)
                .or_default()
                .insert(name.to_ascii_lowercase(), found);
        }
        macros
    }

    pub(crate) fn homes(
        &self,
        codebase: &CodebaseMetadata,
        app: &App,
        class: &[u8],
    ) -> Vec<String> {
        const MODEL: &[u8] = b"Illuminate\\Database\\Eloquent\\Model";
        const RELATION: &[u8] = b"Illuminate\\Database\\Eloquent\\Relations\\Relation";
        const FACADE: &[u8] = b"Illuminate\\Support\\Facades\\Facade";
        let builder = String::from_utf8_lossy(BUILDER).into_owned();
        let query_builder = String::from_utf8_lossy(QUERY_BUILDER).into_owned();
        let Some(k) = codebase.get_class_like(class) else {
            return Vec::new();
        };
        let name = String::from_utf8_lossy(k.original_name.as_bytes()).into_owned();
        let is = |parent: &[u8]| {
            class.eq_ignore_ascii_case(parent) || codebase.is_instance_of(class, parent)
        };
        let mut homes: Vec<String> = Vec::new();
        if k.kind.is_interface() && name.starts_with("Illuminate\\Contracts") {
            if let Some(concrete) = app.resolve(&name)
                && codebase.class_uses_trait(concrete.as_bytes(), MACROABLE)
            {
                homes.extend(home(codebase, concrete));
            }
        } else if let Some(h) = home(codebase, &name) {
            homes.push(h);
            if is(BUILDER) || is(RELATION) {
                homes.extend(home(codebase, &builder));
                homes.extend(home(codebase, &query_builder));
            }
        } else if is(FACADE) {
            let classes: Vec<String> = match name.to_ascii_lowercase().as_str() {
                "illuminate\\support\\facades\\auth" => {
                    vec![
                        "Illuminate\\Auth\\SessionGuard".into(),
                        "Illuminate\\Auth\\RequestGuard".into(),
                    ]
                }
                "illuminate\\support\\facades\\cache" => {
                    vec![
                        "Illuminate\\Cache\\CacheManager".into(),
                        "Illuminate\\Cache\\Repository".into(),
                    ]
                }
                _ => self.facade_root(app, &name).into_iter().collect(),
            };
            homes.extend(classes.iter().filter_map(|c| home(codebase, c)));
        } else if is(MODEL) || is(RELATION) {
            homes.extend(home(codebase, &builder));
            homes.extend(home(codebase, &query_builder));
        }
        homes
    }

    pub(crate) fn get(&self, homes: &[String], name: &str) -> Option<&Macro> {
        let name = name.to_ascii_lowercase();
        homes.iter().find_map(|h| self.macros.get(h)?.get(&name))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.macros.is_empty()
    }

    pub(crate) fn facade_root(&self, app: &App, facade: &str) -> Option<String> {
        let accessor = self
            .accessors
            .get(&facade.trim_start_matches('\\').to_ascii_lowercase())?;
        Some(app.resolve(accessor).unwrap_or(accessor).to_string())
    }
}

fn macroable(codebase: &CodebaseMetadata, class: &[u8]) -> bool {
    codebase.class_uses_trait(class, MACROABLE)
        || [BUILDER, QUERY_BUILDER]
            .iter()
            .any(|b| class.eq_ignore_ascii_case(b) || codebase.is_instance_of(class, b))
}

/// A literal array's string keys and string values.
fn literal_array(e: &Expression) -> Option<(Vec<String>, Vec<String>)> {
    let elements = match e {
        Expression::Array(a) => &a.elements,
        Expression::LegacyArray(a) => &a.elements,
        _ => return None,
    };
    let literal = |e: &Expression| match e {
        Expression::Literal(Literal::String(s)) => {
            Some(String::from_utf8_lossy(s.value?).into_owned())
        }
        _ => None,
    };
    let (mut keys, mut values) = (Vec::new(), Vec::new());
    for element in elements {
        match element {
            ArrayElement::KeyValue(kv) => {
                keys.extend(literal(kv.key));
                values.extend(literal(kv.value));
            }
            ArrayElement::Value(v) => values.extend(literal(v.value)),
            ArrayElement::Variadic(_) | ArrayElement::Missing(_) => {}
        }
    }
    Some((keys, values))
}

/// A type read from a closure's or method's native return type, with its class names looked up (`populate`); a
/// class Mago does not know leaves it `mixed`.
fn populated(codebase: &CodebaseMetadata, ty: Option<TUnion>) -> TUnion {
    let Some(mut ty) = ty else {
        return get_mixed();
    };
    mago_codex::ttype::union::populate_union_type(
        &mut ty,
        &codebase.symbols,
        None,
        &mut mago_codex::reference::SymbolReferences::new(),
        false,
    );
    if ty.types.iter().any(|t| matches!(t, TAtomic::Reference(_))) {
        return get_mixed();
    }
    ty
}

pub(crate) fn home(codebase: &CodebaseMetadata, class: &str) -> Option<String> {
    let mut current = codebase.get_class_like(class.trim_start_matches('\\').as_bytes())?;
    if !macroable(codebase, current.name.as_bytes()) {
        return None;
    }
    while let Some(parent) = current
        .direct_parent_class
        .and_then(|p| codebase.get_class_like(p.as_bytes()))
        && macroable(codebase, parent.name.as_bytes())
    {
        current = parent;
    }
    Some(String::from_utf8_lossy(current.name.as_bytes()).to_ascii_lowercase())
}

struct Local {
    ty: Option<TUnion>,
    instance: Option<(String, String)>,
}

#[derive(Default)]
struct Reader {
    registrations: Vec<(String, String, Definition)>,
    methods: HashMap<(String, String), Local>,
    accessors: HashMap<String, String>,
    /// The classes read, lowercased.
    classes: HashSet<String>,
    /// The class constants whose value is a literal array (lowercased class and name): its keys and values.
    constants: HashMap<(String, String), (Vec<String>, Vec<String>)>,
    /// A mixin class's public and protected methods (lowercased class), each with the native return type of the
    /// closure it returns.
    mixin_methods: HashMap<String, Vec<(String, Option<TUnion>)>>,
    /// Each method that returns a closure (lowercased class and method): the closure's native return type.
    returned: HashMap<(String, String), Option<TUnion>>,
    /// The classes `new` or a `::class` constant names in the files read.
    instantiated: HashSet<String>,
}

/// The `X::macro()` calls of `files` and of the files they include: the class each is called on, the macro's name
/// and its definition.
pub(crate) fn registrations(files: Vec<PathBuf>) -> Vec<(String, String, Definition)> {
    let mut reader = Reader::default();
    reader.read_all(files, &mut HashSet::new());
    reader.registrations
}

impl Reader {
    /// Reads `files`, and the files they include, once each.
    fn read_all(&mut self, mut files: Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
        while let Some(path) = files.pop() {
            if !seen.insert(path.clone()) {
                continue;
            }
            files.extend(self.read(&path));
        }
    }

    fn read(&mut self, path: &Path) -> Vec<PathBuf> {
        let Some(file) = read(path, FileType::Host).ok().flatten() else {
            return Vec::new();
        };
        let arena = LocalArena::new();
        let (program, names) = parse(&arena, &file);
        let mut walker = Walker {
            reader: self,
            file: &file,
            names: &names,
            dir: path.parent().unwrap_or(Path::new("")).to_path_buf(),
            namespace: String::new(),
            class: None,
            includes: Vec::new(),
            loops: Vec::new(),
        };
        walker.walk_program(program, &mut ());
        walker.includes
    }
}

struct Walker<'r, 'n, 'arena> {
    reader: &'r mut Reader,
    file: &'n File,
    names: &'n ResolvedNames<'arena>,
    dir: PathBuf,
    namespace: String,
    class: Option<String>,
    includes: Vec<PathBuf>,
    /// The macro names each enclosing `foreach` over a literal array gives its key and value variables.
    loops: Vec<Vec<(Vec<u8>, Vec<String>)>>,
}

impl Walker<'_, '_, '_> {
    fn name(&self, id: &Identifier) -> String {
        String::from_utf8_lossy(self.names.get(&id.span())).into_owned()
    }

    fn include(&mut self, e: &Expression) {
        let path = match e {
            Expression::Literal(Literal::String(s)) => s
                .value
                .map(|v| self.dir.join(String::from_utf8_lossy(v).as_ref())),
            Expression::Binary(b) if b.operator.is_concatenation() => match (b.lhs, b.rhs) {
                (
                    Expression::MagicConstant(MagicConstant::Directory(_)),
                    Expression::Literal(Literal::String(s)),
                ) => s.value.map(|v| {
                    PathBuf::from(format!(
                        "{}{}",
                        self.dir.display(),
                        String::from_utf8_lossy(v)
                    ))
                }),
                _ => None,
            },
            _ => None,
        };
        self.includes
            .extend(path.filter(|p| crate::io::is_file(&p)));
    }

    fn class_of(&self, e: &Expression) -> Option<String> {
        match e {
            Expression::Literal(Literal::String(s)) => {
                Some(String::from_utf8_lossy(s.value?).into_owned())
            }
            Expression::Access(Access::ClassConstant(c)) => match (&c.class, &c.constant) {
                (class, ClassLikeConstantSelector::Identifier(i))
                    if i.value.eq_ignore_ascii_case(b"class") =>
                {
                    match class {
                        Expression::Self_(_) | Expression::Static(_) => self.class.clone(),
                        Expression::Identifier(id) => Some(self.name(id)),
                        _ => None,
                    }
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn string(&self, e: &Expression) -> Option<String> {
        match e {
            Expression::Literal(Literal::String(s)) => {
                Some(String::from_utf8_lossy(s.value?).into_owned())
            }
            Expression::Binary(b) if b.operator.is_concatenation() => {
                let left = self.string(b.lhs).or_else(|| self.class_of(b.lhs))?;
                Some(left + &self.string(b.rhs)?)
            }
            _ => None,
        }
    }

    /// The closure a method body returns, directly or through the variable it was assigned to: its native return
    /// type.
    fn returned_closure(&self, statements: &[Statement]) -> Option<Option<TUnion>> {
        let returned = statements.iter().find_map(|s| match s {
            Statement::Return(Return { value, .. }) => Some(*value),
            _ => None,
        })??;
        let returned = match returned {
            Expression::Variable(Variable::Direct(v)) => statements.iter().find_map(|s| match s {
                Statement::Expression(ExpressionStatement {
                    expression: Expression::Assignment(a),
                    ..
                }) if matches!(a.lhs, Expression::Variable(Variable::Direct(l)) if l.name == v.name) => {
                    Some(a.rhs)
                }
                _ => None,
            })?,
            e => e,
        };
        match returned {
            Expression::Closure(c) => {
                Some(c.return_type_hint.as_ref().and_then(|h| self.hint(&h.hint)))
            }
            Expression::ArrowFunction(f) => {
                Some(f.return_type_hint.as_ref().and_then(|h| self.hint(&h.hint)))
            }
            _ => None,
        }
    }

    fn definition(&self, e: &Expression) -> Option<Definition> {
        match e {
            Expression::Call(Call::Method(MethodCall {
                object: Expression::Variable(Variable::Direct(this)),
                method: ClassLikeMemberSelector::Identifier(method),
                ..
            })) if this.name == b"$this" => Some(Definition::Returned(
                self.class.clone()?,
                String::from_utf8_lossy(method.value).into_owned(),
            )),
            Expression::Closure(c) => Some(Definition::Closure(
                c.return_type_hint.as_ref().and_then(|h| self.hint(&h.hint)),
                build_synthetic_name("closure", self.file, c.span()),
                c.r#static.is_some(),
            )),
            Expression::ArrowFunction(f) => Some(Definition::Closure(
                f.return_type_hint.as_ref().and_then(|h| self.hint(&h.hint)),
                build_synthetic_name("closure", self.file, f.span()),
                f.r#static.is_some(),
            )),
            Expression::Array(a) => self.callable_array(&a.elements),
            Expression::LegacyArray(a) => self.callable_array(&a.elements),
            e => {
                let s = self.string(e)?;
                Some(match s.split_once("::") {
                    Some((class, method)) => {
                        Definition::Method(class.to_string(), method.to_string())
                    }
                    None => Definition::Function(s),
                })
            }
        }
    }

    fn callable_array(
        &self,
        elements: &TokenSeparatedSequence<ArrayElement>,
    ) -> Option<Definition> {
        let [ArrayElement::Value(object), ArrayElement::Value(method)] = elements.as_slice() else {
            return None;
        };
        let class = match object.value {
            Expression::Variable(Variable::Direct(v)) if v.name == b"$this" => {
                self.class.clone()?
            }
            e => self.class_of(e)?,
        };
        let Expression::Literal(Literal::String(m)) = method.value else {
            return None;
        };
        Some(Definition::Method(
            class,
            String::from_utf8_lossy(m.value?).into_owned(),
        ))
    }

    /// The native type, unpopulated: `self` is the class the closure or method is declared in, and `static` is
    /// marked static, for the class the macro is called on.
    fn hint(&self, hint: &Hint) -> Option<TUnion> {
        crate::larastan::config::type_from_text_in(
            self.hint_text(hint)?.as_bytes(),
            self.class.as_deref(),
        )
    }

    fn hint_text(&self, hint: &Hint) -> Option<String> {
        Some(match hint {
            Hint::Identifier(id) => format!("\\{}", self.name(id)),
            Hint::Parenthesized(p) => format!("({})", self.hint_text(p.hint)?),
            Hint::Nullable(n) => format!("?{}", self.hint_text(n.hint)?),
            Hint::Union(u) => format!("{}|{}", self.hint_text(u.left)?, self.hint_text(u.right)?),
            Hint::Intersection(i) => {
                format!("{}&{}", self.hint_text(i.left)?, self.hint_text(i.right)?)
            }
            Hint::Static(k) | Hint::Self_(k) if self.class.is_some() => {
                String::from_utf8_lossy(k.value).to_ascii_lowercase()
            }
            Hint::Static(_) | Hint::Self_(_) | Hint::Parent(_) => return None,
            Hint::Null(k) | Hint::True(k) | Hint::False(k) | Hint::Array(k) | Hint::Callable(k) => {
                String::from_utf8_lossy(k.value).to_ascii_lowercase()
            }
            Hint::Void(i)
            | Hint::Never(i)
            | Hint::Float(i)
            | Hint::Bool(i)
            | Hint::Integer(i)
            | Hint::String(i)
            | Hint::Object(i)
            | Hint::Mixed(i)
            | Hint::Iterable(i) => String::from_utf8_lossy(i.value).to_ascii_lowercase(),
        })
    }
}

impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for Walker<'_, '_, 'arena> {
    fn walk_in_namespace(&mut self, n: &'ast Namespace<'arena>, (): &mut ()) {
        self.namespace = n.name.as_ref().map_or_else(String::new, |n| {
            String::from_utf8_lossy(n.value()).into_owned()
        });
    }

    fn walk_in_class(&mut self, c: &'ast Class<'arena>, (): &mut ()) {
        let name = String::from_utf8_lossy(c.name.value).into_owned();
        self.class = Some(if self.namespace.is_empty() {
            name
        } else {
            format!("{}\\{name}", self.namespace)
        });
        self.reader
            .classes
            .extend(self.class.as_ref().map(|c| c.to_ascii_lowercase()));
    }

    fn walk_out_class(&mut self, _: &'ast Class<'arena>, (): &mut ()) {
        self.class = None;
    }

    fn walk_in_method(&mut self, m: &'ast Method<'arena>, (): &mut ()) {
        if let Some(class) = &self.class {
            let ty = m.return_type_hint.as_ref().and_then(|h| self.hint(&h.hint));
            let key = (
                class.to_ascii_lowercase(),
                String::from_utf8_lossy(m.name.value).to_ascii_lowercase(),
            );
            if m.name.value.eq_ignore_ascii_case(b"getFacadeAccessor")
                && let MethodBody::Concrete(body) = &m.body
                && let [
                    Statement::Return(Return {
                        value: Some(value), ..
                    }),
                ] = body.statements.as_slice()
                && let Some(accessor) = self.class_of(value)
            {
                self.reader
                    .accessors
                    .insert(class.to_ascii_lowercase(), accessor);
            }
            let is_static = m.modifiers.iter().any(|m| matches!(m, Modifier::Static(_)));
            let instance = (!is_static).then(|| {
                (
                    class.clone(),
                    String::from_utf8_lossy(m.name.value).into_owned(),
                )
            });
            let private = m
                .modifiers
                .iter()
                .any(|m| matches!(m, Modifier::Private(_)));
            let returned = match &m.body {
                MethodBody::Concrete(body) => self.returned_closure(body.statements.as_slice()),
                MethodBody::Abstract(_) => None,
            };
            if let Some(ty) = &returned {
                self.reader.returned.insert(key.clone(), ty.clone());
            }
            if !private && !m.name.value.starts_with(b"__") {
                let ty = returned.flatten();
                let name = String::from_utf8_lossy(m.name.value).into_owned();
                (self.reader.mixin_methods.entry(class.to_ascii_lowercase()))
                    .or_default()
                    .push((name, ty));
            }
            self.reader.methods.insert(key, Local { ty, instance });
        }
    }

    fn walk_in_include_construct(&mut self, c: &'ast IncludeConstruct<'arena>, (): &mut ()) {
        self.include(c.value);
    }

    fn walk_in_include_once_construct(
        &mut self,
        c: &'ast IncludeOnceConstruct<'arena>,
        (): &mut (),
    ) {
        self.include(c.value);
    }

    fn walk_in_require_construct(&mut self, c: &'ast RequireConstruct<'arena>, (): &mut ()) {
        self.include(c.value);
    }

    fn walk_in_require_once_construct(
        &mut self,
        c: &'ast RequireOnceConstruct<'arena>,
        (): &mut (),
    ) {
        self.include(c.value);
    }

    fn walk_in_class_like_constant(&mut self, k: &'ast ClassLikeConstant<'arena>, (): &mut ()) {
        let Some(class) = self.class.as_ref().map(|c| c.to_ascii_lowercase()) else {
            return;
        };
        for item in &k.items {
            if let Some(literals) = literal_array(item.value) {
                let name = String::from_utf8_lossy(item.name.value).to_ascii_lowercase();
                self.reader
                    .constants
                    .insert((class.clone(), name), literals);
            }
        }
    }

    fn walk_in_foreach(&mut self, f: &'ast Foreach<'arena>, (): &mut ()) {
        let literals = match f.expression {
            Expression::Access(Access::ClassConstant(c)) => match (&c.class, &c.constant) {
                (class, ClassLikeConstantSelector::Identifier(name)) => {
                    let class = match class {
                        Expression::Self_(_) | Expression::Static(_) => self.class.clone(),
                        Expression::Identifier(id) => Some(self.name(id)),
                        _ => None,
                    };
                    class.and_then(|class| {
                        let name = String::from_utf8_lossy(name.value).to_ascii_lowercase();
                        (self.reader.constants)
                            .get(&(class.to_ascii_lowercase(), name))
                            .cloned()
                    })
                }
                _ => None,
            },
            e => literal_array(e),
        };
        let variable = |e: &Expression| match e {
            Expression::Variable(Variable::Direct(v)) => Some(v.name.to_vec()),
            _ => None,
        };
        let mut bound = Vec::new();
        if let Some((keys, values)) = literals {
            if let ForeachTarget::KeyValue(kv) = &f.target {
                bound.extend(variable(kv.key).map(|name| (name, keys)));
            }
            bound.extend(variable(f.target.value()).map(|name| (name, values)));
        }
        self.loops.push(bound);
    }

    fn walk_out_foreach(&mut self, _: &'ast Foreach<'arena>, (): &mut ()) {
        self.loops.pop();
    }

    fn walk_in_instantiation(&mut self, new: &'ast Instantiation<'arena>, (): &mut ()) {
        if let Expression::Identifier(class) = new.class {
            let class = self
                .name(class)
                .trim_start_matches('\\')
                .to_ascii_lowercase();
            self.reader.instantiated.insert(class);
        }
    }

    /// Livewire's provider names its feature classes (`SupportPageComponents::class`, whose `provide()` registers the
    /// View macros) for the component hook registry to boot.
    fn walk_in_class_constant_access(
        &mut self,
        access: &'ast ClassConstantAccess<'arena>,
        (): &mut (),
    ) {
        if let (Expression::Identifier(class), ClassLikeConstantSelector::Identifier(constant)) =
            (access.class, &access.constant)
            && constant.value.eq_ignore_ascii_case(b"class")
        {
            let class = self
                .name(class)
                .trim_start_matches('\\')
                .to_ascii_lowercase();
            self.reader.instantiated.insert(class);
        }
    }

    fn walk_in_static_method_call(&mut self, call: &'ast StaticMethodCall<'arena>, (): &mut ()) {
        let ClassLikeMemberSelector::Identifier(method) = &call.method else {
            return;
        };
        let Expression::Identifier(class) = call.class else {
            return;
        };
        let arguments = &call.argument_list.arguments;
        if method.value.eq_ignore_ascii_case(b"mixin") {
            let mixin = match arguments.get(0).map(|a| a.value()) {
                Some(Expression::Instantiation(Instantiation {
                    class: Expression::Identifier(id),
                    ..
                })) => Some(self.name(id)),
                _ => None,
            };
            if let Some(mixin) = mixin {
                let class = self.name(class);
                (self.reader.registrations).push((class, String::new(), Definition::Mixin(mixin)));
            }
            return;
        }
        if !method.value.eq_ignore_ascii_case(b"macro") {
            return;
        }
        let (Some(name), Some(definition)) = (arguments.get(0), arguments.get(1)) else {
            return;
        };
        let names = match name.value() {
            Expression::Literal(Literal::String(LiteralString {
                value: Some(name), ..
            })) => vec![String::from_utf8_lossy(name).into_owned()],
            // `foreach (['a', 'b'] as $name) { X::macro($name, ...) }`: one macro per literal
            Expression::Variable(Variable::Direct(v)) => {
                match (self.loops.iter().rev().flatten()).find(|(name, _)| name == v.name) {
                    Some((_, names)) => names.clone(),
                    None => return,
                }
            }
            _ => return,
        };
        let Some(definition) = self.definition(definition.value()) else {
            return;
        };
        let class = self.name(class);
        for name in names {
            (self.reader.registrations).push((class.clone(), name, definition.clone()));
        }
    }
}
