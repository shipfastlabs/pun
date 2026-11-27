use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use mago_codex::ttype::union::TUnion;
use mago_database::file::File;
use mago_span::{HasSpan, Span};
use mago_syntax::cst::*;
use mago_syntax::walker::MutWalker;
use pun_phpstan::types::Type;
use sha2::Digest;

use super::types::flow::unparen;
use super::{Checker, infer, scope};

pub(super) struct Settings {
    pub cwd: PathBuf,
    pub include_paths: Vec<PathBuf>,
    pub magic_dir: bool,
    pub path_constants: bool,
}

pub(super) struct State {
    types: HashMap<Span, TUnion>,
}

struct Wanted {
    spans: Vec<Span>,
    in_include: usize,
}

impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for Wanted {
    fn walk_in_expression(&mut self, e: &'ast Expression<'arena>, (): &mut ()) {
        if self.in_include > 0 {
            self.spans.push(e.span());
        }
    }
    fn walk_in_construct(&mut self, construct: &'ast Construct<'arena>, (): &mut ()) {
        self.in_include += usize::from(included(construct).is_some());
    }
    fn walk_out_construct(&mut self, construct: &'ast Construct<'arena>, (): &mut ()) {
        self.in_include -= usize::from(included(construct).is_some());
    }
}

pub(super) fn prepare(run: &infer::Run<'_, '_>, settings: &Settings) -> (Vec<Span>, State) {
    let program = run.program;
    let mut wanted = Wanted {
        spans: Vec::new(),
        in_include: 0,
    };
    wanted.walk_program(program, &mut ());
    if settings.path_constants || wanted.spans.is_empty() {
        return (
            wanted.spans,
            State {
                types: HashMap::new(),
            },
        );
    }
    let file = File::new(
        run.file.name.clone(),
        run.file.file_type,
        None,
        run.file.contents.clone(),
    );
    let run = infer::Run {
        file: &file,
        ..*run
    };
    let types = infer::analyse(&run, wanted.spans, Vec::new()).types;
    (Vec::new(), State { types })
}

fn included<'a, 'arena>(
    construct: &'a Construct<'arena>,
) -> Option<(&'a Expression<'arena>, &'static str, &'static str)> {
    Some(match construct {
        Construct::Include(i) => (i.value, "include", "include.fileNotFound"),
        Construct::IncludeOnce(i) => (i.value, "include_once", "includeOnce.fileNotFound"),
        Construct::Require(i) => (i.value, "require", "require.fileNotFound"),
        Construct::RequireOnce(i) => (i.value, "require_once", "requireOnce.fileNotFound"),
        _ => return None,
    })
}

pub(super) fn construct(c: &mut Checker, construct: &Construct) {
    let Some((expr, kind, identifier)) = included(construct) else {
        return;
    };
    let key = print_expr(c, expr);
    if c.guards
        .iter()
        .any(|g| matches!(g, scope::Guard::File(g) if g.key == key && g.valid))
    {
        return;
    }
    let mut fallback = false;
    for path in resolve_file_paths(c, expr, &mut fallback) {
        if does_file_exist(c, &path) {
            continue;
        }
        let path_expr = if fallback {
            key.clone()
        } else {
            format!("\"{path}\"")
        };
        c.report(
            construct,
            identifier,
            format!("Path in {kind}() {path_expr} is not a file or it does not exist."),
            None,
        );
    }
}

