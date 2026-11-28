use std::collections::HashMap;
use std::path::Path;

use mago_allocator::LocalArena;
use mago_database::file::FileType;
use mago_names::ResolvedNames;
use mago_span::HasSpan;
use mago_syntax::cst::*;
use mago_syntax::walker::MutWalker;

use crate::codebase::{parse, read};

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
enum Value {
    Array(Vec<(String, Value)>),
    String(String),
    Null,
    Unknown,
}

impl Value {
    fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Array(items) => items.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    fn truthy_str(&self) -> Option<&str> {
        self.as_str().filter(|s| !s.is_empty() && *s != "0")
    }

    fn merge(&self, other: &Value) -> Value {
        match (self, other) {
            (Value::Array(a), Value::Array(b)) => {
                let mut items = a.clone();
                for (k, v) in b {
                    match items.iter_mut().find(|(key, _)| key == k) {
                        Some(item) => item.1 = v.clone(),
                        None => items.push((k.clone(), v.clone())),
                    }
                }
                Value::Array(items)
            }
            _ => Value::Unknown,
        }
    }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct App {
    #[serde(serialize_with = "crate::fingerprint::unordered")]
    container: HashMap<String, String>,
    auth: Option<Value>,
    pub(crate) date_class: String,
    pub(crate) laravel_12: bool,
}

impl App {
    pub(crate) fn load(root: &Path) -> App {
        let framework = root.join("vendor/laravel/framework");
        let env = dot_env(root);
        let auth = auth_config(root, &framework, &env);
        let mut app = App {
            container: HashMap::new(),
            auth,
            date_class: date_class(root)
                .unwrap_or_else(|| "Illuminate\\Support\\Carbon".to_string()),
            laravel_12: super::stubs::laravel_version(root)
                .is_some_and(|v| super::stubs::compare(&v, "12.0.0").is_ge()),
        };
        let aliases = core_aliases(&framework.join("src/Illuminate/Foundation/Application.php"));
        for (key, classes) in aliases {
            let concrete = match key.as_str() {
                "auth.driver" => app.guard_class(None).map(str::to_string),
                "filesystem.disk" | "filesystem.cloud" => {
                    Some("Illuminate\\Filesystem\\FilesystemAdapter".to_string())
                }
                _ => classes.first().cloned(),
            };
            let Some(concrete) = concrete else { continue };
            for class in classes.iter().chain([&key]) {
                app.container
                    .insert(class.to_ascii_lowercase(), concrete.clone());
            }
        }
        // Larastan asks the booted container, which also holds the framework providers' string bindings
        // (`$this->app->singleton('db.transactions', fn () => new DatabaseTransactionsManager)`).
        for (key, concrete) in framework_bindings(root) {
            app.container
                .entry(key.to_ascii_lowercase())
                .or_insert(concrete);
        }
        for (key, concrete) in bootstrap_bindings(root) {
            app.container
                .entry(key.to_ascii_lowercase())
                .or_insert(concrete);
        }
        app
    }

    pub(crate) fn resolve(&self, abstract_: &str) -> Option<&str> {
        self.container
            .get(&abstract_.trim_start_matches('\\').to_ascii_lowercase())
            .map(String::as_str)
    }

