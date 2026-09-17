#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::{
    MemoryViewFormat, MemoryViewFormatKind, MoltObject, PyToken, TYPE_ID_BYTEARRAY, TYPE_ID_BYTES,
    TYPE_ID_MEMORYVIEW, TYPE_ID_STRING, alloc_bytes, bigint_bits, bytes_data, bytes_len,
    index_bigint_from_obj, is_truthy, memoryview_base_bits, memoryview_data,
    memoryview_format_bits, memoryview_itemsize, memoryview_offset, memoryview_owner_bits,
    memoryview_readonly, memoryview_released, memoryview_shape, memoryview_strides, obj_from_bits,
    object_type_id, raise_exception, string_bytes, string_len, string_obj_to_owned,
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
    pub backing_capacity: u64,
    // Canonical bool exported as 0/1; importers reject every other value.
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
            backing_capacity: 0,
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
    pub(crate) span_len: usize,
    pub(crate) min_offset: isize,
    pub(crate) max_end_offset: isize,
    pub(crate) readonly: bool,
    pub(crate) itemsize: usize,
    pub(crate) offset: isize,
    pub(crate) base_bits: u64,
    pub(crate) owner_bits: u64,
    pub(crate) format_bits: u64,
    pub(crate) format: [u8; MOLT_BUFFER_FORMAT_CAP],
    pub(crate) shape: Vec<isize>,
    pub(crate) strides: Vec<isize>,
}

