//! Pun: a codebase table that tells [`crate::reads`] which keys an analysis looked up.
//!
//! `get`, `get_key_value` and `contains_key` record the key under the table's kind. Every other read goes through
//! `Deref` to the map and records that the whole table was read, so a read nobody taught to record still counts.
//! Writes go through `DerefMut` and record nothing: the codebase is only written while it is built.

use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use mago_word::{Word, WordMap};

use crate::reads;

/// Which recorder a table's lookups go to.
pub trait Kind {
    fn record(key: Word);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClassLikes;

impl Kind for ClassLikes {
    #[inline]
    fn record(key: Word) {
        reads::record_class_like(key);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Constants;

impl Kind for Constants {
    #[inline]
    fn record(key: Word) {
        reads::record_constant(key);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Descendants;

impl Kind for Descendants {
    #[inline]
    fn record(key: Word) {
        reads::record_descendants(key);
    }
}

pub struct Recorded<V, K> {
    map: WordMap<V>,
    kind: PhantomData<K>,
}

impl<V, K: Kind> Recorded<V, K> {
    #[inline]
    pub fn get(&self, key: &Word) -> Option<&V> {
        K::record(*key);
        self.map.get(key)
    }

    #[inline]
    pub fn get_key_value(&self, key: &Word) -> Option<(&Word, &V)> {
        K::record(*key);
        self.map.get_key_value(key)
    }

    #[inline]
    pub fn contains_key(&self, key: &Word) -> bool {
        K::record(*key);
        self.map.contains_key(key)
    }

    /// The map, without recording: for code that knows its read does not depend on what the table holds.
    #[inline]
    pub fn unrecorded(&self) -> &WordMap<V> {
        &self.map
    }
}

impl<V, K> Deref for Recorded<V, K> {
    type Target = WordMap<V>;

    #[inline]
    fn deref(&self) -> &WordMap<V> {
        reads::record_all();
        &self.map
    }
}

impl<V, K> DerefMut for Recorded<V, K> {
    #[inline]
    fn deref_mut(&mut self) -> &mut WordMap<V> {
        &mut self.map
    }
}

impl<V, K> Default for Recorded<V, K> {
    fn default() -> Self {
        Self { map: WordMap::default(), kind: PhantomData }
    }
}

impl<V: Clone, K> Clone for Recorded<V, K> {
    fn clone(&self) -> Self {
        Self { map: self.map.clone(), kind: PhantomData }
    }
}

impl<V: std::fmt::Debug, K> std::fmt::Debug for Recorded<V, K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.map.fmt(f)
    }
}

impl<V: PartialEq, K> PartialEq for Recorded<V, K> {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map
    }
}

impl<V: Eq, K> Eq for Recorded<V, K> {}

impl<V, K> From<WordMap<V>> for Recorded<V, K> {
    fn from(map: WordMap<V>) -> Self {
        Self { map, kind: PhantomData }
    }
}

impl<V, K> FromIterator<(Word, V)> for Recorded<V, K> {
    fn from_iter<I: IntoIterator<Item = (Word, V)>>(iter: I) -> Self {
        WordMap::from_iter(iter).into()
    }
}

impl<V, K> Extend<(Word, V)> for Recorded<V, K> {
    fn extend<I: IntoIterator<Item = (Word, V)>>(&mut self, iter: I) {
        self.map.extend(iter);
    }
}

impl<V, K> IntoIterator for Recorded<V, K> {
    type Item = (Word, V);
    type IntoIter = <WordMap<V> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.map.into_iter()
    }
}

impl<'a, V, K> IntoIterator for &'a Recorded<V, K> {
    type Item = (&'a Word, &'a V);
    type IntoIter = std::collections::hash_map::Iter<'a, Word, V>;

    fn into_iter(self) -> Self::IntoIter {
        reads::record_all();
        self.map.iter()
    }
}

impl<'a, V, K> IntoIterator for &'a mut Recorded<V, K> {
    type Item = (&'a Word, &'a mut V);
    type IntoIter = std::collections::hash_map::IterMut<'a, Word, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.map.iter_mut()
    }
}

impl<V, K: Kind> std::ops::Index<&Word> for Recorded<V, K> {
    type Output = V;

    fn index(&self, key: &Word) -> &V {
        K::record(*key);
        &self.map[key]
    }
}

#[cfg(feature = "serde")]
impl<V: serde::Serialize, K> serde::Serialize for Recorded<V, K> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.map.serialize(serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de, V: serde::Deserialize<'de>, K> serde::Deserialize<'de> for Recorded<V, K> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        WordMap::deserialize(deserializer).map(Into::into)
    }
}
