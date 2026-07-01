#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use crate::bytearray_vec;
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::{
    MemoryViewFormat, MemoryViewFormatKind, MoltObject, PyToken, TYPE_ID_BYTEARRAY, TYPE_ID_BYTES,
    TYPE_ID_MEMORYVIEW, TYPE_ID_STRING, alloc_bytes, bigint_bits, bytes_data, bytes_len,
    index_bigint_from_obj, is_truthy, memoryview_base_bits, memoryview_data,
    memoryview_format_bits, memoryview_itemsize, memoryview_len, memoryview_offset,
    memoryview_readonly, memoryview_released, memoryview_shape, memoryview_stride,
    memoryview_strides, obj_from_bits, object_type_id, raise_exception, string_bytes, string_len,
    string_obj_to_owned, to_f64,
};
use num_bigint::BigInt;
use num_traits::ToPrimitive;

pub const MOLT_BUFFER_MAX_NDIM: usize = 64;
pub const MOLT_BUFFER_FORMAT_CAP: usize = 16;
pub(crate) const RELEASED_MEMORYVIEW_ERROR: &str =
    "operation forbidden on released memoryview object";

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MoltBufferView {
    pub data: *mut u8,
    pub len: u64,
    pub readonly: u32,
    pub ndim: u32,
    pub itemsize: u64,
    pub offset: isize,
    pub owner: u64,
    pub base: u64,
    pub shape: [isize; MOLT_BUFFER_MAX_NDIM],
    pub strides: [isize; MOLT_BUFFER_MAX_NDIM],
    pub format: [u8; MOLT_BUFFER_FORMAT_CAP],
}