fn resolve_file_paths(c: &Checker, expr: &Expression, fallback: &mut bool) -> Vec<String> {
    *fallback = false;
    if !c.include_settings.magic_dir {
        return strings(c, expr);
    }
    let Expression::Binary(b) = unparen(expr) else {
        return strings(c, expr);
    };
    if !matches!(b.operator, BinaryOperator::StringConcat(_)) {
        return strings(c, expr);
    }
    if matches!(
        unparen(b.lhs),
        Expression::MagicConstant(MagicConstant::Directory(_))
    ) {
        *fallback = true;
        let dir = Path::new(&c.file_name())
            .parent()
            .unwrap_or(Path::new(""))
            .to_string_lossy()
            .into_owned();
        return strings(c, b.rhs)
            .into_iter()
            .map(|s| format!("{dir}{s}"))
            .collect();
    }
    let right = resolve_file_paths(c, b.rhs, fallback);
    let left = resolve_file_paths(c, b.lhs, fallback);
    let mut paths = Vec::new();
    for left in left {
        for right in &right {
            let path = normalize_separator(&format!("{left}{right}"));
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths
}

fn expression_type(c: &Checker, expr: &Expression) -> Option<Type> {
    if c.include_settings.path_constants {
        c.phpstan_type(expr)
    } else {
        c.include_state
            .types
            .get(&expr.span())
            .map(|t| c.to_phpstan(t))
    }
}

fn strings(c: &Checker, expr: &Expression) -> Vec<String> {
    match unparen(expr) {
        Expression::ConstantAccess(a)
            if c.written_or_resolved(&a.name).trim_start_matches('\\') == "DIRECTORY_SEPARATOR"
                && (c.resolved(&a.name) == "DIRECTORY_SEPARATOR"
                    || !c.codebase.constant_exists(c.names.get(&a.name.span()))) =>
        {
            vec!["/".into(), "\\".into()]
        }
        Expression::ConstantAccess(a)
            if c.written_or_resolved(&a.name).trim_start_matches('\\') == "PATH_SEPARATOR"
                && (c.resolved(&a.name) == "PATH_SEPARATOR"
                    || !c.codebase.constant_exists(c.names.get(&a.name.span()))) =>
        {
            vec![":".into(), ";".into()]
        }
        Expression::Binary(b) if matches!(b.operator, BinaryOperator::StringConcat(_)) => {
            let left = strings(c, b.lhs);
            let right = strings(c, b.rhs);
            if left.is_empty() || right.is_empty() {
                return expression_type(c, expr).map_or_else(Vec::new, |t| constant_strings(&t));
            }
            left.iter()
                .flat_map(|l| right.iter().map(move |r| format!("{l}{r}")))
                .collect()
        }
        _ => expression_type(c, expr).map_or_else(Vec::new, |t| constant_strings(&t)),
    }
}

/// PHPStan's `getConstantStrings()`, through `pun-phpstan`'s port (a template bounded by a template bounded by one
/// constant string has none).
fn constant_strings(ty: &Type) -> Vec<String> {
    ty.constant_strings()
        .into_iter()
        .map(|s| String::from_utf8_lossy(&s).into_owned())
        .collect()
}

fn does_file_exist(c: &Checker, path: &str) -> bool {
    if let Some((scheme, rest)) = path.split_once("://")
        && scheme.eq_ignore_ascii_case("phar")
    {
        return phar_file(&c.include_settings.cwd, rest).unwrap_or(false);
    }
    candidates(c, path).iter().any(|p| p.is_file())
}

fn phar_file(cwd: &Path, path: &str) -> Option<bool> {
    if path.contains('\0') {
        return None;
    }
    for (end, _) in path.match_indices('/') {
        let archive = &path[..end];
        let name = archive.rsplit('/').next()?;
        if !name.match_indices('.').any(|(dot, _)| {
            dot > 0
                && name.len() - dot < 50
                && name.as_bytes().get(dot + 1).is_some_and(|b| *b != b'.')
        }) {
            continue;
        }
        let archive = cwd.join(archive);
        if !archive.is_file() {
            continue;
        }
        let mut parts = Vec::new();
        for part in path[end + 1..].split('/').filter(|s| !s.is_empty()) {
            if part.len() >= 2 && part.bytes().all(|b| b == b'.') {
                parts.pop();
            } else if part != "." {
                parts.push(part);
            }
        }
        let member = parts.join("/");
        let bytes = std::fs::read(archive).ok()?;
        return archive_file(&bytes, member.as_bytes(), 3);
    }
    None
}

fn archive_file(bytes: &[u8], member: &[u8], recursion: u8) -> Option<bool> {
    if bytes.starts_with(b"\x1f\x8b\x08") {
        const LIMIT: u64 = 100 * 1024 * 1024;
        let mut unpacked = Vec::new();
        flate2::read::GzDecoder::new(bytes)
            .take(LIMIT + 1)
            .read_to_end(&mut unpacked)
            .ok()?;
        if unpacked.len() as u64 > LIMIT {
            return None;
        }
        let recursion = recursion - 1;
        if recursion == 0 {
            return None;
        }
        return archive_file(&unpacked, member, recursion);
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return zip_file(bytes, member);
    }
    if let Some(found) = tar_file(bytes, member) {
        return Some(found);
    }
    const HALT: &[u8] = b"__HALT_COMPILER();";
    if let Some(halt) = bytes
        .windows(HALT.len())
        .position(|s| s.eq_ignore_ascii_case(HALT))
    {
        return native_phar_file(bytes, halt + HALT.len(), member);
    }
    None
}

fn native_phar_file(bytes: &[u8], mut halt: usize, member: &[u8]) -> Option<bool> {
    if matches!(bytes.get(halt..halt + 3), Some(b" ?>" | b"\n?>")) {
        halt += 3;
        if bytes.get(halt) == Some(&b'\r') {
            halt += 1;
            if bytes.get(halt) != Some(&b'\n') {
                return None;
            }
        }
        if bytes.get(halt) == Some(&b'\n') {
            halt += 1;
        }
    }
    let mut data = bytes.get(halt..)?;
    let size = word(&mut data)? as usize;
    if size > 100 * 1024 * 1024 {
        return None;
    }
    let mut manifest = take(&mut data, size)?;
    let count = word(&mut manifest)?;
    let version = u16::from_be_bytes(take(&mut manifest, 2)?.try_into().ok()?);
    let flags = word(&mut manifest)?;
    if count == 0 || version & 0xfff0 < 0x1000 || flags & 0x10000 == 0 {
        return None;
    }
    let mut trailer = bytes.get(bytes.len().checked_sub(8)?..)?;
    let kind = word(&mut trailer)?;
    if trailer != b"GBMB" {
        return None;
    }
    let signature_size = match kind {
        1 => 16,
        2 => 20,
        3 => 32,
        4 => 64,
        _ => return None,
    };
    let signed_end = bytes.len().checked_sub(8 + signature_size)?;
    if !signature(
        kind,
        &[&bytes[..signed_end]],
        &bytes[signed_end..bytes.len() - 8],
    ) {
        return None;
    }
    for _ in 0..2 {
        let size = word(&mut manifest)? as usize;
        take(&mut manifest, size)?;
    }
    let mut found = false;
    for _ in 0..count {
        let size = word(&mut manifest)? as usize;
        let name = take(&mut manifest, size)?;
        if name.is_empty() {
            return None;
        }
        let size = word(&mut manifest)?;
        word(&mut manifest)?;
        let compressed_size = word(&mut manifest)?;
        word(&mut manifest)?;
        let flags = word(&mut manifest)?;
        let metadata_size = word(&mut manifest)? as usize;
        take(&mut manifest, metadata_size)?;
        if flags & 0xf000 == 0x2000 || (flags & 0xf000 != 0x1000 && size != compressed_size) {
            return None;
        }
        found |= name == member && !name.ends_with(b"/");
    }
    Some(found)
}

fn word(data: &mut &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(take(data, 4)?.try_into().ok()?))
}

fn take<'a>(data: &mut &'a [u8], size: usize) -> Option<&'a [u8]> {
    let (head, tail) = data.split_at_checked(size)?;
    *data = tail;
    Some(head)
}

