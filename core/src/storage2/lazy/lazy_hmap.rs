// Copyright 2019-2020 Parity Technologies (UK) Ltd.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::{
    CacheCell,
    Entry,
    EntryState,
};
use crate::{
    hash::{
        hasher::Hasher,
        HashBuilder,
    },
    storage2::traits::{
        clear_packed_root,
        pull_packed_root_opt,
        ExtKeyPtr,
        KeyPtr,
        PackedLayout,
        SpreadLayout,
    },
};
use core::{
    borrow::Borrow,
    cell::RefCell,
    cmp::{
        Eq,
        Ord,
    },
    fmt,
    fmt::Debug,
    ptr::NonNull,
};
use ink_prelude::{
    borrow::ToOwned,
    boxed::Box,
    collections::BTreeMap,
    vec::Vec,
};
use ink_primitives::Key;

/// The map for the contract storage entries.
///
/// # Note
///
/// We keep the whole entry in a `Box<T>` in order to prevent pointer
/// invalidation upon updating the cache through `&self` methods as in
/// [`LazyMap::get`].
pub type EntryMap<K, V> = BTreeMap<K, Box<Entry<V>>>;

/// A lazy storage mapping that stores entries under their SCALE encoded key hashes.
///
/// # Note
///
/// This is mainly used as low-level storage primitives by other high-level
/// storage primitives in order to manage the contract storage for a whole
/// mapping of storage cells.
///
/// This storage data structure might store its entires anywhere in the contract
/// storage. It is the users responsibility to keep track of the entries if it
/// is necessary to do so.
pub struct LazyHashMap<K, V, H> {
    /// The offset key for the storage mapping.
    ///
    /// This offsets the mapping for the entries stored in the contract storage
    /// so that all lazy hash map instances store equal entries at different
    /// locations of the contract storage and avoid collissions.
    key: Option<Key>,
    /// The currently cached entries of the lazy storage mapping.
    ///
    /// This normally only represents a subset of the total set of elements.
    /// An entry is cached as soon as it is loaded or written.
    cached_entries: CacheCell<EntryMap<K, V>>,
    /// The used hash builder.
    hash_builder: RefCell<HashBuilder<H, Vec<u8>>>,
}

struct DebugEntryMap<'a, K, V>(&'a CacheCell<EntryMap<K, V>>);

impl<'a, K, V> Debug for DebugEntryMap<'a, K, V>
where
    K: Debug,
    V: Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_map().entries(self.0.as_inner().iter()).finish()
    }
}

impl<K, V, H> Debug for LazyHashMap<K, V, H>
where
    K: Debug,
    V: Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // The `hash_builder` field is not really required or needed for debugging purposes.
        f.debug_struct("LazyHashMap")
            .field("key", &self.key)
            .field("cached_entries", &DebugEntryMap(&self.cached_entries))
            .finish()
    }
}

#[test]
fn debug_impl_works() {
    use crate::hash::hasher::Blake2x256Hasher;
    let mut hmap = <LazyHashMap<char, i32, Blake2x256Hasher>>::new();
    // Empty hmap.
    assert_eq!(
        format!("{:?}", &hmap),
        "LazyHashMap { key: None, cached_entries: {} }",
    );
    // Filled hmap.
    hmap.put('A', Some(1));
    hmap.put('B', Some(2));
    hmap.put('C', None);
    assert_eq!(
        format!("{:?}", &hmap),
        "LazyHashMap { \
            key: None, \
            cached_entries: {\
                'A': Entry { \
                    value: Some(1), \
                    state: Mutated \
                }, \
                'B': Entry { \
                    value: Some(2), \
                    state: Mutated \
                }, \
                'C': Entry { \
                    value: None, \
                    state: Mutated \
                }\
            } \
        }",
    );
}