impl Default for MoltBufferView {
    fn default() -> Self {
        Self {
            data: std::ptr::null_mut(),
            len: 0,
            readonly: 1,
            ndim: 1,
            itemsize: 1,
            offset: 0,
            owner: 0,
            base: 0,
            shape: [0; MOLT_BUFFER_MAX_NDIM],
            strides: [0; MOLT_BUFFER_MAX_NDIM],
            format: default_buffer_format(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TypedStridedStorageError {
    NotBuffer,
    ReleasedMemoryView,
    InvalidDescriptor,
}

pub(crate) fn raise_released_memoryview<T: crate::ExceptionSentinel>(_py: &PyToken<'_>) -> T {
    raise_exception(_py, "ValueError", RELEASED_MEMORYVIEW_ERROR)
}

fn default_buffer_format() -> [u8; MOLT_BUFFER_FORMAT_CAP] {
    let mut format = [0; MOLT_BUFFER_FORMAT_CAP];
    format[0] = b'B';
    format
}

fn buffer_format_from_bytes(format: &[u8]) -> [u8; MOLT_BUFFER_FORMAT_CAP] {
    let mut out = [0; MOLT_BUFFER_FORMAT_CAP];
    let count = format.len().min(MOLT_BUFFER_FORMAT_CAP.saturating_sub(1));
    out[..count].copy_from_slice(&format[..count]);
    out
}

pub(crate) unsafe fn memoryview_format_export_bytes(
    format_bits: u64,
) -> Option<[u8; MOLT_BUFFER_FORMAT_CAP]> {
    unsafe {
        let obj = obj_from_bits(format_bits);
        let ptr = obj.as_ptr()?;
        if object_type_id(ptr) != TYPE_ID_STRING {
            return None;
        }
        let len = string_len(ptr);
        let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
        Some(buffer_format_from_bytes(bytes))
    }
}

#[derive(Clone)]
pub(crate) struct TypedStridedStorage {
    pub(crate) data: *mut u8,
    pub(crate) len: usize,
    pub(crate) readonly: bool,
    pub(crate) itemsize: usize,
    pub(crate) offset: isize,
    pub(crate) base_bits: u64,
    pub(crate) format_bits: u64,
    pub(crate) format: [u8; MOLT_BUFFER_FORMAT_CAP],
    pub(crate) shape: Vec<isize>,
    pub(crate) strides: Vec<isize>,
}

impl TypedStridedStorage {
    pub(crate) fn new(
        data: *mut u8,
        readonly: bool,
        itemsize: usize,
        offset: isize,
        base_bits: u64,
        format_bits: u64,
        shape: Vec<isize>,
        strides: Vec<isize>,
    ) -> Option<Self> {
        if itemsize == 0 || shape.len() != strides.len() || shape.len() > MOLT_BUFFER_MAX_NDIM {
            return None;
        }
        let len = memoryview_nbytes_big(shape.as_slice(), itemsize)?;
        if len < 0 || len > usize::MAX as i128 {
            return None;
        }
        let format = if format_bits == 0 {
            default_buffer_format()
        } else {
            unsafe { memoryview_format_export_bytes(format_bits)? }
        };
        Some(Self {
            data,
            len: len as usize,
            readonly,
            itemsize,
            offset,
            base_bits,
            format_bits,
            format,
            shape,
            strides,
        })
    }

    pub(crate) fn one_dim(
        data: *mut u8,
        readonly: bool,
        len: usize,
        itemsize: usize,
        stride: isize,
        offset: isize,
        base_bits: u64,
        format_bits: u64,
    ) -> Option<Self> {
        Self::new(
            data,
            readonly,
            itemsize,
            offset,
            base_bits,
            format_bits,
            vec![len as isize],
            vec![stride],
        )
    }

    pub(crate) fn memoryview_len_field(&self) -> usize {
        self.shape.first().copied().unwrap_or(0).max(0) as usize
    }

    pub(crate) fn memoryview_stride_field(&self) -> isize {
        self.strides.first().copied().unwrap_or(0)
    }

    pub(crate) fn with_readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    pub(crate) unsafe fn from_object_bits(obj_bits: u64) -> Result<Self, TypedStridedStorageError> {
        unsafe {
            let obj = obj_from_bits(obj_bits);
            let ptr = obj.as_ptr().ok_or(TypedStridedStorageError::NotBuffer)?;
            match object_type_id(ptr) {
                TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => Self::from_bytes_like_ptr(obj_bits, ptr),
                TYPE_ID_MEMORYVIEW => Self::from_memoryview_ptr(ptr),
                _ => Err(TypedStridedStorageError::NotBuffer),
            }
        }
    }

    unsafe fn from_bytes_like_ptr(
        obj_bits: u64,
        ptr: *mut u8,
    ) -> Result<Self, TypedStridedStorageError> {
        unsafe {
            let type_id = object_type_id(ptr);
            let data = bytes_data(ptr) as *mut u8;
            let len = bytes_len(ptr);
            let readonly = type_id == TYPE_ID_BYTES;
            Self::one_dim(data, readonly, len, 1, 1, 0, obj_bits, 0)
                .ok_or(TypedStridedStorageError::InvalidDescriptor)
        }
    }

    unsafe fn from_memoryview_ptr(ptr: *mut u8) -> Result<Self, TypedStridedStorageError> {
        unsafe {
            if memoryview_released(ptr) {
                return Err(TypedStridedStorageError::ReleasedMemoryView);
            }
            let base_bits = memoryview_base_bits(ptr);
            let data = memoryview_data(ptr);
            if data.is_null() {
                return Err(TypedStridedStorageError::InvalidDescriptor);
            }
            let offset = memoryview_offset(ptr);
            if offset < 0 {
                return Err(TypedStridedStorageError::InvalidDescriptor);
            }
            let shape = memoryview_shape(ptr)
                .ok_or(TypedStridedStorageError::InvalidDescriptor)?
                .to_vec();
            let strides = memoryview_strides(ptr)
                .ok_or(TypedStridedStorageError::InvalidDescriptor)?
                .to_vec();
            Self::new(
                data,
                memoryview_readonly(ptr),
                memoryview_itemsize(ptr),
                offset,
                base_bits,
                memoryview_format_bits(ptr),
                shape,
                strides,
            )
            .ok_or(TypedStridedStorageError::InvalidDescriptor)
        }
    }
}

impl MoltBufferView {
    pub(crate) fn from_typed_storage(storage: &TypedStridedStorage) -> Option<Self> {
        if storage.shape.len() != storage.strides.len()
            || storage.shape.len() > MOLT_BUFFER_MAX_NDIM
        {
            return None;
        }
        let mut out = Self {
            data: storage.data,
            len: u64::try_from(storage.len).ok()?,
            readonly: if storage.readonly { 1 } else { 0 },
            ndim: storage.shape.len() as u32,
            itemsize: u64::try_from(storage.itemsize).ok()?,
            offset: storage.offset,
            base: storage.base_bits,
            format: storage.format,
            ..Self::default()
        };
        for (slot, value) in out.shape.iter_mut().zip(storage.shape.iter().copied()) {
            *slot = value;
        }
        for (slot, value) in out.strides.iter_mut().zip(storage.strides.iter().copied()) {
            *slot = value;
        }
        Some(out)
    }
}

pub(crate) fn memoryview_format_from_str(format: &str) -> Option<MemoryViewFormat> {
    let code = if format.len() == 1 {
        format.as_bytes()[0]
    } else if format.len() == 2 && format.as_bytes()[0] == b'@' {
        format.as_bytes()[1]
    } else {
        return None;
    };
    let (itemsize, kind) = match code {
        b'b' => (1, MemoryViewFormatKind::Signed),
        b'B' => (1, MemoryViewFormatKind::Unsigned),
        b'h' => (2, MemoryViewFormatKind::Signed),
        b'H' => (2, MemoryViewFormatKind::Unsigned),
        b'i' => (4, MemoryViewFormatKind::Signed),
        b'I' => (4, MemoryViewFormatKind::Unsigned),
        b'l' => (
            std::mem::size_of::<libc::c_long>(),
            MemoryViewFormatKind::Signed,
        ),
        b'L' => (
            std::mem::size_of::<libc::c_long>(),
            MemoryViewFormatKind::Unsigned,
        ),
        b'q' => (8, MemoryViewFormatKind::Signed),
        b'Q' => (8, MemoryViewFormatKind::Unsigned),
        b'n' => (std::mem::size_of::<isize>(), MemoryViewFormatKind::Signed),
        b'N' => (std::mem::size_of::<isize>(), MemoryViewFormatKind::Unsigned),
        b'P' => (
            std::mem::size_of::<*const u8>(),
            MemoryViewFormatKind::Unsigned,
        ),
        b'f' => (4, MemoryViewFormatKind::Float),
        b'd' => (8, MemoryViewFormatKind::Float),
        b'?' => (1, MemoryViewFormatKind::Bool),
        b'c' => (1, MemoryViewFormatKind::Char),
        _ => return None,
    };
    Some(MemoryViewFormat {
        code,
        itemsize,
        kind,
    })
}

pub(crate) fn memoryview_format_from_bits(bits: u64) -> Option<MemoryViewFormat> {
    let format = string_obj_to_owned(obj_from_bits(bits))?;
    memoryview_format_from_str(&format)
}

pub(crate) fn memoryview_shape_product(shape: &[isize]) -> Option<i128> {
    let mut total: i128 = 1;
    for &dim in shape {
        if dim < 0 {
            return None;
        }
        let dim_val: i128 = dim as i128;
        total = total.checked_mul(dim_val)?;
    }
    Some(total)
}

pub(crate) fn memoryview_nbytes_big(shape: &[isize], itemsize: usize) -> Option<i128> {
    let total = memoryview_shape_product(shape)?;
    let itemsize = i128::try_from(itemsize).ok()?;
    total.checked_mul(itemsize)
}

fn memoryview_is_c_contiguous(shape: &[isize], strides: &[isize], itemsize: usize) -> bool {
    if shape.len() != strides.len() {
        return false;
    }
    let mut expected = itemsize as isize;
    for idx in (0..shape.len()).rev() {
        let dim = shape[idx];
        let stride = strides[idx];
        if dim > 1 && stride != expected {
            return false;
        }
        expected = expected.saturating_mul(dim.max(1));
    }
    true
}

pub(crate) unsafe fn memoryview_is_c_contiguous_view(ptr: *mut u8) -> bool {
    unsafe {
        if memoryview_released(ptr) {
            return false;
        }
        let shape = memoryview_shape(ptr).unwrap_or(&[]);
        let strides = memoryview_strides(ptr).unwrap_or(&[]);
        memoryview_is_c_contiguous(shape, strides, memoryview_itemsize(ptr))
    }
}

pub(crate) unsafe fn memoryview_nbytes(ptr: *mut u8) -> usize {
    unsafe {
        if memoryview_released(ptr) {
            return 0;
        }
        let shape = memoryview_shape(ptr).unwrap_or(&[]);
        let itemsize = memoryview_itemsize(ptr);
        if let Some(total) = memoryview_nbytes_big(shape, itemsize)
            && total >= 0
            && total <= usize::MAX as i128
        {
            return total as usize;
        }
        0
    }
}

pub(crate) unsafe fn bytes_like_slice_raw(ptr: *mut u8) -> Option<&'static [u8]> {
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
            let len = bytes_len(ptr);
            let data = bytes_data(ptr);
            return Some(std::slice::from_raw_parts(data, len));
        }
        None
    }
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
unsafe fn bytes_like_slice_raw_mut(ptr: *mut u8) -> Option<&'static mut [u8]> {
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_BYTEARRAY {
            let vec = bytearray_vec(ptr);
            return Some(vec.as_mut_slice());
        }
        None
    }
}