    pub(crate) fn contract_mixins(&self, metadata: &mut mago_codex::metadata::CodebaseMetadata) {
        use mago_codex::metadata::ttype::TypeMetadata;
        use mago_codex::ttype::atomic::TAtomic;
        use mago_codex::ttype::atomic::object::{TObject, named::TNamedObject};
        use mago_codex::ttype::union::TUnion;
        let concretes: Vec<_> = metadata
            .class_likes
            .iter()
            .filter(|(_, k)| {
                k.kind.is_interface()
                    && k.original_name
                        .as_bytes()
                        .starts_with(b"Illuminate\\Contracts\\")
            })
            .filter_map(|(key, k)| {
                let concrete =
                    self.resolve(&String::from_utf8_lossy(k.original_name.as_bytes()))?;
                let concrete = metadata.get_class_like(concrete.as_bytes())?;
                (concrete.name != k.name).then_some((*key, concrete.original_name))
            })
            .collect();
        for (key, concrete) in concretes {
            if let Some(k) = metadata.class_likes.get_mut(&key) {
                let object = TAtomic::Object(TObject::Named(TNamedObject::new(concrete)));
                k.mixins
                    .push(TypeMetadata::new(TUnion::from_atomic(object), k.span));
            }
        }
        // AuthsMethodsExtension: the auth contracts have the methods of the auth models, first model first
        let models: Vec<_> = (self.auth_models(None).iter())
            .filter_map(|model| Some(metadata.get_class_like(model.as_bytes())?.original_name))
            .collect();
        for contract in [
            "Illuminate\\Contracts\\Auth\\Authenticatable",
            "Illuminate\\Contracts\\Auth\\CanResetPassword",
            "Illuminate\\Contracts\\Auth\\Access\\Authorizable",
        ] {
            let Some(key) = metadata.get_class_like(contract.as_bytes()).map(|k| k.name) else {
                continue;
            };
            if let Some(k) = metadata.class_likes.get_mut(&key) {
                for model in &models {
                    let object = TAtomic::Object(TObject::Named(TNamedObject::new(*model)));
                    k.mixins
                        .push(TypeMetadata::new(TUnion::from_atomic(object), k.span));
                }
            }
        }
    }

    pub(crate) fn auth_models(&self, guard: Option<&str>) -> Vec<String> {
        let Some(auth) = &self.auth else {
            return Vec::new();
        };
        let (Some(Value::Array(guards)), Some(providers @ Value::Array(_))) =
            (auth.get("guards"), auth.get("providers"))
        else {
            return Vec::new();
        };
        let names: Vec<&str> = match guard {
            Some(g) => vec![g],
            None => guards.iter().map(|(k, _)| k.as_str()).collect(),
        };
        let mut models: Vec<String> = Vec::new();
        for name in names {
            let Some(provider) = auth
                .get("guards")
                .and_then(|g| g.get(name)?.get("provider")?.truthy_str())
            else {
                continue;
            };
            let Some(model) = providers
                .get(provider)
                .and_then(|p| p.get("model")?.truthy_str())
            else {
                continue;
            };
            if !models.iter().any(|m| m == model) {
                models.push(model.to_string());
            }
        }
        models
    }

    pub(crate) fn guard_class(&self, guard: Option<&str>) -> Option<&'static str> {
        let auth = self.auth.as_ref()?;
        let guard = match guard {
            Some(g) => g,
            None => auth.get("defaults")?.get("guard")?.as_str()?,
        };
        driver_class(auth.get("guards")?.get(guard)?.get("driver")?.as_str()?)
    }
}

pub(crate) fn driver_class(driver: &str) -> Option<&'static str> {
    match driver {
        "session" => Some("Illuminate\\Auth\\SessionGuard"),
        "token" => Some("Illuminate\\Auth\\TokenGuard"),
        "passport" => Some("Illuminate\\Auth\\RequestGuard"),
        _ => None,
    }
}

