// === FILE: runtime/molt-runtime/src/builtins/array_mod.rs ===
//
// Typed array module intrinsics (CPython `array` module semantics).
// Handles are heap-allocated ArrayHandle structs registered with the
// provenance registry so the GC can track them.

use crate::builtins::numbers::index_i64_with_overflow;
use crate::object::ops::string_obj_to_owned;
use crate::{
    MoltObject, PyToken, alloc_bytes, alloc_list, alloc_string, alloc_tuple, bits_from_ptr,
    dec_ref_bits, int_bits_from_i64, obj_from_bits, ptr_from_bits, raise_exception, release_ptr,
    slice_start_bits, slice_step_bits, slice_stop_bits, to_f64, to_i64, TYPE_ID_SLICE,
};
use crate::object::ops_sys::{collect_slice_indices, normalize_slice_indices, slice_error};

// ---------------------------------------------------------------------------
// Typecode metadata
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Typecode {
    B,  // 'b' signed char  (1 byte)
    UB, // 'B' unsigned char (1 byte)
    U,  // 'u' Py_UCS4 (4 bytes, deprecated but supported)
    H,  // 'h' signed short (2 bytes)
    UH, // 'H' unsigned short (2 bytes)
    I,  // 'i' signed int (2 bytes per CPython minimum, typically 4)
    UI, // 'I' unsigned int (4 bytes)
    L,  // 'l' signed long (4 bytes)
    UL, // 'L' unsigned long (4 bytes)
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
        match self {
            Typecode::B | Typecode::UB => 1,
            Typecode::H | Typecode::UH => 2,
            Typecode::U | Typecode::I | Typecode::UI | Typecode::L | Typecode::UL | Typecode::F => {
                4
            }
            Typecode::Q | Typecode::UQ | Typecode::D => 8,
        }
    }

    fn is_float(self) -> bool {
        matches!(self, Typecode::F | Typecode::D)
    }
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

struct ArrayHandle {
    typecode: Typecode,
    data: Vec<u8>,
}

impl ArrayHandle {
    fn new(typecode: Typecode) -> Self {
        ArrayHandle {
            typecode,
            data: Vec::new(),
        }
    }