#[cfg(feature = "std")]
const _: () = {
    use crate::storage2::traits::{
        LayoutCryptoHasher,
        StorageLayout,
    };
    use ink_abi::layout2::{
        CellLayout,
        HashLayout,
        HashingStrategy,
        Layout,
        LayoutKey,
    };
    use scale_info::Metadata;

    impl<K, V, H> StorageLayout for LazyHashMap<K, V, H>
    where
        K: Ord + scale::Encode,
        V: Metadata,
        H: Hasher + LayoutCryptoHasher,
        Key: From<<H as Hasher>::Output>,
    {
        fn layout(key_ptr: &mut KeyPtr) -> Layout {
            Layout::Hash(HashLayout::new(
                LayoutKey::from(key_ptr.advance_by(1)),
                HashingStrategy::new(
                    <H as LayoutCryptoHasher>::crypto_hasher(),
                    b"ink hashmap".to_vec(),
                    Vec::new(),
                ),
                Layout::Cell(CellLayout::new::<V>(LayoutKey::from(
                    key_ptr.advance_by(0),
                ))),
            ))
        }
    }
};

impl<K, V, H> SpreadLayout for LazyHashMap<K, V, H>
where
    K: Ord + scale::Encode,
    V: PackedLayout,
    H: Hasher,
    Key: From<<H as Hasher>::Output>,
{
    const FOOTPRINT: u64 = 1;

    fn pull_spread(ptr: &mut KeyPtr) -> Self {
        Self::lazy(ExtKeyPtr::next_for::<Self>(ptr))
    }

    fn push_spread(&self, ptr: &mut KeyPtr) {
        let offset_key = ExtKeyPtr::next_for::<Self>(ptr);
        for (index, entry) in self.entries().iter() {
            let root_key = self.to_offset_key(&offset_key, index);
            entry.push_packed_root(&root_key);
        }
    }

    #[inline]
    fn clear_spread(&self, _ptr: &mut KeyPtr) {
        // Low-level lazy abstractions won't perform automated clean-up since
        // they generally are not aware of their entire set of associated
        // elements. The high-level abstractions that build upon them are
        // responsible for cleaning up.
    }
}

// # Developer Note
//
// Even thought `LazyHashMap` would require storing just a single key a thus
// be a packable storage entity we cannot really make it one since this could
// allow for overlapping lazy hash map instances.
// An example for this would be a `Pack<(LazyHashMap, LazyHashMap)>` where
// both lazy hash maps would use the same underlying key and thus would apply
// the same underlying key mapping.

impl<K, V, H> Default for LazyHashMap<K, V, H>
where
    K: Ord,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V, H> LazyHashMap<K, V, H>
where
    K: Ord,
{
    /// Creates a new empty lazy hash map.
    ///
    /// # Note
    ///
    /// A lazy map created this way cannot be used to load from the contract storage.
    /// All operations that directly or indirectly load from storage will panic.
    pub fn new() -> Self {
        Self {
            key: None,
            cached_entries: CacheCell::new(EntryMap::new()),
            hash_builder: RefCell::new(HashBuilder::from(Vec::new())),
        }
    }

    /// Creates a new empty lazy hash map positioned at the given key.
    ///
    /// # Note
    ///
    /// This constructor is private and should never need to be called from
    /// outside this module. It is used to construct a lazy index map from a
    /// key that is only useful upon a contract call. Use [`LazyIndexMap::new`]
    /// for construction during contract initialization.
    fn lazy(key: Key) -> Self {
        Self {
            key: Some(key),
            cached_entries: CacheCell::new(EntryMap::new()),
            hash_builder: RefCell::new(HashBuilder::from(Vec::new())),
        }
    }

    /// Returns the offset key of the lazy map if any.
    pub fn key(&self) -> Option<&Key> {
        self.key.as_ref()
    }

    /// Returns a shared reference to the underlying entries.
    fn entries(&self) -> &EntryMap<K, V> {
        self.cached_entries.as_inner()
    }

    /// Returns an exclusive reference to the underlying entries.
    fn entries_mut(&mut self) -> &mut EntryMap<K, V> {
        self.cached_entries.as_inner_mut()
    }

    /// Puts the new value under the given key.
    ///
    /// # Note
    ///
    /// - Use [`LazyHashMap::put`]`(None)` in order to remove an element.
    /// - Prefer this method over [`LazyHashMap::put_get`] if you are not interested
    ///   in the old value of the same cell index.
    ///
    /// # Panics
    ///
    /// - If the lazy hash map is in an invalid state that forbids interaction
    ///   with the underlying contract storage.
    /// - If the decoding of the old element at the given index failed.
    pub fn put(&mut self, key: K, new_value: Option<V>) {
        self.entries_mut()
            .insert(key, Box::new(Entry::new(new_value, EntryState::Mutated)));
    }
}

