//! Pun: the symbols and files one unit of analysis reads, so a result cache can tell which earlier results still hold.
//!
//! A reader calls [`recording`] around the work it wants to track. Every lookup of a class-like, function, constant,
//! descendant set or namespace in the codebase made on that thread meanwhile lands in the innermost recording, as does
//! every file or path the caller reports with [`record_file`] and [`record_path`]. A lookup made with no recording
//! active costs one thread-local read. Iterating a whole table is recorded as [`Reads::all`]: the result then depends
//! on every declaration.
//!
//! A path is recorded with the answer the reader got when it read it, so a later change, or one during the run, is
//! seen: two different answers for the same path and question become [`RACY`], which no answer equals.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use mago_database::file::FileId;
use mago_word::{Word, WordSet};

/// What one recording saw.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reads {
    /// Lowercase class-like names looked up, found or not.
    pub class_likes: WordSet,
    /// Lowercase function names (and closure names) looked up, found or not.
    pub functions: WordSet,
    /// Constant names as the codebase keys them, looked up, found or not.
    pub constants: WordSet,
    /// Lowercase class-like names whose descendants were read.
    pub descendants: WordSet,
    /// A whole table was iterated.
    pub all: bool,
    /// Source files read.
    pub files: HashSet<FileId>,
    /// Source files whose declarations and doc comments were read, not their function and method bodies.
    pub outlines: HashSet<FileId>,
    /// Other paths read from the file system, with what was asked of each and the answer the reader got.
    pub paths: HashMap<(PathBuf, PathRead), Vec<u8>>,
}

/// The answer of a path whose answer changed while it was being read: never equal to a real answer, so whatever read it
/// is checked again.
pub const RACY: &[u8] = b"racy";

/// Keeps `answer` for `key`, or [`RACY`] when `paths` already holds another answer for it.
pub fn keep_answer(paths: &mut HashMap<(PathBuf, PathRead), Vec<u8>>, key: (PathBuf, PathRead), answer: Vec<u8>) {
    match paths.entry(key) {
        std::collections::hash_map::Entry::Vacant(e) => {
            e.insert(answer);
        }
        std::collections::hash_map::Entry::Occupied(mut e) => {
            if *e.get() != answer {
                *e.get_mut() = RACY.to_vec();
            }
        }
    }
}

/// What a reader asked of a path outside the sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PathRead {
    /// Its contents (or that it is missing).
    Contents,
    /// The names in the directory (or that it is missing).
    Listing,
    /// Whether it exists, and as a file or a directory.
    Kind,
}

impl Reads {
    /// Adds everything `other` saw.
    pub fn extend(&mut self, other: &Reads) {
        self.class_likes.extend(other.class_likes.iter().copied());
        self.functions.extend(other.functions.iter().copied());
        self.constants.extend(other.constants.iter().copied());
        self.descendants.extend(other.descendants.iter().copied());
        self.all |= other.all;
        self.files.extend(other.files.iter().copied());
        self.outlines.extend(other.outlines.iter().copied());
        for (key, answer) in &other.paths {
            keep_answer(&mut self.paths, key.clone(), answer.clone());
        }
    }
}

/// The last names each recorder took, by a hash of the name: a lookup repeats the same few names thousands of times,
/// and a name already in this table is already in the recording.
const RECENT: usize = 256;

thread_local! {
    static RECENT_CLASS_LIKES: RefCell<[Option<Word>; RECENT]> = const { RefCell::new([None; RECENT]) };
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static STACK: RefCell<Vec<Reads>> = const { RefCell::new(Vec::new()) };
}

/// Runs `f` and returns what it read alongside its result. Recordings nest: the reads of an inner recording also
/// count for the outer ones once it ends.
pub fn recording<R>(f: impl FnOnce() -> R) -> (R, Reads) {
    record_in(true, f)
}

/// [`recording`], without counting the reads for the recordings around it: for independent units of work that a
/// thread pool may run inside one another.
pub fn recording_alone<R>(f: impl FnOnce() -> R) -> (R, Reads) {
    record_in(false, f)
}

