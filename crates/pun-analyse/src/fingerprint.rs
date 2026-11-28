//! What the populated codebase says about a symbol, as one hash that leaves out every source position: a class's
//! fingerprint changes when its declaration, its inherited members or a value resolved from another file changes, and
//! not when only a method body or the lines around it move.

use mago_codex::metadata::CodebaseMetadata;
use mago_word::Word;
use serde::ser::{self, Serialize};
use std::collections::HashMap;

pub(crate) type Fingerprint = [u8; 16];

/// The fingerprint of a symbol the codebase does not hold.
pub(crate) const MISSING: Fingerprint = [0; 16];

fn finish(h: &blake3::Hasher) -> Fingerprint {
    let mut out = [0; 16];
    out.copy_from_slice(&h.finalize().as_bytes()[..16]);
    // a present symbol never fingerprints as `MISSING`
    out[0] |= 1;
    out
}

/// The methods (and other members) the function-like table keys on each class, by lowercase class name.
pub(crate) struct Members<'a>(
    HashMap<Word, Vec<&'a mago_codex::metadata::function_like::FunctionLikeMetadata>>,
);

impl<'a> Members<'a> {
    pub(crate) fn of(codebase: &'a CodebaseMetadata) -> Members<'a> {
        let mut out: HashMap<Word, Vec<_>> = HashMap::new();
        // iterating the whole table outside any analysis: no recording should see it
        let ((), _) = mago_codex::reads::recording_alone(|| {
            for ((class, _), f) in codebase.function_likes.iter() {
                if !class.is_empty() {
                    out.entry(*class).or_default().push(f.as_ref());
                }
            }
        });
        for list in out.values_mut() {
            list.sort_by_key(|f| f.name);
        }
        Members(out)
    }
}

/// A class-like's fingerprint: its populated metadata and every function-like keyed on it.
pub(crate) fn class_like(
    codebase: &CodebaseMetadata,
    members: &Members,
    name: Word,
) -> Fingerprint {
    let Some(k) = codebase.get_class_like_by_name(name) else {
        return MISSING;
    };
    hashed(b"class", |h| {
        write(h, k)?;
        (members
            .0
            .get(&k.name.to_ascii_lowercase())
            .into_iter()
            .flatten())
        .try_for_each(|f| write(h, *f))
    })
}

pub(crate) fn function(codebase: &CodebaseMetadata, name: Word) -> Fingerprint {
    let Some(f) = codebase
        .get_function(name.as_bytes())
        .or_else(|| codebase.get_closure(&name))
    else {
        return MISSING;
    };
    hashed(b"function", |h| write(h, f))
}

pub(crate) fn constant(codebase: &CodebaseMetadata, name: Word) -> Fingerprint {
    let Some(k) = codebase.constants.get(&name) else {
        return MISSING;
    };
    hashed(b"constant", |h| write(h, k))
}

/// The sorted names of a class-like's descendants.
pub(crate) fn descendants(codebase: &CodebaseMetadata, name: Word) -> Fingerprint {
    let mut names: Vec<Word> = codebase
        .get_all_descendants(name.as_bytes())
        .into_iter()
        .collect();
    names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    let mut h = blake3::Hasher::new();
    h.update(b"descendants");
    for n in names {
        bytes(&mut h, n.as_bytes());
    }
    finish(&h)
}

/// The hash of `value` as the [`Hashing`] serializer sees it: its whole structure, every sequence and map in order
/// except the collections marked [`Unordered`].
pub(crate) fn structure(value: &impl Serialize) -> Fingerprint {
    hashed(b"", |h| write(h, value))
}

/// The fingerprint of what `feed` writes after `tag`, or [`unique`] when part of it would not hash.
fn hashed(
    tag: &[u8],
    feed: impl FnOnce(&mut blake3::Hasher) -> Result<(), Unhashable>,
) -> Fingerprint {
    let mut h = blake3::Hasher::new();
    h.update(tag);
    if feed(&mut h).is_err() {
        return unique();
    }
    finish(&h)
}

/// Marks a collection whose iteration order means nothing (a std `HashMap` or `HashSet`, seeded at random per
/// process): [`Hashing`] combines its elements so any order hashes alike. It wraps only the outer collection; a
/// collection inside an element needs its own mark ([`unordered_nested`]).
pub(crate) struct Unordered<'a, T: ?Sized>(pub(crate) &'a T);