fn core_aliases(path: &Path) -> Vec<(String, Vec<String>)> {
    let Some(file) = read(path, FileType::Vendored).ok().flatten() else {
        return Vec::new();
    };
    let arena = LocalArena::new();
    let (program, names) = parse(&arena, &file);
    struct Find<'n, 'arena> {
        names: &'n ResolvedNames<'arena>,
        found: Vec<(String, Vec<String>)>,
    }
    impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for Find<'_, 'arena> {
        fn walk_in_method(&mut self, m: &'ast Method<'arena>, (): &mut ()) {
            if !m
                .name
                .value
                .eq_ignore_ascii_case(b"registerCoreContainerAliases")
            {
                return;
            }
            let MethodBody::Concrete(body) = &m.body else {
                return;
            };
            for s in &body.statements {
                let Statement::Foreach(f) = s else { continue };
                let Expression::Array(a) = f.expression else {
                    continue;
                };
                for e in &a.elements {
                    let ArrayElement::KeyValue(kv) = e else {
                        continue;
                    };
                    let Expression::Literal(Literal::String(key)) = kv.key else {
                        continue;
                    };
                    let Some(key) = key.value else { continue };
                    let Expression::Array(classes) = kv.value else {
                        continue;
                    };
                    let classes = classes
                        .elements
                        .iter()
                        .filter_map(|c| match c {
                            ArrayElement::Value(v) => class_constant(self.names, v.value),
                            _ => None,
                        })
                        .collect();
                    self.found
                        .push((String::from_utf8_lossy(key).into_owned(), classes));
                }
            }
        }
    }
    let mut find = Find {
        names: &names,
        found: Vec::new(),
    };
    find.walk_program(program, &mut ());
    find.found
}

/// The string bindings of the framework's service providers, each with the class its closure instantiates.
pub(crate) fn framework_bindings(root: &Path) -> Vec<(String, String)> {
    let framework = root.join("vendor/laravel/framework/src/Illuminate");
    crate::php_files(&framework, &["php".to_string()])
        .iter()
        .filter(|path| path.to_string_lossy().ends_with("ServiceProvider.php"))
        .flat_map(|path| provider_bindings(path, false))
        .collect()
}

/// The class bindings of booting: the discovered packages' providers, which register after bootstrap/app.php and so
/// replace its bindings, then ApplicationBuilder::withExceptions()'s handler when bootstrap/app.php calls it.
pub(crate) fn bootstrap_bindings(root: &Path) -> Vec<(String, String)> {
    let mut bindings = package_bindings(root);
    if crate::io::read_to_string(root.join("bootstrap/app.php"))
        .is_ok_and(|app| app.contains("->withExceptions("))
    {
        bindings.push((
            "Illuminate\\Contracts\\Debug\\ExceptionHandler".to_string(),
            "Illuminate\\Foundation\\Exceptions\\Handler".to_string(),
        ));
    }
    bindings
}

/// The bindings of the discovered packages' providers (Collision's `singleton(ExceptionHandler::class, fn () => new
/// ExceptionHandler(...))`), with a class constant or a string as the abstract.
fn package_bindings(root: &Path) -> Vec<(String, String)> {
    (super::macros::discovered_providers(root).iter())
        .flat_map(|path| provider_bindings(path, true))
        .collect()
}

/// A provider's `bind()`, `singleton()` and `scoped()` calls whose abstract is a string and whose closure returns one
/// `new` instance.
fn provider_bindings(path: &Path, class_keys: bool) -> Vec<(String, String)> {
    let Some(file) = read(path, FileType::Vendored).ok().flatten() else {
        return Vec::new();
    };
    let arena = LocalArena::new();
    let (program, names) = parse(&arena, &file);
    struct Find<'n, 'arena> {
        names: &'n ResolvedNames<'arena>,
        class_keys: bool,
        found: Vec<(String, String)>,
    }
    impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for Find<'_, 'arena> {
        fn walk_in_method_call(&mut self, call: &'ast MethodCall<'arena>, (): &mut ()) {
            let ClassLikeMemberSelector::Identifier(method) = &call.method else {
                return;
            };
            if ![&b"bind"[..], b"singleton", b"scoped"]
                .iter()
                .any(|m| method.value.eq_ignore_ascii_case(m))
            {
                return;
            }
            let [abstract_, concrete] = call.argument_list.arguments.as_slice() else {
                return;
            };
            let key = match abstract_.value() {
                Expression::Literal(Literal::String(LiteralString {
                    value: Some(key), ..
                })) => String::from_utf8_lossy(key).into_owned(),
                e if self.class_keys => {
                    let Some(key) = class_constant(self.names, e) else {
                        return;
                    };
                    key
                }
                _ => return,
            };
            let returned = match concrete.value() {
                Expression::Closure(c) => match c.body.statements.as_slice().last() {
                    Some(Statement::Return(Return { value: Some(e), .. })) => *e,
                    _ => return,
                },
                Expression::ArrowFunction(f) => f.expression,
                _ => return,
            };
            if let Expression::Instantiation(Instantiation {
                class: Expression::Identifier(id),
                ..
            }) = returned
            {
                self.found.push((
                    key,
                    String::from_utf8_lossy(self.names.get(&id.span())).into_owned(),
                ));
            }
        }
    }
    let mut find = Find {
        names: &names,
        class_keys,
        found: Vec::new(),
    };
    find.walk_program(program, &mut ());
    find.found
}

