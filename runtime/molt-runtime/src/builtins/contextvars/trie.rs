//! Persistent identity-keyed Context storage. Every physical edge belongs to a
//! managed node; snapshots own one root. No host allocator or leaf boxes.
use crate::object::builders::PtrDropGuard;
use crate::object::{
    ObjectAuxPreselection, TYPE_ID_CONTEXT_BITMAP_NODE, TYPE_ID_CONTEXT_COLLISION_NODE,
};
use crate::{MoltObject, PyToken, inc_ref_bits, obj_from_bits};

#[repr(C)]
struct Bitmap {
    data: u32,
    children: u32,
}
#[repr(C)]
struct Collision {
    hash: u32,
    len: u32,
}
const PREFIX: usize = 8;
const DEPTH: usize = 8; // seven 5-bit chunks of a 32-bit hash, plus collision

#[inline]
fn ptr(bits: u64) -> *mut u8 {
    obj_from_bits(bits).as_ptr().expect("context trie root")
}
#[inline]
unsafe fn words(p: *mut u8) -> *mut u64 {
    unsafe { p.add(PREFIX).cast() }
}
#[inline]
fn rank(bitmap: u32, bit: u32) -> usize {
    (bitmap & bit.wrapping_sub(1)).count_ones() as usize
}
#[inline]
fn bit(hash: u32, shift: u32) -> u32 {
    1 << ((hash >> shift) & 31)
}
#[inline]
unsafe fn collision(p: *mut u8) -> bool {
    unsafe { crate::object::object_type_id(p) == TYPE_ID_CONTEXT_COLLISION_NODE }
}

fn allocate(py: &PyToken<'_>, kind: u32, len: usize) -> Option<*mut u8> {
    let Some(size) = len
        .checked_mul(8)
        .and_then(|n| n.checked_add(PREFIX))
        .and_then(|n| n.checked_add(std::mem::size_of::<crate::object::MoltHeader>()))
    else {
        crate::record_memory_error_without_allocation(py);
        return None;
    };
    let p = crate::object::alloc_object_zeroed_unpublished_with_aux(
        py,
        size,
        kind,
        ObjectAuxPreselection::Default,
    );
    (!p.is_null()).then_some(p)
}
unsafe fn publish(py: &PyToken<'_>, p: *mut u8, len: usize) -> u64 {
    for i in 0..len {
        unsafe { inc_ref_bits(py, *words(p).add(i)) };
    }
    unsafe {
        (*crate::object::header_from_obj_ptr(p))
            .fetch_or_flags(crate::object::HEADER_FLAG_CONTAINS_REFS);
    }
    unsafe {
        crate::object::gc::gc_publish_initialized(py, p);
    }
    MoltObject::from_ptr(p).bits()
}
fn bitmap(py: &PyToken<'_>, data: u32, children: u32, values: &[u64]) -> Option<u64> {
    debug_assert_eq!(data & children, 0);
    debug_assert_eq!(
        values.len(),
        data.count_ones() as usize * 2 + children.count_ones() as usize
    );
    let p = allocate(py, TYPE_ID_CONTEXT_BITMAP_NODE, values.len())?;
    unsafe {
        p.cast::<Bitmap>().write(Bitmap { data, children });
        std::ptr::copy_nonoverlapping(values.as_ptr(), words(p), values.len());
        Some(publish(py, p, values.len()))
    }
}
fn retain(py: &PyToken<'_>, root: u64) -> u64 {
    if root != 0 {
        inc_ref_bits(py, root);
    }
    root
}
fn leaf(py: &PyToken<'_>, key: u64, value: u64, hash: u32, shift: u32) -> Option<u64> {
    bitmap(py, bit(hash, shift), 0, &[key, value])
}

