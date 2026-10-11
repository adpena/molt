// === FILE: runtime/molt-runtime/src/builtins/array_mod.rs ===
//
// Typed array module intrinsics (CPython `array` module semantics).
//
// The native storage (`ArrayHandle`) is wrapped in a first-class refcounted
// runtime object via the generic `native_handle` mechanism
// (`TYPE_ID_NATIVE_HANDLE`). This means the handle the Python `array` wrapper
// stores in `self._handle` is a real `MoltObject` with a valid header, so it
// participates in ordinary reference counting: the runtime increments it when
// it is stored into an attribute slot and decrements (and finalizes, freeing
// the boxed `ArrayHandle`) when the owning `array` instance is collected.
//
// Storing the raw `Box<ArrayHandle>` pointer as a fake object pointer (the old
// design) was unsound: the bare struct has no `MoltHeader`, so the first
// `inc_ref`/`dec_ref` of the attribute slot read garbage where the header would
// be and aborted with "invalid object header before dec_ref".

use std::sync::{Arc, Mutex};

use crate::builtins::numbers::{index_i64_with_overflow, sequence_index_i64_with_type_error};
use crate::object::memoryview::{
    TypedStridedStorage, TypedStridedStorageError, memoryview_format_from_str,
};
use crate::object::native_handle::{native_handle_arc, native_handle_new};
use crate::object::ops::string_obj_to_owned;
use crate::object::ops_sys::{DecodedSlice, collect_slice_indices, slice_error};
use crate::{
    MoltObject, PyToken, TYPE_ID_OBJECT, TYPE_ID_SLICE, alloc_bytes, alloc_string, alloc_tuple,
    inc_ref_bits, instance_dict_bits, int_bits_from_i64, is_missing_bits, obj_from_bits,
    object_class_bits, object_type_id, raise_exception, slice_start_bits, slice_step_bits,
    slice_stop_bits, to_f64, to_i64,
};
use molt_runtime_core::OwnedRuntimeValue;

// ---------------------------------------------------------------------------
// Typecode metadata
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Typecode {
    B,  // 'b' signed char  (1 byte)
    UB, // 'B' unsigned char (1 byte)
    U,  // 'u' wchar_t (deprecated but supported)
    H,  // 'h' signed short (2 bytes)
    UH, // 'H' unsigned short (2 bytes)
    I,  // 'i' signed int (4 bytes in Molt's supported target profiles)
    UI, // 'I' unsigned int (4 bytes)
    L,  // 'l' signed long
    UL, // 'L' unsigned long
    Q,  // 'q' signed long long (8 bytes)
    UQ, // 'Q' unsigned long long (8 bytes)
    F,  // 'f' float (4 bytes)
    D,  // 'd' double (8 bytes)
}

impl Typecode {
    fn from_char(ch: char) -> Option<Self> {
        match ch {
            'b' => Some(Typecode::B),
            'B' => Some(Typecode::UB),
            'u' => Some(Typecode::U),
            'h' => Some(Typecode::H),
            'H' => Some(Typecode::UH),
            'i' => Some(Typecode::I),
            'I' => Some(Typecode::UI),
            'l' => Some(Typecode::L),
            'L' => Some(Typecode::UL),
            'q' => Some(Typecode::Q),
            'Q' => Some(Typecode::UQ),
            'f' => Some(Typecode::F),
            'd' => Some(Typecode::D),
            _ => None,
        }
    }

    fn as_char(self) -> char {
        match self {
            Typecode::B => 'b',
            Typecode::UB => 'B',
            Typecode::U => 'u',
            Typecode::H => 'h',
            Typecode::UH => 'H',
            Typecode::I => 'i',
            Typecode::UI => 'I',
            Typecode::L => 'l',
            Typecode::UL => 'L',
            Typecode::Q => 'q',
            Typecode::UQ => 'Q',
            Typecode::F => 'f',
            Typecode::D => 'd',
        }
    }

    fn itemsize(self) -> usize {
        if self == Typecode::U {
            return wchar_itemsize();
        }
        memoryview_format_from_str(&self.as_char().to_string())
            .map(|format| format.itemsize)
            .unwrap_or(1)
    }

    fn is_float(self) -> bool {
        matches!(self, Typecode::F | Typecode::D)
    }
}

fn wchar_itemsize() -> usize {
    if cfg!(windows) { 2 } else { 4 }
}

// ---------------------------------------------------------------------------
// Array element – uniform tagged union stored as little-endian bytes
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum ArrayElem {
    Int(i64),
    Uint(u64),
    Float(f64),
}

impl ArrayElem {
    fn to_i64(self) -> Option<i64> {
        match self {
            ArrayElem::Int(v) => Some(v),
            ArrayElem::Uint(v) => i64::try_from(v).ok(),
            ArrayElem::Float(_) => None,
        }
    }

    fn to_f64(self) -> f64 {
        match self {
            ArrayElem::Int(v) => v as f64,
            ArrayElem::Uint(v) => v as f64,
            ArrayElem::Float(v) => v,
        }
    }
}

// ---------------------------------------------------------------------------
// ArrayHandle
// ---------------------------------------------------------------------------

pub(crate) struct ArrayHandle {
    typecode: Typecode,
    data: Vec<u8>,
    exports: crate::object::buffer_exports::BufferExports,
}

impl ArrayHandle {
    fn new(typecode: Typecode) -> Self {
        ArrayHandle {
            typecode,
            data: Vec::new(),
            exports: crate::object::buffer_exports::BufferExports::new(),
        }
    }

    fn len(&self) -> usize {
        self.data
            .len()
            .checked_div(self.typecode.itemsize())
            .unwrap_or(0)
    }

    fn read_elem(&self, idx: usize) -> Option<ArrayElem> {
        let sz = self.typecode.itemsize();
        let offset = idx.checked_mul(sz)?;
        let end = offset.checked_add(sz)?;
        if end > self.data.len() {
            return None;
        }
        let bytes = &self.data[offset..end];
        Some(self.decode_elem(bytes))
    }

