//! The result cache of `pun analyse`, as PHPStan's result cache works: a later run analyses again only the files
//! whose own contents changed, or that read something that changed, and reuses every other file's errors.
//!
//! Analysing a file records what it reads (`mago_codex::reads`): the source files, and the class-likes, functions,
//! constants and descendant sets it looks up in the codebase, found or not, and other paths. A symbol is
//! compared by its [`fingerprint`](crate::fingerprint), what the populated codebase holds about it without source
//! positions, so editing a method body re-analyses only that file. The rest of the run (the Laravel, Mockery and
//! `@internal` facts the checks share, the project's files) is one fingerprint: when it changes, every file is
//! analysed again. With nothing changed at all, a run reuses the whole analysis without building the codebase.
//!
//! The caller keys a record on everything else that can change a result: the binary, the configuration and the flags.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mago_codex::metadata::CodebaseMetadata;
use mago_codex::reads::{PathRead, RACY, Reads};
use mago_database::file::FileId;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::fingerprint::{self, Fingerprint, Members};

/// The format of a [`Record`]: a record of another version is no record.
const VERSION: u32 = 6;

/// What one run analysed and read, for the next run.
#[derive(Serialize, Deserialize, Default)]
pub struct Record {
    version: u32,
    project: Fingerprint,
    global: Fingerprint,
    sources: Vec<Source>,
    setup: Vec<Path_>,
    names: Vec<Name>,
    paths: Vec<Path_>,
    files: Vec<FileRecord>,
    analysis: StoredAnalysis,
}

/// A source file the codebase was built from: its path, its stat (`crate::cache_stat`, or [`RACY`] when it changed
/// during the run), the hash of its contents and the hash of its outline (the contents without function and method
/// bodies). A file read from a stub bundled into the binary keeps its path and the empty stat of a missing file.
#[derive(Serialize, Deserialize, Clone)]
struct Source {
    /// The path's bytes, as the file system holds them (`key`).
    path: Vec<u8>,
    stat: Vec<u8>,
    hash: Fingerprint,
    outline: Fingerprint,
}

/// A path read outside the sources, with what was asked of it and the answer the reader got (`crate::io`), or
/// [`RACY`] when it got two.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Hash)]
struct Path_ {
    /// The path's bytes (`key`).
    path: Vec<u8>,
    read: u8,
    answer: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Kind {
    ClassLike,
    Function,
    Constant,
    Descendants,
}

#[derive(Serialize, Deserialize, Clone)]
struct Name {
    kind: Kind,
    /// The name's bytes as the codebase keys it: PHP names may hold bytes that are not UTF-8.
    name: Vec<u8>,
    fingerprint: Fingerprint,
}

#[derive(Serialize, Deserialize, Clone)]
struct FileRecord {
    /// Index of the analysed file in `sources`.
    source: u32,
    errors: Vec<StoredError>,
    fixable_ignored: usize,
    /// Indexes in `sources` (read whole, or only their outlines), `names` and `paths`.
    sources: Vec<u32>,
    outlines: Vec<u32>,
    names: Vec<u32>,
    paths: Vec<u32>,
    /// The analysis iterated a whole table: it depends on every declaration.
    all: bool,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct StoredAnalysis {
    errors: Vec<StoredError>,
    general: Vec<String>,
    fixable_ignored: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct StoredError {
    message: String,
    file: String,
    line: Option<i64>,
    can_be_ignored: bool,
    file_path: String,
    trait_file_path: Option<String>,
    tip: Option<String>,
    identifier: Option<String>,
    fixable: bool,
}

impl From<&pun_phpstan::ignore::Error> for StoredError {
    fn from(e: &pun_phpstan::ignore::Error) -> Self {
        StoredError {
            message: e.message.clone(),
            file: e.file.clone(),
            line: e.line,
            can_be_ignored: e.can_be_ignored,
            file_path: e.file_path.clone(),
            trait_file_path: e.trait_file_path.clone(),
            tip: e.tip.clone(),
            identifier: e.identifier.clone(),
            fixable: e.fixable,
        }
    }
}

impl From<&StoredError> for pun_phpstan::ignore::Error {
    fn from(e: &StoredError) -> Self {
        let mut out = pun_phpstan::ignore::Error::new(e.message.clone(), e.file.clone(), e.line);
        out.can_be_ignored = e.can_be_ignored;
        out.file_path.clone_from(&e.file_path);
        out.trait_file_path.clone_from(&e.trait_file_path);
        out.tip.clone_from(&e.tip);
        out.identifier.clone_from(&e.identifier);
        out.fixable = e.fixable;
        out
    }
}

impl From<&crate::Error> for StoredError {
    fn from(e: &crate::Error) -> Self {
        StoredError {
            message: e.message.clone(),
            file: e.file.clone(),
            line: Some(e.line),
            can_be_ignored: e.can_be_ignored,
            file_path: String::new(),
            trait_file_path: None,
            tip: e.tip.clone(),
            identifier: Some(e.identifier.clone()),
            fixable: e.fixable,
        }
    }
}

impl From<&StoredError> for crate::Error {
    fn from(e: &StoredError) -> Self {
        crate::Error {
            file: e.file.clone(),
            line: e.line.unwrap_or(0),
            identifier: e.identifier.clone().unwrap_or_default(),
            message: e.message.clone(),
            tip: e.tip.clone(),
            can_be_ignored: e.can_be_ignored,
            fixable: e.fixable,
        }
    }
}

/// What a run did with the cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Nothing changed: the whole analysis came from the record, without building the codebase.
    pub unchanged: bool,
    /// Files analysed in this run.
    pub analysed: usize,
    /// Files whose errors came from the record.
    pub reused: usize,
}

impl Record {
    pub fn encode(&self) -> Vec<u8> {
        bincode::serde::encode_to_vec(self, bincode::config::standard()).unwrap_or_default()
    }

