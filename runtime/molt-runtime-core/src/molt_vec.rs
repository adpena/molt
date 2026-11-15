//! `MoltVec<T>`: the growable buffer of object-owned sequence storage, with a
//! fixed `#[repr(C)]` layout that compiled code reads.
//!
//! `Vec<T>` keeps its fields in an unspecified order (on aarch64-apple-darwin
//! with Rust 1.94 it was `[cap, data, len]`), so compiled code could not load
//! a list's data pointer or length without probing the compiler's own layout
//! and trusting that the runtime's rustc agreed. `MoltVec` fixes the order:
//! the data pointer at offset 0 and the length at offset 8, each a 64-bit word
//! on every target (a 32-bit target stores a zero upper half)
//! (`molt_codegen_abi::LIST_STORAGE_DATA_OFFSET` / `LIST_STORAGE_LEN_OFFSET`).
//!
//! The buffer is a `Vec<T>` allocation in every other respect: it comes from
//! and returns to the global allocator with `Vec`'s layout, so `with_vec`
//! rebuilds the `Vec` from its parts, runs one operation and stores the parts
//! back, even when the operation panics. Reads go through the slice.

use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

#[repr(C)]
pub struct MoltVec<T> {
    data: NonNull<T>,
    #[cfg(target_pointer_width = "32")]
    _data_high: u32,
    len: usize,
    #[cfg(target_pointer_width = "32")]
    _len_high: u32,
    cap: usize,
}

/// The upper halves of the 64-bit field words on a 32-bit target: always zero,
/// so a 64-bit load of a field reads its value on every target.
#[cfg(target_pointer_width = "32")]
const HIGH: u32 = 0;

// The same auto-trait contract as `Vec<T>`, which owns the same buffer.
unsafe impl<T: Send> Send for MoltVec<T> {}
unsafe impl<T: Sync> Sync for MoltVec<T> {}

const _: () = {
    assert!(
        std::mem::offset_of!(MoltVec<u64>, data) as i32
            == molt_codegen_abi::LIST_STORAGE_DATA_OFFSET
    );
    assert!(
        std::mem::offset_of!(MoltVec<u64>, len) as i32 == molt_codegen_abi::LIST_STORAGE_LEN_OFFSET
    );
};

impl<T> MoltVec<T> {
    #[inline]
    pub const fn new() -> Self {
        Self {
            data: NonNull::dangling(),
            #[cfg(target_pointer_width = "32")]
            _data_high: HIGH,
            len: 0,
            #[cfg(target_pointer_width = "32")]
            _len_high: HIGH,
            cap: 0,
        }
    }

    /// Take ownership of `vec`'s buffer.
    #[inline]
    pub fn from_vec(vec: Vec<T>) -> Self {
        let mut vec = ManuallyDrop::new(vec);
        Self {
            // A Vec's pointer is never null, dangling when it owns no buffer.
            data: unsafe { NonNull::new_unchecked(vec.as_mut_ptr()) },
            #[cfg(target_pointer_width = "32")]
            _data_high: HIGH,
            len: vec.len(),
            #[cfg(target_pointer_width = "32")]
            _len_high: HIGH,
            cap: vec.capacity(),
        }
    }

    /// Give the buffer back as the `Vec` it was allocated as.
    #[inline]
    pub fn into_vec(self) -> Vec<T> {
        let this = ManuallyDrop::new(self);
        unsafe { Vec::from_raw_parts(this.data.as_ptr(), this.len, this.cap) }
    }