fn signature(kind: u32, parts: &[&[u8]], expected: &[u8]) -> bool {
    fn digest<D: Digest>(parts: &[&[u8]], expected: &[u8]) -> bool {
        let mut digest = D::new();
        for part in parts {
            digest.update(part);
        }
        digest.finalize().as_slice() == expected
    }
    match kind {
        1 => digest::<md5::Md5>(parts, expected),
        2 => digest::<sha1::Sha1>(parts, expected),
        3 => digest::<sha2::Sha256>(parts, expected),
        4 => digest::<sha2::Sha512>(parts, expected),
        _ => false,
    }
}

fn zip_file(bytes: &[u8], member: &[u8]) -> Option<bool> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let central = zip.central_directory_start() as usize;
    let comment = zip.comment().to_vec();
    let mut executable = false;
    let mut signed = false;
    let mut found = false;
    let count = zip.len();
    for index in 0..count {
        let file = zip.by_index_raw(index).ok()?;
        if file.encrypted()
            || !matches!(
                file.compression(),
                zip::CompressionMethod::STORE | zip::CompressionMethod::DEFLATE
            )
        {
            return None;
        }
        let name = file.name_raw();
        if name == b".phar/signature.bin" {
            let start = file.data_start()?;
            let end = start.checked_add(file.size())?;
            let mut contents = bytes.get(start as usize..end as usize)?;
            let kind = word(&mut contents)?;
            word(&mut contents)?;
            signed = signature(
                kind,
                &[
                    bytes.get(..file.header_start() as usize)?,
                    bytes.get(central..file.central_header_start() as usize)?,
                    &comment,
                ],
                contents,
            );
            if !signed || index + 1 != count {
                return None;
            }
        } else {
            executable |= name == b".phar/stub.php";
            found |= name == member && !name.ends_with(b"/");
        }
    }
    Some(found && (!executable || signed))
}