impl<K, V, H> LazyHashMap<K, V, H>
where
    K: Ord + scale::Encode,
    H: Hasher,
    Key: From<<H as Hasher>::Output>,
{
    /// Returns an offset key for the given key pair.
    fn to_offset_key<Q>(&self, storage_key: &Key, key: &Q) -> Key
    where
        K: Borrow<Q>,
        Q: scale::Encode,
    {
        #[derive(scale::Encode)]
        struct KeyPair<'a, Q> {
            prefix: [u8; 11],
            storage_key: &'a Key,
            value_key: &'a Q,
        }
        let key_pair = KeyPair {
            prefix: [
                b'i', b'n', b'k', b' ', b'h', b'a', b's', b'h', b'm', b'a', b'p',
            ],
            storage_key,
            value_key: key,
        };
        self.hash_builder
            .borrow_mut()
            .hash_encoded(&key_pair)
            .into()
    }

    /// Returns an offset key for the given key.
    fn key_at<Q>(&self, key: &Q) -> Option<Key>
    where
        K: Borrow<Q>,
        Q: scale::Encode,
    {
        self.key
            .map(|storage_key| self.to_offset_key(&storage_key, key))
    }
}

impl<K, V, H> LazyHashMap<K, V, H>
where
    K: Ord + Eq + scale::Encode,
    V: PackedLayout,
    H: Hasher,
    Key: From<<H as Hasher>::Output>,
{
    /// Lazily loads the value at the given index.
    ///
    /// # Note
    ///
    /// Only loads a value if `key` is set and if the value has not been loaded yet.
    /// Returns the freshly loaded or already loaded entry of the value.
    ///
    /// # Safety
    ///
    /// This function has a `&self` receiver while returning an `Option<*mut T>`
    /// which is unsafe in isolation. The caller has to determine how to forward
    /// the returned `*mut T`.
    ///
    /// # Safety
    ///
    /// This is an `unsafe` operation because it has a `&self` receiver but returns
    /// a `*mut Entry<T>` pointer that allows for exclusive access. This is safe
    /// within internal use only and should never be given outside of the lazy
    /// entity for public `&self` methods.
    unsafe fn lazily_load<Q>(&self, key: &Q) -> NonNull<Entry<V>>
    where
        K: Borrow<Q>,
        Q: Ord + scale::Encode + ToOwned<Owned = K>,
    {
        // SAFETY: We have put the whole `cached_entries` mapping into an
        //         `UnsafeCell` because of this caching functionality. The
        //         trick here is that due to using `Box<T>` internally
        //         we are able to return references to the cached entries
        //         while maintaining the invariant that mutating the caching
        //         `BTreeMap` will never invalidate those references.
        //         By returning a raw pointer we enforce an `unsafe` block at
        //         the caller site to underline that guarantees are given by the
        //         caller.
        let cached_entries = &mut *self.cached_entries.get_ptr().as_ptr();
        use ink_prelude::collections::btree_map::Entry as BTreeMapEntry;
        // We have to clone the key here because we do not have access to the unsafe
        // raw entry API for Rust hash maps, yet since it is unstable. We can remove
        // the contraints on `K: Clone` once we have access to this API.
        // Read more about the issue here: https://github.com/rust-lang/rust/issues/56167
        match cached_entries.entry(key.to_owned()) {
            BTreeMapEntry::Occupied(occupied) => {
                NonNull::from(&mut **occupied.into_mut())
            }
            BTreeMapEntry::Vacant(vacant) => {
                let value = self
                    .key_at(key)
                    .map(|key| pull_packed_root_opt::<V>(&key))
                    .unwrap_or(None);
                NonNull::from(
                    &mut **vacant
                        .insert(Box::new(Entry::new(value, EntryState::Preserved))),
                )
            }
        }
    }

    /// Lazily loads the value associated with the given key.
    ///
    /// # Note
    ///
    /// Only loads a value if `key` is set and if the value has not been loaded yet.
    /// Returns a pointer to the freshly loaded or already loaded entry of the value.
    ///
    /// # Panics
    ///
    /// - If the lazy chunk is in an invalid state that forbids interaction.
    /// - If the lazy chunk is not in a state that allows lazy loading.
    fn lazily_load_mut<Q>(&mut self, index: &Q) -> &mut Entry<V>
    where
        K: Borrow<Q>,
        Q: Ord + scale::Encode + ToOwned<Owned = K>,
    {
        // SAFETY:
        // - Returning a `&mut Entry<T>` is safe because entities inside the
        //   cache are stored within a `Box` to not invalidate references into
        //   them upon operating on the outer cache.
        unsafe { &mut *self.lazily_load(index).as_ptr() }
    }

    /// Clears the underlying storage of the entry at the given index.
    ///
    /// # Safety
    ///
    /// For performance reasons this does not synchronize the lazy index map's
    /// memory-side cache which invalidates future accesses the cleared entry.
    /// Care should be taken when using this API.
    ///
    /// The general use of this API is to streamline `Drop` implementations of
    /// high-level abstractions that build upon this low-level data strcuture.
    pub fn clear_packed_at<Q>(&self, index: &Q)
    where
        K: Borrow<Q>,
        V: PackedLayout,
        Q: Ord + scale::Encode + ToOwned<Owned = K>,
    {
        let root_key = self.key_at(index).expect("cannot clear in lazy state");
        if <V as SpreadLayout>::REQUIRES_DEEP_CLEAN_UP {
            // We need to load the entity before we remove its associated contract storage
            // because it requires a deep clean-up which propagates clearing to its fields,
            // for example in the case of `T` being a `storage::Box`.
            let entity = self.get(index).expect("cannot clear a non existing entity");
            clear_packed_root::<V>(&entity, &root_key);
        } else {
            // The type does not require deep clean-up so we can simply clean-up
            // its associated storage cell and be done without having to load it first.
            crate::env::clear_contract_storage(root_key);
        }
    }

    /// Returns a shared reference to the value associated with the given key if any.
    ///
    /// # Panics
    ///
    /// - If the lazy chunk is in an invalid state that forbids interaction.
    /// - If the decoding of the element at the given index failed.
    pub fn get<Q>(&self, index: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + scale::Encode + ToOwned<Owned = K>,
    {
        // SAFETY: Dereferencing the `*mut T` pointer into a `&T` is safe
        //         since this method's receiver is `&self` so we do not
        //         leak non-shared references to the outside.
        unsafe { &*self.lazily_load(index).as_ptr() }.value().into()
    }

    /// Returns an exclusive reference to the value associated with the given key if any.
    ///
    /// # Panics
    ///
    /// - If the lazy chunk is in an invalid state that forbids interaction.
    /// - If the decoding of the element at the given index failed.
    pub fn get_mut<Q>(&mut self, index: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Ord + scale::Encode + ToOwned<Owned = K>,
    {
        self.lazily_load_mut(index).value_mut().into()
    }

    /// Puts the new value under the given key and returns the old value if any.
    ///
    /// # Note
    ///
    /// - Use [`LazyHashMap::put_get`]`(None)` in order to remove an element
    ///   and retrieve the old element back.
    ///
    /// # Panics
    ///
    /// - If the lazy hashmap is in an invalid state that forbids interaction.
    /// - If the decoding of the old element at the given index failed.
    pub fn put_get<Q>(&mut self, key: &Q, new_value: Option<V>) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Ord + scale::Encode + ToOwned<Owned = K>,
    {
        self.lazily_load_mut(key).put(new_value)
    }

    /// Swaps the values at entries with associated keys `x` and `y`.
    ///
    /// This operation tries to be as efficient as possible and reuse allocations.
    ///
    /// # Panics
    ///
    /// - If the lazy hashmap is in an invalid state that forbids interaction.
    /// - If the decoding of one of the elements failed.
    pub fn swap<Q1, Q2>(&mut self, x: &Q1, y: &Q2)
    where
        K: Borrow<Q1> + Borrow<Q2>,
        Q1: Ord + PartialEq<Q2> + scale::Encode + ToOwned<Owned = K>,
        Q2: Ord + PartialEq<Q1> + scale::Encode + ToOwned<Owned = K>,
    {
        if x == y {
            // Bail out early if both indices are the same.
            return
        }
        let (loaded_x, loaded_y) =
            // SAFETY: The loaded `x` and `y` entries are distinct from each
            //         other guaranteed by the previous check. Also `lazily_load`
            //         guarantees to return a pointer to a pinned entity
            //         so that the returned references do not conflict with
            //         each other.
            unsafe { (
                &mut *self.lazily_load(x).as_ptr(),
                &mut *self.lazily_load(y).as_ptr(),
            ) };
        if loaded_x.value().is_none() && loaded_y.value().is_none() {
            // Bail out since nothing has to be swapped if both values are `None`.
            return
        }
        // Set the `mutate` flag since at this point at least one of the loaded
        // values is guaranteed to be `Some`.
        loaded_x.replace_state(EntryState::Mutated);
        loaded_y.replace_state(EntryState::Mutated);
        core::mem::swap(loaded_x.value_mut(), loaded_y.value_mut());
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Entry,
        EntryState,
        LazyHashMap,
    };
    use crate::{
        env,
        hash::hasher::{
            Blake2x256Hasher,
            Sha2x256Hasher,
        },
        storage2::traits::{
            KeyPtr,
            SpreadLayout,
        },
    };
    use ink_primitives::Key;

    /// Asserts that the cached entries of the given `imap` is equal to the `expected` slice.
    fn assert_cached_entries<H>(
        hmap: &LazyHashMap<i32, u8, H>,
        expected: &[(i32, Entry<u8>)],
    ) {
        assert_eq!(hmap.entries().len(), expected.len());
        for (given, expected) in hmap
            .entries()
            .iter()
            .map(|(index, boxed_entry)| (*index, &**boxed_entry))
            .zip(expected.iter().map(|(index, entry)| (*index, entry)))
        {
            assert_eq!(given, expected);
        }
    }

    fn new_hmap() -> LazyHashMap<i32, u8, Blake2x256Hasher> {
        <LazyHashMap<i32, u8, Blake2x256Hasher>>::new()
    }

    #[test]
    fn new_works() {
        let hmap = new_hmap();
        // Key must be none.
        assert_eq!(hmap.key(), None);
        assert_eq!(hmap.key_at(&0), None);
        // Cached elements must be empty.
        assert_cached_entries(&hmap, &[]);
        // Same as default:
        let default_hmap = <LazyHashMap<i32, u8, Blake2x256Hasher>>::default();
        assert_eq!(hmap.key(), default_hmap.key());
        assert_eq!(hmap.entries(), default_hmap.entries());
    }

    #[test]
    fn key_at_works() {
        let key = Key([0x42; 32]);

        // BLAKE2 256-bit hasher:
        let hmap1 = <LazyHashMap<i32, u8, Blake2x256Hasher>>::lazy(key);
        // Key must be some.
        assert_eq!(hmap1.key(), Some(&key));
        // Cached elements must be empty.
        assert_cached_entries(&hmap1, &[]);
        let hmap1_at_0 = b"\
        \x67\x7E\xD3\xA4\x72\x2A\x83\x60\
        \x96\x65\x0E\xCD\x1F\x2C\xE8\x5D\
        \xBF\x7E\xC0\xFF\x16\x40\x8A\xD8\
        \x75\x88\xDE\x52\xF5\x8B\x99\xAF";
        assert_eq!(hmap1.key_at(&0), Some(Key(*hmap1_at_0)));
        // Same parameters must yield the same key:
        //
        // This tests an actual regression that happened because the
        // hash accumulator was not reset after a hash finalization.
        assert_cached_entries(&hmap1, &[]);
        assert_eq!(hmap1.key_at(&0), Some(Key(*hmap1_at_0)));
        assert_eq!(
            hmap1.key_at(&1),
            Some(Key(*b"\
                \x9A\x46\x1F\xB3\xA1\xC4\x20\xF8\
                \xA0\xD9\xA7\x79\x2F\x07\xFB\x7D\
                \x49\xDD\xAB\x08\x67\x90\x96\x15\
                \xFB\x85\x36\x3B\x82\x94\x85\x3F"))
        );
        // SHA2 256-bit hasher:
        let hmap2 = <LazyHashMap<i32, u8, Sha2x256Hasher>>::lazy(key);
        // Key must be some.
        assert_eq!(hmap2.key(), Some(&key));
        // Cached elements must be empty.
        assert_cached_entries(&hmap2, &[]);
        assert_eq!(
            hmap1.key_at(&0),
            Some(Key(*b"\
                \x67\x7E\xD3\xA4\x72\x2A\x83\x60\
                \x96\x65\x0E\xCD\x1F\x2C\xE8\x5D\
                \xBF\x7E\xC0\xFF\x16\x40\x8A\xD8\
                \x75\x88\xDE\x52\xF5\x8B\x99\xAF"))
        );
        assert_eq!(
            hmap1.key_at(&1),
            Some(Key(*b"\
                \x9A\x46\x1F\xB3\xA1\xC4\x20\xF8\
                \xA0\xD9\xA7\x79\x2F\x07\xFB\x7D\
                \x49\xDD\xAB\x08\x67\x90\x96\x15\
                \xFB\x85\x36\x3B\x82\x94\x85\x3F"))
        );
    }

    #[test]
    fn put_get_works() {
        let mut hmap = new_hmap();
        // Put some values.
        assert_eq!(hmap.put_get(&1, Some(b'A')), None);
        assert_eq!(hmap.put_get(&2, Some(b'B')), None);
        assert_eq!(hmap.put_get(&4, Some(b'C')), None);
        assert_cached_entries(
            &hmap,
            &[
                (1, Entry::new(Some(b'A'), EntryState::Mutated)),
                (2, Entry::new(Some(b'B'), EntryState::Mutated)),
                (4, Entry::new(Some(b'C'), EntryState::Mutated)),
            ],
        );
        // Put none values.
        assert_eq!(hmap.put_get(&3, None), None);
        assert_eq!(hmap.put_get(&5, None), None);
        assert_cached_entries(
            &hmap,
            &[
                (1, Entry::new(Some(b'A'), EntryState::Mutated)),
                (2, Entry::new(Some(b'B'), EntryState::Mutated)),
                (3, Entry::new(None, EntryState::Preserved)),
                (4, Entry::new(Some(b'C'), EntryState::Mutated)),
                (5, Entry::new(None, EntryState::Preserved)),
            ],
        );
        // Override some values with none.
        assert_eq!(hmap.put_get(&2, None), Some(b'B'));
        assert_eq!(hmap.put_get(&4, None), Some(b'C'));
        assert_cached_entries(
            &hmap,
            &[
                (1, Entry::new(Some(b'A'), EntryState::Mutated)),
                (2, Entry::new(None, EntryState::Mutated)),
                (3, Entry::new(None, EntryState::Preserved)),
                (4, Entry::new(None, EntryState::Mutated)),
                (5, Entry::new(None, EntryState::Preserved)),
            ],
        );
        // Override none values with some.
        assert_eq!(hmap.put_get(&3, Some(b'X')), None);
        assert_eq!(hmap.put_get(&5, Some(b'Y')), None);
        assert_cached_entries(
            &hmap,
            &[
                (1, Entry::new(Some(b'A'), EntryState::Mutated)),
                (2, Entry::new(None, EntryState::Mutated)),
                (3, Entry::new(Some(b'X'), EntryState::Mutated)),
                (4, Entry::new(None, EntryState::Mutated)),
                (5, Entry::new(Some(b'Y'), EntryState::Mutated)),
            ],
        );
    }

    #[test]
    fn get_works() {
        let mut hmap = new_hmap();
        let nothing_changed = &[
            (1, Entry::new(None, EntryState::Preserved)),
            (2, Entry::new(Some(b'B'), EntryState::Mutated)),
            (3, Entry::new(None, EntryState::Preserved)),
            (4, Entry::new(Some(b'D'), EntryState::Mutated)),
        ];
        // Put some values.
        assert_eq!(hmap.put_get(&1, None), None);
        assert_eq!(hmap.put_get(&2, Some(b'B')), None);
        assert_eq!(hmap.put_get(&3, None), None);
        assert_eq!(hmap.put_get(&4, Some(b'D')), None);
        assert_cached_entries(&hmap, nothing_changed);
        // `get` works:
        assert_eq!(hmap.get(&1), None);
        assert_eq!(hmap.get(&2), Some(&b'B'));
        assert_eq!(hmap.get(&3), None);
        assert_eq!(hmap.get(&4), Some(&b'D'));
        assert_cached_entries(&hmap, nothing_changed);
        // `get_mut` works:
        assert_eq!(hmap.get_mut(&1), None);
        assert_eq!(hmap.get_mut(&2), Some(&mut b'B'));
        assert_eq!(hmap.get_mut(&3), None);
        assert_eq!(hmap.get_mut(&4), Some(&mut b'D'));
        assert_cached_entries(&hmap, nothing_changed);
        // `get` or `get_mut` without cache:
        assert_eq!(hmap.get(&5), None);
        assert_eq!(hmap.get_mut(&5), None);
    }

    #[test]
    fn put_works() {
        let mut hmap = new_hmap();
        // Put some values.
        hmap.put(1, None);
        hmap.put(2, Some(b'B'));
        hmap.put(4, None);
        // The main difference between `put` and `put_get` is that `put` never
        // loads from storage which also has one drawback: Putting a `None`
        // value always ends-up in `Mutated` state for the entry even if the
        // entry is already `None`.
        assert_cached_entries(
            &hmap,
            &[
                (1, Entry::new(None, EntryState::Mutated)),
                (2, Entry::new(Some(b'B'), EntryState::Mutated)),
                (4, Entry::new(None, EntryState::Mutated)),
            ],
        );
        // Overwrite entries:
        hmap.put(1, Some(b'A'));
        hmap.put(2, None);
        assert_cached_entries(
            &hmap,
            &[
                (1, Entry::new(Some(b'A'), EntryState::Mutated)),
                (2, Entry::new(None, EntryState::Mutated)),
                (4, Entry::new(None, EntryState::Mutated)),
            ],
        );
    }

    #[test]
    fn swap_works() {
        let mut hmap = new_hmap();
        let nothing_changed = &[
            (1, Entry::new(Some(b'A'), EntryState::Mutated)),
            (2, Entry::new(Some(b'B'), EntryState::Mutated)),
            (3, Entry::new(None, EntryState::Preserved)),
            (4, Entry::new(None, EntryState::Preserved)),
        ];
        // Put some values.
        assert_eq!(hmap.put_get(&1, Some(b'A')), None);
        assert_eq!(hmap.put_get(&2, Some(b'B')), None);
        assert_eq!(hmap.put_get(&3, None), None);
        assert_eq!(hmap.put_get(&4, None), None);
        assert_cached_entries(&hmap, nothing_changed);
        // Swap same indices: Check that nothing has changed.
        for i in 0..4 {
            hmap.swap(&i, &i);
        }
        assert_cached_entries(&hmap, nothing_changed);
        // Swap `None` values: Check that nothing has changed.
        hmap.swap(&3, &4);
        hmap.swap(&4, &3);
        assert_cached_entries(&hmap, nothing_changed);
        // Swap `Some` and `None`:
        hmap.swap(&1, &3);
        assert_cached_entries(
            &hmap,
            &[
                (1, Entry::new(None, EntryState::Mutated)),
                (2, Entry::new(Some(b'B'), EntryState::Mutated)),
                (3, Entry::new(Some(b'A'), EntryState::Mutated)),
                (4, Entry::new(None, EntryState::Preserved)),
            ],
        );
        // Swap `Some` and `Some`:
        hmap.swap(&2, &3);
        assert_cached_entries(
            &hmap,
            &[
                (1, Entry::new(None, EntryState::Mutated)),
                (2, Entry::new(Some(b'A'), EntryState::Mutated)),
                (3, Entry::new(Some(b'B'), EntryState::Mutated)),
                (4, Entry::new(None, EntryState::Preserved)),
            ],
        );
        // Swap out of bounds: `None` and `None`
        hmap.swap(&4, &5);
        assert_cached_entries(
            &hmap,
            &[
                (1, Entry::new(None, EntryState::Mutated)),
                (2, Entry::new(Some(b'A'), EntryState::Mutated)),
                (3, Entry::new(Some(b'B'), EntryState::Mutated)),
                (4, Entry::new(None, EntryState::Preserved)),
                (5, Entry::new(None, EntryState::Preserved)),
            ],
        );
        // Swap out of bounds: `Some` and `None`
        hmap.swap(&3, &6);
        assert_cached_entries(
            &hmap,
            &[
                (1, Entry::new(None, EntryState::Mutated)),
                (2, Entry::new(Some(b'A'), EntryState::Mutated)),
                (3, Entry::new(None, EntryState::Mutated)),
                (4, Entry::new(None, EntryState::Preserved)),
                (5, Entry::new(None, EntryState::Preserved)),
                (6, Entry::new(Some(b'B'), EntryState::Mutated)),
            ],
        );
    }

    #[test]
    fn spread_layout_works() -> env::Result<()> {
        env::test::run_test::<env::DefaultEnvTypes, _>(|_| {
            let mut hmap = new_hmap();
            let nothing_changed = &[
                (1, Entry::new(Some(b'A'), EntryState::Mutated)),
                (2, Entry::new(Some(b'B'), EntryState::Mutated)),
                (3, Entry::new(None, EntryState::Preserved)),
                (4, Entry::new(None, EntryState::Preserved)),
            ];
            // Put some values.
            assert_eq!(hmap.put_get(&1, Some(b'A')), None);
            assert_eq!(hmap.put_get(&2, Some(b'B')), None);
            assert_eq!(hmap.put_get(&3, None), None);
            assert_eq!(hmap.put_get(&4, None), None);
            assert_cached_entries(&hmap, nothing_changed);
            // Push the lazy index map onto the contract storage and then load
            // another instance of it from the contract stoarge.
            // Then: Compare both instances to be equal.
            let root_key = Key([0x42; 32]);
            SpreadLayout::push_spread(&hmap, &mut KeyPtr::from(root_key));
            let hmap2 =
                <LazyHashMap<i32, u8, Blake2x256Hasher> as SpreadLayout>::pull_spread(
                    &mut KeyPtr::from(root_key),
                );
            assert_cached_entries(&hmap2, &[]);
            assert_eq!(hmap2.key(), Some(&Key([0x42; 32])));
            assert_eq!(hmap2.get(&1), Some(&b'A'));
            assert_eq!(hmap2.get(&2), Some(&b'B'));
            assert_eq!(hmap2.get(&3), None);
            assert_eq!(hmap2.get(&4), None);
            assert_cached_entries(
                &hmap2,
                &[
                    (1, Entry::new(Some(b'A'), EntryState::Preserved)),
                    (2, Entry::new(Some(b'B'), EntryState::Preserved)),
                    (3, Entry::new(None, EntryState::Preserved)),
                    (4, Entry::new(None, EntryState::Preserved)),
                ],
            );
            // Clear the first lazy index map instance and reload another instance
            // to check whether the associated storage has actually been freed
            // again:
            SpreadLayout::clear_spread(&hmap2, &mut KeyPtr::from(root_key));
            // The above `clear_spread` call is a no-op since lazy index map is
            // generally not aware of its associated elements. So we have to
            // manually clear them from the contract storage which is what the
            // high-level data structures like `storage::Vec` would command:
            hmap2.clear_packed_at(&1);
            hmap2.clear_packed_at(&2);
            hmap2.clear_packed_at(&3); // Not really needed here.
            hmap2.clear_packed_at(&4); // Not really needed here.
            let hmap3 =
                <LazyHashMap<i32, u8, Blake2x256Hasher> as SpreadLayout>::pull_spread(
                    &mut KeyPtr::from(root_key),
                );
            assert_cached_entries(&hmap3, &[]);
            assert_eq!(hmap3.get(&1), None);
            assert_eq!(hmap3.get(&2), None);
            assert_eq!(hmap3.get(&3), None);
            assert_eq!(hmap3.get(&4), None);
            assert_cached_entries(
                &hmap3,
                &[
                    (1, Entry::new(None, EntryState::Preserved)),
                    (2, Entry::new(None, EntryState::Preserved)),
                    (3, Entry::new(None, EntryState::Preserved)),
                    (4, Entry::new(None, EntryState::Preserved)),
                ],
            );
            Ok(())
        })
    }
}