    fn len(&self) -> usize {
        let itemsize = self.typecode.itemsize();
        if itemsize == 0 {
            0
        } else {
            self.data.len() / itemsize
        }
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
                let v = u32::from_le_bytes(bytes[..4].try_into().unwrap_or([0u8; 4]));
                ArrayElem::Uint(v as u64)
            }
            Typecode::H => {
                let v = i16::from_le_bytes(bytes[..2].try_into().unwrap_or([0u8; 2]));
                ArrayElem::Int(v as i64)
            }
            Typecode::UH => {
                let v = u16::from_le_bytes(bytes[..2].try_into().unwrap_or([0u8; 2]));
                ArrayElem::Uint(v as u64)
            }
            Typecode::I => {
                let v = i32::from_le_bytes(bytes[..4].try_into().unwrap_or([0u8; 4]));
                ArrayElem::Int(v as i64)
            }
            Typecode::UI => {
                let v = u32::from_le_bytes(bytes[..4].try_into().unwrap_or([0u8; 4]));
                ArrayElem::Uint(v as u64)
            }
            Typecode::L => {
                let v = i32::from_le_bytes(bytes[..4].try_into().unwrap_or([0u8; 4]));
                ArrayElem::Int(v as i64)
            }
            Typecode::UL => {
                let v = u32::from_le_bytes(bytes[..4].try_into().unwrap_or([0u8; 4]));
                ArrayElem::Uint(v as u64)
            }
            Typecode::Q => {
                let v = i64::from_le_bytes(bytes[..8].try_into().unwrap_or([0u8; 8]));
                ArrayElem::Int(v)
            }
            Typecode::UQ => {
                let v = u64::from_le_bytes(bytes[..8].try_into().unwrap_or([0u8; 8]));
                ArrayElem::Uint(v)
            }
            Typecode::F => {
                let v = f32::from_le_bytes(bytes[..4].try_into().unwrap_or([0u8; 4]));
                ArrayElem::Float(v as f64)
            }
            Typecode::D => {
                let v = f64::from_le_bytes(bytes[..8].try_into().unwrap_or([0u8; 8]));
                ArrayElem::Float(v)
            }
        }
    }

    fn encode_elem(&self, elem: ArrayElem) -> Result<Vec<u8>, &'static str> {
        let sz = self.typecode.itemsize();
        let mut buf = vec![0u8; sz];
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
                buf.copy_from_slice(&(v as u32).to_le_bytes());
            }
            Typecode::H => {
                let v = elem.to_i64().ok_or("expected int for typecode 'h'")?;
                if !(-32768..=32767).contains(&v) {
                    return Err("value out of range for typecode 'h'");
                }
                buf.copy_from_slice(&(v as i16).to_le_bytes());
            }
            Typecode::UH => {
                let v = elem.to_i64().ok_or("expected int for typecode 'H'")?;
                if !(0..=65535).contains(&v) {
                    return Err("value out of range for typecode 'H'");
                }
                buf.copy_from_slice(&(v as u16).to_le_bytes());
            }
            Typecode::I | Typecode::L => {
                let v = elem.to_i64().ok_or("expected int for typecode 'i'/'l'")?;
                if !(-2147483648..=2147483647).contains(&v) {
                    return Err("value out of range for typecode 'i'/'l'");
                }
                buf.copy_from_slice(&(v as i32).to_le_bytes());
            }
            Typecode::UI | Typecode::UL => {
                let v = elem.to_i64().ok_or("expected int for typecode 'I'/'L'")?;
                if !(0..=4294967295).contains(&v) {
                    return Err("value out of range for typecode 'I'/'L'");
                }
                buf.copy_from_slice(&(v as u32).to_le_bytes());
            }
            Typecode::Q => {
                let v = elem.to_i64().ok_or("expected int for typecode 'q'")?;
                buf.copy_from_slice(&v.to_le_bytes());
            }
            Typecode::UQ => match elem {
                ArrayElem::Uint(v) => buf.copy_from_slice(&v.to_le_bytes()),
                ArrayElem::Int(v) if v >= 0 => buf.copy_from_slice(&(v as u64).to_le_bytes()),
                _ => return Err("value out of range for typecode 'Q'"),
            },
            Typecode::F => {
                let v = elem.to_f64() as f32;
                buf.copy_from_slice(&v.to_le_bytes());
            }
            Typecode::D => {
                let v = elem.to_f64();
                buf.copy_from_slice(&v.to_le_bytes());
            }
        }
        Ok(buf)
    }

    fn push_elem(&mut self, elem: ArrayElem) -> Result<(), &'static str> {
        let encoded = self.encode_elem(elem)?;
        self.data.extend_from_slice(&encoded);
        Ok(())
    }

    fn set_elem(&mut self, idx: usize, elem: ArrayElem) -> Result<(), &'static str> {
        let sz = self.typecode.itemsize();
        let offset = idx.checked_mul(sz).ok_or("index overflow")?;
        let end = offset.checked_add(sz).ok_or("index overflow")?;
        if end > self.data.len() {
            return Err("index out of range");
        }
        let encoded = self.encode_elem(elem)?;
        self.data[offset..end].copy_from_slice(&encoded);
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

    fn insert_at(&mut self, idx: usize, elem: ArrayElem) -> Result<(), &'static str> {
        let sz = self.typecode.itemsize();
        let clamped = idx.min(self.len());
        let offset = clamped * sz;
        let encoded = self.encode_elem(elem)?;
        self.data.splice(offset..offset, encoded);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn array_handle_from_bits(bits: u64) -> Option<&'static mut ArrayHandle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    // SAFETY: pointer originates from Box::into_raw for an ArrayHandle.
    Some(unsafe { &mut *(ptr as *mut ArrayHandle) })
}

fn array_handle_ptr_from_bits(bits: u64) -> *mut ArrayHandle {
    ptr_from_bits(bits) as *mut ArrayHandle
}

fn array_bits(handle: ArrayHandle) -> u64 {
    bits_from_ptr(Box::into_raw(Box::new(handle)) as *mut u8)
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

fn elem_from_bits(_py: &PyToken<'_>, tc: Typecode, value_bits: u64) -> Result<ArrayElem, u64> {
    let obj = obj_from_bits(value_bits);
    if tc.is_float() {
        let Some(v) = to_f64(obj) else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "a float is required",
            ));
        };
        return Ok(ArrayElem::Float(v));
    }
    if let Some(v) = to_i64(obj) {
        return Ok(ArrayElem::Int(v));
    }
    Err(raise_exception::<u64>(
        _py,
        "TypeError",
        "an integer is required",
    ))
}