/// The struct name [`Hashing`] reads as the mark of [`Unordered`].
const UNORDERED: &str = "PunUnordered";

impl<T: Serialize + ?Sized> Serialize for Unordered<'_, T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_struct(UNORDERED, self.0)
    }
}

/// `serialize_with` for a field holding an unordered collection.
pub(crate) fn unordered<T: Serialize + ?Sized, S: serde::Serializer>(
    value: &T,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    Unordered(value).serialize(serializer)
}

/// `serialize_with` for an unordered map whose values are unordered collections too (`HashMap<K, HashSet<V>>`,
/// `HashMap<K, HashMap<K2, V>>`).
pub(crate) fn unordered_nested<K: Serialize, C: Serialize, S: serde::Serializer>(
    value: &std::collections::HashMap<K, C>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    struct Inner<'a, K, C>(&'a std::collections::HashMap<K, C>);
    impl<K: Serialize, C: Serialize> Serialize for Inner<'_, K, C> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_map(self.0.iter().map(|(k, v)| (k, Unordered(v))))
        }
    }
    Unordered(&Inner(value)).serialize(serializer)
}

/// A path as the bytes the file system holds: a `PathBuf`'s own `Serialize` fails on one that is not UTF-8.
struct PathBytes<'a>(&'a std::path::Path);

impl Serialize for PathBytes<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.0.as_os_str().as_encoded_bytes())
    }
}

/// `serialize_with` for a path field.
pub(crate) fn path_bytes<S: serde::Serializer>(
    path: &std::path::Path,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    PathBytes(path).serialize(serializer)
}

/// `serialize_with` for a list of paths.
pub(crate) fn paths_bytes<S: serde::Serializer>(
    paths: &[std::path::PathBuf],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(paths.iter().map(|p| PathBytes(p)))
}

/// `serialize_with` for an ordered map keyed by path.
pub(crate) fn path_keyed<V: Serialize, S: serde::Serializer>(
    map: &std::collections::BTreeMap<std::path::PathBuf, V>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_map(map.iter().map(|(k, v)| (PathBytes(k), v)))
}

fn bytes(h: &mut blake3::Hasher, b: &[u8]) {
    h.update(&(b.len() as u64).to_le_bytes());
    h.update(b);
}

/// Feeds `value` into `h`; an error is a value that would not serialize (a `PathBuf` that is not UTF-8): the hash is
/// then incomplete, and the caller must not keep it ([`unique`]).
fn write<T: Serialize + ?Sized>(h: &mut blake3::Hasher, value: &T) -> Result<(), Unhashable> {
    value.serialize(Hashing {
        out: h,
        unordered: false,
    })
}

/// A fingerprint no record holds: what a symbol or a structure gets when part of it would not hash, so whatever depends
/// on it is analysed again instead of trusting an incomplete hash.
fn unique() -> Fingerprint {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut h = blake3::Hasher::new();
    h.update(b"unhashable");
    h.update(&std::process::id().to_le_bytes());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    h.update(&now.to_le_bytes());
    h.update(
        &COUNT
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_le_bytes(),
    );
    finish(&h)
}

/// A serializer that feeds a hasher: tagged, length-prefixed, so two different values never hash alike, except that
/// a struct named `Span` (a source position) is left out. Sequences and maps hash in order, as an `IndexMap`'s order
/// can decide a type (a list or an array shape); a collection marked [`Unordered`] hashes in any order.
struct Hashing<'h> {
    out: &'h mut blake3::Hasher,
    /// The next sequence or map is the one an [`Unordered`] mark wraps.
    unordered: bool,
}

/// A value that would not serialize (a `PathBuf` that is not UTF-8): serde's own error type, as nothing reads it.
type Unhashable = serde::de::value::Error;