impl TypedStridedStorage {
    // clippy: the 8 params mirror the buffer-protocol fields (data/readonly/
    // format/itemsize/shape/strides/offset/base); a params struct would just
    // duplicate the struct being constructed.
    #[allow(clippy::too_many_arguments)]
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
        if len < 0 || len > isize::MAX as i128 {
            return None;
        }
        let bounds = memoryview_strided_bounds(shape.as_slice(), strides.as_slice(), itemsize)?;
        let format = if format_bits == 0 {
            default_buffer_format()
        } else {
            unsafe { memoryview_format_export_bytes(format_bits)? }
        };
        Some(Self {
            data,
            len: len as usize,
            span_len: bounds.span_len,
            min_offset: bounds.min_offset,
            max_end_offset: bounds.max_end_offset,
            readonly,
            itemsize,
            offset,
            base_bits,
            owner_bits: base_bits,
            format_bits,
            format,
            shape,
            strides,
        })
    }

    // clippy: the 8 params mirror the buffer-protocol fields; see `new` above.
    #[allow(clippy::too_many_arguments)]
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

    pub(crate) fn with_owner(mut self, owner_bits: u64) -> Self {
        self.owner_bits = owner_bits;
        self
    }

    pub(crate) fn memoryview_stride_field(&self) -> isize {
        self.strides.first().copied().unwrap_or(0)
    }

    pub(crate) fn fits_in_backing_len(&self, backing_len: usize) -> bool {
        if self.min_offset < 0 || self.max_end_offset < 0 {
            return false;
        }
        usize::try_from(self.max_end_offset)
            .map(|end| end <= backing_len)
            .unwrap_or(false)
    }

    pub(crate) fn fits_in_base_len(&self, base_len: usize) -> bool {
        memoryview_bounds_fit_base(self.offset, self.min_offset, self.max_end_offset, base_len)
    }

    pub(crate) unsafe fn backing_capacity_len(&self) -> Option<usize> {
        unsafe {
            if self.base_bits != 0 {
                let base = obj_from_bits(self.base_bits);
                if let Some(base_ptr) = base.as_ptr()
                    && let Some(base_slice) = bytes_like_slice_raw(base_ptr)
                {
                    if !self.fits_in_base_len(base_slice.len()) {
                        return None;
                    }
                    let start = (self.offset as i128 + self.min_offset as i128) as usize;
                    return Some(base_slice.len() - start);
                }
            }
            Some(self.span_len)
        }
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
            let storage = memoryview_borrowed_storage(ptr).map_err(|err| match err {
                BytesLikeSliceError::ReleasedMemoryView => {
                    TypedStridedStorageError::ReleasedMemoryView
                }
                _ => TypedStridedStorageError::InvalidDescriptor,
            })?;
            Self::new(
                storage.data,
                memoryview_readonly(ptr),
                storage.itemsize,
                memoryview_offset(ptr),
                memoryview_base_bits(ptr),
                memoryview_format_bits(ptr),
                storage.shape.to_vec(),
                storage.strides.to_vec(),
            )
            .map(|storage| storage.with_owner(memoryview_owner_bits(ptr)))
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
        if storage.base_bits == 0 && storage.min_offset < 0 {
            return None;
        }
        let mut out = Self {
            data: storage.data,
            len: u64::try_from(storage.len).ok()?,
            backing_capacity: u64::try_from(unsafe { storage.backing_capacity_len()? }).ok()?,
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

#[derive(Clone, Copy)]
pub(crate) struct MemoryViewStridedBounds {
    pub(crate) min_offset: isize,
    pub(crate) max_end_offset: isize,
    pub(crate) span_len: usize,
}

fn memoryview_bounds_fit_base(
    offset: isize,
    min_offset: isize,
    max_end_offset: isize,
    base_len: usize,
) -> bool {
    let start = offset as i128 + min_offset as i128;
    let end = offset as i128 + max_end_offset as i128;
    start >= 0 && end >= 0 && end <= base_len as i128
}

pub(crate) fn memoryview_strided_bounds(
    shape: &[isize],
    strides: &[isize],
    itemsize: usize,
) -> Option<MemoryViewStridedBounds> {
    if itemsize == 0 || shape.len() != strides.len() {
        return None;
    }
    let total = memoryview_shape_product(shape)?;
    if total == 0 {
        return Some(MemoryViewStridedBounds {
            min_offset: 0,
            max_end_offset: 0,
            span_len: 0,
        });
    }
    let mut min_offset = 0i128;
    let mut max_offset = 0i128;
    for (&dim, &stride) in shape.iter().zip(strides.iter()) {
        if dim < 0 {
            return None;
        }
        if dim > 1 {
            let dim_max = (dim - 1) as i128;
            let stride = i128::try_from(stride).ok()?;
            let delta = dim_max.checked_mul(stride)?;
            if delta < 0 {
                min_offset = min_offset.checked_add(delta)?;
            } else {
                max_offset = max_offset.checked_add(delta)?;
            }
        }
    }
    let itemsize = i128::try_from(itemsize).ok()?;
    let max_end_offset = max_offset.checked_add(itemsize)?;
    let span = max_end_offset.checked_sub(min_offset)?;
    if min_offset < isize::MIN as i128
        || min_offset > isize::MAX as i128
        || max_end_offset < isize::MIN as i128
        || max_end_offset > isize::MAX as i128
        || span < 0
        || span > usize::MAX as i128
    {
        return None;
    }
    Some(MemoryViewStridedBounds {
        min_offset: min_offset as isize,
        max_end_offset: max_end_offset as isize,
        span_len: span as usize,
    })
}

pub(crate) fn memoryview_strided_offset(indices: &[isize], strides: &[isize]) -> Option<isize> {
    if indices.len() != strides.len() {
        return None;
    }
    let mut offset = 0i128;
    for (&idx, &stride) in indices.iter().zip(strides.iter()) {
        let term = i128::try_from(idx)
            .ok()?
            .checked_mul(i128::try_from(stride).ok()?)?;
        offset = offset.checked_add(term)?;
    }
    if offset < isize::MIN as i128 || offset > isize::MAX as i128 {
        return None;
    }
    Some(offset as isize)
}

pub(crate) fn memoryview_linear_offset(index: usize, stride: isize) -> Option<isize> {
    let offset = i128::try_from(index)
        .ok()?
        .checked_mul(i128::try_from(stride).ok()?)?;
    if offset < isize::MIN as i128 || offset > isize::MAX as i128 {
        return None;
    }
    Some(offset as isize)
}

pub(crate) fn memoryview_is_c_contiguous(
    shape: &[isize],
    strides: &[isize],
    itemsize: usize,
) -> bool {
    if shape.len() != strides.len() || itemsize == 0 || shape.iter().any(|&dim| dim < 0) {
        return false;
    }
    // CPython keeps the rank-one stride flag even for an empty slice, whereas
    // multidimensional empty buffers are contiguous regardless of strides.
    if shape.len() > 1 && shape.contains(&0) {
        return true;
    }
    let Ok(mut expected) = isize::try_from(itemsize) else {
        return false;
    };
    for idx in (0..shape.len()).rev() {
        let dim = shape[idx];
        let stride = strides[idx];
        // A singleton axis has no second element whose stride can matter.
        // An empty strided view still retains its non-contiguous layout.
        if dim < 0 || (dim != 1 && stride != expected) {
            return false;
        }
        let Some(next_expected) = expected.checked_mul(dim.max(1)) else {
            return false;
        };
        expected = next_expected;
    }
    true
}

pub(crate) unsafe fn memoryview_is_c_contiguous_view(ptr: *mut u8) -> bool {
    unsafe { memoryview_contiguous_byte_span(ptr).is_ok() }
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

pub(crate) unsafe fn memoryview_bytes_slice(ptr: *mut u8) -> Option<&'static [u8]> {
    unsafe {
        let storage = memoryview_contiguous_byte_span(ptr).ok()?;
        Some(std::slice::from_raw_parts(
            storage.data.cast_const(),
            storage.len,
        ))
    }
}

pub(crate) unsafe fn memoryview_collect_bytes(ptr: *mut u8) -> Option<Vec<u8>> {
    unsafe {
        let storage = memoryview_borrowed_storage(ptr).ok()?;
        let mut out = Vec::with_capacity(storage.len);
        storage.for_each_byte_chunk(storage.len, |source, range| {
            out.extend_from_slice(std::slice::from_raw_parts(source.cast_const(), range.len()));
        });
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
    view: *mut u8,
    offset: isize,
    fmt: MemoryViewFormat,
) -> Option<u64> {
    let data = unsafe { memoryview_scalar_pointer(_py, view, offset, fmt)? };
    let item = unsafe { std::slice::from_raw_parts(data.cast_const(), fmt.itemsize) };
    unsafe { memoryview_read_scalar(_py, item, 0, fmt) }
}

/// Encode into caller-owned stack storage. No exporter bytes are borrowed while
/// `__index__`, `__float__` or `__bool__` can execute Python or release the view.
fn memoryview_encode_scalar(
    _py: &PyToken<'_>,
    data: &mut [u8],
    fmt: MemoryViewFormat,
    val_bits: u64,
) -> Option<()> {
    unsafe {
        let offset = 0;
        if !matches!(fmt.itemsize, 1 | 2 | 4 | 8) || fmt.itemsize > data.len() {
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
                if crate::exception_pending(_py) {
                    return None;
                }
                Some(())
            }
            MemoryViewFormatKind::Float => {
                let val = crate::builtins::numbers::float_as_double(_py, val_bits)?;
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
                    let bytes = value
                        .to_i64()
                        .expect("range checked signed scalar")
                        .to_ne_bytes();
                    let start = if cfg!(target_endian = "big") {
                        8 - fmt.itemsize
                    } else {
                        0
                    };
                    data[..fmt.itemsize].copy_from_slice(&bytes[start..start + fmt.itemsize]);
                    return Some(());
                }
                let bytes = value
                    .to_u64()
                    .expect("range checked unsigned scalar")
                    .to_ne_bytes();
                let start = if cfg!(target_endian = "big") {
                    8 - fmt.itemsize
                } else {
                    0
                };
                data[..fmt.itemsize].copy_from_slice(&bytes[start..start + fmt.itemsize]);
                Some(())
            }
        }
    }
}

pub(crate) unsafe fn memoryview_write_scalar_at(
    _py: &PyToken<'_>,
    view: *mut u8,
    offset: isize,
    fmt: MemoryViewFormat,
    val_bits: u64,
) -> Option<()> {
    // Admission precedes user conversion. Revalidation after it is equally
    // necessary: a conversion callback can release this view.
    if unsafe { memoryview_released(view) } {
        return raise_released_memoryview(_py);
    }
    if unsafe { memoryview_readonly(view) } {
        return raise_exception(_py, "TypeError", "cannot modify read-only memory");
    }
    let mut encoded = [0u8; 8];
    if memoryview_encode_scalar(_py, &mut encoded, fmt, val_bits).is_none() {
        return memoryview_pack_error(_py, fmt);
    }
    let data = unsafe { memoryview_scalar_pointer(_py, view, offset, fmt)? };
    unsafe { std::ptr::copy_nonoverlapping(encoded.as_ptr(), data, fmt.itemsize) };
    Some(())
}

/// CPython pack_single translates numeric protocol type/range failures, but
/// preserves other callback exceptions. This is distinct from scalar coercion.
fn memoryview_pack_error(_py: &PyToken<'_>, fmt: MemoryViewFormat) -> Option<()> {
    use crate::builtins::exceptions::{
        exception_matches_builtin_name, molt_exception_last_pending,
    };
    if !crate::exception_pending(_py) {
        return raise_exception(_py, "BufferError", "invalid memoryview scalar format");
    }
    if !matches!(
        fmt.kind,
        MemoryViewFormatKind::Signed | MemoryViewFormatKind::Unsigned | MemoryViewFormatKind::Float
    ) {
        // Truth testing ('?') propagates its original exception, unlike numeric
        // packing; char admission already supplies its exact diagnostic.
        return None;
    }
    let error = molt_exception_last_pending();
    let translation = if exception_matches_builtin_name(_py, error, "TypeError") {
        Some(("TypeError", "type"))
    } else if exception_matches_builtin_name(_py, error, "OverflowError") {
        Some(("ValueError", "value"))
    } else {
        None
    };
    if let Some((class, detail)) = translation {
        crate::clear_exception(_py);
        crate::dec_ref_bits(_py, error);
        return raise_exception(
            _py,
            class,
            &format!(
                "memoryview: invalid {detail} for format '{}'",
                fmt.code as char
            ),
        );
    }
    crate::dec_ref_bits(_py, error);
    None
}

unsafe fn memoryview_scalar_pointer(
    _py: &PyToken<'_>,
    view: *mut u8,
    offset: isize,
    fmt: MemoryViewFormat,
) -> Option<*mut u8> {
    let storage = match unsafe { memoryview_borrowed_storage(view) } {
        Ok(storage) => storage,
        Err(BytesLikeSliceError::ReleasedMemoryView) => return raise_released_memoryview(_py),
        Err(_) => return raise_exception(_py, "BufferError", "invalid memoryview storage"),
    };
    let end = (offset as i128) + (fmt.itemsize as i128);
    if fmt.itemsize != storage.itemsize
        || offset < storage.min_offset
        || end > storage.max_end_offset as i128
        || storage.len == 0
    {
        return raise_exception(_py, "IndexError", "memoryview index out of bounds");
    }
    Some(unsafe { storage.data.offset(offset) })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BytesLikeSliceError {
    NotBytesLike,
    ReleasedMemoryView,
    NonContiguousMemoryView,
}

struct BorrowedMemoryView<'a> {
    data: *mut u8,
    len: usize,
    itemsize: usize,
    shape: &'a [isize],
    strides: &'a [isize],
    min_offset: isize,
    max_end_offset: isize,
}

impl BorrowedMemoryView<'_> {
    fn is_c_contiguous(&self) -> bool {
        memoryview_is_c_contiguous(self.shape, self.strides, self.itemsize)
    }

    /// The descriptor has already proved every element's signed extent. Copy
    /// callbacks cannot run Python or mutate the descriptor/export lifetime.
    /// Gathering consumes this C-order traversal without heap index allocation.
    unsafe fn for_each_byte_chunk(
        &self,
        len: usize,
        mut copy: impl FnMut(*mut u8, std::ops::Range<usize>),
    ) {
        debug_assert!(len <= self.len);
        if len == 0 {
            return;
        }
        if self.is_c_contiguous() {
            copy(self.data, 0..len);
            return;
        }
        let mut indices = [0isize; MOLT_BUFFER_MAX_NDIM];
        let indices = &mut indices[..self.shape.len()];
        let mut copied = 0;
        while copied < len {
            let offset = memoryview_strided_offset(indices, self.strides)
                .expect("validated memoryview element extent");
            let end = copied + self.itemsize.min(len - copied);
            unsafe { copy(self.data.offset(offset), copied..end) };
            copied = end;
            for axis in (0..indices.len()).rev() {
                indices[axis] += 1;
                if indices[axis] < self.shape[axis] {
                    break;
                }
                indices[axis] = 0;
            }
        }
    }
}