    fn decode_elem(&self, bytes: &[u8]) -> ArrayElem {
        match self.typecode {
            Typecode::B => ArrayElem::Int(bytes[0] as i8 as i64),
            Typecode::UB => ArrayElem::Uint(bytes[0] as u64),
            Typecode::U => {
                if self.typecode.itemsize() == 2 {
                    let v = u16::from_ne_bytes(bytes[..2].try_into().unwrap_or([0u8; 2]));
                    ArrayElem::Uint(v as u64)
                } else {
                    let v = u32::from_ne_bytes(bytes[..4].try_into().unwrap_or([0u8; 4]));
                    ArrayElem::Uint(v as u64)
                }
            }
            Typecode::H => {
                let v = i16::from_ne_bytes(bytes[..2].try_into().unwrap_or([0u8; 2]));
                ArrayElem::Int(v as i64)
            }
            Typecode::UH => {
                let v = u16::from_ne_bytes(bytes[..2].try_into().unwrap_or([0u8; 2]));
                ArrayElem::Uint(v as u64)
            }
            Typecode::I => {
                let v = i32::from_ne_bytes(bytes[..4].try_into().unwrap_or([0u8; 4]));
                ArrayElem::Int(v as i64)
            }
            Typecode::UI => {
                let v = u32::from_ne_bytes(bytes[..4].try_into().unwrap_or([0u8; 4]));
                ArrayElem::Uint(v as u64)
            }
            Typecode::L => {
                if self.typecode.itemsize() == 4 {
                    let v = i32::from_ne_bytes(bytes[..4].try_into().unwrap_or([0u8; 4]));
                    ArrayElem::Int(v as i64)
                } else {
                    let v = i64::from_ne_bytes(bytes[..8].try_into().unwrap_or([0u8; 8]));
                    ArrayElem::Int(v)
                }
            }
            Typecode::UL => {
                if self.typecode.itemsize() == 4 {
                    let v = u32::from_ne_bytes(bytes[..4].try_into().unwrap_or([0u8; 4]));
                    ArrayElem::Uint(v as u64)
                } else {
                    let v = u64::from_ne_bytes(bytes[..8].try_into().unwrap_or([0u8; 8]));
                    ArrayElem::Uint(v)
                }
            }
            Typecode::Q => {
                let v = i64::from_ne_bytes(bytes[..8].try_into().unwrap_or([0u8; 8]));
                ArrayElem::Int(v)
            }
            Typecode::UQ => {
                let v = u64::from_ne_bytes(bytes[..8].try_into().unwrap_or([0u8; 8]));
                ArrayElem::Uint(v)
            }
            Typecode::F => {
                let v = f32::from_ne_bytes(bytes[..4].try_into().unwrap_or([0u8; 4]));
                ArrayElem::Float(v as f64)
            }
            Typecode::D => {
                let v = f64::from_ne_bytes(bytes[..8].try_into().unwrap_or([0u8; 8]));
                ArrayElem::Float(v)
            }
        }
    }

    fn encode_elem_into(&self, elem: ArrayElem, buf: &mut [u8; 8]) -> Result<usize, &'static str> {
        let sz = self.typecode.itemsize();
        match self.typecode {
            Typecode::B => {
                let v = elem.to_i64().ok_or("expected int for typecode 'b'")?;
                if !(-128..=127).contains(&v) {
                    return Err("value out of range for typecode 'b'");
                }
                buf[0] = (v as i8) as u8;
            }
            Typecode::UB => {
                let v = elem.to_i64().ok_or("expected int for typecode 'B'")?;
                if !(0..=255).contains(&v) {
                    return Err("value out of range for typecode 'B'");
                }
                buf[0] = v as u8;
            }
            Typecode::U => {
                let v = elem.to_i64().ok_or("expected int for typecode 'u'")?;
                if !(0..=0x10FFFF).contains(&v) {
                    return Err("value out of range for typecode 'u'");
                }
                if sz == 2 {
                    if v > u16::MAX as i64 {
                        return Err("value out of range for typecode 'u'");
                    }
                    buf[..2].copy_from_slice(&(v as u16).to_ne_bytes());
                } else {
                    buf[..4].copy_from_slice(&(v as u32).to_ne_bytes());
                }
            }
            Typecode::H => {
                let v = elem.to_i64().ok_or("expected int for typecode 'h'")?;
                if !(-32768..=32767).contains(&v) {
                    return Err("value out of range for typecode 'h'");
                }
                buf[..2].copy_from_slice(&(v as i16).to_ne_bytes());
            }
            Typecode::UH => {
                let v = elem.to_i64().ok_or("expected int for typecode 'H'")?;
                if !(0..=65535).contains(&v) {
                    return Err("value out of range for typecode 'H'");
                }
                buf[..2].copy_from_slice(&(v as u16).to_ne_bytes());
            }
            Typecode::I => {
                let v = elem.to_i64().ok_or("expected int for typecode 'i'")?;
                if !(-2147483648..=2147483647).contains(&v) {
                    return Err("value out of range for typecode 'i'");
                }
                buf[..4].copy_from_slice(&(v as i32).to_ne_bytes());
            }
            Typecode::UI => {
                let v = elem.to_i64().ok_or("expected int for typecode 'I'")?;
                if !(0..=4294967295).contains(&v) {
                    return Err("value out of range for typecode 'I'");
                }
                buf[..4].copy_from_slice(&(v as u32).to_ne_bytes());
            }
            Typecode::L => {
                let v = elem.to_i64().ok_or("expected int for typecode 'l'")?;
                if sz == 4 {
                    if !(-2147483648..=2147483647).contains(&v) {
                        return Err("value out of range for typecode 'l'");
                    }
                    buf[..4].copy_from_slice(&(v as i32).to_ne_bytes());
                } else {
                    buf[..8].copy_from_slice(&v.to_ne_bytes());
                }
            }
            Typecode::UL => {
                let v = elem.to_i64().ok_or("expected int for typecode 'L'")?;
                if v < 0 {
                    return Err("value out of range for typecode 'L'");
                }
                if sz == 4 {
                    if v > u32::MAX as i64 {
                        return Err("value out of range for typecode 'L'");
                    }
                    buf[..4].copy_from_slice(&(v as u32).to_ne_bytes());
                } else {
                    buf[..8].copy_from_slice(&(v as u64).to_ne_bytes());
                }
            }
            Typecode::Q => {
                let v = elem.to_i64().ok_or("expected int for typecode 'q'")?;
                buf[..8].copy_from_slice(&v.to_ne_bytes());
            }
            Typecode::UQ => match elem {
                ArrayElem::Uint(v) => buf[..8].copy_from_slice(&v.to_ne_bytes()),
                ArrayElem::Int(v) if v >= 0 => buf[..8].copy_from_slice(&(v as u64).to_ne_bytes()),
                _ => return Err("value out of range for typecode 'Q'"),
            },
            Typecode::F => {
                let v = elem.to_f64() as f32;
                buf[..4].copy_from_slice(&v.to_ne_bytes());
            }
            Typecode::D => {
                let v = elem.to_f64();
                buf[..8].copy_from_slice(&v.to_ne_bytes());
            }
        }
        Ok(sz)
    }

    fn set_elem(&mut self, idx: usize, elem: ArrayElem) -> Result<(), &'static str> {
        let sz = self.typecode.itemsize();
        let offset = idx.checked_mul(sz).ok_or("index overflow")?;
        let end = offset.checked_add(sz).ok_or("index overflow")?;
        if end > self.data.len() {
            return Err("index out of range");
        }
        let mut encoded = [0_u8; 8];
        let len = self.encode_elem_into(elem, &mut encoded)?;
        self.data[offset..end].copy_from_slice(&encoded[..len]);
        Ok(())
    }

    fn remove_at(&mut self, idx: usize) -> Result<ArrayElem, &'static str> {
        let sz = self.typecode.itemsize();
        let offset = idx.checked_mul(sz).ok_or("index out of range")?;
        let end = offset.checked_add(sz).ok_or("index out of range")?;
        if end > self.data.len() {
            return Err("index out of range");
        }
        let elem = self.decode_elem(&self.data[offset..end]);
        self.data.drain(offset..end);
        Ok(elem)
    }

    fn resize_blocked(&self) -> bool {
        self.exports.is_exported()
    }
}