    /// The record in `bytes`, or none when they hold another version's record or are not a record.
    pub fn decode(bytes: &[u8]) -> Option<Record> {
        let (record, _): (Record, _) =
            bincode::serde::decode_from_slice(bytes, bincode::config::standard()).ok()?;
        (record.version == VERSION).then_some(record)
    }

    /// Whether the record comes from a run with the same settings and shared facts, whose file results may hold.
    pub(crate) fn matches(&self, global: Fingerprint) -> bool {
        self.global == global
    }

    pub(crate) fn clone_record(&self) -> Record {
        Record {
            version: self.version,
            project: self.project,
            global: self.global,
            sources: self.sources.clone(),
            setup: self.setup.clone(),
            names: self.names.clone(),
            paths: self.paths.clone(),
            files: self.files.clone(),
            analysis: self.analysis.clone(),
        }
    }

    pub(crate) fn analysis(&self) -> crate::Analysis {
        crate::Analysis {
            errors: self.analysis.errors.iter().map(Into::into).collect(),
            general: self.analysis.general.clone(),
            fixable_ignored: self.analysis.fixable_ignored,
        }
    }
}

fn read_code(read: PathRead) -> u8 {
    match read {
        PathRead::Contents => 0,
        PathRead::Listing => 1,
        PathRead::Kind => 2,
    }
}

fn read_of(code: u8) -> PathRead {
    match code {
        0 => PathRead::Contents,
        1 => PathRead::Listing,
        _ => PathRead::Kind,
    }
}

fn answer(path: &Path, read: PathRead) -> Vec<u8> {
    crate::io::fingerprint(path, read)
}

/// A path as the bytes the file system holds, which a record keeps and looks paths up by: a lossy text form would make
/// two paths that are not UTF-8 one.
pub(crate) fn key(p: &Path) -> Vec<u8> {
    p.as_os_str().as_encoded_bytes().to_vec()
}

/// The path whose bytes `key` gave (pun's cache exists on unix only, where a path is any bytes).
fn path_of(bytes: &[u8]) -> &Path {
    use std::os::unix::ffi::OsStrExt;
    Path::new(std::ffi::OsStr::from_bytes(bytes))
}

/// What the project says about every file's analysis besides the files themselves: a file added to or gone from
/// the project changes only the results that looked up what it declares, which their recorded names catch.
pub(crate) fn settings_fingerprint(project: &crate::Project) -> Fingerprint {
    let mut h = blake3::Hasher::new();
    let mut add = |label: &str, value: &[u8]| {
        for part in [label.as_bytes(), value] {
            h.update(&(part.len() as u64).to_le_bytes());
            h.update(part);
        }
    };
    add("php", &project.php_version.id().to_le_bytes());
    add("level", &[project.level]);
    for p in &project.stubs {
        add("stub", &key(p));
    }
    add("larastan", format!("{:?}", project.larastan).as_bytes());
    let mut out = [0; 16];
    out.copy_from_slice(&h.finalize().as_bytes()[..16]);
    out
}

/// The files the project names, which decide what the codebase is built from.
pub(crate) fn project_fingerprint(project: &crate::Project) -> Fingerprint {
    let mut h = blake3::Hasher::new();
    let mut add = |label: &str, value: &[u8]| {
        for part in [label.as_bytes(), value] {
            h.update(&(part.len() as u64).to_le_bytes());
            h.update(part);
        }
    };
    add("php", &project.php_version.id().to_le_bytes());
    add("level", &[project.level]);
    for p in &project.analysed {
        add("analysed", &key(p));
    }
    for p in &project.scanned {
        add("scanned", &key(p));
    }
    for a in &project.autoloaded {
        add("autoloaded", &key(&a.path));
        add("class", &a.class);
        add("psr0", &[u8::from(a.psr0)]);
    }
    for p in &project.stubs {
        add("stub", &key(p));
    }
    add("larastan", format!("{:?}", project.larastan).as_bytes());
    let mut out = [0; 16];
    out.copy_from_slice(&h.finalize().as_bytes()[..16]);
    out
}

/// Whether nothing the previous run read has changed: the project names the same files, every source file, every
/// path read while preparing the analysis and every path a file read while analysed are as they were.
pub(crate) fn unchanged(project: &crate::Project, previous: &Record) -> bool {
    let same = |p: &Path_| answer(path_of(&p.path), read_of(p.read)) == p.answer;
    previous.project == project_fingerprint(project)
        && previous.sources.par_iter().all(|s| {
            let path = path_of(&s.path);
            crate::cache_stat(path) == s.stat
                || std::fs::read(path)
                    .map_or(s.stat.is_empty() && !path.exists(), |b| hash(&b) == s.hash)
        })
        && previous.setup.par_iter().all(same)
        && previous.paths.par_iter().all(same)
}

pub(crate) fn hash(bytes: &[u8]) -> Fingerprint {
    let mut out = [0; 16];
    out.copy_from_slice(&blake3::hash(bytes).as_bytes()[..16]);
    out
}

/// One analysed file's result in this run: reused from the record, or checked now with what it read.
pub(crate) enum Checked {
    Reused(FileRecordRef),
    Fresh {
        errors: Vec<pun_phpstan::ignore::Error>,
        fixable_ignored: usize,
        reads: Box<Reads>,
    },
}

/// A file record of the previous run, by index.
#[derive(Clone, Copy)]
pub(crate) struct FileRecordRef(usize);

impl Checked {
    pub(crate) fn errors(
        &self,
        previous: Option<&Record>,
    ) -> (Vec<pun_phpstan::ignore::Error>, usize) {
        match self {
            Checked::Reused(r) => {
                let f = &previous.expect("a reused file has a record").files[r.0];
                (f.errors.iter().map(Into::into).collect(), f.fixable_ignored)
            }
            Checked::Fresh {
                errors,
                fixable_ignored,
                ..
            } => (errors.clone(), *fixable_ignored),
        }
    }
}

/// The state of this run that decides which previous results still hold.
pub(crate) struct Run<'a> {
    codebase: &'a CodebaseMetadata,
    sources: Vec<Source>,
    source_index: HashMap<Vec<u8>, u32>,
    /// The indices in `sources` of the files with each name, the text Mago names a closure with
    /// (`closure_file`): a lossy form of the path, which two paths that are not UTF-8 can share.
    closure_index: HashMap<String, Vec<u32>>,
    /// The index in `sources` of each file the codebase knows by id.
    by_id: HashMap<FileId, u32>,
    members: Members<'a>,
    /// The fingerprints [`Self::valid`] computed, which [`Self::record`] keeps without computing again.
    known_names: std::sync::OnceLock<HashMap<(Kind, Vec<u8>), Fingerprint>>,
}

impl<'a> Run<'a> {
    /// The run's state over `sources`, every file the codebase was built from. `start` is when the run began: a
    /// file changed since then may hold other contents than the ones the run read, so its stat is not trusted.
    pub(crate) fn new(
        codebase: &'a CodebaseMetadata,
        sources: &crate::codebase::Sources,
        start: std::time::SystemTime,
    ) -> Run<'a> {
        let mut files = sources.every_file();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let table: Vec<(FileId, String, Source)> = files
            .par_iter()
            .filter_map(|file| {
                let path = file.path.as_deref()?;
                let contents = hash(&file.contents);
                Some((
                    file.id,
                    String::from_utf8_lossy(&file.name).into_owned(),
                    Source {
                        path: key(path),
                        stat: crate::source_stat(path, start),
                        hash: contents,
                        outline: sources.outline(file.id).unwrap_or(contents),
                    },
                ))
            })
            .collect();
        let mut by_id = HashMap::new();
        let mut source_index = HashMap::new();
        let mut closure_index: HashMap<String, Vec<u32>> = HashMap::new();
        let mut table_sources = Vec::with_capacity(table.len());
        for (id, name, source) in table {
            let i = *source_index.entry(source.path.clone()).or_insert_with(|| {
                table_sources.push(source);
                u32::try_from(table_sources.len() - 1).expect("fewer files than u32::MAX")
            });
            by_id.insert(id, i);
            closure_index.entry(name).or_default().push(i);
        }
        for named in closure_index.values_mut() {
            named.sort_unstable();
            named.dedup();
        }
        Run {
            codebase,
            sources: table_sources,
            source_index,
            closure_index,
            by_id,
            members: Members::of(codebase),
            known_names: std::sync::OnceLock::new(),
        }
    }