pub(crate) unsafe fn memoryview_bytes_slice(ptr: *mut u8) -> Option<&'static [u8]> {
    unsafe {
        if memoryview_released(ptr) {
            return None;
        }
        if memoryview_itemsize(ptr) != 1 || memoryview_stride(ptr) != 1 {
            return None;
        }
        let data = memoryview_data(ptr);
        if data.is_null() {
            return None;
        }
        let len = memoryview_len(ptr);
        Some(std::slice::from_raw_parts(data.cast_const(), len))
    }
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(crate) unsafe fn memoryview_bytes_slice_mut(ptr: *mut u8) -> Option<&'static mut [u8]> {
    unsafe {
        if memoryview_released(ptr) {
            return None;
        }
        if memoryview_itemsize(ptr) != 1 || memoryview_stride(ptr) != 1 {
            return None;
        }
        let data = memoryview_data(ptr);
        if data.is_null() {
            return None;
        }
        let len = memoryview_len(ptr);
        Some(std::slice::from_raw_parts_mut(data, len))
    }
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(crate) unsafe fn memoryview_write_bytes(ptr: *mut u8, data: &[u8]) -> Result<usize, String> {
    unsafe {
        if memoryview_released(ptr) {
            return Err(RELEASED_MEMORYVIEW_ERROR.to_string());
        }
        if memoryview_readonly(ptr) {
            return Err("memoryview is readonly".to_string());
        }
        if let Some(slice) = memoryview_bytes_slice_mut(ptr) {
            let n = data.len().min(slice.len());
            slice[..n].copy_from_slice(&data[..n]);
            return Ok(n);
        }
        let base_ptr = memoryview_data(ptr);
        if base_ptr.is_null() {
            return Err("invalid memoryview data".to_string());
        }
        let shape = memoryview_shape(ptr).ok_or_else(|| "invalid memoryview shape".to_string())?;
        let strides =
            memoryview_strides(ptr).ok_or_else(|| "invalid memoryview strides".to_string())?;
        if shape.len() != strides.len() {
            return Err("invalid memoryview strides".to_string());
        }
        let itemsize = memoryview_itemsize(ptr);
        let total_bytes = memoryview_nbytes_big(shape, itemsize)
            .ok_or_else(|| "invalid memoryview size".to_string())?;
        if total_bytes < 0 {
            return Err("invalid memoryview size".to_string());
        }
        let total_bytes = total_bytes as usize;
        let write_bytes = data.len().min(total_bytes);
        let offset = memoryview_offset(ptr);
        if offset < 0 {
            return Err("invalid memoryview offset".to_string());
        }
        if memoryview_is_c_contiguous(shape, strides, itemsize) {
            let base = std::slice::from_raw_parts_mut(base_ptr, write_bytes);
            base.copy_from_slice(&data[..write_bytes]);
            return Ok(write_bytes);
        }
        let total = memoryview_shape_product(shape)
            .ok_or_else(|| "invalid memoryview shape".to_string())?;
        if total < 0 {
            return Err("invalid memoryview shape".to_string());
        }
        let total = total as usize;
        let mut indices = vec![0isize; shape.len()];
        let mut written = 0usize;
        for _ in 0..total {
            if written >= write_bytes {
                break;
            }
            let mut pos = 0isize;
            for (idx, stride) in indices.iter().zip(strides.iter()) {
                pos = pos
                    .checked_add(idx.saturating_mul(*stride))
                    .ok_or_else(|| "memoryview out of bounds".to_string())?;
            }
            if pos < 0 {
                return Err("memoryview out of bounds".to_string());
            }
            let remaining = write_bytes - written;
            let copy_len = itemsize.min(remaining);
            let base = std::slice::from_raw_parts_mut(base_ptr.offset(pos), copy_len);
            base.copy_from_slice(&data[written..written + copy_len]);
            written += copy_len;
            for dim in (0..indices.len()).rev() {
                indices[dim] += 1;
                if indices[dim] < shape[dim] {
                    break;
                }
                indices[dim] = 0;
            }
        }
        Ok(written)
    }
}