fn class_constant(names: &ResolvedNames, e: &Expression) -> Option<String> {
    let Expression::Access(Access::ClassConstant(c)) = e else {
        return None;
    };
    let ClassLikeConstantSelector::Identifier(k) = &c.constant else {
        return None;
    };
    if !k.value.eq_ignore_ascii_case(b"class") {
        return None;
    }
    match c.class {
        Expression::Identifier(id) => {
            Some(String::from_utf8_lossy(names.get(&id.span())).into_owned())
        }
        Expression::Self_(_) => Some("Illuminate\\Foundation\\Application".to_string()),
        _ => None,
    }
}

fn dot_env(root: &Path) -> HashMap<String, String> {
    let Ok(text) = crate::io::read_to_string(root.join(".env")) else {
        return HashMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let line = line.strip_prefix("export ").unwrap_or(line);
            if line.starts_with('#') {
                return None;
            }
            let (k, v) = line.split_once('=')?;
            let v = v.trim();
            let v = match v.as_bytes().first() {
                Some(q @ (b'"' | b'\'')) => {
                    let rest = &v[1..];
                    rest[..rest.find(*q as char).unwrap_or(rest.len())].to_string()
                }
                _ => v.split(" #").next().unwrap_or(v).trim().to_string(),
            };
            Some((k.trim().to_string(), v))
        })
        .collect()
}

fn auth_config(root: &Path, framework: &Path, env: &HashMap<String, String>) -> Option<Value> {
    let cached = root.join("bootstrap/cache/config.php");
    if crate::io::is_file(&cached) {
        return returned(&cached, env)?.get("auth").cloned();
    }
    let base = returned(&framework.join("config/auth.php"), env);
    let app = root.join("config/auth.php");
    let app = if crate::io::is_file(&app) {
        returned(&app, env)
    } else {
        None
    };
    match (base, app) {
        (Some(base), Some(app)) => {
            let mut config = base.merge(&app);
            for option in ["guards", "providers", "passwords"] {
                if let (Some(b), Some(a)) = (base.get(option), app.get(option)) {
                    let merged = b.merge(a);
                    if let Value::Array(items) = &mut config {
                        for item in items.iter_mut().filter(|(k, _)| k == option) {
                            item.1 = merged.clone();
                        }
                    }
                }
            }
            Some(config)
        }
        (base, app) => app.or(base),
    }
}

fn returned(path: &Path, env: &HashMap<String, String>) -> Option<Value> {
    let file = read(path, FileType::Vendored).ok()??;
    let arena = LocalArena::new();
    let (program, names) = parse(&arena, &file);
    program.statements.iter().find_map(|s| match s {
        Statement::Return(r) => r.value.map(|e| value(e, &names, env)),
        _ => None,
    })
}