/// Borrow and validate the existing descriptor without cloning shape/strides
/// or reconstructing format. Contiguous, gather and derived-descriptor
/// consumers share this boundary; no failed contiguous admission may be retried
/// through an unchecked strided path. The caller retains the live export owner.
unsafe fn memoryview_borrowed_storage(
    ptr: *mut u8,
) -> Result<BorrowedMemoryView<'static>, BytesLikeSliceError> {
    unsafe {
        if memoryview_released(ptr) {
            return Err(BytesLikeSliceError::ReleasedMemoryView);
        }
        let shape = memoryview_shape(ptr).ok_or(BytesLikeSliceError::NotBytesLike)?;
        let strides = memoryview_strides(ptr).ok_or(BytesLikeSliceError::NotBytesLike)?;
        let itemsize = memoryview_itemsize(ptr);
        if itemsize == 0 || shape.len() != strides.len() || shape.len() > MOLT_BUFFER_MAX_NDIM {
            return Err(BytesLikeSliceError::NotBytesLike);
        }
        let len = memoryview_nbytes_big(shape, itemsize)
            .and_then(|value| usize::try_from(value).ok())
            .filter(|&value| value <= isize::MAX as usize)
            .ok_or(BytesLikeSliceError::NotBytesLike)?;
        let bounds = memoryview_strided_bounds(shape, strides, itemsize)
            .ok_or(BytesLikeSliceError::NotBytesLike)?;
        let data = memoryview_data(ptr);
        let offset = memoryview_offset(ptr);
        if data.is_null() || offset < 0 {
            return Err(BytesLikeSliceError::NotBytesLike);
        }
        if let Some(base) = obj_from_bits(memoryview_base_bits(ptr)).as_ptr()
            && let Some(bytes) = bytes_like_slice_raw(base)
        {
            if !memoryview_bounds_fit_base(
                offset,
                bounds.min_offset,
                bounds.max_end_offset,
                bytes.len(),
            ) || bytes.as_ptr().add(offset as usize).cast_mut() != data
            {
                return Err(BytesLikeSliceError::NotBytesLike);
            }
        }
        Ok(BorrowedMemoryView {
            data,
            len,
            itemsize,
            shape,
            strides,
            min_offset: bounds.min_offset,
            max_end_offset: bounds.max_end_offset,
        })
    }
}