fn record_in<R>(merge: bool, f: impl FnOnce() -> R) -> (R, Reads) {
    /// Ends the recording when `f` unwinds, so a caught panic leaves the stack as it was.
    struct Unwind(bool);
    impl Drop for Unwind {
        fn drop(&mut self) {
            finish(self.0);
        }
    }
    STACK.with_borrow_mut(|s| s.push(Reads::default()));
    RECENT_CLASS_LIKES.with_borrow_mut(|r| *r = [None; RECENT]);
    ACTIVE.set(true);
    let unwind = Unwind(merge);
    let result = f();
    std::mem::forget(unwind);
    (result, finish(merge))
}

fn finish(merge: bool) -> Reads {
    RECENT_CLASS_LIKES.with_borrow_mut(|r| *r = [None; RECENT]);
    STACK.with_borrow_mut(|s| {
        let top = s.pop().unwrap_or_default();
        if merge && let Some(outer) = s.last_mut() {
            outer.extend(&top);
        }
        ACTIVE.set(!s.is_empty());
        top
    })
}

static SETUP_ON: AtomicBool = AtomicBool::new(false);
static SETUP: Mutex<Vec<((PathBuf, PathRead), Vec<u8>)>> = Mutex::new(Vec::new());

/// Runs `f` and returns the paths any thread read meanwhile outside a [`recording`], with their answers: what preparing
/// an analysis read, which every result depends on.
pub fn setup_recording<R>(f: impl FnOnce() -> R) -> (R, HashMap<(PathBuf, PathRead), Vec<u8>>) {
    struct Off;
    impl Drop for Off {
        fn drop(&mut self) {
            SETUP_ON.store(false, Ordering::SeqCst);
        }
    }
    SETUP.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    SETUP_ON.store(true, Ordering::SeqCst);
    let result = {
        let _off = Off;
        f()
    };
    let seen = std::mem::take(&mut *SETUP.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    let mut paths = HashMap::new();
    for (key, answer) in seen {
        keep_answer(&mut paths, key, answer);
    }
    (result, paths)
}

/// Whether a recording is active on this thread.
#[inline]
#[must_use]
pub fn active() -> bool {
    ACTIVE.get()
}

#[inline]
fn record(f: impl FnOnce(&mut Reads)) {
    if ACTIVE.get() {
        STACK.with_borrow_mut(|s| {
            if let Some(top) = s.last_mut() {
                f(top);
            }
        });
    }
}

/// Counts `reads` as read by the active recording: a memoized value replays what computing it read.
pub fn replay(reads: &Reads) {
    record(|r| r.extend(reads));
}

#[inline]
pub fn record_class_like(name: Word) {
    if !ACTIVE.get() {
        return;
    }
    let slot = {
        use std::hash::BuildHasher;
        WordSet::default().hasher().hash_one(name) as usize % RECENT
    };
    if RECENT_CLASS_LIKES.with_borrow(|r| r[slot] == Some(name)) {
        return;
    }
    record(|r| {
        r.class_likes.insert(name);
    });
    RECENT_CLASS_LIKES.with_borrow_mut(|r| r[slot] = Some(name));
}

#[inline]
pub fn record_function(name: Word) {
    record(|r| {
        r.functions.insert(name);
    });
}

#[inline]
pub fn record_constant(name: Word) {
    record(|r| {
        r.constants.insert(name);
    });
}

#[inline]
pub fn record_descendants(name: Word) {
    record(|r| {
        r.descendants.insert(name);
    });
}

#[inline]
pub fn record_all() {
    record(|r| r.all = true);
}

#[inline]
pub fn record_file(file: FileId) {
    record(|r| {
        r.files.insert(file);
    });
}

#[inline]
pub fn record_outline(file: FileId) {
    record(|r| {
        r.outlines.insert(file);
    });
}

/// Records that `path` was read as `read` and gave `answer`.
pub fn record_path(path: PathBuf, read: PathRead, answer: Vec<u8>) {
    if ACTIVE.get() {
        record(|r| keep_answer(&mut r.paths, (path, read), answer));
    } else if SETUP_ON.load(Ordering::Relaxed) {
        SETUP.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(((path, read), answer));
    }
}

/// Whether [`record_path`] keeps what it is given.
#[inline]
#[must_use]
pub fn paths_recorded() -> bool {
    ACTIVE.get() || SETUP_ON.load(Ordering::Relaxed)
}