pub(crate) unsafe fn memoryview_collect_bytes(ptr: *mut u8) -> Option<Vec<u8>> {
    unsafe {
        if memoryview_released(ptr) {
            return None;
        }
        let base_ptr = memoryview_data(ptr);
        if base_ptr.is_null() {
            return None;
        }
        let shape = memoryview_shape(ptr)?;
        let strides = memoryview_strides(ptr)?;
        if shape.len() != strides.len() {
            return None;
        }
        let nbytes = memoryview_nbytes_big(shape, memoryview_itemsize(ptr))?;
        if nbytes < 0 {
            return None;
        }
        let nbytes = nbytes as usize;
        let offset = memoryview_offset(ptr);
        if offset < 0 {
            return None;
        }
        let mut out = Vec::with_capacity(nbytes);
        if memoryview_is_c_contiguous(shape, strides, memoryview_itemsize(ptr)) {
            let base = std::slice::from_raw_parts(base_ptr.cast_const(), nbytes);
            out.extend_from_slice(base);
            return Some(out);
        }
        let total = memoryview_shape_product(shape)?;
        if total < 0 {
            return None;
        }
        let total = total as usize;
        let mut indices = vec![0isize; shape.len()];
        for _ in 0..total {
            let mut pos = 0isize;
            for (idx, stride) in indices.iter().zip(strides.iter()) {
                pos = pos.checked_add(idx.saturating_mul(*stride))?;
            }
            let itemsize = memoryview_itemsize(ptr);
            let base = std::slice::from_raw_parts(base_ptr.offset(pos).cast_const(), itemsize);
            out.extend_from_slice(base);
            for axis in (0..indices.len()).rev() {
                indices[axis] += 1;
                if indices[axis] < shape[axis] {
                    break;
                }
                indices[axis] = 0;
            }
        }
        Some(out)
    }
}