// ---------------------------------------------------------------------------
// Handle <-> object plumbing
//
// An array handle is a refcounted `TYPE_ID_NATIVE_HANDLE` object whose payload
// is an `Arc<Mutex<ArrayHandle>>`. `Mutex` provides the interior mutability the
// `array` mutators need while satisfying the `Send + Sync` bound the native
// handle mechanism requires. The runtime executes under the GIL so there is no
// real contention. The lock protects only primitive storage work: Python
// coercion, result allocation and exception selection happen after it is released.
// ---------------------------------------------------------------------------

type ArrayCell = Mutex<ArrayHandle>;

// The array buffer-export lease. While a buffer is exported from an array (via
// the C buffer protocol — `molt_buffer_acquire` -> `array_buffer_owner_bits`),
// exactly one live `ArrayBufferLease` holds an `ArrayCell::exports` increment.
// The lease is wrapped in a refcounted native-handle object stored as the
// exported view's owner; when the view is released and that handle's refcount
// reaches zero, this `Drop` decrements `exports`. So `resize_blocked` reports
// true for precisely the window a buffer is live-exported (use-after-free
// guard: an array cannot be reallocated while a live view points into it).
pub(crate) struct ArrayBufferLease {
    cell: Arc<ArrayCell>,
}

impl Drop for ArrayBufferLease {
    fn drop(&mut self) {
        let guard = self
            .cell
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.exports.release();
    }
}

fn array_arc_from_bits(bits: u64) -> Option<Arc<ArrayCell>> {
    native_handle_arc::<ArrayCell>(bits)
}

fn array_lease_arc_from_bits(bits: u64) -> Option<Arc<ArrayBufferLease>> {
    native_handle_arc::<ArrayBufferLease>(bits)
}

fn array_bits(_py: &PyToken<'_>, handle: ArrayHandle) -> u64 {
    let bits = native_handle_new(_py, Arc::new(Mutex::new(handle)));
    if bits == 0 {
        return MoltObject::none().bits();
    }
    bits
}

// No error selection or Python-owned value may cross this storage phase. In
// particular, selecting OOM can retire an old exception and reenter the array.
#[derive(Clone, Copy)]
enum ArrayStorageError {
    Exception(&'static str, &'static str),
    Allocation,
    ExtendedSlice { replacement: usize, expected: usize },
}

impl ArrayStorageError {
    fn raise(self, py: &PyToken<'_>) -> u64 {
        match self {
            Self::Exception(kind, message) => raise_exception::<u64>(py, kind, message),
            Self::Allocation => {
                crate::record_memory_error_without_allocation(py);
                MoltObject::none().bits()
            }
            Self::ExtendedSlice {
                replacement,
                expected,
            } => raise_exception::<u64>(
                py,
                "ValueError",
                &format!(
                    "attempt to assign array of size {} to extended slice of size {}",
                    replacement, expected,
                ),
            ),
        }
    }
}

fn array_cell(py: &PyToken<'_>, bits: u64) -> Result<Arc<ArrayCell>, u64> {
    array_arc_from_bits(bits)
        .ok_or_else(|| raise_exception::<u64>(py, "TypeError", "invalid array handle"))
}

fn with_array_storage<T>(
    cell: &ArrayCell,
    f: impl FnOnce(&mut ArrayHandle) -> Result<T, ArrayStorageError>,
) -> Result<T, ArrayStorageError> {
    let mut guard = cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(&mut guard)
}

fn array_status(py: &PyToken<'_>, result: Result<(), ArrayStorageError>) -> u64 {
    match result {
        Ok(()) => MoltObject::none().bits(),
        Err(error) => error.raise(py),
    }
}

fn ensure_array_resizable(handle: &ArrayHandle) -> Result<(), ArrayStorageError> {
    if handle.resize_blocked() {
        Err(ArrayStorageError::Exception(
            "BufferError",
            "cannot resize an array that is exporting buffers",
        ))
    } else {
        Ok(())
    }
}

fn storage_elem(tc: Typecode, bits: u64) -> Result<ArrayElem, ArrayStorageError> {
    elem_from_bits_unraised(tc, bits)
        .map_err(|message| ArrayStorageError::Exception("TypeError", message))
}

fn copy_array_bytes(bytes: &[u8]) -> Result<Vec<u8>, ArrayStorageError> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len())
        .map_err(|_| ArrayStorageError::Allocation)?;
    copy.extend_from_slice(bytes);
    Ok(copy)
}

fn decode_array_slice(py: &PyToken<'_>, index: u64) -> Result<Option<DecodedSlice>, u64> {
    let Some(ptr) = obj_from_bits(index).as_ptr() else {
        return Ok(None);
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_SLICE {
            return Ok(None);
        }
        DecodedSlice::decode(
            py,
            obj_from_bits(slice_start_bits(ptr)),
            obj_from_bits(slice_stop_bits(ptr)),
            obj_from_bits(slice_step_bits(ptr)),
        )
        .map(Some)
        .map_err(|error| slice_error(py, error))
    }
}

#[cfg(test)]
pub(crate) fn array_storage_lock_available_for_test(bits: u64) -> bool {
    let Some(cell) = array_arc_from_bits(bits) else {
        return false;
    };
    cell.try_lock().is_ok()
}