    fn fingerprint(&self, kind: Kind, name: &[u8]) -> Fingerprint {
        let word = mago_word::word(name);
        match kind {
            Kind::ClassLike => fingerprint::class_like(self.codebase, &self.members, word),
            Kind::Function => fingerprint::function(self.codebase, word),
            Kind::Constant => fingerprint::constant(self.codebase, word),
            Kind::Descendants => fingerprint::descendants(self.codebase, word),
        }
    }

    /// Which files of `previous` still hold, by analysed path: each such file's own contents are unchanged and so is
    /// everything it read.
    pub(crate) fn valid(&self, previous: &Record) -> HashMap<Vec<u8>, FileRecordRef> {
        let source_ok: Vec<bool> = (previous.sources.iter())
            .map(|s| {
                (self.source_index.get(&s.path))
                    .is_some_and(|&i| self.sources[i as usize].hash == s.hash)
            })
            .collect();
        let outline_ok: Vec<bool> = (previous.sources.iter())
            .map(|s| {
                (self.source_index.get(&s.path))
                    .is_some_and(|&i| self.sources[i as usize].outline == s.outline)
            })
            .collect();
        let names: Vec<Fingerprint> = (previous.names.par_iter())
            .map(|n| self.fingerprint(n.kind, &n.name))
            .collect();
        let name_ok: Vec<bool> = (previous.names.iter().zip(&names))
            .map(|(n, now)| *now == n.fingerprint)
            .collect();
        let path_ok: Vec<bool> = (previous.paths.par_iter())
            .map(|p| answer(path_of(&p.path), read_of(p.read)) == p.answer)
            .collect();
        let _ = self.known_names.set(
            (previous.names.iter().zip(names))
                .map(|(n, now)| ((n.kind, n.name.clone()), now))
                .collect(),
        );
        let nothing_changed = source_ok.iter().all(|ok| *ok)
            && previous.sources.len() == self.sources.len()
            && name_ok.iter().all(|ok| *ok)
            && path_ok.iter().all(|ok| *ok);
        (previous.files.iter().enumerate())
            .filter(|(_, f)| {
                source_ok[f.source as usize]
                    && f.sources.iter().all(|&i| source_ok[i as usize])
                    && f.outlines.iter().all(|&i| outline_ok[i as usize])
                    && f.names.iter().all(|&i| name_ok[i as usize])
                    && f.paths.iter().all(|&i| path_ok[i as usize])
                    && (!f.all || nothing_changed)
            })
            .map(|(i, f)| {
                (
                    previous.sources[f.source as usize].path.clone(),
                    FileRecordRef(i),
                )
            })
            .collect()
    }