unsafe fn memoryview_contiguous_byte_span(
    ptr: *mut u8,
) -> Result<BorrowedMemoryView<'static>, BytesLikeSliceError> {
    let storage = unsafe { memoryview_borrowed_storage(ptr)? };
    if !storage.is_c_contiguous() {
        return Err(BytesLikeSliceError::NonContiguousMemoryView);
    }
    Ok(storage)
}

pub(crate) unsafe fn bytes_like_slice_checked(
    ptr: *mut u8,
) -> Result<&'static [u8], BytesLikeSliceError> {
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_MEMORYVIEW {
            let storage = memoryview_contiguous_byte_span(ptr)?;
            return Ok(std::slice::from_raw_parts(
                storage.data.cast_const(),
                storage.len,
            ));
        }
        bytes_like_slice_raw(ptr).ok_or(BytesLikeSliceError::NotBytesLike)
    }
}

pub(crate) unsafe fn bytes_like_slice(ptr: *mut u8) -> Option<&'static [u8]> {
    unsafe { bytes_like_slice_checked(ptr).ok() }
}

#[cfg(test)]
mod split_buffer_contract_tests {
    use super::*;
    use crate::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SCALAR_RELEASE_VIEW: AtomicU64 = AtomicU64::new(0);
    static SCALAR_KIND: AtomicU64 = AtomicU64::new(0);
    static SCALAR_CONVERSION_CALLS: AtomicU64 = AtomicU64::new(0);