/// Resolve the actual typed array handle from sealed physical storage. Buffer
/// capability checks must not allocate names or run arbitrary attribute/key
/// equality; acquisition and storage description use this same projection.
fn array_cell_from_export_source(py: &PyToken<'_>, bits: u64) -> Option<Arc<ArrayCell>> {
    if let Some(cell) = array_arc_from_bits(bits) {
        return Some(cell);
    }
    if let Some(lease) = array_lease_arc_from_bits(bits) {
        return Some(Arc::clone(&lease.cell));
    }
    let object = obj_from_bits(bits).as_ptr()?;
    unsafe {
        if object_type_id(object) != TYPE_ID_OBJECT {
            return None;
        }
        let handle = object_array_handle_bits(py, object)?;
        array_arc_from_bits(handle)
    }
}

unsafe fn object_array_handle_bits(py: &PyToken<'_>, object: *mut u8) -> Option<u64> {
    use crate::object::field_storage::{self, FieldStorage};
    use crate::object::ops::dict_get_str_bytes_borrowed;
    unsafe {
        if let Some(class) = obj_from_bits(object_class_bits(object)).as_ptr()
            && object_type_id(class) == crate::TYPE_ID_TYPE
            && let Some(offsets) =
                obj_from_bits(crate::object::layout::class_field_offsets_bits(class)).as_ptr()
            && let Some(offset) = dict_get_str_bytes_borrowed(py, offsets, b"_handle")
            && let Some(offset) =
                to_i64(obj_from_bits(offset)).and_then(|n| usize::try_from(n).ok())
            && let Some(field) = field_storage::field_at_offset(py, object, offset)
        {
            let slot = object.add(field.offset).cast::<u64>();
            let bits = match field_storage::resolve(py, object, field.offset, slot)? {
                FieldStorage::Inline(slot) => *slot,
                FieldStorage::Dictionary { dictionary, .. } => dict_get_str_bytes_borrowed(
                    py,
                    obj_from_bits(dictionary).as_ptr()?,
                    b"_handle",
                )?,
            };
            return (!is_missing_bits(py, bits)).then_some(bits);
        }
        let dictionary = obj_from_bits(instance_dict_bits(object)).as_ptr()?;
        dict_get_str_bytes_borrowed(py, dictionary, b"_handle")
    }
}

pub(crate) fn array_supports_buffer(py: &PyToken<'_>, bits: u64) -> bool {
    array_cell_from_export_source(py, bits).is_some()
}

pub(crate) fn array_storage_from_object_bits(
    _py: &PyToken<'_>,
    bits: u64,
) -> Result<TypedStridedStorage, TypedStridedStorageError> {
    let Some(cell) = array_cell_from_export_source(_py, bits) else {
        return Err(TypedStridedStorageError::NotBuffer);
    };
    let guard = cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let itemsize = guard.typecode.itemsize();
    let stride =
        isize::try_from(itemsize).map_err(|_| TypedStridedStorageError::InvalidDescriptor)?;
    let data = guard.data.as_ptr().cast_mut();
    // CPython exports native wchar as u (16-bit) or w (32-bit). The
    // descriptor is valid even though memoryview scalar access rejects it.
    let format = if guard.typecode == Typecode::U && itemsize == 4 {
        b'w'
    } else {
        guard.typecode.as_char() as u8
    };
    let storage =
        TypedStridedStorage::one_dim(data, false, guard.len(), itemsize, stride, 0, bits, 0);
    // C descriptors carry their format inline. No transient Python allocation
    // may run finalizers while the backing lock is held.
    let mut storage = storage.ok_or(TypedStridedStorageError::InvalidDescriptor)?;
    storage.format[0] = format;
    Ok(storage)
}

pub(crate) fn array_buffer_owner_bits(_py: &PyToken<'_>, bits: u64) -> Result<Option<u64>, ()> {
    if array_lease_arc_from_bits(bits).is_some() {
        // Derived consumers share the already-counted lease. Its native-handle
        // refcount keeps the pin until the last owner disappears; allocating a
        // second lease here would duplicate both the counter and heap work.
        inc_ref_bits(_py, bits);
        return Ok(Some(bits));
    }
    let Some(cell) = array_cell_from_export_source(_py, bits) else {
        return Ok(None);
    };
    let acquired = {
        let guard = cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.exports.acquire()
    };
    acquired.map_err(|()| {
        let _ = raise_exception::<u64>(_py, "OverflowError", "too many exported buffers");
    })?;
    let lease_bits = native_handle_new(
        _py,
        Arc::new(ArrayBufferLease {
            cell: Arc::clone(&cell),
        }),
    );
    if lease_bits == 0 {
        // native_handle_new consumes its Arc on failure; lease Drop already
        // released the pin. A second decrement steals another live export.
        return Err(());
    }
    Ok(Some(lease_bits))
}

fn repeat_count_from_bits(_py: &PyToken<'_>, count_bits: u64) -> Option<i64> {
    let err = format!(
        "can't multiply array by non-int of type '{}'",
        crate::class_name_for_error(crate::type_of_bits(_py, count_bits))
    );
    index_i64_with_overflow(
        _py,
        count_bits,
        &err,
        Some("cannot fit 'int' into an index-sized integer"),
    )
}

fn array_subscript_index_from_bits(_py: &PyToken<'_>, index_bits: u64) -> Option<i64> {
    sequence_index_i64_with_type_error(_py, index_bits, "array indices must be integers")
}

fn array_index_arg_from_bits(_py: &PyToken<'_>, index_bits: u64) -> Option<i64> {
    let err = format!(
        "'{}' object cannot be interpreted as an integer",
        crate::class_name_for_error(crate::type_of_bits(_py, index_bits))
    );
    index_i64_with_overflow(_py, index_bits, &err, None)
}

fn elem_from_bits_unraised(tc: Typecode, value_bits: u64) -> Result<ArrayElem, &'static str> {
    let obj = obj_from_bits(value_bits);
    if tc.is_float() {
        let Some(v) = to_f64(obj) else {
            return Err("a float is required");
        };
        return Ok(ArrayElem::Float(v));
    }
    if let Some(v) = to_i64(obj) {
        return Ok(ArrayElem::Int(v));
    }
    Err("an integer is required")
}