pub(crate) unsafe fn memoryview_read_scalar(
    _py: &PyToken<'_>,
    data: &[u8],
    offset: isize,
    fmt: MemoryViewFormat,
) -> Option<u64> {
    if offset < 0 {
        return None;
    }
    let offset = offset as usize;
    if offset + fmt.itemsize > data.len() {
        return None;
    }
    match fmt.kind {
        MemoryViewFormatKind::Char => {
            let ptr = alloc_bytes(_py, &[data[offset]]);
            if ptr.is_null() {
                return None;
            }
            Some(MoltObject::from_ptr(ptr).bits())
        }
        MemoryViewFormatKind::Bool => Some(MoltObject::from_bool(data[offset] != 0).bits()),
        MemoryViewFormatKind::Float => {
            if fmt.itemsize == 4 {
                let bytes: [u8; 4] = data[offset..offset + 4].try_into().ok()?;
                let val = f32::from_ne_bytes(bytes) as f64;
                Some(MoltObject::from_float(val).bits())
            } else if fmt.itemsize == 8 {
                let bytes: [u8; 8] = data[offset..offset + 8].try_into().ok()?;
                let val = f64::from_ne_bytes(bytes);
                Some(MoltObject::from_float(val).bits())
            } else {
                None
            }
        }
        MemoryViewFormatKind::Signed => {
            let val = match fmt.itemsize {
                1 => i64::from(i8::from_ne_bytes([data[offset]])),
                2 => {
                    let bytes: [u8; 2] = data[offset..offset + 2].try_into().ok()?;
                    i64::from(i16::from_ne_bytes(bytes))
                }
                4 => {
                    let bytes: [u8; 4] = data[offset..offset + 4].try_into().ok()?;
                    i64::from(i32::from_ne_bytes(bytes))
                }
                8 => {
                    let bytes: [u8; 8] = data[offset..offset + 8].try_into().ok()?;
                    i64::from_ne_bytes(bytes)
                }
                _ => return None,
            };
            Some(MoltObject::from_int(val).bits())
        }
        MemoryViewFormatKind::Unsigned => {
            let val = match fmt.itemsize {
                1 => u64::from(data[offset]),
                2 => {
                    let bytes: [u8; 2] = data[offset..offset + 2].try_into().ok()?;
                    u64::from(u16::from_ne_bytes(bytes))
                }
                4 => {
                    let bytes: [u8; 4] = data[offset..offset + 4].try_into().ok()?;
                    u64::from(u32::from_ne_bytes(bytes))
                }
                8 => {
                    let bytes: [u8; 8] = data[offset..offset + 8].try_into().ok()?;
                    u64::from_ne_bytes(bytes)
                }
                _ => return None,
            };
            if val <= i64::MAX as u64 {
                Some(MoltObject::from_int(val as i64).bits())
            } else {
                Some(bigint_bits(_py, BigInt::from(val)))
            }
        }
    }
}