    /// Run `operation` on the buffer as a `Vec`, storing its parts back after,
    /// including when `operation` unwinds.
    #[inline]
    pub fn with_vec<R>(&mut self, operation: impl FnOnce(&mut Vec<T>) -> R) -> R {
        struct WriteBack<'a, T> {
            slot: &'a mut MoltVec<T>,
            vec: ManuallyDrop<Vec<T>>,
        }
        impl<T> Drop for WriteBack<'_, T> {
            fn drop(&mut self) {
                self.slot.data = unsafe { NonNull::new_unchecked(self.vec.as_mut_ptr()) };
                self.slot.len = self.vec.len();
                self.slot.cap = self.vec.capacity();
            }
        }
        let vec = unsafe { Vec::from_raw_parts(self.data.as_ptr(), self.len, self.cap) };
        let mut guard = WriteBack {
            slot: self,
            vec: ManuallyDrop::new(vec),
        };
        operation(&mut guard.vec)
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.cap
    }

    #[inline]
    pub fn as_ptr(&self) -> *const T {
        self.data.as_ptr()
    }

    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.data.as_ptr()
    }

    #[inline]
    pub fn as_slice(&self) -> &[T] {
        unsafe { std::slice::from_raw_parts(self.data.as_ptr(), self.len) }
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe { std::slice::from_raw_parts_mut(self.data.as_ptr(), self.len) }
    }

    /// Set the length without touching elements.
    ///
    /// # Safety
    /// As `Vec::set_len`: `new_len <= capacity()` and the first `new_len`
    /// elements are initialized.
    #[inline]
    pub unsafe fn set_len(&mut self, new_len: usize) {
        debug_assert!(new_len <= self.cap);
        self.len = new_len;
    }

    /// Room for one more element. Kept out of line, as `Vec`'s `grow_one`
    /// is, so every `push` and `insert` site stays a test and a store.
    #[cold]
    #[inline(never)]
    fn grow_one(&mut self) {
        self.with_vec(|vec| vec.reserve(1));
    }

    #[inline]
    pub fn push(&mut self, value: T) {
        if self.len == self.cap {
            self.grow_one();
        }
        unsafe { self.data.as_ptr().add(self.len).write(value) };
        self.len += 1;
    }

    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(unsafe { self.data.as_ptr().add(self.len).read() })
    }

    /// As `Vec::insert`, in place after at most one out-of-line growth.
    #[inline]
    pub fn insert(&mut self, index: usize, value: T) {
        assert!(
            index <= self.len,
            "insertion index {index} exceeds length {}",
            self.len
        );
        if self.len == self.cap {
            self.grow_one();
        }
        unsafe {
            let at = self.data.as_ptr().add(index);
            if index < self.len {
                std::ptr::copy(at, at.add(1), self.len - index);
            }
            at.write(value);
        }
        self.len += 1;
    }

    #[inline]
    pub fn remove(&mut self, index: usize) -> T {
        assert!(
            index < self.len,
            "removal index {index} exceeds length {}",
            self.len
        );
        unsafe {
            let at = self.data.as_ptr().add(index);
            let value = at.read();
            std::ptr::copy(at.add(1), at, self.len - index - 1);
            self.len -= 1;
            value
        }
    }

    #[inline]
    pub fn truncate(&mut self, len: usize) {
        if len >= self.len {
            return;
        }
        let tail = std::ptr::slice_from_raw_parts_mut(
            unsafe { self.data.as_ptr().add(len) },
            self.len - len,
        );
        // Shorten first, as Vec does, so a panicking drop cannot drop twice.
        self.len = len;
        unsafe { std::ptr::drop_in_place(tail) };
    }

    #[inline]
    pub fn clear(&mut self) {
        self.truncate(0);
    }

    pub fn reserve(&mut self, additional: usize) {
        self.with_vec(|vec| vec.reserve(additional));
    }

    pub fn try_reserve_exact(
        &mut self,
        additional: usize,
    ) -> Result<(), std::collections::TryReserveError> {
        self.with_vec(|vec| vec.try_reserve_exact(additional))
    }

    pub fn extend<I: IntoIterator<Item = T>>(&mut self, items: I) {
        self.with_vec(|vec| vec.extend(items));
    }

    /// Move every element of `other` to the end of `self`, leaving `other`
    /// empty with its capacity.
    pub fn append(&mut self, other: &mut Vec<T>) {
        self.with_vec(|vec| vec.append(other));
    }

    /// Remove `range` and return its elements, in order.
    pub fn drain_to_vec<R: std::ops::RangeBounds<usize>>(&mut self, range: R) -> Vec<T> {
        self.with_vec(|vec| vec.drain(range).collect())
    }

    /// Replace `range` with `items` and return the removed elements, in order.
    pub fn splice_to_vec<R, I>(&mut self, range: R, items: I) -> Vec<T>
    where
        R: std::ops::RangeBounds<usize>,
        I: IntoIterator<Item = T>,
    {
        self.with_vec(|vec| vec.splice(range, items).collect())
    }

    pub fn retain(&mut self, keep: impl FnMut(&T) -> bool) {
        self.with_vec(|vec| vec.retain(keep));
    }
}

impl<'a, T> IntoIterator for &'a MoltVec<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}

impl<'a, T> IntoIterator for &'a mut MoltVec<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.as_mut_slice().iter_mut()
    }
}

impl<T> IntoIterator for MoltVec<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.into_vec().into_iter()
    }
}