    /// The record of this run.
    pub(crate) fn record(
        self,
        project: Fingerprint,
        global: Fingerprint,
        setup: HashMap<(PathBuf, PathRead), Vec<u8>>,
        analysed: &[PathBuf],
        checked: &[Checked],
        previous: Option<&Record>,
        analysis: &crate::Analysis,
    ) -> Record {
        let mut tables = Tables::default();
        let mut files = Vec::with_capacity(checked.len());
        let known = (analysed.iter().zip(checked))
            .filter_map(|(path, c)| Some((*self.source_index.get(&key(path))?, c)));
        for (source, c) in known {
            let record = match c {
                Checked::Reused(r) => {
                    let previous = previous.expect("a reused file has a record");
                    let f = &previous.files[r.0];
                    let mut names = Vec::new();
                    for &i in &f.names {
                        let n = &previous.names[i as usize];
                        names.push(tables.name(n.kind, &n.name));
                    }
                    let mut paths = Vec::new();
                    for &i in &f.paths {
                        let p = &previous.paths[i as usize];
                        let path = path_of(&p.path).to_path_buf();
                        paths.push(tables.path(path, read_of(p.read), &p.answer));
                    }
                    let remap = |list: &[u32]| -> Vec<u32> {
                        (list.iter())
                            .filter_map(|&i| {
                                self.source_index.get(&previous.sources[i as usize].path)
                            })
                            .copied()
                            .collect()
                    };
                    FileRecord {
                        source,
                        errors: f.errors.clone(),
                        fixable_ignored: f.fixable_ignored,
                        sources: remap(&f.sources),
                        outlines: remap(&f.outlines),
                        names,
                        paths,
                        all: f.all,
                    }
                }
                Checked::Fresh {
                    errors,
                    fixable_ignored,
                    reads,
                } => {
                    // A closure is named by its position in its file's body: reading one is reading that file.
                    let closures = (reads.functions.iter())
                        .filter_map(|w| closure_file(&String::from_utf8_lossy(w.as_bytes())))
                        .filter_map(|p| self.closure_index.get(&p))
                        .flatten()
                        .copied()
                        .collect::<Vec<_>>();
                    let mut sources: Vec<u32> = (reads.files.iter())
                        .filter_map(|id| self.by_id.get(id))
                        .copied()
                        .chain(closures)
                        .collect();
                    sources.sort_unstable();
                    sources.dedup();
                    let mut outlines: Vec<u32> = (reads.outlines.iter())
                        .filter_map(|id| self.by_id.get(id))
                        .copied()
                        .filter(|i| sources.binary_search(i).is_err())
                        .collect();
                    outlines.sort_unstable();
                    outlines.dedup();
                    let mut names = Vec::new();
                    for (kind, set) in [
                        (Kind::ClassLike, &reads.class_likes),
                        (Kind::Function, &reads.functions),
                        (Kind::Constant, &reads.constants),
                        (Kind::Descendants, &reads.descendants),
                    ] {
                        for w in set {
                            let name = w.as_bytes();
                            if kind == Kind::Function
                                && closure_file(&String::from_utf8_lossy(name)).is_some()
                            {
                                continue;
                            }
                            names.push(tables.name(kind, name));
                        }
                    }
                    names.sort_unstable();
                    let mut paths: Vec<u32> = (reads.paths.iter())
                        .map(|((p, read), answer)| tables.path(p.clone(), *read, answer))
                        .collect();
                    paths.sort_unstable();
                    FileRecord {
                        source,
                        errors: errors.iter().map(Into::into).collect(),
                        fixable_ignored: *fixable_ignored,
                        sources,
                        outlines,
                        names,
                        paths,
                        all: reads.all,
                    }
                }
            };
            files.push(record);
        }
        let known_names = self.known_names.get();
        let names = (tables.names.par_iter())
            .map(|(kind, name)| Name {
                kind: *kind,
                name: name.clone(),
                fingerprint: (known_names.and_then(|k| k.get(&(*kind, name.clone()))))
                    .copied()
                    .unwrap_or_else(|| self.fingerprint(*kind, name)),
            })
            .collect();
        let paths = (tables.paths.into_iter())
            .map(|((path, read), answer)| Path_ {
                path: key(&path),
                read: read_code(read),
                answer,
            })
            .collect();
        // A setup read of a source path's contents that answered what the source table already checks is checked
        // there, by stat or contents; keeping it would hash every source on every run. Any other answer (racy, other
        // contents) stays, and so does every other read, so the fast path checks it.
        let mut setup: Vec<Path_> = (setup.into_iter())
            .filter(|((path, read), answer)| {
                let Some(&i) = self.source_index.get(&key(path)) else {
                    return true;
                };
                *read != PathRead::Contents
                    || answer.get(..16) != Some(&self.sources[i as usize].hash[..])
            })
            .map(|((path, read), answer)| Path_ {
                path: key(&path),
                read: read_code(read),
                answer,
            })
            .collect();
        setup.sort_by(|a, b| (&a.path, a.read).cmp(&(&b.path, b.read)));
        Record {
            version: VERSION,
            project,
            global,
            sources: self.sources,
            setup,
            names,
            paths,
            files,
            analysis: StoredAnalysis {
                errors: analysis.errors.iter().map(Into::into).collect(),
                general: analysis.general.clone(),
                fixable_ignored: analysis.fixable_ignored,
            },
        }
    }
}

