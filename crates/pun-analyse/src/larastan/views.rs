use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use mago_allocator::LocalArena;
use mago_database::file::FileType;
use mago_span::HasSpan;
use mago_syntax::cst::*;
use mago_syntax::parser::parse_file;
use pun_tokens::php_compat::preg_compile;

use crate::codebase::read;

fn files(dir: &Path, blade: bool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut entries: Vec<PathBuf> = crate::io::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    for p in entries {
        if crate::io::is_dir(&p) {
            found.extend(files(&p, blade));
        } else if !blade
            || p.to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".blade.php")
        {
            found.push(p);
        }
    }
    found
}

pub(crate) struct Finder {
    pub(crate) paths: Vec<PathBuf>,
    /// The view namespaces the providers register (`ns::view`), each with its directories.
    hints: Hints,
}

/// ServiceProvider::loadViewsFrom($path, $namespace) calls whose path PHP resolves without running anything:
/// `__DIR__ . '/../resources/views'` or a literal path.
static LOAD_VIEWS_FROM: LazyLock<pcre2::bytes::Regex> = LazyLock::new(|| {
    preg_compile(
        r#"/->loadViewsFrom\(\s*(__DIR__\s*\.\s*)?(['"])([^'"$]*)\2\s*,\s*(['"])([^'"$]+)\4\s*\)/"#,
    )
    .expect("loadViewsFrom pattern")
});

/// The namespaces the app's and the discovered packages' providers register, as Larastan's booted application has
/// them: FileViewFinder looks in the app's `resources/views/vendor/<namespace>` before the provider's directory
/// (ServiceProvider::loadViewsFrom()). Read once per project root, with what reading them read, which every call
/// replays: a file that looks a namespace up depends on the providers even when another file computed them.
fn hints(root: &Path) -> Hints {
    static HINTS: LazyLock<
        std::sync::Mutex<
            std::collections::HashMap<PathBuf, (Hints, std::sync::Arc<mago_codex::reads::Reads>)>,
        >,
    > = LazyLock::new(Default::default);
    let mut cache = HINTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (hints, read) = cache.entry(root.to_path_buf()).or_insert_with(|| {
        let (hints, read) = mago_codex::reads::recording(|| {
            let mut providers = crate::php_files(&root.join("app/Providers"), &["php".to_string()]);
            providers.extend(super::macros::discovered_providers(root));
            let mut hints: Vec<(String, Vec<PathBuf>)> = Vec::new();
            for provider in providers {
                let (Ok(code), Some(dir)) = (crate::io::read(&provider), provider.parent()) else {
                    continue;
                };
                for found in LOAD_VIEWS_FROM.captures_iter(&code).flatten() {
                    let text = |i| {
                        String::from_utf8_lossy(found.get(i).map_or(&b""[..], |m| m.as_bytes()))
                            .into_owned()
                    };
                    let path = if found.get(1).is_some() {
                        PathBuf::from(format!("{}{}", dir.display(), text(3)))
                    } else {
                        PathBuf::from(text(3))
                    };
                    let namespace = text(5);
                    let vendor = root.join("resources/views/vendor").join(&namespace);
                    match hints.iter_mut().find(|(n, _)| *n == namespace) {
                        Some((_, paths)) => paths.push(path),
                        None => hints.push((namespace, vec![vendor, path])),
                    }
                }
            }
            std::sync::Arc::new(hints)
        });
        (hints, std::sync::Arc::new(read))
    });
    mago_codex::reads::replay(read);
    hints.clone()
}

/// The view namespaces the providers register, each with its directories.
type Hints = std::sync::Arc<Vec<(String, Vec<PathBuf>)>>;

impl Finder {
    pub(crate) fn new(root: &Path) -> Finder {
        Finder {
            paths: vec![root.join("resources/views")],
            hints: hints(root),
        }
    }