fn elem_to_bits(_py: &PyToken<'_>, elem: ArrayElem) -> u64 {
    match elem {
        ArrayElem::Int(v) => int_bits_from_i64(_py, v),
        ArrayElem::Uint(v) => {
            if v <= i64::MAX as u64 {
                int_bits_from_i64(_py, v as i64)
            } else {
                // Encode as bigint via string conversion path.
                use num_bigint::BigInt;
                let big = BigInt::from(v);
                crate::int_bits_from_bigint(_py, big)
            }
        }
        ArrayElem::Float(v) => MoltObject::from_float(v).bits(),
    }
}

// ---------------------------------------------------------------------------
// Public intrinsics
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_new(typecode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(s) = string_obj_to_owned(obj_from_bits(typecode_bits)) else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "array() argument 1 must be a unicode character, not int",
            );
        };
        let mut chars = s.chars();
        let ch = match chars.next() {
            Some(c) if chars.next().is_none() => c,
            _ => {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "array() argument 1 must be a unicode character, not str",
                );
            }
        };
        let Some(tc) = Typecode::from_char(ch) else {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "bad typecode (must be b, B, u, h, H, i, I, l, L, q, Q, f or d)",
            );
        };
        array_bits(ArrayHandle::new(tc))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_from_list(typecode_bits: u64, items_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(s) = string_obj_to_owned(obj_from_bits(typecode_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "typecode must be str");
        };
        let mut chars = s.chars();
        let ch = match chars.next() {
            Some(c) if chars.next().is_none() => c,
            _ => {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "typecode must be a single character",
                );
            }
        };
        let Some(tc) = Typecode::from_char(ch) else {
            return raise_exception::<u64>(_py, "ValueError", "bad typecode");
        };
        let mut handle = ArrayHandle::new(tc);

        let items_obj = obj_from_bits(items_bits);
        let Some(list_ptr) = items_obj.as_ptr() else {
            return array_bits(handle);
        };
        let n = unsafe { crate::list_len(list_ptr) };
        let seq_vec_ptr = unsafe { crate::seq_vec_ptr(list_ptr) };
        let seq = unsafe { &*seq_vec_ptr };

        for &elem_bits in seq.iter().take(n) {
            let elem = match elem_from_bits(_py, tc, elem_bits) {
                Ok(e) => e,
                Err(exc) => return exc,
            };
            if let Err(msg) = handle.push_elem(elem) {
                return raise_exception::<u64>(_py, "OverflowError", msg);
            }
        }
        array_bits(handle)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_append(handle_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        let tc = handle.typecode;
        let elem = match elem_from_bits(_py, tc, value_bits) {
            Ok(e) => e,
            Err(exc) => return exc,
        };
        if let Err(msg) = handle.push_elem(elem) {
            return raise_exception::<u64>(_py, "OverflowError", msg);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_extend(handle_bits: u64, items_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        let tc = handle.typecode;
        let items_obj = obj_from_bits(items_bits);
        let Some(list_ptr) = items_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        let n = unsafe { crate::list_len(list_ptr) };
        let seq_vec_ptr = unsafe { crate::seq_vec_ptr(list_ptr) };
        let seq = unsafe { &*seq_vec_ptr };
        // Collect first to avoid partial mutation on error.
        let mut encoded: Vec<Vec<u8>> = Vec::with_capacity(n);
        for &elem_bits in seq.iter().take(n) {
            let elem = match elem_from_bits(_py, tc, elem_bits) {
                Ok(e) => e,
                Err(exc) => return exc,
            };
            match handle.encode_elem(elem) {
                Ok(bytes) => encoded.push(bytes),
                Err(msg) => return raise_exception::<u64>(_py, "OverflowError", msg),
            }
        }
        for bytes in encoded {
            handle.data.extend_from_slice(&bytes);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_repeat(handle_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        let Some(count) = repeat_count_from_bits(_py, count_bits) else {
            return MoltObject::none().bits();
        };
        let mut out = ArrayHandle::new(handle.typecode);
        if count <= 0 || handle.data.is_empty() {
            return array_bits(out);
        }
        let repeat = match usize::try_from(count) {
            Ok(value) => value,
            Err(_) => {
                return raise_exception::<u64>(
                    _py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            }
        };
        let item_count = handle.len();
        let total_items = match item_count.checked_mul(repeat) {
            Some(total) => total,
            None => {
                return raise_exception::<u64>(
                    _py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            }
        };
        let total_bytes = match total_items.checked_mul(handle.typecode.itemsize()) {
            Some(total) => total,
            None => {
                return raise_exception::<u64>(
                    _py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            }
        };
        out.data.reserve(total_bytes);
        for _ in 0..repeat {
            out.data.extend_from_slice(&handle.data);
        }
        array_bits(out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_repeat_in_place(handle_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        let Some(count) = repeat_count_from_bits(_py, count_bits) else {
            return MoltObject::none().bits();
        };
        if count <= 0 {
            handle.data.clear();
            return MoltObject::none().bits();
        }
        let repeat = match usize::try_from(count) {
            Ok(value) => value,
            Err(_) => {
                return raise_exception::<u64>(
                    _py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            }
        };
        if repeat == 1 || handle.data.is_empty() {
            return MoltObject::none().bits();
        }
        let snapshot = handle.data.clone();
        let total_len = match snapshot.len().checked_mul(repeat) {
            Some(total) => total,
            None => {
                return raise_exception::<u64>(
                    _py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            }
        };
        handle
            .data
            .reserve(total_len.saturating_sub(snapshot.len()));
        for _ in 1..repeat {
            handle.data.extend_from_slice(&snapshot);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_getitem(handle_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        let index_obj = obj_from_bits(index_bits);
        if let Some(slice_ptr) = index_obj.as_ptr() {
            unsafe {
                if crate::object_type_id(slice_ptr) == TYPE_ID_SLICE {
                    let len = handle.len() as isize;
                    let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                    let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                    let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                    let (start, stop, step) = match normalize_slice_indices(
                        _py, len, start_obj, stop_obj, step_obj,
                    ) {
                        Ok(value) => value,
                        Err(err) => return slice_error(_py, err),
                    };
                    let itemsize = handle.typecode.itemsize();
                    let mut out = ArrayHandle::new(handle.typecode);
                    if step == 1 {
                        let start_byte = start as usize * itemsize;
                        let stop_byte = stop as usize * itemsize;
                        out.data.extend_from_slice(&handle.data[start_byte..stop_byte]);
                    } else {
                        for idx in collect_slice_indices(start, stop, step) {
                            let start_byte = idx * itemsize;
                            let end_byte = start_byte + itemsize;
                            out.data.extend_from_slice(&handle.data[start_byte..end_byte]);
                        }
                    }
                    return array_bits(out);
                }
            }
        }
        let Some(idx_raw) = to_i64(index_obj) else {
            return raise_exception::<u64>(_py, "TypeError", "array indices must be integers");
        };
        let len = handle.len() as i64;
        let idx = if idx_raw < 0 { len + idx_raw } else { idx_raw };
        if idx < 0 || idx >= len {
            return raise_exception::<u64>(_py, "IndexError", "array index out of range");
        }
        let Some(elem) = handle.read_elem(idx as usize) else {
            return raise_exception::<u64>(_py, "IndexError", "array index out of range");
        };
        elem_to_bits(_py, elem)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_setitem(handle_bits: u64, index_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_ptr = array_handle_ptr_from_bits(handle_bits);
        if handle_ptr.is_null() {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        }
        let index_obj = obj_from_bits(index_bits);
        unsafe {
            if let Some(slice_ptr) = index_obj.as_ptr()
                && crate::object_type_id(slice_ptr) == TYPE_ID_SLICE
            {
                let tc = (*handle_ptr).typecode;
                let value_ptr = array_handle_ptr_from_bits(value_bits);
                if value_ptr.is_null() {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        &format!(
                            "can only assign array (not \"{}\") to array slice",
                            crate::class_name_for_error(crate::type_of_bits(_py, value_bits))
                        ),
                    );
                }
                if (*value_ptr).typecode != tc {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "bad argument type for built-in operation",
                    );
                }
                let replacement = (*value_ptr).data.clone();
                let replacement_len = replacement.len() / tc.itemsize();
                let len = (*handle_ptr).len() as isize;
                let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                let (start, stop, step) = match normalize_slice_indices(
                    _py, len, start_obj, stop_obj, step_obj,
                ) {
                    Ok(value) => value,
                    Err(err) => return slice_error(_py, err),
                };
                let itemsize = tc.itemsize();
                if step == 1 {
                    let start_byte = start as usize * itemsize;
                    let stop_byte = stop as usize * itemsize;
                    (*handle_ptr)
                        .data
                        .splice(start_byte..stop_byte, replacement);
                    return MoltObject::none().bits();
                }
                let indices = collect_slice_indices(start, stop, step);
                if indices.len() != replacement_len {
                    return raise_exception::<u64>(
                        _py,
                        "ValueError",
                        &format!(
                            "attempt to assign array of size {} to extended slice of size {}",
                            replacement_len,
                            indices.len()
                        ),
                    );
                }
                let handle = &mut *handle_ptr;
                for (slot_idx, idx) in indices.iter().copied().enumerate() {
                    let src_start = slot_idx * itemsize;
                    let src_end = src_start + itemsize;
                    let dst_start = idx * itemsize;
                    let dst_end = dst_start + itemsize;
                    handle.data[dst_start..dst_end]
                        .copy_from_slice(&replacement[src_start..src_end]);
                }
                return MoltObject::none().bits();
            }
        }
        let handle = unsafe { &mut *handle_ptr };
        let tc = handle.typecode;
        let Some(idx_raw) = to_i64(index_obj) else {
            return raise_exception::<u64>(_py, "TypeError", "array indices must be integers");
        };
        let len = handle.len() as i64;
        let idx = if idx_raw < 0 { len + idx_raw } else { idx_raw };
        if idx < 0 || idx >= len {
            return raise_exception::<u64>(
                _py,
                "IndexError",
                "array assignment index out of range",
            );
        }
        let elem = match elem_from_bits(_py, tc, value_bits) {
            Ok(e) => e,
            Err(exc) => return exc,
        };
        if let Err(msg) = handle.set_elem(idx as usize, elem) {
            return raise_exception::<u64>(_py, "OverflowError", msg);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_delitem(handle_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        let Some(idx_raw) = to_i64(obj_from_bits(index_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "array indices must be integers");
        };
        let len = handle.len() as i64;
        let idx = if idx_raw < 0 { len + idx_raw } else { idx_raw };
        if idx < 0 || idx >= len {
            return raise_exception::<u64>(
                _py,
                "IndexError",
                "array assignment index out of range",
            );
        }
        if let Err(msg) = handle.remove_at(idx as usize) {
            return raise_exception::<u64>(_py, "IndexError", msg);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_len(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        int_bits_from_i64(_py, handle.len() as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_typecode(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        let ch = handle.typecode.as_char();
        let s = ch.to_string();
        let ptr = alloc_string(_py, s.as_bytes());
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_itemsize(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        int_bits_from_i64(_py, handle.typecode.itemsize() as i64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_tobytes(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        let ptr = alloc_bytes(_py, &handle.data);
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_frombytes(handle_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        let data_obj = obj_from_bits(data_bits);
        let Some(data_ptr) = data_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "a bytes-like object is required");
        };
        let data_slice = unsafe {
            let Some(slice) = crate::bytes_like_slice(data_ptr) else {
                return raise_exception::<u64>(_py, "TypeError", "a bytes-like object is required");
            };
            slice.to_vec()
        };
        let sz = handle.typecode.itemsize();
        if data_slice.len() % sz != 0 {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "bytes length not a multiple of item size",
            );
        }
        handle.data.extend_from_slice(&data_slice);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_tolist(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        let n = handle.len();
        let mut elems: Vec<u64> = Vec::with_capacity(n);
        for i in 0..n {
            let Some(elem) = handle.read_elem(i) else {
                return raise_exception::<u64>(_py, "RuntimeError", "array data corrupted");
            };
            elems.push(elem_to_bits(_py, elem));
        }
        let list_ptr = alloc_list(_py, &elems);
        // dec_ref the element bits since alloc_list increments them.
        for b in &elems {
            dec_ref_bits(_py, *b);
        }
        if list_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_buffer_info(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        let addr = handle.data.as_ptr() as usize as i64;
        let length = handle.len() as i64;
        let addr_bits = int_bits_from_i64(_py, addr);
        let len_bits = int_bits_from_i64(_py, length);
        let tuple_ptr = alloc_tuple(_py, &[addr_bits, len_bits]);
        dec_ref_bits(_py, addr_bits);
        dec_ref_bits(_py, len_bits);
        if tuple_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_pop(handle_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        let len = handle.len() as i64;
        if len == 0 {
            return raise_exception::<u64>(_py, "IndexError", "pop from empty array");
        }
        let idx_raw = to_i64(obj_from_bits(index_bits)).unwrap_or(-1);
        let idx = if idx_raw < 0 { len + idx_raw } else { idx_raw };
        if idx < 0 || idx >= len {
            return raise_exception::<u64>(_py, "IndexError", "pop index out of range");
        }
        match handle.remove_at(idx as usize) {
            Ok(elem) => elem_to_bits(_py, elem),
            Err(msg) => raise_exception::<u64>(_py, "IndexError", msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_insert(handle_bits: u64, index_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        let tc = handle.typecode;
        let Some(idx_raw) = to_i64(obj_from_bits(index_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "array indices must be integers");
        };
        let elem = match elem_from_bits(_py, tc, value_bits) {
            Ok(e) => e,
            Err(exc) => return exc,
        };
        let len = handle.len() as i64;
        let idx = if idx_raw < 0 {
            (len + idx_raw).max(0) as usize
        } else {
            idx_raw.min(len) as usize
        };
        if let Err(msg) = handle.insert_at(idx, elem) {
            return raise_exception::<u64>(_py, "OverflowError", msg);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_remove(handle_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        let tc = handle.typecode;
        let target = match elem_from_bits(_py, tc, value_bits) {
            Ok(e) => e,
            Err(exc) => return exc,
        };
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
                let _ = handle.remove_at(idx);
                MoltObject::none().bits()
            }
            None => raise_exception::<u64>(_py, "ValueError", "array.remove(x): x not in array"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_reverse(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
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
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_count(handle_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        let tc = handle.typecode;
        let target = match elem_from_bits(_py, tc, value_bits) {
            Ok(e) => e,
            Err(exc) => return exc,
        };
        // SIMD fast path for byte typecodes: use memchr for O(n/16) counting
        if matches!(tc, Typecode::UB) {
            if let ArrayElem::Uint(v) = target {
                if v <= 255 {
                    let needle = v as u8;
                    let count = memchr::memchr_iter(needle, &handle.data).count() as i64;
                    return int_bits_from_i64(_py, count);
                }
            } else if let ArrayElem::Int(v) = target
                && (0..=255).contains(&v)
            {
                let needle = v as u8;
                let count = memchr::memchr_iter(needle, &handle.data).count() as i64;
                return int_bits_from_i64(_py, count);
            }
        }
        if matches!(tc, Typecode::B)
            && let ArrayElem::Int(v) = target
            && (-128..=127).contains(&v)
        {
            let needle = v as u8; // Two's complement byte
            let count = memchr::memchr_iter(needle, &handle.data).count() as i64;
            return int_bits_from_i64(_py, count);
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
        int_bits_from_i64(_py, count)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_index(handle_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = array_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid array handle");
        };
        let tc = handle.typecode;
        let target = match elem_from_bits(_py, tc, value_bits) {
            Ok(e) => e,
            Err(exc) => return exc,
        };
        // SIMD fast path for byte typecodes: use memchr for O(n/16) search
        if matches!(tc, Typecode::UB) {
            if let ArrayElem::Uint(v) = target {
                if v <= 255 {
                    if let Some(pos) = memchr::memchr(v as u8, &handle.data) {
                        return int_bits_from_i64(_py, pos as i64);
                    }
                    return raise_exception::<u64>(
                        _py,
                        "ValueError",
                        "array.index(x): x not in array",
                    );
                }
            } else if let ArrayElem::Int(v) = target
                && (0..=255).contains(&v)
            {
                if let Some(pos) = memchr::memchr(v as u8, &handle.data) {
                    return int_bits_from_i64(_py, pos as i64);
                }
                return raise_exception::<u64>(_py, "ValueError", "array.index(x): x not in array");
            }
        }
        if matches!(tc, Typecode::B)
            && let ArrayElem::Int(v) = target
            && (-128..=127).contains(&v)
        {
            if let Some(pos) = memchr::memchr(v as u8, &handle.data) {
                return int_bits_from_i64(_py, pos as i64);
            }
            return raise_exception::<u64>(_py, "ValueError", "array.index(x): x not in array");
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
                    return int_bits_from_i64(_py, i as i64);
                }
            }
        }
        raise_exception::<u64>(_py, "ValueError", "array.index(x): x not in array")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_array_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        // SAFETY: pointer is owned by this runtime.
        unsafe {
            drop(Box::from_raw(ptr as *mut ArrayHandle));
        }
        MoltObject::none().bits()
    })
}