impl Hashing<'_> {
    fn tag(&mut self, t: u8) {
        self.out.update(&[t]);
    }
}

macro_rules! number {
    ($name:ident, $t:ty, $tag:expr) => {
        fn $name(mut self, v: $t) -> Result<(), Unhashable> {
            self.tag($tag);
            self.out.update(&v.to_le_bytes());
            Ok(())
        }
    };
}

impl<'h> ser::Serializer for Hashing<'h> {
    type Ok = ();
    type Error = Unhashable;
    type SerializeSeq = Compound<'h>;
    type SerializeTuple = Compound<'h>;
    type SerializeTupleStruct = Compound<'h>;
    type SerializeTupleVariant = Compound<'h>;
    type SerializeMap = Compound<'h>;
    type SerializeStruct = Compound<'h>;
    type SerializeStructVariant = Compound<'h>;

    fn serialize_bool(mut self, v: bool) -> Result<(), Unhashable> {
        self.tag(1);
        self.out.update(&[u8::from(v)]);
        Ok(())
    }
    number!(serialize_i8, i8, 2);
    number!(serialize_i16, i16, 3);
    number!(serialize_i32, i32, 4);
    number!(serialize_i64, i64, 5);
    number!(serialize_i128, i128, 6);
    number!(serialize_u8, u8, 7);
    number!(serialize_u16, u16, 8);
    number!(serialize_u32, u32, 9);
    number!(serialize_u64, u64, 10);
    number!(serialize_u128, u128, 11);
    number!(serialize_f32, f32, 12);
    number!(serialize_f64, f64, 13);

    fn serialize_char(mut self, v: char) -> Result<(), Unhashable> {
        self.tag(14);
        self.out.update(&u32::from(v).to_le_bytes());
        Ok(())
    }

    fn serialize_str(mut self, v: &str) -> Result<(), Unhashable> {
        self.tag(15);
        bytes(self.out, v.as_bytes());
        Ok(())
    }

    fn serialize_bytes(mut self, v: &[u8]) -> Result<(), Unhashable> {
        self.tag(16);
        bytes(self.out, v);
        Ok(())
    }

    fn serialize_none(mut self) -> Result<(), Unhashable> {
        self.tag(17);
        Ok(())
    }

    fn serialize_some<T: Serialize + ?Sized>(mut self, value: &T) -> Result<(), Unhashable> {
        self.tag(18);
        value.serialize(self)
    }

    fn serialize_unit(mut self) -> Result<(), Unhashable> {
        self.tag(19);
        Ok(())
    }

    fn serialize_unit_struct(mut self, name: &'static str) -> Result<(), Unhashable> {
        self.tag(20);
        bytes(self.out, name.as_bytes());
        Ok(())
    }

    fn serialize_unit_variant(
        mut self,
        _: &'static str,
        index: u32,
        _: &'static str,
    ) -> Result<(), Unhashable> {
        self.tag(21);
        self.out.update(&index.to_le_bytes());
        Ok(())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        mut self,
        name: &'static str,
        value: &T,
    ) -> Result<(), Unhashable> {
        if name == UNORDERED {
            self.tag(31);
            return value.serialize(Hashing {
                out: self.out,
                unordered: true,
            });
        }
        self.tag(22);
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        mut self,
        _: &'static str,
        index: u32,
        _: &'static str,
        value: &T,
    ) -> Result<(), Unhashable> {
        self.tag(23);
        self.out.update(&index.to_le_bytes());
        value.serialize(self)
    }