    /// FileViewFinder::find(): `namespace::view` in the namespace's directories, any other name in the view paths.
    pub(crate) fn find(&self, name: &str) -> Option<PathBuf> {
        let (paths, name) = match name.trim().split_once("::") {
            Some((namespace, view)) => {
                let (_, paths) = self.hints.iter().find(|(n, _)| n == namespace)?;
                (paths.as_slice(), view)
            }
            None => (self.paths.as_slice(), name.trim()),
        };
        let file = name.replace('.', "/");
        paths.iter().find_map(|path| {
            ["blade.php", "php", "css", "html"]
                .iter()
                .map(|e| path.join(format!("{file}.{e}")))
                .find(|p| crate::io::exists(&p))
        })
    }

    pub(crate) fn root_view_files(&self) -> Vec<PathBuf> {
        self.paths.iter().flat_map(|p| files(p, true)).collect()
    }
}

pub(crate) fn view_names(directories: &[PathBuf]) -> Vec<String> {
    let mut names = Vec::new();
    for dir in directories {
        let prefix = format!("{}/", dir.to_string_lossy().trim_end_matches(['/', '\\']));
        for file in files(dir, true) {
            let path = file.to_string_lossy();
            if path.contains("views/vendor") || path.contains("views/errors") {
                continue;
            }
            let rest = path.split(&prefix).nth(1).unwrap_or_default();
            names.push(rest.replace('/', ".").replace(".blade.php", ""));
        }
    }
    names
}

fn inline_html(path: &Path) -> Vec<(String, u32)> {
    let mut found = Vec::new();
    if let Some(file) = read(path, FileType::Host).ok().flatten() {
        let arena = LocalArena::new();
        let program = parse_file(&arena, &file);
        for s in program.statements.iter().filter(|_| !program.has_errors()) {
            if let Statement::Inline(i) = s {
                found.push((
                    String::from_utf8_lossy(i.value).into_owned(),
                    file.line_number(i.span().start.offset) + 1,
                ));
            }
        }
    }
    found
}

