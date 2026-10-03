//! Object-owned PEP 393 exports and the Unicode construction transaction.

use super::*;
use crate::api::strings::{PythonStringBytes, push_codepoint_utf8};
use std::ffi::c_void;

enum UnicodeUnits {
    One(Box<[u8]>),
    Two(Box<[u16]>),
    Four(Box<[u32]>),
}

impl UnicodeUnits {
    fn kind(&self) -> u32 {
        match self {
            Self::One(_) => 1,
            Self::Two(_) => 2,
            Self::Four(_) => 4,
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::One(v) => v.len() - 1,
            Self::Two(v) => v.len() - 1,
            Self::Four(v) => v.len() - 1,
        }
    }

    fn data(&mut self) -> *mut c_void {
        match self {
            Self::One(v) => v.as_mut_ptr().cast(),
            Self::Two(v) => v.as_mut_ptr().cast(),
            Self::Four(v) => v.as_mut_ptr().cast(),
        }
    }

    fn read(&self, index: usize) -> u32 {
        match self {
            Self::One(v) => u32::from(v[index]),
            Self::Two(v) => u32::from(v[index]),
            Self::Four(v) => v[index],
        }
    }

    fn write(&mut self, index: usize, code: u32) -> bool {
        match self {
            Self::One(v) if code <= 0xff => v[index] = code as u8,
            Self::Two(v) if code <= 0xffff => v[index] = code as u16,
            Self::Four(v) if code <= 0x10ffff => v[index] = code,
            _ => return false,
        }
        true
    }
}

fn zeroed_units<T: Default + Clone>(len: usize) -> Option<Box<[T]>> {
    let total = len.checked_add(1)?;
    let mut values = Vec::new();
    values.try_reserve_exact(total).ok()?;
    values.resize(total, T::default());
    Some(values.into_boxed_slice())
}

pub(super) struct UnicodeProjection {
    units: UnsafeCell<UnicodeUnits>,
    /// An ASCII construction uses a 1-byte buffer but admits only ASCII.
    maxchar: u32,
    open: bool,
    utf8: Option<Box<[u8]>>,
}

impl UnicodeProjection {
    fn zeroed(len: usize, maxchar: u32, open: bool) -> Option<Self> {
        let (units, maxchar) = if maxchar < 0x80 {
            (UnicodeUnits::One(zeroed_units(len)?), 0x7f)
        } else if maxchar <= 0xff {
            (UnicodeUnits::One(zeroed_units(len)?), 0xff)
        } else if maxchar <= 0xffff {
            (UnicodeUnits::Two(zeroed_units(len)?), 0xffff)
        } else if maxchar <= 0x10ffff {
            (UnicodeUnits::Four(zeroed_units(len)?), 0x10ffff)
        } else {
            return None;
        };
        Some(Self {
            units: UnsafeCell::new(units),
            maxchar,
            open,
            utf8: None,
        })
    }

    pub(super) fn from_text(text: PythonStringBytes<'_>) -> Option<Self> {
        let len = text.code_points().count();
        let maxchar = text.code_points().max().unwrap_or(0);
        let projection = Self::zeroed(len, maxchar, false)?;
        for (index, code) in text.code_points().enumerate() {
            unsafe { &mut *projection.units.get() }.write(index, code);
        }
        Some(projection)
    }

    fn python_bytes(&self) -> Option<Vec<u8>> {
        let units = unsafe { &*self.units.get() };
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(units.len().checked_mul(4)?).ok()?;
        for index in 0..units.len() {
            let code = units.read(index);
            if code > self.maxchar {
                return None;
            }
            push_codepoint_utf8(&mut bytes, code)?;
        }
        Some(bytes)
    }
}

impl ObjectBridge {
    /// Prepared before the C object is published: DATA and KIND only borrow
    /// this allocation and never cross the strict UTF-8 codec boundary.
    pub fn unicode_layout(&self, op: *mut PyObject) -> Option<(u32, *mut c_void, usize)> {
        let bits = self.molt_handle_for_pyobj(op)?.bits();
        let handle = self.handle_shard(bits).lock();
        let projection = handle.to_py.get(&bits)?.unicode.as_ref()?;
        let units = unsafe { &mut *projection.units.get() };
        Some((units.kind(), units.data(), units.len()))
    }