    extern "C" fn release_during_scalar_conversion(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            SCALAR_CONVERSION_CALLS.fetch_add(1, Ordering::SeqCst);
            molt_memoryview_release(SCALAR_RELEASE_VIEW.load(Ordering::SeqCst));
            assert!(!exception_pending(py));
            match SCALAR_KIND.load(Ordering::SeqCst) {
                0 => MoltObject::from_int(120).bits(),
                1 => MoltObject::from_bool(true).bits(),
                _ => MoltObject::from_float(1.25).bits(),
            }
        })
    }

    fn releasing_scalar(py: &PyToken<'_>, protocol: &[u8]) -> u64 {
        let name = attr_name_bits_from_bytes(py, b"ReleasingBufferScalar").unwrap();
        let class = molt_class_new(name);
        molt_class_set_base(class, builtin_classes(py).object);
        let method = attr_name_bits_from_bytes(py, protocol).unwrap();
        let function = alloc_runtime_function_obj(
            py,
            runtime_fn_addr(
                "release_during_scalar_conversion",
                release_during_scalar_conversion as *const (),
            ),
            1,
        );
        assert!(!function.is_null());
        let function = MoltObject::from_ptr(function).bits();
        molt_set_attr_name(class, method, function);
        let class_ptr = obj_from_bits(class).as_ptr().unwrap();
        unsafe { crate::object::class_finish_definition(py, class_ptr).unwrap() };
        let size = unsafe { crate::object::layout::class_cached_layout_size(class_ptr).unwrap() };
        let value = crate::object::builders::alloc_class_instance(py, size, class);
        unsafe {
            crate::object::gc::gc_publish_initialized(py, obj_from_bits(value).as_ptr().unwrap())
        };
        for bits in [name, method, function, class] {
            dec_ref_bits(py, bits);
        }
        assert!(!exception_pending(py));
        value
    }

    #[test]
    fn split_contract_scalar_conversion_revalidates_released_destination() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for (kind, code, protocol) in [
                (0, "B", b"__index__".as_slice()),
                (1, "?", b"__bool__".as_slice()),
                (2, "f", b"__float__".as_slice()),
            ] {
                let base = alloc_bytearray(py, &[0; 4]);
                let format = alloc_string(py, code.as_bytes());
                assert!(!base.is_null() && !format.is_null());
                let base_bits = MoltObject::from_ptr(base).bits();
                let format_bits = MoltObject::from_ptr(format).bits();
                let fmt = memoryview_format_from_str(code).unwrap();
                let storage = TypedStridedStorage::one_dim(
                    unsafe { bytes_data(base).cast_mut() },
                    false,
                    4 / fmt.itemsize,
                    fmt.itemsize,
                    fmt.itemsize as isize,
                    0,
                    base_bits,
                    format_bits,
                )
                .unwrap();
                let view = crate::object::builders::alloc_memoryview_from_storage(py, storage);
                assert!(!view.is_null());
                let view_bits = MoltObject::from_ptr(view).bits();
                SCALAR_RELEASE_VIEW.store(view_bits, Ordering::SeqCst);
                SCALAR_KIND.store(kind, Ordering::SeqCst);
                let value = releasing_scalar(py, protocol);
                assert!(unsafe { memoryview_write_scalar_at(py, view, 0, fmt, value) }.is_none());
                assert!(exception_pending(py));
                let error = crate::builtins::exceptions::molt_exception_last_pending();
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    error,
                    "ValueError"
                ));
                clear_exception(py);
                dec_ref_bits(py, error);
                assert_eq!(unsafe { bytes_like_slice_checked(base) }.unwrap(), &[0; 4]);
                for bits in [value, view_bits, format_bits, base_bits] {
                    dec_ref_bits(py, bits);
                }
            }
            SCALAR_RELEASE_VIEW.store(0, Ordering::SeqCst);
        });
    }

    #[test]
    fn split_contract_scalar_admission_precedes_conversion() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let base = alloc_bytes(py, &[0]);
            assert!(!base.is_null());
            let base_bits = MoltObject::from_ptr(base).bits();
            let view_bits = molt_memoryview_new(base_bits);
            let view = obj_from_bits(view_bits).as_ptr().unwrap();
            let value = releasing_scalar(py, b"__index__");
            SCALAR_RELEASE_VIEW.store(view_bits, Ordering::SeqCst);
            SCALAR_CONVERSION_CALLS.store(0, Ordering::SeqCst);
            let fmt = memoryview_format_from_str("B").unwrap();
            // If the callback executes on this readonly view, it releases it.
            assert!(unsafe { memoryview_write_scalar_at(py, view, 0, fmt, value) }.is_none());
            assert!(!unsafe { memoryview_released(view) });
            assert!(exception_pending(py));
            clear_exception(py);
            molt_memoryview_release(view_bits);
            // A second invocation must reject the released view before callback.
            assert!(unsafe { memoryview_write_scalar_at(py, view, 0, fmt, value) }.is_none());
            assert_eq!(SCALAR_CONVERSION_CALLS.load(Ordering::SeqCst), 0);
            let error = crate::builtins::exceptions::molt_exception_last_pending();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                error,
                "ValueError"
            ));
            clear_exception(py);
            dec_ref_bits(py, error);
            for bits in [value, view_bits, base_bits] {
                dec_ref_bits(py, bits);
            }
            SCALAR_RELEASE_VIEW.store(0, Ordering::SeqCst);
        });
    }

    #[test]
    fn split_contract_scalar_packing_translates_only_type_and_range_errors() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for format in ["B", "f", "d", "?"] {
                let fmt = memoryview_format_from_str(format).unwrap();
                for (input, output, detail) in [
                    ("TypeError", "TypeError", Some("type")),
                    ("OverflowError", "ValueError", Some("value")),
                    ("LookupError", "LookupError", None),
                ] {
                    let (output, detail) = if format == "?" {
                        (input, None)
                    } else {
                        (output, detail)
                    };
                    let _ = raise_exception::<u64>(py, input, "scalar callback");
                    assert!(memoryview_pack_error(py, fmt).is_none());
                    let error = crate::builtins::exceptions::molt_exception_last_pending();
                    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                        py, error, output
                    ));
                    let message = crate::builtins::exceptions::exception_materialized_message_bits(
                        py,
                        obj_from_bits(error).as_ptr().unwrap(),
                    );
                    let expected = detail.map_or_else(
                        || "scalar callback".to_string(),
                        |kind| format!("memoryview: invalid {kind} for format '{format}'"),
                    );
                    assert_eq!(
                        string_obj_to_owned(obj_from_bits(message)).unwrap(),
                        expected
                    );
                    clear_exception(py);
                    dec_ref_bits(py, error);
                }
            }
        });
    }

    #[test]
    fn split_contract_typed_buffer_admission_uses_shape_stride_and_byte_extent() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for (data, itemsize, shape, strides, expected) in [
                (b"".as_slice(), 1, vec![0], vec![1], Ok(b"".as_slice())),
                (
                    b"|x|".as_slice(),
                    1,
                    vec![0],
                    vec![2],
                    Err(BytesLikeSliceError::NonContiguousMemoryView),
                ),
                (b"|x".as_slice(), 1, vec![1], vec![2], Ok(b"|".as_slice())),
                (
                    b"|x|".as_slice(),
                    1,
                    vec![2],
                    vec![2],
                    Err(BytesLikeSliceError::NonContiguousMemoryView),
                ),
                (b"||".as_slice(), 2, vec![1], vec![2], Ok(b"||".as_slice())),
                (
                    b"||".as_slice(),
                    1,
                    vec![1, 2],
                    vec![2, 1],
                    Ok(b"||".as_slice()),
                ),
                (
                    b"".as_slice(),
                    1,
                    vec![0, 2],
                    vec![4, 1],
                    Ok(b"".as_slice()),
                ),
            ] {
                let base = alloc_bytes(py, data);
                assert!(!base.is_null());
                let base_bits = MoltObject::from_ptr(base).bits();
                let format = alloc_string(py, if itemsize == 2 { b"H" } else { b"B" });
                assert!(!format.is_null());
                let format_bits = MoltObject::from_ptr(format).bits();
                let storage = TypedStridedStorage::new(
                    unsafe { bytes_data(base) } as *mut u8,
                    true,
                    itemsize,
                    0,
                    base_bits,
                    format_bits,
                    shape,
                    strides,
                )
                .unwrap();
                let view = crate::object::builders::alloc_memoryview_from_storage(py, storage);
                assert!(!view.is_null());
                let view_bits = MoltObject::from_ptr(view).bits();
                unsafe {
                    assert_eq!(memoryview_is_c_contiguous_view(view), expected.is_ok());
                    assert_eq!(bytes_like_slice_checked(view), expected);
                    assert_eq!(memoryview_bytes_slice(view), expected.ok());
                }
                molt_memoryview_release(view_bits);
                unsafe {
                    assert!(!memoryview_is_c_contiguous_view(view));
                    assert_eq!(
                        bytes_like_slice_checked(view),
                        Err(BytesLikeSliceError::ReleasedMemoryView)
                    );
                }
                for bits in [view_bits, format_bits, base_bits] {
                    dec_ref_bits(py, bits);
                }
                assert!(!exception_pending(py));
            }
        });
    }

    #[test]
    fn split_contract_mutable_typed_buffer_uses_the_same_byte_extent() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let base = alloc_bytearray(py, b"ab");
            assert!(!base.is_null());
            let base_bits = MoltObject::from_ptr(base).bits();
            let format = alloc_string(py, b"H");
            assert!(!format.is_null());
            let format_bits = MoltObject::from_ptr(format).bits();
            let storage = TypedStridedStorage::one_dim(
                unsafe { bytes_data(base) } as *mut u8,
                false,
                1,
                2,
                2,
                0,
                base_bits,
                format_bits,
            )
            .unwrap();
            let view = crate::object::builders::alloc_memoryview_from_storage(py, storage);
            assert!(!view.is_null());
            {
                let buffer = crate::object::buffer_exports::ScopedWritableBuffer::new(
                    py,
                    MoltObject::from_ptr(view).bits(),
                )
                .unwrap();
                assert_eq!(buffer.len(), 2);
                unsafe {
                    std::slice::from_raw_parts_mut(buffer.as_mut_ptr(), buffer.len())
                        .copy_from_slice(b"cd");
                    assert_eq!(bytes_like_slice_checked(base).unwrap(), b"cd");
                }
            }
            for bits in [MoltObject::from_ptr(view).bits(), format_bits, base_bits] {
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn split_contract_gather_uses_validated_storage_and_c_order() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for (shape, strides, offset, collected) in [
                (vec![4], vec![1], 0, b"abcd".as_slice()),
                (vec![2], vec![-2], 2, b"ca".as_slice()),
                (vec![2, 2], vec![1, 2], 0, b"acbd".as_slice()),
                (vec![0, 2], vec![4, 1], 0, b"".as_slice()),
            ] {
                let base = alloc_bytearray(py, b"abcd");
                let format = alloc_string(py, b"B");
                assert!(!base.is_null() && !format.is_null());
                let base_bits = MoltObject::from_ptr(base).bits();
                let format_bits = MoltObject::from_ptr(format).bits();
                let storage = TypedStridedStorage::new(
                    unsafe { bytes_data(base).add(offset as usize).cast_mut() },
                    false,
                    1,
                    offset,
                    base_bits,
                    format_bits,
                    shape,
                    strides,
                )
                .unwrap();
                let view = crate::object::builders::alloc_memoryview_from_storage(py, storage);
                assert!(!view.is_null());
                unsafe {
                    assert_eq!(memoryview_collect_bytes(view).unwrap(), collected);
                    assert_eq!(bytes_like_slice_checked(base).unwrap(), b"abcd");
                }
                for bits in [MoltObject::from_ptr(view).bits(), format_bits, base_bits] {
                    dec_ref_bits(py, bits);
                }
                assert!(!exception_pending(py));
            }
        });
    }

    #[test]
    fn split_contract_invalid_backing_cannot_retry_through_gather() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let base = alloc_bytearray(py, b"abcd");
            assert!(!base.is_null());
            let base_bits = MoltObject::from_ptr(base).bits();
            let view_bits = molt_memoryview_new(base_bits);
            assert!(!exception_pending(py));
            let view = obj_from_bits(view_bits).as_ptr().unwrap();
            unsafe {
                let descriptor = crate::object::memoryview_ptr(view);
                let original_data = (*descriptor).data;
                // Corrupt an internal descriptor, never the exporter allocation.
                // Both a pointer/offset mismatch and an out-of-range extent must
                // be rejected before any slice creation, including fallbacks.
                for offset in [0, 1] {
                    (*descriptor).offset = offset;
                    (*descriptor).data = original_data.add(1);
                    assert_eq!(
                        bytes_like_slice_checked(view),
                        Err(BytesLikeSliceError::NotBytesLike)
                    );
                    assert!(memoryview_collect_bytes(view).is_none());
                    assert!(TypedStridedStorage::from_object_bits(view_bits).is_err());
                    assert_eq!(bytes_like_slice_checked(base).unwrap(), b"abcd");
                }
                (*descriptor).offset = 0;
                (*descriptor).data = original_data;
                // The same admission boundary protects non-contiguous copying.
                let strides = crate::object::memoryview_strides_ptr(view);
                (&mut *strides)[0] = 2;
                assert!(memoryview_collect_bytes(view).is_none());
                (&mut *strides)[0] = 1;
            }
            molt_memoryview_release(view_bits);
            unsafe {
                assert!(memoryview_collect_bytes(view).is_none());
            }
            dec_ref_bits(py, view_bits);
            dec_ref_bits(py, base_bits);
            assert!(!exception_pending(py));
        });
    }
}