pub(crate) unsafe fn memoryview_read_scalar_at(
    _py: &PyToken<'_>,
    data: *const u8,
    offset: isize,
    fmt: MemoryViewFormat,
) -> Option<u64> {
    if data.is_null() {
        return None;
    }
    let item = unsafe { std::slice::from_raw_parts(data.offset(offset), fmt.itemsize) };
    unsafe { memoryview_read_scalar(_py, item, 0, fmt) }
}

pub(crate) unsafe fn memoryview_write_scalar(
    _py: &PyToken<'_>,
    data: &mut [u8],
    offset: isize,
    fmt: MemoryViewFormat,
    val_bits: u64,
) -> Option<()> {
    unsafe {
        if offset < 0 {
            return None;
        }
        let offset = offset as usize;
        if offset + fmt.itemsize > data.len() {
            return None;
        }
        match fmt.kind {
            MemoryViewFormatKind::Char => {
                let val_obj = obj_from_bits(val_bits);
                let Some(ptr) = val_obj.as_ptr() else {
                    crate::raise_exception::<u64>(
                        _py,
                        "TypeError",
                        &format!("memoryview: invalid type for format '{}'", fmt.code as char),
                    );
                    return None;
                };
                if object_type_id(ptr) != TYPE_ID_BYTES {
                    crate::raise_exception::<u64>(
                        _py,
                        "TypeError",
                        &format!("memoryview: invalid type for format '{}'", fmt.code as char),
                    );
                    return None;
                }
                let bytes = bytes_like_slice_raw(ptr).unwrap_or(&[]);
                if bytes.len() != 1 {
                    crate::raise_exception::<u64>(
                        _py,
                        "ValueError",
                        &format!(
                            "memoryview: invalid value for format '{}'",
                            fmt.code as char
                        ),
                    );
                    return None;
                }
                data[offset] = bytes[0];
                Some(())
            }
            MemoryViewFormatKind::Bool => {
                data[offset] = if is_truthy(_py, obj_from_bits(val_bits)) {
                    1
                } else {
                    0
                };
                Some(())
            }
            MemoryViewFormatKind::Float => {
                let Some(val) = to_f64(obj_from_bits(val_bits)) else {
                    crate::raise_exception::<u64>(
                        _py,
                        "TypeError",
                        &format!("memoryview: invalid type for format '{}'", fmt.code as char),
                    );
                    return None;
                };
                if fmt.itemsize == 4 {
                    let bytes = (val as f32).to_ne_bytes();
                    data[offset..offset + 4].copy_from_slice(&bytes);
                    return Some(());
                }
                if fmt.itemsize == 8 {
                    let bytes = val.to_ne_bytes();
                    data[offset..offset + 8].copy_from_slice(&bytes);
                    return Some(());
                }
                None
            }
            MemoryViewFormatKind::Signed | MemoryViewFormatKind::Unsigned => {
                let err_msg = format!("memoryview: invalid type for format '{}'", fmt.code as char);
                let value = index_bigint_from_obj(_py, val_bits, &err_msg)?;
                let bits = (fmt.itemsize * 8) as u32;
                let (min, max) = if fmt.kind == MemoryViewFormatKind::Signed {
                    let limit = BigInt::from(1u64) << (bits - 1);
                    (-limit.clone(), limit - 1)
                } else {
                    (BigInt::from(0u8), (BigInt::from(1u64) << bits) - 1)
                };
                if value < min || value > max {
                    crate::raise_exception::<u64>(
                        _py,
                        "ValueError",
                        &format!(
                            "memoryview: invalid value for format '{}'",
                            fmt.code as char
                        ),
                    );
                    return None;
                }
                if fmt.kind == MemoryViewFormatKind::Signed {
                    let val_i64 = value.to_i64().unwrap_or(0);
                    let bytes = match fmt.itemsize {
                        1 => (val_i64 as i8).to_ne_bytes().to_vec(),
                        2 => (val_i64 as i16).to_ne_bytes().to_vec(),
                        4 => (val_i64 as i32).to_ne_bytes().to_vec(),
                        8 => val_i64.to_ne_bytes().to_vec(),
                        _ => return None,
                    };
                    data[offset..offset + fmt.itemsize].copy_from_slice(&bytes);
                    return Some(());
                }
                let val_u64 = value.to_u64().unwrap_or(0);
                let bytes = match fmt.itemsize {
                    1 => (val_u64 as u8).to_ne_bytes().to_vec(),
                    2 => (val_u64 as u16).to_ne_bytes().to_vec(),
                    4 => (val_u64 as u32).to_ne_bytes().to_vec(),
                    8 => val_u64.to_ne_bytes().to_vec(),
                    _ => return None,
                };
                data[offset..offset + fmt.itemsize].copy_from_slice(&bytes);
                Some(())
            }
        }
    }
}