fn value(e: &Expression, names: &ResolvedNames, env: &HashMap<String, String>) -> Value {
    match e {
        Expression::Literal(Literal::String(s)) => s.value.map_or(Value::Unknown, |s| {
            Value::String(String::from_utf8_lossy(s).into_owned())
        }),
        Expression::Literal(Literal::Null(_)) => Value::Null,
        Expression::Access(Access::ClassConstant(_)) => {
            class_constant(names, e).map_or(Value::Unknown, Value::String)
        }
        Expression::Parenthesized(p) => value(p.expression, names, env),
        Expression::Array(a) => array(&a.elements, names, env),
        Expression::LegacyArray(a) => array(&a.elements, names, env),
        Expression::Call(Call::Function(call)) => {
            let Expression::Identifier(id) = call.function else {
                return Value::Unknown;
            };
            let name = names.get(&id.span());
            let name = name.rsplit(|&b| b == b'\\').next().unwrap_or(name);
            if !name.eq_ignore_ascii_case(b"env") {
                return Value::Unknown;
            }
            let args = &call.argument_list.arguments;
            let Some(Argument::Positional(key)) = args.first() else {
                return Value::Unknown;
            };
            let Value::String(key) = value(key.value, names, env) else {
                return Value::Unknown;
            };
            match env.get(&key) {
                Some(v) => match v
                    .to_ascii_lowercase()
                    .trim_matches(|c| c == '(' || c == ')')
                {
                    "null" => Value::Null,
                    "empty" => Value::String(String::new()),
                    "true" | "false" => Value::Unknown,
                    _ => Value::String(v.clone()),
                },
                None => match args.get(1) {
                    Some(Argument::Positional(d)) => value(d.value, names, env),
                    _ => Value::Null,
                },
            }
        }
        _ => Value::Unknown,
    }
}

fn array(
    elements: &TokenSeparatedSequence<ArrayElement>,
    names: &ResolvedNames,
    env: &HashMap<String, String>,
) -> Value {
    let mut items = Vec::new();
    let mut next = 0;
    for element in elements {
        match element {
            ArrayElement::KeyValue(kv) => {
                let Value::String(key) = value(kv.key, names, env) else {
                    return Value::Unknown;
                };
                items.push((key, value(kv.value, names, env)));
            }
            ArrayElement::Value(v) => {
                items.push((next.to_string(), value(v.value, names, env)));
                next += 1;
            }
            _ => return Value::Unknown,
        }
    }
    Value::Array(items)
}

fn date_class(root: &Path) -> Option<String> {
    let mut found = None;
    let mut files: Vec<_> = crate::io::read_dir(root.join("app/Providers"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "php"))
        .collect();
    files.sort();
    for path in files {
        let Some(file) = read(&path, FileType::Host).ok().flatten() else {
            continue;
        };
        let arena = LocalArena::new();
        let (program, names) = parse(&arena, &file);
        struct Find<'n, 'arena> {
            names: &'n ResolvedNames<'arena>,
            found: Option<String>,
        }
        impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for Find<'_, 'arena> {
            fn walk_in_static_method_call(
                &mut self,
                call: &'ast StaticMethodCall<'arena>,
                (): &mut (),
            ) {
                let Expression::Identifier(class) = call.class else {
                    return;
                };
                let class = self.names.get(&class.span());
                let date = class.eq_ignore_ascii_case(b"Illuminate\\Support\\Facades\\Date")
                    || class.eq_ignore_ascii_case(b"Illuminate\\Support\\DateFactory");
                let ClassLikeMemberSelector::Identifier(m) = &call.method else {
                    return;
                };
                if date
                    && (m.value.eq_ignore_ascii_case(b"use")
                        || m.value.eq_ignore_ascii_case(b"useClass"))
                    && let Some(Argument::Positional(arg)) = call.argument_list.arguments.first()
                    && let Some(class) = class_constant(self.names, arg.value)
                {
                    self.found = Some(class);
                }
            }
        }
        let mut find = Find {
            names: &names,
            found: None,
        };
        find.walk_program(program, &mut ());
        found = find.found.or(found);
    }
    found
}