fn elem_to_bits(py: &PyToken<'_>, elem: ArrayElem) -> Option<u64> {
    let bits = match elem {
        ArrayElem::Int(v) => int_bits_from_i64(py, v),
        ArrayElem::Uint(v) => {
            if v <= i64::MAX as u64 {
                int_bits_from_i64(py, v as i64)
            } else {
                crate::int_bits_from_bigint(py, num_bigint::BigInt::from(v))
            }
        }
        ArrayElem::Float(v) => MoltObject::from_float(v).bits(),
    };
    (!obj_from_bits(bits).is_none()).then_some(bits)
}

fn typecode_from_obj(
    _py: &PyToken<'_>,
    typecode_bits: u64,
    argument_one: bool,
) -> Result<Typecode, u64> {
    let Some(s) = string_obj_to_owned(obj_from_bits(typecode_bits)) else {
        let msg = if argument_one {
            "array() argument 1 must be a unicode character, not int"
        } else {
            "typecode must be str"
        };
        return Err(raise_exception::<u64>(_py, "TypeError", msg));
    };
    let mut chars = s.chars();
    let ch = match chars.next() {
        Some(c) if chars.next().is_none() => c,
        _ => {
            let msg = if argument_one {
                "array() argument 1 must be a unicode character, not str"
            } else {
                "typecode must be a single character"
            };
            return Err(raise_exception::<u64>(_py, "ValueError", msg));
        }
    };
    Typecode::from_char(ch).ok_or_else(|| {
        let msg = if argument_one {
            "bad typecode (must be b, B, u, h, H, i, I, l, L, q, Q, f or d)"
        } else {
            "bad typecode"
        };
        raise_exception::<u64>(_py, "ValueError", msg)
    })
}