fn tar_file(bytes: &[u8], member: &[u8]) -> Option<bool> {
    let mut tar = tar::Archive::new(Cursor::new(bytes));
    let mut executable = false;
    let mut signed = false;
    let mut found = false;
    let mut names = HashSet::<Vec<u8>>::new();
    for entry in tar.entries().ok()? {
        let entry = entry.ok()?;
        if signed {
            return None;
        }
        let name = entry.path_bytes();
        if name.as_ref() == b".phar/signature.bin" {
            let start = entry.raw_file_position() as usize;
            let end = start.checked_add(entry.size() as usize)?;
            let mut contents = bytes.get(start..end)?;
            let kind = word(&mut contents)?;
            word(&mut contents)?;
            signed = signature(
                kind,
                &[bytes.get(..entry.raw_header_position() as usize)?],
                contents,
            );
            if !signed {
                return None;
            }
        } else {
            if entry.header().entry_type().is_hard_link()
                && !names.contains(entry.link_name_bytes()?.as_ref())
            {
                return None;
            }
            names.insert(name.to_vec());
            executable |= name.as_ref() == b".phar/stub.php";
            found |= name == member && !entry.header().entry_type().is_dir();
        }
    }
    Some(found && (!executable || signed))
}

static SCHEME: LazyLock<pcre2::bytes::Regex> = LazyLock::new(|| {
    pcre2::bytes::Regex::new("(?i)^([a-z0-9+\\-.]+)://").expect("upstream wrapper pattern")
});

fn candidates(c: &Checker, path: &str) -> Vec<PathBuf> {
    let path = if SCHEME.is_match(path.as_bytes()).expect("wrapper match") {
        let (scheme, rest) = path.split_once("://").expect("matched separator");
        if !scheme.eq_ignore_ascii_case("file") {
            return Vec::new();
        }
        let local = rest
            .strip_prefix("localhost/")
            .map_or_else(|| rest.to_string(), |s| format!("/{s}"));
        if !Path::new(&local).is_absolute() {
            return Vec::new();
        }
        local
    } else {
        path.to_string()
    };
    let script = PathBuf::from(c.file_name());
    let mut paths = Vec::new();
    for dir in std::iter::once(c.include_settings.cwd.as_path())
        .chain(
            c.include_settings
                .include_paths
                .iter()
                .map(PathBuf::as_path),
        )
        .chain(script.parent())
    {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let dir = normalize_directory(&dir.to_string_lossy());
        let candidate = if Path::new(&path).is_absolute() {
            PathBuf::from(&path)
        } else {
            let joined = PathBuf::from(format!(
                "{}/{}",
                dir.trim_end_matches('/'),
                path.trim_start_matches(['/', '\\'])
            ));
            if joined.is_absolute() {
                joined
            } else {
                c.include_settings.cwd.join(joined)
            }
        };
        if !paths.contains(&candidate) {
            paths.push(candidate);
        }
    }
    paths
}

fn normalize_separator(path: &str) -> String {
    path.replace('\\', "/")
        .replace("//", "/")
        .replace("///", "/")
        .replace("////", "/")
}