    fn serialize_seq(mut self, len: Option<usize>) -> Result<Compound<'h>, Unhashable> {
        self.tag(24);
        self.out
            .update(&(len.unwrap_or(usize::MAX) as u64).to_le_bytes());
        Ok(Compound::new(self.out, false, self.unordered))
    }

    fn serialize_tuple(mut self, len: usize) -> Result<Compound<'h>, Unhashable> {
        self.tag(25);
        self.out.update(&(len as u64).to_le_bytes());
        Ok(Compound::new(self.out, false, false))
    }

    fn serialize_tuple_struct(
        mut self,
        _: &'static str,
        _: usize,
    ) -> Result<Compound<'h>, Unhashable> {
        self.tag(26);
        Ok(Compound::new(self.out, false, false))
    }

    fn serialize_tuple_variant(
        mut self,
        _: &'static str,
        index: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Compound<'h>, Unhashable> {
        self.tag(27);
        self.out.update(&index.to_le_bytes());
        Ok(Compound::new(self.out, false, false))
    }

    fn serialize_map(mut self, len: Option<usize>) -> Result<Compound<'h>, Unhashable> {
        self.tag(28);
        self.out
            .update(&(len.unwrap_or(usize::MAX) as u64).to_le_bytes());
        Ok(Compound::new(self.out, false, self.unordered))
    }

    fn serialize_struct(
        mut self,
        name: &'static str,
        _: usize,
    ) -> Result<Compound<'h>, Unhashable> {
        let skip = name == "Span";
        if !skip {
            self.tag(29);
        }
        Ok(Compound::new(self.out, skip, false))
    }

    fn serialize_struct_variant(
        mut self,
        _: &'static str,
        index: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Compound<'h>, Unhashable> {
        self.tag(30);
        self.out.update(&index.to_le_bytes());
        Ok(Compound::new(self.out, false, false))
    }
}

/// The parts of a sequence, tuple, struct or map, hashed in order; for a collection marked [`Unordered`], each element
/// (a map's key and value together) hashed on its own and the hashes summed, so the order does not matter.
struct Compound<'h> {
    out: &'h mut blake3::Hasher,
    skip: bool,
    /// The sum of the elements' hashes, and the element being hashed, for an unordered collection.
    unordered: Option<(u128, blake3::Hasher)>,
}

impl<'h> Compound<'h> {
    fn new(out: &'h mut blake3::Hasher, skip: bool, unordered: bool) -> Self {
        Compound {
            out,
            skip,
            unordered: unordered.then(|| (0, blake3::Hasher::new())),
        }
    }

    fn part<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unhashable> {
        if self.skip {
            return Ok(());
        }
        match &mut self.unordered {
            Some((_, entry)) => write(entry, value),
            None => write(self.out, value),
        }
    }

    /// Ends one element of an unordered collection: its hash joins the sum.
    fn element_done(&mut self) {
        if let Some((sum, entry)) = &mut self.unordered {
            let mut out = [0; 16];
            out.copy_from_slice(&entry.finalize().as_bytes()[..16]);
            *sum = sum.wrapping_add(u128::from_le_bytes(out));
            *entry = blake3::Hasher::new();
        }
    }

    fn end(self) {
        if let Some((sum, _)) = self.unordered {
            self.out.update(&sum.to_le_bytes());
        }
    }
}

impl ser::SerializeSeq for Compound<'_> {
    type Ok = ();
    type Error = Unhashable;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unhashable> {
        self.part(value)?;
        self.element_done();
        Ok(())
    }
    fn end(self) -> Result<(), Unhashable> {
        Compound::end(self);
        Ok(())
    }
}

impl ser::SerializeTuple for Compound<'_> {
    type Ok = ();
    type Error = Unhashable;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unhashable> {
        self.part(value)?;
        Ok(())
    }
    fn end(self) -> Result<(), Unhashable> {
        Ok(())
    }
}

impl ser::SerializeTupleStruct for Compound<'_> {
    type Ok = ();
    type Error = Unhashable;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unhashable> {
        self.part(value)?;
        Ok(())
    }
    fn end(self) -> Result<(), Unhashable> {
        Ok(())
    }
}

impl ser::SerializeTupleVariant for Compound<'_> {
    type Ok = ();
    type Error = Unhashable;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unhashable> {
        self.part(value)?;
        Ok(())
    }
    fn end(self) -> Result<(), Unhashable> {
        Ok(())
    }
}