    /// Called only for a newly allocated runtime string before its owned C
    /// reference is returned to the extension. No exported pointer can change.
    pub fn begin_unicode_construction(&self, op: *mut PyObject, maxchar: u32) -> bool {
        let Some(bits) = self.molt_handle_for_pyobj(op).map(MoltValueHandle::bits) else {
            return false;
        };
        let mut handle = self.handle_shard(bits).lock();
        let Some(entry) = handle.to_py.get_mut(&bits) else {
            return false;
        };
        let Some(previous) = entry.unicode.as_ref() else {
            return false;
        };
        let len = unsafe { &*previous.units.get() }.len();
        let Some(projection) = UnicodeProjection::zeroed(len, maxchar, true) else {
            return false;
        };
        entry.unicode = Some(projection);
        true
    }

    pub fn unicode_write(&self, op: *mut PyObject, index: usize, code: u32) -> bool {
        let Some(bits) = self.molt_handle_for_pyobj(op).map(MoltValueHandle::bits) else {
            return false;
        };
        let handle = self.handle_shard(bits).lock();
        let Some(projection) = handle
            .to_py
            .get(&bits)
            .and_then(|entry| entry.unicode.as_ref())
        else {
            return false;
        };
        let units = unsafe { &mut *projection.units.get() };
        projection.open
            && unsafe { (*op).ob_refcnt == 1 }
            && index < units.len()
            && code <= projection.maxchar
            && units.write(index, code)
    }

    pub fn unicode_is_open(&self, op: *mut PyObject) -> bool {
        let Some(bits) = self.molt_handle_for_pyobj(op).map(MoltValueHandle::bits) else {
            return false;
        };
        let handle = self.handle_shard(bits).lock();
        handle
            .to_py
            .get(&bits)
            .and_then(|entry| entry.unicode.as_ref())
            .is_some_and(|projection| projection.open)
    }

    pub fn unicode_is_ascii(&self, op: *mut PyObject) -> bool {
        self.unicode_maxchar(op)
            .is_some_and(|maxchar| maxchar < 0x80)
    }

    pub fn unicode_maxchar(&self, op: *mut PyObject) -> Option<u32> {
        let bits = self.molt_handle_for_pyobj(op)?.bits();
        let handle = self.handle_shard(bits).lock();
        handle
            .to_py
            .get(&bits)
            .and_then(|entry| entry.unicode.as_ref())
            .map(|projection| projection.maxchar)
    }

    pub fn unicode_utf8_cache(&self, bits: AbiHandle, bytes: &[u8]) -> Option<(*const u8, usize)> {
        let mut handle = self.handle_shard(bits).lock();
        let projection = handle.to_py.get_mut(&bits)?.unicode.as_mut()?;
        if projection.open {
            return None;
        }
        if projection.utf8.is_none() {
            let mut terminated = Vec::new();
            terminated
                .try_reserve_exact(bytes.len().checked_add(1)?)
                .ok()?;
            terminated.extend_from_slice(bytes);
            terminated.push(0);
            projection.utf8 = Some(terminated.into_boxed_slice());
        }
        let cache = projection.utf8.as_ref()?;
        Some((cache.as_ptr(), cache.len() - 1))
    }

    pub(super) fn commit_unicode_view(&self, bits: AbiHandle) -> bool {
        let bytes = {
            let handle = self.handle_shard(bits).lock();
            let Some(projection) = handle
                .to_py
                .get(&bits)
                .and_then(|entry| entry.unicode.as_ref())
            else {
                return true;
            };
            if !projection.open {
                return true;
            }
            projection.python_bytes()
        };
        let Some(bytes) = bytes else {
            unsafe {
                ensure_result_error(c"invalid Unicode construction data or allocation failure")
            };
            return false;
        };
        let status = unsafe {
            (crate::hooks::hooks_or_stubs().unicode_commit)(bits, bytes.as_ptr(), bytes.len())
        };
        if status != 0 {
            unsafe { ensure_result_error(c"Unicode construction commit failed") };
            return false;
        }
        let mut handle = self.handle_shard(bits).lock();
        let Some(projection) = handle
            .to_py
            .get_mut(&bits)
            .and_then(|entry| entry.unicode.as_mut())
        else {
            return false;
        };
        projection.open = false;
        projection.utf8 = None;
        true
    }
}