#[derive(Default)]
struct Tables {
    names: Vec<(Kind, Vec<u8>)>,
    name_index: HashMap<(Kind, Vec<u8>), u32>,
    paths: Vec<((PathBuf, PathRead), Vec<u8>)>,
    path_index: HashMap<(PathBuf, PathRead), u32>,
}

impl Tables {
    fn name(&mut self, kind: Kind, name: &[u8]) -> u32 {
        let key = (kind, name.to_vec());
        if let Some(&i) = self.name_index.get(&key) {
            return i;
        }
        let i = u32::try_from(self.names.len()).expect("fewer names than u32::MAX");
        self.names.push(key.clone());
        self.name_index.insert(key, i);
        i
    }

    /// The index of `path` read as `read`, with the answer the reader got: two files that got different answers
    /// for the same path (it changed during the run) leave [`RACY`], so both are checked again.
    fn path(&mut self, path: PathBuf, read: PathRead, answer: &[u8]) -> u32 {
        let key = (path, read);
        if let Some(&i) = self.path_index.get(&key) {
            let kept = &mut self.paths[i as usize].1;
            if kept.as_slice() != answer {
                *kept = RACY.to_vec();
            }
            return i;
        }
        let i = u32::try_from(self.paths.len()).expect("fewer paths than u32::MAX");
        self.paths.push((key.clone(), answer.to_vec()));
        self.path_index.insert(key, i);
        i
    }
}