fn normalize_directory(path: &str) -> String {
    let path = normalize_separator(path);
    let mut parts = Vec::new();
    for part in path.trim_matches('/').split('/') {
        match part {
            "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    format!(
        "{}{}",
        if path.starts_with('/') { "/" } else { "" },
        parts.join("/")
    )
}

fn print_expr(c: &Checker, expr: &Expression) -> String {
    match unparen(expr) {
        Expression::Binary(b) if matches!(b.operator, BinaryOperator::StringConcat(_)) => {
            let left = print_expr(c, b.lhs);
            let right = print_expr(c, b.rhs);
            let right = if matches!(unparen(b.rhs), Expression::Binary(b) if matches!(b.operator, BinaryOperator::StringConcat(_)))
            {
                format!("({right})")
            } else {
                right
            };
            format!("{left} . {right}")
        }
        Expression::CompositeString(CompositeString::Interpolated(s)) => {
            let mut out = String::from("\"");
            for part in &s.parts {
                match part {
                    StringPart::Literal(l) => out.push_str(&String::from_utf8_lossy(l.raw)),
                    StringPart::Expression(e) => out.push_str(&format!("{{{}}}", print_expr(c, e))),
                    StringPart::BracedExpression(e) => {
                        out.push_str(&format!("{{{}}}", print_expr(c, e.expression)));
                    }
                }
            }
            out.push('"');
            out
        }
        e => c.written(e.span()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FileGuard {
    function: String,
    key: String,
    variables: Vec<Vec<u8>>,
    valid: bool,
}

fn variables(expr: &Expression) -> Vec<Vec<u8>> {
    struct Vars(Vec<Vec<u8>>);
    impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for Vars {
        fn walk_in_direct_variable(&mut self, v: &'ast DirectVariable<'arena>, (): &mut ()) {
            self.0.push(v.name.to_vec());
        }
    }
    let mut vars = Vars(Vec::new());
    vars.walk_expression(expr, &mut ());
    vars.0
}

pub(super) fn guards(c: &Checker, expr: &Expression, truth: bool) -> Vec<scope::Guard> {
    let recurse = |e, truth| guards(c, e, truth);
    match unparen(expr) {
        Expression::UnaryPrefix(u) if matches!(u.operator, UnaryPrefixOperator::Not(_)) => {
            recurse(u.operand, !truth)
        }
        Expression::Binary(b)
            if matches!(
                b.operator,
                BinaryOperator::And(_)
                    | BinaryOperator::LowAnd(_)
                    | BinaryOperator::Or(_)
                    | BinaryOperator::LowOr(_)
            ) =>
        {
            let left = recurse(b.lhs, truth);
            let right = recurse(b.rhs, truth);
            if matches!(
                b.operator,
                BinaryOperator::And(_) | BinaryOperator::LowAnd(_)
            ) == truth
            {
                left.into_iter().chain(right).collect()
            } else {
                left.into_iter().filter(|g| right.contains(g)).collect()
            }
        }
        Expression::Binary(b) => {
            let literal = |e| match unparen(e) {
                Expression::Literal(Literal::True(_)) => Some(true),
                Expression::Literal(Literal::False(_)) => Some(false),
                _ => None,
            };
            let equal = matches!(
                b.operator,
                BinaryOperator::Equal(_) | BinaryOperator::Identical(_)
            );
            if equal
                || matches!(
                    b.operator,
                    BinaryOperator::NotEqual(_) | BinaryOperator::NotIdentical(_)
                )
            {
                if let Some(value) = literal(b.rhs) {
                    return recurse(b.lhs, value == (truth == equal));
                }
                if let Some(value) = literal(b.lhs) {
                    return recurse(b.rhs, value == (truth == equal));
                }
            }
            Vec::new()
        }
        Expression::Call(Call::Function(call)) if truth => {
            if let Expression::Identifier(id) = call.function
                && matches!(
                    c.resolved(id).as_str(),
                    "file_exists"
                        | "is_file"
                        | "is_readable"
                        | "is_writable"
                        | "is_writeable"
                        | "is_executable"
                )
                && let Some(arg) = call.argument_list.arguments.first()
            {
                return vec![scope::Guard::File(FileGuard {
                    function: c.resolved(id),
                    key: print_expr(c, arg.value()),
                    variables: variables(arg.value()),
                    valid: true,
                })];
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

pub(super) fn assigned(c: &mut Checker, lhs: &Expression) {
    let vars = variables(lhs);
    for guard in &mut c.guards {
        if let scope::Guard::File(g) = guard
            && g.variables.iter().any(|v| vars.contains(v))
        {
            g.valid = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::archive_file;
    use std::io::Write;

    fn gzipped_tar(padding: usize) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_cksum();
        tar.append_data(&mut header, "x.php", &[][..]).unwrap();
        let tar = tar.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.write_all(&vec![0; padding]).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn gzip_archives_unpack_to_at_most_100_mib() {
        assert_eq!(archive_file(&gzipped_tar(0), b"x.php", 3), Some(true));
        assert_eq!(
            archive_file(&gzipped_tar(100 * 1024 * 1024), b"x.php", 3),
            None
        );
    }
}