impl ser::SerializeMap for Compound<'_> {
    type Ok = ();
    type Error = Unhashable;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Unhashable> {
        self.part(key)?;
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unhashable> {
        self.part(value)?;
        self.element_done();
        Ok(())
    }
    fn end(self) -> Result<(), Unhashable> {
        Compound::end(self);
        Ok(())
    }
}

impl ser::SerializeStruct for Compound<'_> {
    type Ok = ();
    type Error = Unhashable;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Unhashable> {
        if !self.skip {
            bytes(self.out, key.as_bytes());
        }
        self.part(value)?;
        Ok(())
    }
    fn end(self) -> Result<(), Unhashable> {
        Ok(())
    }
}

impl ser::SerializeStructVariant for Compound<'_> {
    type Ok = ();
    type Error = Unhashable;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Unhashable> {
        bytes(self.out, key.as_bytes());
        self.part(value)?;
        Ok(())
    }
    fn end(self) -> Result<(), Unhashable> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[derive(serde::Serialize)]
    struct Tables {
        #[serde(serialize_with = "unordered_nested")]
        columns: HashMap<String, HashSet<String>>,
    }

    fn tables(entries: &[(&str, &[&str])]) -> Tables {
        Tables {
            columns: (entries.iter())
                .map(|(t, cs)| (t.to_string(), cs.iter().map(|c| c.to_string()).collect()))
                .collect(),
        }
    }

    #[test]
    fn unordered_collections_hash_alike_in_any_order_at_every_level() {
        let a = tables(&[
            ("users", &["id", "email", "name"]),
            ("posts", &["id", "title"]),
        ]);
        let b = tables(&[
            ("posts", &["title", "id"]),
            ("users", &["name", "id", "email"]),
        ]);
        assert_eq!(structure(&a), structure(&b));
    }

    #[test]
    fn a_value_moved_to_another_entry_changes_the_hash() {
        let a = tables(&[("users", &["id", "email"]), ("profiles", &["id", "bio"])]);
        let b = tables(&[("users", &["id"]), ("profiles", &["id", "bio", "email"])]);
        assert_ne!(structure(&a), structure(&b));
    }

    /// A map that serializes its entries in insertion order, as an `IndexMap` (mago's `KnownItems`) does.
    struct Ordered(Vec<(i32, i32)>);

    impl Serialize for Ordered {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.collect_map(self.0.iter().map(|(k, v)| (k, v)))
        }
    }

    /// A path that is not UTF-8 hashes by its bytes where it is marked so, and otherwise makes the fingerprint one no
    /// record holds, never a truncated hash that collides with another path's.
    #[test]
    fn a_path_that_is_not_utf8_never_collides() {
        use std::os::unix::ffi::OsStrExt;
        let path = |b: &[u8]| std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b));
        #[derive(serde::Serialize)]
        struct Plain(std::path::PathBuf);
        #[derive(serde::Serialize)]
        struct Bytes(#[serde(serialize_with = "path_bytes")] std::path::PathBuf);
        assert_ne!(
            structure(&Plain(path(b"\xffa"))),
            structure(&Plain(path(b"\xffa")))
        );
        assert_eq!(
            structure(&Bytes(path(b"\xffa"))),
            structure(&Bytes(path(b"\xffa")))
        );
        assert_ne!(
            structure(&Bytes(path(b"\xffa"))),
            structure(&Bytes(path(b"\xffb")))
        );
    }

    /// Every shape serde hands the hasher is tagged: values that differ only in it never hash alike.
    #[test]
    fn every_shape_hashes_apart() {
        #[derive(serde::Serialize)]
        struct Pair(u8, u8);
        assert_ne!(structure(&'a'), structure(&'b'));
        assert_ne!(structure(&()), structure(&None::<u8>));
        assert_ne!(structure(&Pair(1, 2)), structure(&Pair(2, 1)));
        assert_ne!(structure(&Pair(1, 2)), structure(&(1u8, 2u8)));
    }

    #[test]
    fn an_ordered_map_hashes_its_order() {
        let a = Ordered(vec![(0, 1), (1, 2)]);
        let b = Ordered(vec![(1, 2), (0, 1)]);
        assert_ne!(structure(&a), structure(&b));
    }
}