/// The file a closure's or arrow function's synthetic name (`{closure:<file>:<line>:<column>}`) points into.
fn closure_file(name: &str) -> Option<String> {
    let rest = name.strip_prefix('{')?.strip_suffix('}')?;
    let (_, rest) = rest.split_once(':')?;
    let (rest, _column) = rest.rsplit_once(':')?;
    let (file, _line) = rest.rsplit_once(':')?;
    Some(file.to_string())
}

/// The class aliases of the codebase, which decide what a name a file looks up resolves to.
pub(crate) fn aliases(codebase: &CodebaseMetadata) -> Fingerprint {
    let mut aliases: Vec<(&[u8], &[u8])> = (codebase.class_like_aliases.iter())
        .map(|(a, t)| (a.as_bytes(), t.as_bytes()))
        .collect();
    aliases.sort_unstable();
    let mut h = blake3::Hasher::new();
    for (a, t) in aliases {
        for part in [a, t] {
            h.update(&(part.len() as u64).to_le_bytes());
            h.update(part);
        }
    }
    let mut out = [0; 16];
    out.copy_from_slice(&h.finalize().as_bytes()[..16]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two files that read one path with different answers (it changed during the run) leave the racy marker, so
    /// both are checked again; the same answer is kept.
    #[test]
    fn a_path_read_with_two_answers_is_racy() {
        let mut tables = Tables::default();
        let path = PathBuf::from("a.php");
        let i = tables.path(path.clone(), PathRead::Contents, b"one");
        assert_eq!(tables.path(path.clone(), PathRead::Contents, b"one"), i);
        assert_eq!(tables.paths[i as usize].1, b"one");
        assert_eq!(tables.path(path, PathRead::Contents, b"two"), i);
        assert_eq!(tables.paths[i as usize].1, RACY);
    }
}