static VIEW: LazyLock<pcre2::bytes::Regex> = LazyLock::new(|| {
    preg_compile(r#"/@(extends|include(If|Unless|When|First)?)(\(.*?(['"])(.*?)(['"])([),]))/m"#)
        .expect("a valid pattern")
});

pub(crate) fn views_used_in_views(directories: &[PathBuf]) -> Vec<String> {
    let mut used = Vec::new();
    for file in directories.iter().flat_map(|d| files(d, true)) {
        for (html, _) in inline_html(&file) {
            used.extend(
                VIEW.captures_iter(html.as_bytes())
                    .flatten()
                    .map(|c| String::from_utf8_lossy(&c[5]).into_owned()),
            );
        }
    }
    used
}

static TRANSLATION: LazyLock<pcre2::bytes::Regex> = LazyLock::new(|| {
    preg_compile(
        r#"/
    (
        (
            (?<!\w)
            (trans|trans_choice|Lang::get|Lang::choice|Lang::trans|Lang::transChoice|__|\$t)
        )
        |
        (@lang|@choice)
    )
    \(
    (?P<quote>['"])
    (?P<string>(\\.|(?!(?P=quote))[^\\\\])*?)
    (?P=quote)
    [),]
/mix"#,
    )
    .expect("a valid pattern")
});

pub(crate) fn translations_in_views(finder: &Finder) -> Vec<(PathBuf, Vec<(String, u32)>)> {
    finder
        .root_view_files()
        .into_iter()
        .map(|file| {
            let mut found = Vec::new();
            for (html, line) in inline_html(&file) {
                for c in TRANSLATION.captures_iter(html.as_bytes()).flatten() {
                    let quote = &c["quote"];
                    let string = String::from_utf8_lossy(&c["string"]).into_owned();
                    let start = c.get(0).map_or(0, |m| m.start());
                    let lines = html.as_bytes()[..start]
                        .iter()
                        .filter(|b| **b == b'\n')
                        .count() as u32;
                    found.push((unescape(quote == b"\"", &string), line + lines));
                }
            }
            (file, found)
        })
        .collect()
}

fn unescape(double: bool, s: &str) -> String {
    if double {
        stripcslashes(&s.replace("\\'", "\\\\'"))
    } else {
        s.replace("\\\\", "\\").replace("\\'", "'")
    }
}

fn stripcslashes(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' || i + 1 == b.len() {
            out.push(b[i]);
            i += 1;
            continue;
        }
        i += 1;
        let c = b[i];
        i += 1;
        match c {
            b'x' if i < b.len() && b[i].is_ascii_hexdigit() => {
                let end = (i + 2).min(b.len());
                let digits = b[i..end]
                    .iter()
                    .take_while(|d| d.is_ascii_hexdigit())
                    .count();
                out.push(u8::from_str_radix(&s[i..i + digits], 16).unwrap_or_default());
                i += digits;
            }
            b'0'..=b'7' => {
                let end = (i + 2).min(b.len());
                let digits = 1 + b[i..end]
                    .iter()
                    .take_while(|d| (b'0'..=b'7').contains(*d))
                    .count();
                let value = u32::from_str_radix(&s[i - 1..i - 1 + digits], 8).unwrap_or_default();
                out.push(value as u8);
                i += digits - 1;
            }
            c => out.push(
                b"ntravbf"
                    .iter()
                    .position(|e| *e == c)
                    .map_or(c, |p| b"\n\t\r\x07\x0b\x08\x0c"[p]),
            ),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn available_translations(directories: &[PathBuf]) -> Vec<String> {
    let mut available = Vec::new();
    for dir in directories.iter().filter(|d| crate::io::is_dir(&d)) {
        for file in files(dir, false) {
            let name = file.file_name().unwrap_or_default().to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            match file.extension().and_then(|e| e.to_str()) {
                Some("php") => {
                    let relative = file.strip_prefix(dir).unwrap_or(&file);
                    let parts: Vec<String> = relative
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect();
                    let prefix = if parts.len() > 2 {
                        parts[1..parts.len() - 1].join("/")
                    } else {
                        String::new()
                    };
                    let stem = file.file_stem().unwrap_or_default().to_string_lossy();
                    let root = if prefix.is_empty() {
                        stem.into_owned()
                    } else {
                        format!("{prefix}/{stem}")
                    };
                    available.push(root.clone());
                    php_keys(&file, &root, &mut available);
                }
                Some("json") => {
                    if let Ok(serde_json::Value::Object(o)) =
                        serde_json::from_slice(&crate::io::read(&file).unwrap_or_default())
                    {
                        available.extend(o.keys().cloned());
                    }
                }
                _ => {}
            }
        }
    }
    available
}

fn php_keys(path: &Path, root: &str, out: &mut Vec<String>) {
    if let Some(file) = read(path, FileType::Host).ok().flatten() {
        let arena = LocalArena::new();
        let program = parse_file(&arena, &file);
        let returned = program.statements.iter().find_map(|s| match s {
            Statement::Return(r) => r.value,
            _ => None,
        });
        if let Some(value) = returned {
            keys(value, root, out);
        }
    }
}

fn keys(e: &Expression, prefix: &str, out: &mut Vec<String>) {
    let elements = match e {
        Expression::Array(a) => &a.elements,
        Expression::LegacyArray(a) => &a.elements,
        _ => return,
    };
    let mut next = 0i64;
    for element in elements {
        let (key, value) = match element {
            ArrayElement::KeyValue(kv) => {
                let key = match kv.key {
                    Expression::Literal(Literal::String(s)) => {
                        String::from_utf8_lossy(s.value.unwrap_or_default()).into_owned()
                    }
                    Expression::Literal(Literal::Integer(i)) => {
                        let v = i.value.unwrap_or_default() as i64;
                        next = next.max(v + 1);
                        v.to_string()
                    }
                    _ => continue,
                };
                (key, kv.value)
            }
            ArrayElement::Value(v) => {
                next += 1;
                ((next - 1).to_string(), v.value)
            }
            _ => continue,
        };
        let key = format!("{prefix}.{key}");
        out.push(key.clone());
        keys(value, &key, out);
    }
}