impl<T: Clone> MoltVec<T> {
    // Vec's own extend and fill specialize to memcpy and memset; a hand loop
    // here measured worse (op_dict +2.2 points on Cranelift, 2026-11-14).
    pub fn extend_from_slice(&mut self, items: &[T]) {
        self.with_vec(|vec| vec.extend_from_slice(items));
    }

    pub fn resize(&mut self, len: usize, value: T) {
        self.with_vec(|vec| vec.resize(len, value));
    }

    pub fn extend_from_within<R: std::ops::RangeBounds<usize>>(&mut self, range: R) {
        self.with_vec(|vec| vec.extend_from_within(range));
    }

    pub fn to_vec(&self) -> Vec<T> {
        self.as_slice().to_vec()
    }
}

impl<T: Clone> Clone for MoltVec<T> {
    fn clone(&self) -> Self {
        Self::from_vec(self.to_vec())
    }
}

impl<T: PartialEq> PartialEq for MoltVec<T> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T: Eq> Eq for MoltVec<T> {}

impl<T: PartialEq> PartialEq<[T]> for MoltVec<T> {
    fn eq(&self, other: &[T]) -> bool {
        self.as_slice() == other
    }
}

impl<T: PartialEq> PartialEq<&[T]> for MoltVec<T> {
    fn eq(&self, other: &&[T]) -> bool {
        self.as_slice() == *other
    }
}

impl<T: PartialEq, const N: usize> PartialEq<[T; N]> for MoltVec<T> {
    fn eq(&self, other: &[T; N]) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T: PartialEq, const N: usize> PartialEq<&[T; N]> for MoltVec<T> {
    fn eq(&self, other: &&[T; N]) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T: PartialEq> PartialEq<Vec<T>> for MoltVec<T> {
    fn eq(&self, other: &Vec<T>) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T> Default for MoltVec<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for MoltVec<T> {
    fn drop(&mut self) {
        drop(unsafe { Vec::from_raw_parts(self.data.as_ptr(), self.len, self.cap) });
    }
}

impl<T> Deref for MoltVec<T> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T> DerefMut for MoltVec<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for MoltVec<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_slice().fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::MoltVec;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn grows_and_round_trips_through_vec() {
        let mut values = MoltVec::new();
        for value in 0..100u64 {
            values.push(value);
        }
        values.insert(0, 1000);
        assert_eq!(values.remove(1), 0);
        assert_eq!(values.pop(), Some(99));
        assert_eq!(values.drain_to_vec(..2), vec![1000, 1]);
        values.truncate(3);
        assert_eq!(values.as_slice(), &[2, 3, 4]);
        assert_eq!(values.into_vec(), vec![2, 3, 4]);
    }

    #[test]
    fn in_place_insert_remove_and_truncate_match_vec() {
        let mut ours = MoltVec::new();
        let mut oracle = Vec::new();
        for step in 0..200usize {
            let index = (step * 7) % (oracle.len() + 1);
            ours.insert(index, step);
            oracle.insert(index, step);
            if step % 3 == 0 {
                let at = (step * 5) % oracle.len();
                assert_eq!(ours.remove(at), oracle.remove(at));
            }
            assert_eq!(ours.as_slice(), oracle.as_slice());
        }
        ours.truncate(17);
        oracle.truncate(17);
        assert_eq!(ours.as_slice(), oracle.as_slice());
        ours.clear();
        assert!(ours.is_empty() && ours.capacity() >= 17);
    }

    #[test]
    fn a_panicking_operation_leaves_the_parts_it_made() {
        // The operation reallocates, then panics: the slot must hold the new
        // buffer, not the freed one, or its drop would free twice.
        let mut values = MoltVec::from_vec(vec![1u64]);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            values.with_vec(|vec| {
                vec.reserve(1 << 12);
                vec.push(2);
                panic!("after the reallocation");
            })
        }));
        assert!(result.is_err());
        assert_eq!(values.as_slice(), &[1, 2]);
        assert!(values.capacity() >= 1 << 12);
    }

    #[test]
    fn drop_releases_each_element_once() {
        let drops = Rc::new(Cell::new(0));
        struct Counted(Rc<Cell<usize>>);
        impl Drop for Counted {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let mut values = MoltVec::new();
        for _ in 0..5 {
            values.push(Counted(drops.clone()));
        }
        drop(values.pop());
        drop(values);
        assert_eq!(drops.get(), 5);
    }
}