// ---------------------------------------------------------------------------
// Public intrinsics
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_new(typecode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let tc = match typecode_from_obj(_py, typecode_bits, true) {
            Ok(tc) => tc,
            Err(exc) => return exc,
        };
        array_bits(_py, ArrayHandle::new(tc))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_from_list(typecode_bits: u64, items_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let tc = match typecode_from_obj(py, typecode_bits, false) {
            Ok(tc) => tc,
            Err(error) => return error,
        };
        let mut handle = ArrayHandle::new(tc);
        let Some(list_ptr) = obj_from_bits(items_bits).as_ptr() else {
            return array_bits(py, handle);
        };
        let result: Result<(), ArrayStorageError> = unsafe {
            crate::object::seq_access::with_borrowed(list_ptr, |seq| {
                for &bits in seq {
                    let elem = storage_elem(tc, bits)?;
                    let mut encoded = [0; 8];
                    let size = handle
                        .encode_elem_into(elem, &mut encoded)
                        .map_err(|message| {
                            ArrayStorageError::Exception("OverflowError", message)
                        })?;
                    handle
                        .data
                        .try_reserve(size)
                        .map_err(|_| ArrayStorageError::Allocation)?;
                    handle.data.extend_from_slice(&encoded[..size]);
                }
                Ok(())
            })
        };
        if let Err(error) = result {
            return error.raise(py);
        }
        array_bits(py, handle)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_append(handle_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        array_status(
            py,
            with_array_storage(&cell, |handle| {
                let elem = storage_elem(handle.typecode, value_bits)?;
                let mut encoded = [0; 8];
                let size = handle
                    .encode_elem_into(elem, &mut encoded)
                    .map_err(|message| ArrayStorageError::Exception("OverflowError", message))?;
                ensure_array_resizable(handle)?;
                handle
                    .data
                    .try_reserve(size)
                    .map_err(|_| ArrayStorageError::Allocation)?;
                handle.data.extend_from_slice(&encoded[..size]);
                Ok(())
            }),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_extend(handle_bits: u64, items_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let Some(list_ptr) = obj_from_bits(items_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let result =
            with_array_storage(&cell, |handle| unsafe {
                // The existing list-input transaction stays atomic. Its borrowed
                // input and the array lock end before any error is selected.
                crate::object::seq_access::with_borrowed(list_ptr, |seq| {
                    let additional = seq.len().checked_mul(handle.typecode.itemsize()).ok_or(
                        ArrayStorageError::Exception("OverflowError", "array is too large"),
                    )?;
                    handle.data.len().checked_add(additional).ok_or(
                        ArrayStorageError::Exception("OverflowError", "array is too large"),
                    )?;
                    let mut encoded = Vec::new();
                    encoded
                        .try_reserve_exact(additional)
                        .map_err(|_| ArrayStorageError::Allocation)?;
                    for &bits in seq {
                        let elem = storage_elem(handle.typecode, bits)?;
                        let mut item = [0; 8];
                        let size = handle
                            .encode_elem_into(elem, &mut item)
                            .map_err(|message| {
                                ArrayStorageError::Exception("OverflowError", message)
                            })?;
                        encoded.extend_from_slice(&item[..size]);
                    }
                    if !encoded.is_empty() {
                        ensure_array_resizable(handle)?;
                    }
                    handle
                        .data
                        .try_reserve_exact(encoded.len())
                        .map_err(|_| ArrayStorageError::Allocation)?;
                    handle.data.extend_from_slice(&encoded);
                    Ok(())
                })
            });
        array_status(py, result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_repeat(handle_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let Some(count) = repeat_count_from_bits(py, count_bits) else {
            return MoltObject::none().bits();
        };
        let result = with_array_storage(&cell, |handle| {
            let mut out = ArrayHandle::new(handle.typecode);
            if count <= 0 || handle.data.is_empty() {
                return Ok(out);
            }
            let repeat = usize::try_from(count).map_err(|_| ArrayStorageError::Allocation)?;
            let total = handle
                .data
                .len()
                .checked_mul(repeat)
                .ok_or(ArrayStorageError::Allocation)?;
            out.data
                .try_reserve_exact(total)
                .map_err(|_| ArrayStorageError::Allocation)?;
            for _ in 0..repeat {
                out.data.extend_from_slice(&handle.data);
            }
            Ok(out)
        });
        match result {
            Ok(out) => array_bits(py, out),
            Err(error) => error.raise(py),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_repeat_in_place(handle_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let Some(count) = repeat_count_from_bits(py, count_bits) else {
            return MoltObject::none().bits();
        };
        array_status(
            py,
            with_array_storage(&cell, |handle| {
                if count <= 0 {
                    if !handle.data.is_empty() {
                        ensure_array_resizable(handle)?;
                    }
                    handle.data.clear();
                    return Ok(());
                }
                let repeat = usize::try_from(count).map_err(|_| ArrayStorageError::Allocation)?;
                if repeat == 1 || handle.data.is_empty() {
                    return Ok(());
                }
                ensure_array_resizable(handle)?;
                let snapshot = copy_array_bytes(&handle.data)?;
                let total = snapshot
                    .len()
                    .checked_mul(repeat)
                    .ok_or(ArrayStorageError::Allocation)?;
                handle
                    .data
                    .try_reserve_exact(total - snapshot.len())
                    .map_err(|_| ArrayStorageError::Allocation)?;
                for _ in 1..repeat {
                    handle.data.extend_from_slice(&snapshot);
                }
                Ok(())
            }),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_getitem(handle_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let slice = match decode_array_slice(py, index_bits) {
            Ok(slice) => slice,
            Err(error) => return error,
        };
        if let Some(slice) = slice {
            let result = with_array_storage(&cell, |handle| {
                let (start, stop, step) = slice.adjust(handle.len() as isize);
                let mut out = ArrayHandle::new(handle.typecode);
                let size = handle.typecode.itemsize();
                if step == 1 {
                    let stop = stop.max(start);
                    out.data = copy_array_bytes(
                        &handle.data[start as usize * size..stop as usize * size],
                    )?;
                } else {
                    let indices = collect_slice_indices(start, stop, step);
                    out.data
                        .try_reserve_exact(indices.len() * size)
                        .map_err(|_| ArrayStorageError::Allocation)?;
                    for index in indices {
                        out.data
                            .extend_from_slice(&handle.data[index * size..(index + 1) * size]);
                    }
                }
                Ok(out)
            });
            return match result {
                Ok(out) => array_bits(py, out),
                Err(error) => error.raise(py),
            };
        }
        let Some(index) = array_subscript_index_from_bits(py, index_bits) else {
            return MoltObject::none().bits();
        };
        let result = with_array_storage(&cell, |handle| {
            let len = handle.len() as i64;
            let index = if index < 0 { len + index } else { index };
            if index < 0 || index >= len {
                return Err(ArrayStorageError::Exception(
                    "IndexError",
                    "array index out of range",
                ));
            }
            handle
                .read_elem(index as usize)
                .ok_or(ArrayStorageError::Exception(
                    "IndexError",
                    "array index out of range",
                ))
        });
        match result {
            Ok(elem) => elem_to_bits(py, elem).unwrap_or_else(|| MoltObject::none().bits()),
            Err(error) => error.raise(py),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_setitem(handle_bits: u64, index_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let slice = match decode_array_slice(py, index_bits) {
            Ok(slice) => slice,
            Err(error) => return error,
        };
        if let Some(slice) = slice {
            let Some(value_cell) = array_arc_from_bits(value_bits) else {
                return raise_exception::<u64>(
                    py,
                    "TypeError",
                    &format!(
                        "can only assign array (not \"{}\") to array slice",
                        crate::class_name_for_error(crate::type_of_bits(py, value_bits)),
                    ),
                );
            };
            let replacement = with_array_storage(&value_cell, |other| {
                Ok((copy_array_bytes(&other.data)?, other.typecode))
            });
            let (replacement, value_tc) = match replacement {
                Ok(value) => value,
                Err(error) => return error.raise(py),
            };
            return array_status(
                py,
                with_array_storage(&cell, |handle| {
                    if value_tc != handle.typecode {
                        return Err(ArrayStorageError::Exception(
                            "TypeError",
                            "bad argument type for built-in operation",
                        ));
                    }
                    let (start, stop, step) = slice.adjust(handle.len() as isize);
                    let size = handle.typecode.itemsize();
                    let replacement_len = replacement.len() / size;
                    if step == 1 {
                        let stop = stop.max(start);
                        let removed = (stop - start) as usize;
                        if replacement_len != removed {
                            ensure_array_resizable(handle)?;
                        }
                        let additional = replacement.len().saturating_sub(removed * size);
                        handle
                            .data
                            .try_reserve_exact(additional)
                            .map_err(|_| ArrayStorageError::Allocation)?;
                        handle
                            .data
                            .splice(start as usize * size..stop as usize * size, replacement);
                    } else {
                        let indices = collect_slice_indices(start, stop, step);
                        if indices.len() != replacement_len {
                            return Err(ArrayStorageError::ExtendedSlice {
                                replacement: replacement_len,
                                expected: indices.len(),
                            });
                        }
                        for (source, index) in indices.into_iter().enumerate() {
                            handle.data[index * size..(index + 1) * size]
                                .copy_from_slice(&replacement[source * size..(source + 1) * size]);
                        }
                    }
                    Ok(())
                }),
            );
        }
        let Some(index) = array_subscript_index_from_bits(py, index_bits) else {
            return MoltObject::none().bits();
        };
        array_status(
            py,
            with_array_storage(&cell, |handle| {
                let len = handle.len() as i64;
                let index = if index < 0 { len + index } else { index };
                if index < 0 || index >= len {
                    return Err(ArrayStorageError::Exception(
                        "IndexError",
                        "array assignment index out of range",
                    ));
                }
                let elem = storage_elem(handle.typecode, value_bits)?;
                handle
                    .set_elem(index as usize, elem)
                    .map_err(|message| ArrayStorageError::Exception("OverflowError", message))
            }),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_delitem(handle_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let slice = match decode_array_slice(py, index_bits) {
            Ok(slice) => slice,
            Err(error) => return error,
        };
        if let Some(slice) = slice {
            return array_status(
                py,
                with_array_storage(&cell, |handle| {
                    let (start, stop, step) = slice.adjust(handle.len() as isize);
                    let size = handle.typecode.itemsize();
                    if step == 1 {
                        if start < stop {
                            ensure_array_resizable(handle)?;
                            handle
                                .data
                                .drain(start as usize * size..stop as usize * size);
                        }
                    } else {
                        let mut indices = collect_slice_indices(start, stop, step);
                        if !indices.is_empty() {
                            ensure_array_resizable(handle)?;
                        }
                        indices.sort_unstable_by(|a, b| b.cmp(a));
                        for index in indices {
                            handle.data.drain(index * size..(index + 1) * size);
                        }
                    }
                    Ok(())
                }),
            );
        }
        let Some(index) = array_subscript_index_from_bits(py, index_bits) else {
            return MoltObject::none().bits();
        };
        array_status(
            py,
            with_array_storage(&cell, |handle| {
                let len = handle.len() as i64;
                let index = if index < 0 { len + index } else { index };
                if index < 0 || index >= len {
                    return Err(ArrayStorageError::Exception(
                        "IndexError",
                        "array assignment index out of range",
                    ));
                }
                ensure_array_resizable(handle)?;
                handle
                    .remove_at(index as usize)
                    .map(|_| ())
                    .map_err(|message| ArrayStorageError::Exception("IndexError", message))
            }),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_len(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let value = with_array_storage(&cell, |handle| Ok(handle.len()));
        match value {
            Ok(value) => int_bits_from_i64(py, value as i64),
            Err(error) => error.raise(py),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_typecode(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let value = with_array_storage(&cell, |handle| Ok(handle.typecode.as_char() as u8));
        let byte = match value {
            Ok(byte) => byte,
            Err(error) => return error.raise(py),
        };
        let ptr = alloc_string(py, &[byte]);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_itemsize(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let value = with_array_storage(&cell, |handle| Ok(handle.typecode.itemsize()));
        match value {
            Ok(value) => int_bits_from_i64(py, value as i64),
            Err(error) => error.raise(py),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_tobytes(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        // alloc_bytes may select OOM and reenter this array. Own primitive bytes
        // across that call; an exported-buffer pin would change resize behavior.
        let bytes = match with_array_storage(&cell, |handle| copy_array_bytes(&handle.data)) {
            Ok(bytes) => bytes,
            Err(error) => return error.raise(py),
        };
        let ptr = alloc_bytes(py, &bytes);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_frombytes(handle_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let data = unsafe {
            obj_from_bits(data_bits)
                .as_ptr()
                .and_then(|ptr| crate::bytes_like_slice(ptr))
                .ok_or(ArrayStorageError::Exception(
                    "TypeError",
                    "a bytes-like object is required",
                ))
                .and_then(copy_array_bytes)
        };
        let data = match data {
            Ok(data) => data,
            Err(error) => return error.raise(py),
        };
        array_status(
            py,
            with_array_storage(&cell, |handle| {
                if data.len() % handle.typecode.itemsize() != 0 {
                    return Err(ArrayStorageError::Exception(
                        "ValueError",
                        "bytes length not a multiple of item size",
                    ));
                }
                if !data.is_empty() {
                    ensure_array_resizable(handle)?;
                }
                handle
                    .data
                    .try_reserve(data.len())
                    .map_err(|_| ArrayStorageError::Allocation)?;
                handle.data.extend_from_slice(&data);
                Ok(())
            }),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_tolist(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let result = with_array_storage(&cell, |handle| {
            let mut elems = Vec::new();
            elems
                .try_reserve_exact(handle.len())
                .map_err(|_| ArrayStorageError::Allocation)?;
            for index in 0..handle.len() {
                elems.push(handle.read_elem(index).ok_or(ArrayStorageError::Exception(
                    "RuntimeError",
                    "array data corrupted",
                ))?);
            }
            Ok(elems)
        });
        let elems = match result {
            Ok(elems) => elems,
            Err(error) => return error.raise(py),
        };
        let list = crate::object::builders::alloc_list_with_capacity(py, &[], elems.len());
        if list.is_null() {
            return MoltObject::none().bits();
        }
        let owner = unsafe {
            OwnedRuntimeValue::from_owned_bits(py.core_token(), MoltObject::from_ptr(list).bits())
        };
        for elem in elems {
            let Some(bits) = elem_to_bits(py, elem) else {
                return MoltObject::none().bits();
            };
            unsafe {
                crate::object::list_mutation::append_owned_unpublished(list, bits);
            }
        }
        owner.into_bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_buffer_info(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let result = with_array_storage(&cell, |handle| {
            Ok((handle.data.as_ptr() as usize as i64, handle.len() as i64))
        });
        let (addr, length) = match result {
            Ok(pair) => pair,
            Err(error) => return error.raise(py),
        };
        let Some(addr) = elem_to_bits(py, ArrayElem::Int(addr)) else {
            return MoltObject::none().bits();
        };
        let addr = unsafe { OwnedRuntimeValue::from_owned_bits(py.core_token(), addr) };
        let Some(length) = elem_to_bits(py, ArrayElem::Int(length)) else {
            return MoltObject::none().bits();
        };
        let length = unsafe { OwnedRuntimeValue::from_owned_bits(py.core_token(), length) };
        let tuple = alloc_tuple(py, &[addr.bits(), length.bits()]);
        if tuple.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_pop(handle_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        // Clinic coerces the index before the implementation checks emptiness.
        let Some(index) = array_index_arg_from_bits(py, index_bits) else {
            return MoltObject::none().bits();
        };
        let result = with_array_storage(&cell, |handle| {
            let len = handle.len() as i64;
            if len == 0 {
                return Err(ArrayStorageError::Exception(
                    "IndexError",
                    "pop from empty array",
                ));
            }
            let index = if index < 0 { len + index } else { index };
            if index < 0 || index >= len {
                return Err(ArrayStorageError::Exception(
                    "IndexError",
                    "pop index out of range",
                ));
            }
            let elem = handle
                .read_elem(index as usize)
                .ok_or(ArrayStorageError::Exception(
                    "IndexError",
                    "pop index out of range",
                ))?;
            Ok((index as usize, elem))
        });
        let (index, elem) = match result {
            Ok(value) => value,
            Err(error) => return error.raise(py),
        };
        // CPython materializes the result before deleting its slot. A failed
        // materialization must not remove an element.
        let Some(bits) = elem_to_bits(py, elem) else {
            return MoltObject::none().bits();
        };
        let value = unsafe { OwnedRuntimeValue::from_owned_bits(py.core_token(), bits) };
        let result = with_array_storage(&cell, |handle| {
            // Allocation may have reentered. The original deletion slice is
            // clamped to the current size, as array_del_slice does.
            if index < handle.len() {
                ensure_array_resizable(handle)?;
                handle
                    .remove_at(index)
                    .map_err(|message| ArrayStorageError::Exception("IndexError", message))?;
            }
            Ok(())
        });
        if let Err(error) = result {
            return error.raise(py);
        }
        value.into_bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_insert(handle_bits: u64, index_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let Some(index) = array_index_arg_from_bits(py, index_bits) else {
            return MoltObject::none().bits();
        };
        array_status(
            py,
            with_array_storage(&cell, |handle| {
                let elem = storage_elem(handle.typecode, value_bits)?;
                let mut encoded = [0; 8];
                let size = handle
                    .encode_elem_into(elem, &mut encoded)
                    .map_err(|message| ArrayStorageError::Exception("OverflowError", message))?;
                let len = handle.len() as i64;
                let index = if index < 0 {
                    (len + index).max(0)
                } else {
                    index.min(len)
                } as usize;
                ensure_array_resizable(handle)?;
                handle
                    .data
                    .try_reserve(size)
                    .map_err(|_| ArrayStorageError::Allocation)?;
                handle
                    .data
                    .splice(index * size..index * size, encoded[..size].iter().copied());
                Ok(())
            }),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_remove(handle_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let result = with_array_storage(&cell, |handle| {
            let tc = handle.typecode;
            let target = storage_elem(tc, value_bits)?;
            let n = handle.len();
            let mut found: Option<usize> = None;
            for i in 0..n {
                if let Some(elem) = handle.read_elem(i) {
                    let matches = match (elem, target) {
                        (ArrayElem::Int(a), ArrayElem::Int(b)) => a == b,
                        (ArrayElem::Uint(a), ArrayElem::Uint(b)) => a == b,
                        (ArrayElem::Float(a), ArrayElem::Float(b)) => a == b,
                        (ArrayElem::Int(a), ArrayElem::Uint(b)) => a >= 0 && a as u64 == b,
                        (ArrayElem::Uint(a), ArrayElem::Int(b)) => b >= 0 && a == b as u64,
                        _ => false,
                    };
                    if matches {
                        found = Some(i);
                        break;
                    }
                }
            }
            match found {
                Some(idx) => {
                    ensure_array_resizable(handle)?;
                    let _ = handle.remove_at(idx);
                    Ok(())
                }
                None => Err(ArrayStorageError::Exception(
                    "ValueError",
                    "array.remove(x): x not in array",
                )),
            }
        });
        array_status(py, result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_reverse(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let result = with_array_storage(&cell, |handle| {
            let sz = handle.typecode.itemsize();
            let n = handle.len();
            let mut left = 0usize;
            let mut right = if n > 0 { n - 1 } else { 0 };
            while left < right {
                let lo = left * sz;
                let hi = right * sz;
                for k in 0..sz {
                    handle.data.swap(lo + k, hi + k);
                }
                left += 1;
                right -= 1;
            }
            Ok(())
        });
        array_status(py, result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_count(handle_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let result = with_array_storage(&cell, |handle| {
            let tc = handle.typecode;
            let target = storage_elem(tc, value_bits)?;
            // SIMD fast path for byte typecodes: use memchr for O(n/16) counting
            if matches!(tc, Typecode::UB) {
                if let ArrayElem::Uint(v) = target {
                    if v <= 255 {
                        let needle = v as u8;
                        let count = memchr::memchr_iter(needle, &handle.data).count() as i64;
                        return Ok(count);
                    }
                } else if let ArrayElem::Int(v) = target
                    && (0..=255).contains(&v)
                {
                    let needle = v as u8;
                    let count = memchr::memchr_iter(needle, &handle.data).count() as i64;
                    return Ok(count);
                }
            }
            if matches!(tc, Typecode::B)
                && let ArrayElem::Int(v) = target
                && (-128..=127).contains(&v)
            {
                let needle = v as u8; // Two's complement byte
                let count = memchr::memchr_iter(needle, &handle.data).count() as i64;
                return Ok(count);
            }

            let mut count = 0i64;
            for i in 0..handle.len() {
                if let Some(elem) = handle.read_elem(i) {
                    let matches = match (elem, target) {
                        (ArrayElem::Int(a), ArrayElem::Int(b)) => a == b,
                        (ArrayElem::Uint(a), ArrayElem::Uint(b)) => a == b,
                        (ArrayElem::Float(a), ArrayElem::Float(b)) => a == b,
                        (ArrayElem::Int(a), ArrayElem::Uint(b)) => a >= 0 && a as u64 == b,
                        (ArrayElem::Uint(a), ArrayElem::Int(b)) => b >= 0 && a == b as u64,
                        _ => false,
                    };
                    if matches {
                        count += 1;
                    }
                }
            }
            Ok(count)
        });
        match result {
            Ok(value) => int_bits_from_i64(py, value),
            Err(error) => error.raise(py),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_index(handle_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let cell = match array_cell(py, handle_bits) {
            Ok(cell) => cell,
            Err(error) => return error,
        };
        let result = with_array_storage(&cell, |handle| {
            let tc = handle.typecode;
            let target = storage_elem(tc, value_bits)?;
            // SIMD fast path for byte typecodes: use memchr for O(n/16) search
            if matches!(tc, Typecode::UB) {
                if let ArrayElem::Uint(v) = target {
                    if v <= 255 {
                        if let Some(pos) = memchr::memchr(v as u8, &handle.data) {
                            return Ok(pos as i64);
                        }
                        return Err(ArrayStorageError::Exception(
                            "ValueError",
                            "array.index(x): x not in array",
                        ));
                    }
                } else if let ArrayElem::Int(v) = target
                    && (0..=255).contains(&v)
                {
                    if let Some(pos) = memchr::memchr(v as u8, &handle.data) {
                        return Ok(pos as i64);
                    }
                    return Err(ArrayStorageError::Exception(
                        "ValueError",
                        "array.index(x): x not in array",
                    ));
                }
            }
            if matches!(tc, Typecode::B)
                && let ArrayElem::Int(v) = target
                && (-128..=127).contains(&v)
            {
                if let Some(pos) = memchr::memchr(v as u8, &handle.data) {
                    return Ok(pos as i64);
                }
                return Err(ArrayStorageError::Exception(
                    "ValueError",
                    "array.index(x): x not in array",
                ));
            }

            for i in 0..handle.len() {
                if let Some(elem) = handle.read_elem(i) {
                    let matches = match (elem, target) {
                        (ArrayElem::Int(a), ArrayElem::Int(b)) => a == b,
                        (ArrayElem::Uint(a), ArrayElem::Uint(b)) => a == b,
                        (ArrayElem::Float(a), ArrayElem::Float(b)) => a == b,
                        (ArrayElem::Int(a), ArrayElem::Uint(b)) => a >= 0 && a as u64 == b,
                        (ArrayElem::Uint(a), ArrayElem::Int(b)) => b >= 0 && a == b as u64,
                        _ => false,
                    };
                    if matches {
                        return Ok(i as i64);
                    }
                }
            }
            Err(ArrayStorageError::Exception(
                "ValueError",
                "array.index(x): x not in array",
            ))
        });
        match result {
            Ok(value) => int_bits_from_i64(py, value),
            Err(error) => error.raise(py),
        }
    })
}
