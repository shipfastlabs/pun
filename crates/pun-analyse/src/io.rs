//! File system reads that the active recording (`mago_codex::reads`) sees, so a cached result that depended on a
//! path is checked again when that path changes. Each read is recorded with the answer it got at that moment, as
//! [`fingerprint`] would give it, so a change during the run is not taken for what the run saw. With no recording
//! active they are the plain `std` calls.

use std::path::Path;

use mago_codex::reads::{self, PathRead};

fn seen(path: &Path, read: PathRead, answer: impl FnOnce() -> Vec<u8>) {
    if reads::paths_recorded() {
        reads::record_path(path.to_path_buf(), read, answer());
    }
}

pub(crate) fn read(path: impl AsRef<Path>) -> std::io::Result<Vec<u8>> {
    let path = path.as_ref();
    let bytes = std::fs::read(path);
    seen(path, PathRead::Contents, || {
        contents_answer(bytes.as_deref().ok())
    });
    bytes
}

pub(crate) fn read_to_string(path: impl AsRef<Path>) -> std::io::Result<String> {
    let path = path.as_ref();
    let bytes = std::fs::read(path);
    seen(path, PathRead::Contents, || {
        contents_answer(bytes.as_deref().ok())
    });
    bytes.and_then(|b| {
        String::from_utf8(b).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })
}

/// The directory's entries, read once: the recorded answer comes from the very entries the caller gets, so a change
/// between two reads cannot record a listing the caller never saw.
pub(crate) fn read_dir(
    path: impl AsRef<Path>,
) -> std::io::Result<std::vec::IntoIter<std::io::Result<std::fs::DirEntry>>> {
    let path = path.as_ref();
    let entries: std::io::Result<Vec<_>> = std::fs::read_dir(path).map(Iterator::collect);
    seen(path, PathRead::Listing, || {
        entries
            .as_ref()
            .map_or_else(|_| Vec::new(), |e| listing_answer(e))
    });
    entries.map(Vec::into_iter)
}

/// `Path::is_file`, `is_dir` and `exists` are each `metadata()` and a test on it: one recorded `Kind` read.
pub(crate) fn is_file(path: &(impl AsRef<Path> + ?Sized)) -> bool {
    kind(path.as_ref()).is_ok_and(|m| m.is_file())
}

pub(crate) fn is_dir(path: &(impl AsRef<Path> + ?Sized)) -> bool {
    kind(path.as_ref()).is_ok_and(|m| m.is_dir())
}

pub(crate) fn exists(path: &(impl AsRef<Path> + ?Sized)) -> bool {
    kind(path.as_ref()).is_ok()
}

/// The path's metadata, recorded as a `Kind` read with the answer it gives.
fn kind(path: &Path) -> std::io::Result<std::fs::Metadata> {
    let metadata = std::fs::metadata(path);
    seen(path, PathRead::Kind, || kind_answer(metadata.as_ref().ok()));
    metadata
}

fn contents_answer(bytes: Option<&[u8]>) -> Vec<u8> {
    bytes
        .map(|b| blake3::hash(b).as_bytes().to_vec())
        .unwrap_or_default()
}

/// What `is_file`, `is_dir` and `exists` can tell apart: a regular file, a directory, anything else that exists (a
/// FIFO, a socket, a device), or nothing.
fn kind_answer(metadata: Option<&std::fs::Metadata>) -> Vec<u8> {
    metadata.map_or_else(Vec::new, |m| entry_kind(m.file_type()).to_vec())
}

/// What an entry is: a directory, a regular file, a symbolic link (only an entry not followed) or anything else.
fn entry_kind(t: std::fs::FileType) -> &'static [u8] {
    if t.is_symlink() {
        b"link"
    } else if t.is_dir() {
        b"dir"
    } else if t.is_file() {
        b"file"
    } else {
        b"other"
    }
}

/// The hash of a directory's entries, sorted by name: each name's bytes and its kind (`DirEntry::file_type`, which the
/// callers read to tell a directory from a file), so an entry that changes kind changes the answer.
fn listing_answer(entries: &[std::io::Result<std::fs::DirEntry>]) -> Vec<u8> {
    let mut seen: Vec<(Vec<u8>, &[u8])> = entries
        .iter()
        .flatten()
        .map(|e| {
            let kind = e.file_type().map_or(&b"?"[..], entry_kind);
            (e.file_name().as_encoded_bytes().to_vec(), kind)
        })
        .collect();
    seen.sort();
    let mut h = blake3::Hasher::new();
    h.update(b"dir");
    for (name, kind) in seen {
        h.update(&(name.len() as u64).to_le_bytes());
        h.update(&name);
        h.update(kind);
    }
    h.finalize().as_bytes().to_vec()
}

/// What a read of `path` gives now, as the recorded reads give it: `Contents` its BLAKE3 hash (nothing when it cannot be
/// read), `Listing` [`listing_answer`], `Kind` [`kind_answer`]. Never [`reads::RACY`].
pub fn fingerprint(path: &Path, read: PathRead) -> Vec<u8> {
    match read {
        PathRead::Contents => contents_answer(std::fs::read(path).ok().as_deref()),
        PathRead::Listing => std::fs::read_dir(path).map_or_else(
            |_| Vec::new(),
            |entries| listing_answer(&entries.collect::<Vec<_>>()),
        ),
        PathRead::Kind => kind_answer(std::fs::metadata(path).ok().as_ref()),
    }
}