pub(crate) unsafe fn memoryview_write_scalar_at(
    _py: &PyToken<'_>,
    data: *mut u8,
    offset: isize,
    fmt: MemoryViewFormat,
    val_bits: u64,
) -> Option<()> {
    if data.is_null() {
        return None;
    }
    let item = unsafe { std::slice::from_raw_parts_mut(data.offset(offset), fmt.itemsize) };
    unsafe { memoryview_write_scalar(_py, item, 0, fmt, val_bits) }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BytesLikeSliceError {
    NotBytesLike,
    ReleasedMemoryView,
}

pub(crate) unsafe fn bytes_like_slice_checked(
    ptr: *mut u8,
) -> Result<&'static [u8], BytesLikeSliceError> {
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_MEMORYVIEW {
            if memoryview_released(ptr) {
                return Err(BytesLikeSliceError::ReleasedMemoryView);
            }
            return memoryview_bytes_slice(ptr).ok_or(BytesLikeSliceError::NotBytesLike);
        }
        bytes_like_slice_raw(ptr).ok_or(BytesLikeSliceError::NotBytesLike)
    }
}

pub(crate) unsafe fn bytes_like_slice(ptr: *mut u8) -> Option<&'static [u8]> {
    unsafe { bytes_like_slice_checked(ptr).ok() }
}