/// Caller owns/pins root for this callback-free borrow. ContextVar identity and
/// its immutable folded hash are the only comparison operations.
pub(super) fn lookup(mut root: u64, key: u64, hash: u32) -> Option<u64> {
    let mut shift = 0;
    while root != 0 {
        unsafe {
            let p = ptr(root);
            if collision(p) {
                let c = &*p.cast::<Collision>();
                if c.hash != hash {
                    return None;
                }
                for i in 0..c.len as usize {
                    if *words(p).add(i * 2) == key {
                        return Some(*words(p).add(i * 2 + 1));
                    }
                }
                return None;
            }
            let b = &*p.cast::<Bitmap>();
            let mask = bit(hash, shift);
            if b.data & mask != 0 {
                let i = rank(b.data, mask) * 2;
                return (*words(p).add(i) == key).then(|| *words(p).add(i + 1));
            }
            if b.children & mask == 0 {
                return None;
            }
            root = *words(p).add(b.data.count_ones() as usize * 2 + rank(b.children, mask));
            shift += 5;
        }
    }
    None
}
fn merge_pairs(
    py: &PyToken<'_>,
    a: (u64, u64, u32),
    b: (u64, u64, u32),
    shift: u32,
) -> Option<u64> {
    if a.2 == b.2 {
        let p = allocate(py, TYPE_ID_CONTEXT_COLLISION_NODE, 4)?;
        unsafe {
            p.cast::<Collision>().write(Collision { hash: a.2, len: 2 });
            let w = words(p);
            w.write(a.0);
            w.add(1).write(a.1);
            w.add(2).write(b.0);
            w.add(3).write(b.1);
            return Some(publish(py, p, 4));
        }
    }
    let am = bit(a.2, shift);
    let bm = bit(b.2, shift);
    if am != bm {
        return bitmap(
            py,
            am | bm,
            0,
            if am < bm {
                &[a.0, a.1, b.0, b.1]
            } else {
                &[b.0, b.1, a.0, a.1]
            },
        );
    }
    let child = merge_pairs(py, a, b, shift + 5)?;
    let _guard = PtrDropGuard::preserving(ptr(child));
    bitmap(py, 0, am, &[child])
}
fn merge_bucket(
    py: &PyToken<'_>,
    root: u64,
    old_hash: u32,
    key: u64,
    value: u64,
    hash: u32,
    shift: u32,
) -> Option<u64> {
    let a = bit(old_hash, shift);
    let b = bit(hash, shift);
    if a != b {
        return bitmap(py, b, a, &[key, value, root]);
    }
    let child = merge_bucket(py, root, old_hash, key, value, hash, shift + 5)?;
    let _guard = PtrDropGuard::preserving(ptr(child));
    bitmap(py, 0, a, &[child])
}
/// Returns an owned new root, leaving the old root unchanged on every failure.
pub(super) fn assoc(py: &PyToken<'_>, root: u64, key: u64, value: u64, hash: u32) -> Option<u64> {
    insert(py, root, key, value, hash, 0)
}
fn insert(py: &PyToken<'_>, root: u64, key: u64, value: u64, hash: u32, shift: u32) -> Option<u64> {
    if root == 0 {
        return leaf(py, key, value, hash, shift);
    }
    unsafe {
        let p = ptr(root);
        if collision(p) {
            let c = &*p.cast::<Collision>();
            if c.hash != hash {
                return merge_bucket(py, root, c.hash, key, value, hash, shift);
            }
            let old = c.len as usize;
            let index = (0..old).find(|i| *words(p).add(i * 2) == key);
            if index.is_some_and(|i| *words(p).add(i * 2 + 1) == value) {
                return Some(retain(py, root));
            }
            let Some(len) = old
                .checked_add(usize::from(index.is_none()))
                .filter(|&n| n <= u32::MAX as usize && n <= usize::MAX / 2)
            else {
                crate::record_memory_error_without_allocation(py);
                return None;
            };
            let fresh = allocate(py, TYPE_ID_CONTEXT_COLLISION_NODE, len * 2)?;
            fresh.cast::<Collision>().write(Collision {
                hash,
                len: len as u32,
            });
            std::ptr::copy_nonoverlapping(words(p), words(fresh), old * 2);
            let i = index.unwrap_or(old);
            words(fresh).add(i * 2).write(key);
            words(fresh).add(i * 2 + 1).write(value);
            return Some(publish(py, fresh, len * 2));
        }
        let b = &*p.cast::<Bitmap>();
        let mask = bit(hash, shift);
        let nd = b.data.count_ones() as usize;
        let nc = b.children.count_ones() as usize;
        let mut values = [0u64; 64];
        let len = nd * 2 + nc;
        if b.data & mask != 0 {
            let i = rank(b.data, mask) * 2;
            let old_key = *words(p).add(i);
            let old_value = *words(p).add(i + 1);
            if old_key == key {
                if old_value == value {
                    return Some(retain(py, root));
                }
                std::ptr::copy_nonoverlapping(words(p), values.as_mut_ptr(), len);
                values[i + 1] = value;
                return bitmap(py, b.data, b.children, &values[..len]);
            }
            let child = merge_pairs(
                py,
                (old_key, old_value, super::var_hash(old_key)),
                (key, value, hash),
                shift + 5,
            )?;
            let _guard = PtrDropGuard::preserving(ptr(child));
            let ci = rank(b.children, mask);
            let data_len = nd * 2 - 2;
            std::ptr::copy_nonoverlapping(words(p), values.as_mut_ptr(), i);
            std::ptr::copy_nonoverlapping(
                words(p).add(i + 2),
                values.as_mut_ptr().add(i),
                data_len - i,
            );
            std::ptr::copy_nonoverlapping(
                words(p).add(nd * 2),
                values.as_mut_ptr().add(data_len),
                ci,
            );
            values[data_len + ci] = child;
            std::ptr::copy_nonoverlapping(
                words(p).add(nd * 2 + ci),
                values.as_mut_ptr().add(data_len + ci + 1),
                nc - ci,
            );
            return bitmap(py, b.data ^ mask, b.children | mask, &values[..len - 1]);
        }
        if b.children & mask != 0 {
            let i = nd * 2 + rank(b.children, mask);
            let old = *words(p).add(i);
            let child = insert(py, old, key, value, hash, shift + 5)?;
            let _guard = PtrDropGuard::preserving(ptr(child));
            if child == old {
                return Some(retain(py, root));
            }
            std::ptr::copy_nonoverlapping(words(p), values.as_mut_ptr(), len);
            values[i] = child;
            return bitmap(py, b.data, b.children, &values[..len]);
        }
        let i = rank(b.data, mask) * 2;
        std::ptr::copy_nonoverlapping(words(p), values.as_mut_ptr(), i);
        values[i] = key;
        values[i + 1] = value;
        std::ptr::copy_nonoverlapping(words(p).add(i), values.as_mut_ptr().add(i + 2), len - i);
        bitmap(py, b.data | mask, b.children, &values[..len + 2])
    }
}
unsafe fn singleton(root: u64) -> Option<(u64, u64)> {
    if root == 0 {
        return None;
    }
    let p = ptr(root);
    if unsafe { collision(p) } {
        return None;
    }
    let b = unsafe { &*p.cast::<Bitmap>() };
    if b.data.count_ones() == 1 && b.children == 0 {
        return Some(unsafe { (*words(p), *words(p).add(1)) });
    }
    None
}
pub(super) fn without(py: &PyToken<'_>, root: u64, key: u64, hash: u32) -> Option<u64> {
    remove(py, root, key, hash, 0)
}
fn remove(py: &PyToken<'_>, root: u64, key: u64, hash: u32, shift: u32) -> Option<u64> {
    if root == 0 {
        return Some(0);
    }
    unsafe {
        let p = ptr(root);
        if collision(p) {
            let c = &*p.cast::<Collision>();
            let len = c.len as usize;
            let Some(i) = (0..len).find(|i| *words(p).add(i * 2) == key) else {
                return Some(retain(py, root));
            };
            if len == 2 {
                let j = usize::from(i == 0) * 2;
                return leaf(
                    py,
                    *words(p).add(j),
                    *words(p).add(j + 1),
                    hash,
                    shift.min(30),
                );
            }
            let fresh = allocate(py, TYPE_ID_CONTEXT_COLLISION_NODE, (len - 1) * 2)?;
            fresh.cast::<Collision>().write(Collision {
                hash: c.hash,
                len: (len - 1) as u32,
            });
            std::ptr::copy_nonoverlapping(words(p), words(fresh), i * 2);
            std::ptr::copy_nonoverlapping(
                words(p).add((i + 1) * 2),
                words(fresh).add(i * 2),
                (len - i - 1) * 2,
            );
            return Some(publish(py, fresh, (len - 1) * 2));
        }
        let b = &*p.cast::<Bitmap>();
        let mask = bit(hash, shift);
        let nd = b.data.count_ones() as usize;
        let nc = b.children.count_ones() as usize;
        let len = nd * 2 + nc;
        let mut values = [0u64; 64];
        if b.data & mask != 0 {
            let i = rank(b.data, mask) * 2;
            if *words(p).add(i) != key {
                return Some(retain(py, root));
            }
            if len == 2 {
                return Some(0);
            }
            std::ptr::copy_nonoverlapping(words(p), values.as_mut_ptr(), i);
            std::ptr::copy_nonoverlapping(
                words(p).add(i + 2),
                values.as_mut_ptr().add(i),
                len - i - 2,
            );
            return bitmap(py, b.data ^ mask, b.children, &values[..len - 2]);
        }
        if b.children & mask == 0 {
            return Some(retain(py, root));
        }
        let ci = rank(b.children, mask);
        let i = nd * 2 + ci;
        let old = *words(p).add(i);
        let child = remove(py, old, key, hash, shift + 5)?;
        let _guard = (child != 0).then(|| PtrDropGuard::preserving(ptr(child)));
        if child == old {
            return Some(retain(py, root));
        }
        if child == 0 {
            if len == 1 {
                return Some(0);
            }
            std::ptr::copy_nonoverlapping(words(p), values.as_mut_ptr(), i);
            std::ptr::copy_nonoverlapping(
                words(p).add(i + 1),
                values.as_mut_ptr().add(i),
                len - i - 1,
            );
            return bitmap(py, b.data, b.children ^ mask, &values[..len - 1]);
        }
        if let Some((k, v)) = singleton(child) {
            let di = rank(b.data, mask) * 2;
            std::ptr::copy_nonoverlapping(words(p), values.as_mut_ptr(), di);
            values[di] = k;
            values[di + 1] = v;
            std::ptr::copy_nonoverlapping(
                words(p).add(di),
                values.as_mut_ptr().add(di + 2),
                nd * 2 - di,
            );
            std::ptr::copy_nonoverlapping(
                words(p).add(nd * 2),
                values.as_mut_ptr().add(nd * 2 + 2),
                ci,
            );
            std::ptr::copy_nonoverlapping(
                words(p).add(i + 1),
                values.as_mut_ptr().add(nd * 2 + 2 + ci),
                nc - ci - 1,
            );
            return bitmap(py, b.data | mask, b.children ^ mask, &values[..len + 1]);
        }
        std::ptr::copy_nonoverlapping(words(p), values.as_mut_ptr(), len);
        values[i] = child;
        bitmap(py, b.data, b.children, &values[..len])
    }
}
/// Nodes are immutable while shared. GC breaks cycles through the mutable
/// Context; only terminal destruction clears node edges.
pub(crate) unsafe fn visit(p: *mut u8, mut f: impl FnMut(u64)) {
    let len = unsafe {
        if collision(p) {
            (*p.cast::<Collision>()).len as usize * 2
        } else {
            let b = &*p.cast::<Bitmap>();
            b.data.count_ones() as usize * 2 + b.children.count_ones() as usize
        }
    };
    for i in 0..len {
        f(unsafe { *words(p).add(i) });
    }
}
pub(crate) unsafe fn detach(
    p: *mut u8,
    sink: &mut crate::object::heap_lifecycle::DetachedEdgeSink,
) {
    unsafe {
        visit(p, |bits| sink.detach_if_heap(bits));
        p.cast::<u64>().write(0);
    }
}
#[derive(Clone, Copy)]
#[repr(C)]
struct Frame {
    node: u64,
    next: u64,
}
#[repr(C)]
pub(super) struct Cursor {
    frames: [Frame; DEPTH],
    depth: u64,
}
impl Cursor {
    pub(super) fn new(root: u64) -> Self {
        let mut out = Self {
            frames: [Frame { node: 0, next: 0 }; DEPTH],
            depth: u64::from(root != 0),
        };
        out.frames[0].node = root;
        out
    }
    pub(super) fn next(&mut self) -> Option<(u64, u64)> {
        while self.depth != 0 {
            unsafe {
                let frame = &mut self.frames[self.depth as usize - 1];
                let p = ptr(frame.node);
                let i = frame.next as usize;
                frame.next += 1;
                if collision(p) {
                    if i < (*p.cast::<Collision>()).len as usize {
                        return Some((*words(p).add(i * 2), *words(p).add(i * 2 + 1)));
                    }
                } else {
                    let b = &*p.cast::<Bitmap>();
                    let nd = b.data.count_ones() as usize;
                    let nc = b.children.count_ones() as usize;
                    if i < nd {
                        return Some((*words(p).add(i * 2), *words(p).add(i * 2 + 1)));
                    }
                    if i < nd + nc {
                        let child = *words(p).add(nd * 2 + i - nd);
                        self.frames[self.depth as usize] = Frame {
                            node: child,
                            next: 0,
                        };
                        self.depth += 1;
                        continue;
                    }
                }
                self.depth -= 1;
            }
        }
        None
    }
}
