#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::{
    MemoryViewFormat, MemoryViewFormatKind, MoltObject, PyToken, TYPE_ID_BYTEARRAY, TYPE_ID_BYTES,
    TYPE_ID_MEMORYVIEW, TYPE_ID_STRING, alloc_bytes, bytes_data, bytes_len, index_bigint_from_obj,
    is_truthy, memoryview_base_bits, memoryview_data, memoryview_format_bits, memoryview_itemsize,
    memoryview_offset, memoryview_owner_bits, memoryview_readonly, memoryview_released,
    memoryview_shape, memoryview_strides, obj_from_bits, object_type_id, raise_exception,
    string_bytes, string_len, string_obj_to_owned,
};
use num_bigint::BigInt;
use num_traits::ToPrimitive;

use crate::builtins::compatibility_error::CompatibilityError;

/// Restricted release views may be inspected while the release callback runs,
/// but must never mint a derived view or independently retained buffer export.
pub(crate) unsafe fn require_exportable(py: &PyToken<'_>, view: *mut u8) -> bool {
    unsafe {
        if memoryview_released(view) {
            let _ = raise_released_memoryview::<u64>(py);
            return false;
        }
        if (*crate::memoryview_ptr(view)).restricted != 0 {
            let _ = raise_exception::<u64>(
                py,
                "ValueError",
                "cannot create new references to a restricted memoryview",
            );
            return false;
        }
        true
    }
}

/// Slice the first dimension for ordinary, stepped and C-API subscripting.
/// Like CPython's mbuf_add_view, pin the derived export before index callbacks.
pub(crate) unsafe fn memoryview_slice(
    py: &PyToken<'_>,
    view: *mut u8,
    start_bits: u64,
    stop_bits: u64,
    step_bits: u64,
) -> u64 {
    unsafe {
        if !require_exportable(py, view) {
            return MoltObject::none().bits();
        }
        if memoryview_shape(view).is_some_and(|shape| shape.is_empty()) {
            return raise_exception(py, "TypeError", "invalid indexing of 0-dim memory");
        }
        let Ok(source) = TypedStridedStorage::from_memoryview_ptr(view) else {
            return raise_exception(py, "BufferError", "invalid memoryview storage");
        };
        let mut pinned = match super::builders::PinnedMemoryViewStorage::new(py, source) {
            Ok(pinned) => pinned,
            Err(()) => return MoltObject::none().bits(),
        };
        let (start, stop, step) = match crate::object::ops_sys::normalize_slice_indices(
            py,
            pinned.storage().shape[0],
            obj_from_bits(start_bits),
            obj_from_bits(stop_bits),
            obj_from_bits(step_bits),
        ) {
            Ok(indices) => indices,
            Err(error) => return crate::object::ops_sys::slice_error(py, error),
        };
        // The parent may now be released. Geometry and ownership belong to the
        // independent pin, so no parent field is read after conversion.
        if pinned.slice_first_axis(start, stop, step).is_none() {
            return MoltObject::none().bits();
        }
        let output = pinned.allocate();
        if output.is_null() {
            // The allocator preserves its existing error or raises explicitly.
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(output).bits()
    }
}

/// Slice assignment has operation-entry admission but no derived destination
/// export: release during a callback must succeed, then fail at copy_single's
/// recheck. The source export is acquired first and owns its descriptor until
/// all callbacks, copying, and error cleanup are finished.
pub(crate) unsafe fn memoryview_assign_slice(
    py: &PyToken<'_>,
    view: *mut u8,
    start_bits: u64,
    stop_bits: u64,
    step_bits: u64,
    value: u64,
    format: &str,
) -> Option<()> {
    unsafe {
        let source = match super::buffer_exports::ScopedBuffer::new(py, value) {
            Ok(source) => source,
            Err(error) => {
                error.raise(
                    py,
                    &format!(
                        "a bytes-like object is required, not '{}'",
                        crate::type_name(py, obj_from_bits(value)),
                    ),
                );
                return None;
            }
        };
        let result = (|| {
            // Release preserves shape/strides. Do not retain destination data
            // across source acquisition or slice callbacks, or pin its owner.
            let Some(len) = memoryview_shape(view)
                .and_then(|shape| shape.first())
                .copied()
            else {
                return raise_exception(py, "BufferError", "invalid memoryview slice storage");
            };
            let (start, stop, step) = match crate::object::ops_sys::normalize_slice_indices(
                py,
                len,
                obj_from_bits(start_bits),
                obj_from_bits(stop_bits),
                obj_from_bits(step_bits),
            ) {
                Ok(indices) => indices,
                Err(error) => {
                    crate::object::ops_sys::slice_error(py, error);
                    return None;
                }
            };
            if memoryview_released(view) {
                return raise_released_memoryview(py);
            }
            let Ok(mut destination) = TypedStridedStorage::from_memoryview_ptr(view) else {
                return raise_exception(py, "BufferError", "invalid memoryview slice storage");
            };
            destination.slice_first_axis(py, start, stop, step)?;
            let src = source.view();
            let format_end = src
                .format
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(src.format.len());
            let src_format = &src.format[..format_end];
            if src.ndim != 1
                || src.itemsize != destination.itemsize as u64
                || src.shape[0] != destination.shape[0]
                || src_format.strip_prefix(b"@").unwrap_or(src_format) != format.as_bytes()
            {
                return raise_exception(
                    py,
                    "ValueError",
                    "memoryview assignment: lvalue and rvalue have different structures",
                );
            }
            if src.len != destination.len as u64 || (src.len != 0 && src.data.is_null()) {
                return raise_exception(py, "BufferError", "invalid memoryview source storage");
            }
            let Some(bounds) =
                memoryview_strided_bounds(&src.shape[..1], &src.strides[..1], destination.itemsize)
            else {
                return raise_exception(py, "BufferError", "invalid memoryview source strides");
            };
            let src = BorrowedMemoryView {
                data: src.data,
                len: destination.len,
                itemsize: destination.itemsize,
                shape: &src.shape[..1],
                strides: &src.strides[..1],
                min_offset: bounds.min_offset,
                max_end_offset: bounds.max_end_offset,
            };
            let dst = BorrowedMemoryView {
                data: destination.data,
                len: destination.len,
                itemsize: destination.itemsize,
                shape: &destination.shape,
                strides: &destination.strides,
                min_offset: destination.min_offset,
                max_end_offset: destination.max_end_offset,
            };
            if dst.len == 0 {
                return Some(());
            }
            if src.is_c_contiguous() && dst.is_c_contiguous() {
                // memmove semantics cover overlapping assignment without a copy.
                std::ptr::copy(src.data.cast_const(), dst.data, dst.len);
            } else {
                let mut bytes = Vec::new();
                if bytes.try_reserve_exact(src.len).is_err() {
                    return raise_exception(
                        py,
                        "MemoryError",
                        "memoryview assignment allocation failed",
                    );
                }
                src.for_each_byte_chunk(src.len, MemoryViewOrder::C, |data, range| {
                    bytes.extend_from_slice(std::slice::from_raw_parts(
                        data.cast_const(),
                        range.len(),
                    ));
                });
                dst.for_each_byte_chunk(dst.len, MemoryViewOrder::C, |data, range| {
                    std::ptr::copy_nonoverlapping(
                        bytes.as_ptr().add(range.start),
                        data,
                        range.len(),
                    );
                });
            }
            Some(())
        })();
        // A real exporter release may run foreign/Python finalizers. Preserve
        // both success and the exact callback/geometry/structure failure.
        molt_cpython_abi::api::errors::with_preserved_error(|| drop(source));
        result
    }
}

/// CPython adjust_fmt admits syntax before any scalar-format decision.
pub(crate) unsafe fn memoryview_adjust_format(py: &PyToken<'_>, view: *mut u8) -> Option<String> {
    let format = string_obj_to_owned(obj_from_bits(unsafe { memoryview_format_bits(view) }))?;
    let code = format.strip_prefix('@').unwrap_or(&format);
    if code.len() != 1 {
        return CompatibilityError::MemoryviewFormatSyntax { format: &format }.raise(py);
    }
    Some(code.to_owned())
}

pub(crate) fn memoryview_scalar_format(py: &PyToken<'_>, format: &str) -> Option<MemoryViewFormat> {
    match memoryview_format_from_str(format) {
        Some(fmt) => Some(fmt),
        None => CompatibilityError::MemoryviewFormatScalar { format }.raise(py),
    }
}

pub(crate) unsafe fn memoryview_is_index_key(py: &PyToken<'_>, key: u64) -> bool {
    crate::builtins::numbers::index_integral_payload_bits(key).is_some()
        || unsafe { crate::builtins::attr::has_special_method(py, key, b"__index__") }
}

pub(crate) unsafe fn memoryview_prepare_iter(py: &PyToken<'_>, view: *mut u8) -> Option<String> {
    // Maintained CPython 3.12/3.13/3.14 check release before rank and syntax.
    // Scalar code support waits for next(); historical iterator construction
    // that returned a value with an exception pending is not a stable oracle.
    if unsafe { memoryview_released(view) } {
        return raise_released_memoryview(py);
    }
    let rank = unsafe { crate::memoryview_ndim(view) };
    if rank == 0 {
        return raise_exception(py, "TypeError", "invalid indexing of 0-dim memory");
    }
    if rank != 1 {
        return CompatibilityError::MemoryviewSubView {
            rank,
            indices: 1,
            tuple: false,
            assignment: false,
        }
        .raise(py);
    }
    unsafe { memoryview_adjust_format(py, view) }
}

pub(crate) unsafe fn memoryview_read_item_at(
    py: &PyToken<'_>,
    view: *mut u8,
    offset: isize,
    format: &str,
) -> Option<u64> {
    if unsafe { memoryview_released(view) } {
        return raise_released_memoryview(py);
    }
    let fmt = memoryview_scalar_format(py, format)?;
    unsafe { memoryview_read_scalar_at(py, view, offset, fmt) }
}

pub(crate) unsafe fn memoryview_write_item_at(
    py: &PyToken<'_>,
    view: *mut u8,
    offset: isize,
    format: &str,
    value: u64,
) -> Option<()> {
    // Operation entry already admitted release, format syntax and readonly
    // before key callbacks. Unsupported scalar formats and value conversion
    // errors still precede the final release check after those callbacks.
    let fmt = memoryview_scalar_format(py, format)?;
    unsafe { memoryview_write_scalar_at(py, view, offset, fmt, value) }
}

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
    pub(crate) native_lease: Option<molt_cpython_abi::api::memory::MemoryViewLease>,
    pub(crate) format_bits: u64,
    pub(crate) format: [u8; MOLT_BUFFER_FORMAT_CAP],
    pub(crate) shape: Vec<isize>,
    pub(crate) strides: Vec<isize>,
}

impl TypedStridedStorage {
    /// Derive checked first-axis geometry without copying the owned vectors.
    /// The caller normalized indices while retaining the required ownership;
    /// both derived views and slice assignment use this one geometry authority.
    pub(crate) fn slice_first_axis(
        &mut self,
        py: &PyToken<'_>,
        start: isize,
        stop: isize,
        step: isize,
    ) -> Option<()> {
        if self.shape.is_empty() || self.shape.len() != self.strides.len() || step == 0 {
            return raise_exception(py, "BufferError", "invalid memoryview slice storage");
        }
        let new_len = crate::range_len_i64(start as i64, stop as i64, step as i64).max(0);
        let base_stride = self.strides[0];
        let empty = new_len == 0 || self.len == 0;
        let delta = if empty {
            0
        } else {
            let Some(delta) = usize::try_from(start)
                .ok()
                .and_then(|start| memoryview_linear_offset(start, base_stride))
            else {
                return raise_exception(py, "BufferError", "invalid memoryview slice offset");
            };
            delta
        };
        let Some(offset) = self.offset.checked_add(delta) else {
            return raise_exception(py, "BufferError", "invalid memoryview slice offset");
        };
        let stride = match base_stride.checked_mul(step) {
            Some(stride) => stride,
            // A singleton/empty first axis cannot use its stride in addressing.
            None if new_len <= 1 || empty => base_stride.wrapping_mul(step),
            None => return raise_exception(py, "BufferError", "invalid memoryview slice stride"),
        };
        let old_len = self.shape[0];
        self.shape[0] = new_len as isize;
        self.strides[0] = stride;
        // Geometry validation cannot reenter. Restore the descriptor before
        // raising on failure; inline formats and ownership never move.
        let geometry = memoryview_checked_nbytes(&self.shape, self.itemsize).zip(
            memoryview_strided_bounds(&self.shape, &self.strides, self.itemsize),
        );
        let Some((len, bounds)) = geometry else {
            self.shape[0] = old_len;
            self.strides[0] = base_stride;
            return raise_exception(py, "BufferError", "invalid memoryview slice storage");
        };
        if !empty {
            self.data = unsafe { self.data.offset(delta) };
        }
        self.offset = offset;
        self.len = len;
        self.span_len = bounds.span_len;
        self.min_offset = bounds.min_offset;
        self.max_end_offset = bounds.max_end_offset;
        Some(())
    }

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
        let len = memoryview_checked_nbytes(shape.as_slice(), itemsize)?;
        let bounds = memoryview_strided_bounds(shape.as_slice(), strides.as_slice(), itemsize)?;
        let format = if format_bits == 0 {
            default_buffer_format()
        } else {
            unsafe { memoryview_format_export_bytes(format_bits)? }
        };
        Some(Self {
            data,
            len,
            span_len: bounds.span_len,
            min_offset: bounds.min_offset,
            max_end_offset: bounds.max_end_offset,
            readonly,
            itemsize,
            offset,
            base_bits,
            owner_bits: base_bits,
            native_lease: None,
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

    pub(crate) fn with_native_lease(
        mut self,
        lease: Option<molt_cpython_abi::api::memory::MemoryViewLease>,
    ) -> Self {
        self.native_lease = lease;
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
            .map(|storage| {
                storage
                    .with_owner(memoryview_owner_bits(ptr))
                    .with_native_lease((*crate::memoryview_ptr(ptr)).native_lease.clone())
            })
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
        b'e' => (2, MemoryViewFormatKind::Float),
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

pub(crate) fn memoryview_checked_nbytes(shape: &[isize], itemsize: usize) -> Option<usize> {
    usize::try_from(memoryview_nbytes_big(shape, itemsize)?)
        .ok()
        .filter(|&len| len <= isize::MAX as usize)
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MemoryViewOrder {
    C,
    Fortran,
    Any,
}

pub(crate) fn memoryview_is_c_contiguous(
    shape: &[isize],
    strides: &[isize],
    itemsize: usize,
) -> bool {
    memoryview_is_contiguous(shape, strides, itemsize, MemoryViewOrder::C)
}

fn memoryview_is_contiguous(
    shape: &[isize],
    strides: &[isize],
    itemsize: usize,
    order: MemoryViewOrder,
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
    for step in 0..shape.len() {
        let idx = if order == MemoryViewOrder::Fortran {
            step
        } else {
            shape.len() - 1 - step
        };
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

/// Materialize a borrowed normalized C export using the same validated bounds
/// and byte traversal as memoryview. The ABI caller owns its native lease.
pub(crate) unsafe fn collect_bytes_from_descriptor(
    py: &PyToken<'_>,
    view: &molt_cpython_abi::hooks::MoltBufferView,
) -> Option<Vec<u8>> {
    let rank = view.ndim as usize;
    if rank > MOLT_BUFFER_MAX_NDIM || view.itemsize == 0 || view.itemsize > isize::MAX as u64 {
        raise_exception::<()>(py, "BufferError", "invalid bytes buffer geometry");
        return None;
    }
    let shape = &view.shape[..rank];
    let strides = &view.strides[..rank];
    let itemsize = view.itemsize as usize;
    let Some(len) = memoryview_nbytes_big(shape, itemsize)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|&value| value <= isize::MAX as usize && value as u64 == view.len)
    else {
        raise_exception::<()>(py, "BufferError", "invalid bytes buffer length");
        return None;
    };
    let Some(bounds) = memoryview_strided_bounds(shape, strides, itemsize) else {
        raise_exception::<()>(py, "BufferError", "invalid bytes buffer strides");
        return None;
    };
    if view.data.is_null() && len != 0 {
        raise_exception::<()>(py, "BufferError", "bytes buffer has no data pointer");
        return None;
    }
    let storage = BorrowedMemoryView {
        data: view.data,
        len,
        itemsize,
        shape,
        strides,
        min_offset: bounds.min_offset,
        max_end_offset: bounds.max_end_offset,
    };
    let mut bytes = Vec::new();
    if bytes.try_reserve_exact(len).is_err() {
        raise_exception::<()>(py, "MemoryError", "bytes allocation failed");
        return None;
    }
    unsafe {
        storage.for_each_byte_chunk(len, MemoryViewOrder::C, |source, range| {
            bytes.extend_from_slice(std::slice::from_raw_parts(source.cast_const(), range.len()));
        })
    };
    Some(bytes)
}

pub(crate) unsafe fn memoryview_collect_bytes(ptr: *mut u8) -> Option<Vec<u8>> {
    unsafe { memoryview_collect_bytes_in_order(ptr, MemoryViewOrder::C) }
}

pub(crate) unsafe fn memoryview_collect_bytes_in_order(
    ptr: *mut u8,
    order: MemoryViewOrder,
) -> Option<Vec<u8>> {
    unsafe {
        let storage = memoryview_borrowed_storage(ptr).ok()?;
        let mut out = Vec::with_capacity(storage.len);
        storage.for_each_byte_chunk(storage.len, order, |source, range| {
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
    if offset
        .checked_add(fmt.itemsize)
        .is_none_or(|end| end > data.len())
    {
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
            if fmt.itemsize == 2 {
                let bytes: [u8; 2] = data[offset..offset + 2].try_into().ok()?;
                let val = molt_obj_model::float_bits::f16_bits_to_f64(u16::from_ne_bytes(bytes));
                Some(crate::object::ops::float_result_bits(_py, val))
            } else if fmt.itemsize == 4 {
                let bytes: [u8; 4] = data[offset..offset + 4].try_into().ok()?;
                let val = f32::from_ne_bytes(bytes) as f64;
                Some(crate::object::ops::float_result_bits(_py, val))
            } else if fmt.itemsize == 8 {
                let bytes: [u8; 8] = data[offset..offset + 8].try_into().ok()?;
                let val = f64::from_ne_bytes(bytes);
                Some(crate::object::ops::float_result_bits(_py, val))
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
            Some(crate::int_bits_from_i128(_py, i128::from(val)))
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
            Some(crate::int_bits_from_i128(_py, i128::from(val)))
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
    // Boxing a wide integer or a character may allocate/reenter. Retain no
    // borrowed exporter slice across that boundary.
    let mut item = [0u8; 8];
    if fmt.itemsize > item.len() {
        return None;
    }
    unsafe { std::ptr::copy_nonoverlapping(data.cast_const(), item.as_mut_ptr(), fmt.itemsize) };
    unsafe { memoryview_read_scalar(_py, &item[..fmt.itemsize], 0, fmt) }
}

enum MemoryViewScalar {
    Byte(u8),
    Float(f64),
    Integer(BigInt),
}

fn memoryview_integer_fits(value: &BigInt, bytes: usize, signed: bool) -> bool {
    let bits = (bytes * 8) as u32;
    let (min, max) = if signed {
        let limit = BigInt::from(1u8) << (bits - 1);
        (-limit.clone(), limit - 1)
    } else {
        (BigInt::from(0u8), (BigInt::from(1u8) << bits) - 1)
    };
    value >= &min && value <= &max
}

fn memoryview_scalar_value_error<T>(py: &PyToken<'_>, fmt: MemoryViewFormat) -> Option<T> {
    raise_exception(
        py,
        "ValueError",
        &format!(
            "memoryview: invalid value for format '{}'",
            fmt.code as char
        ),
    )
}

/// Perform Python conversion and its native C-width admission without borrowing
/// destination bytes. Only errors from this phase use CPython's fix_error_int.
fn memoryview_convert_scalar(
    py: &PyToken<'_>,
    fmt: MemoryViewFormat,
    val_bits: u64,
) -> Option<MemoryViewScalar> {
    match fmt.kind {
        MemoryViewFormatKind::Char => unsafe {
            let Some(ptr) = obj_from_bits(val_bits).as_ptr() else {
                return raise_exception(
                    py,
                    "TypeError",
                    &format!("memoryview: invalid type for format '{}'", fmt.code as char),
                );
            };
            if object_type_id(ptr) != TYPE_ID_BYTES {
                return raise_exception(
                    py,
                    "TypeError",
                    &format!("memoryview: invalid type for format '{}'", fmt.code as char),
                );
            }
            let bytes = bytes_like_slice_raw(ptr).unwrap_or(&[]);
            if bytes.len() != 1 {
                return memoryview_scalar_value_error(py, fmt);
            }
            Some(MemoryViewScalar::Byte(bytes[0]))
        },
        MemoryViewFormatKind::Bool => {
            let value = u8::from(is_truthy(py, obj_from_bits(val_bits)));
            if crate::exception_pending(py) {
                return None;
            }
            Some(MemoryViewScalar::Byte(value))
        }
        MemoryViewFormatKind::Float => {
            crate::builtins::numbers::float_as_double(py, val_bits).map(MemoryViewScalar::Float)
        }
        MemoryViewFormatKind::Signed | MemoryViewFormatKind::Unsigned => {
            let err_msg = format!("memoryview: invalid type for format '{}'", fmt.code as char);
            // 'P' uses PyLong_AsVoidPtr, which admits actual integers without
            // __index__ dispatch and accepts signed negative pointer values.
            let mut value = if fmt.code == b'P' {
                match crate::builtins::numbers::index_bigint_integral_bits(val_bits) {
                    Some(value) => value,
                    None => return raise_exception(py, "TypeError", &err_msg),
                }
            } else {
                index_bigint_from_obj(py, val_bits, &err_msg)?
            };
            let native_bytes = match fmt.code {
                b'b' | b'B' | b'h' | b'H' | b'i' | b'I' | b'l' | b'L' => {
                    std::mem::size_of::<libc::c_long>()
                }
                b'q' | b'Q' => 8,
                b'n' | b'N' | b'P' => std::mem::size_of::<usize>(),
                _ => return raise_exception(py, "BufferError", "invalid memoryview scalar format"),
            };
            let signed = fmt.kind == MemoryViewFormatKind::Signed
                || (fmt.code == b'P' && value < BigInt::from(0u8));
            if !memoryview_integer_fits(&value, native_bytes, signed) {
                // C conversion itself failed; this must precede release. The
                // smaller b/B/h/H/i/I destination range is checked later.
                return raise_exception(py, "OverflowError", "integer does not fit native format");
            }
            if fmt.code == b'P' && signed {
                value += BigInt::from(1u8) << (native_bytes * 8);
            }
            Some(MemoryViewScalar::Integer(value))
        }
    }
}

/// The sole encoder consumes converted values after the destination release
/// check. It cannot invoke Python; range errors are already final diagnostics.
fn memoryview_encode_scalar(
    py: &PyToken<'_>,
    data: &mut [u8],
    fmt: MemoryViewFormat,
    value: MemoryViewScalar,
) -> Option<()> {
    if !matches!(fmt.itemsize, 1 | 2 | 4 | 8) || fmt.itemsize > data.len() {
        return raise_exception(py, "BufferError", "invalid memoryview scalar format");
    }
    match value {
        MemoryViewScalar::Byte(value) => data[0] = value,
        MemoryViewScalar::Float(value) => match fmt.itemsize {
            2 => {
                let bits = match molt_obj_model::float_bits::f64_to_f16_bits(value) {
                    Ok(bits) => bits,
                    Err(_) => return memoryview_scalar_value_error(py, fmt),
                };
                data[..2].copy_from_slice(&bits.to_ne_bytes());
            }
            4 => data[..4].copy_from_slice(&(value as f32).to_ne_bytes()),
            8 => data[..8].copy_from_slice(&value.to_ne_bytes()),
            _ => return raise_exception(py, "BufferError", "invalid memoryview scalar format"),
        },
        MemoryViewScalar::Integer(value) => {
            let signed = fmt.kind == MemoryViewFormatKind::Signed;
            if !memoryview_integer_fits(&value, fmt.itemsize, signed) {
                return memoryview_scalar_value_error(py, fmt);
            }
            let bytes = if signed {
                value
                    .to_i64()
                    .expect("range checked signed scalar")
                    .to_ne_bytes()
            } else {
                value
                    .to_u64()
                    .expect("range checked unsigned scalar")
                    .to_ne_bytes()
            };
            let start = if cfg!(target_endian = "big") {
                8 - fmt.itemsize
            } else {
                0
            };
            data[..fmt.itemsize].copy_from_slice(&bytes[start..start + fmt.itemsize]);
        }
    }
    Some(())
}

unsafe fn memoryview_write_scalar_at(
    _py: &PyToken<'_>,
    view: *mut u8,
    offset: isize,
    fmt: MemoryViewFormat,
    val_bits: u64,
) -> Option<()> {
    // The operation admitted release/readonly before its key callbacks. Do not
    // repeat that admission here: value conversion errors must still win over
    // a release performed by an index callback.
    let value = match memoryview_convert_scalar(_py, fmt, val_bits) {
        Some(value) => value,
        None => return memoryview_pack_error(_py, fmt),
    };
    // Release errors bypass numeric conversion-error translation. Conversion
    // succeeded, so release also precedes destination-range and half packing.
    if unsafe { memoryview_released(view) } {
        return raise_released_memoryview(_py);
    }
    let mut encoded = [0u8; 8];
    memoryview_encode_scalar(_py, &mut encoded, fmt, value)?;
    let data = unsafe { memoryview_scalar_pointer(_py, view, offset, fmt)? };
    unsafe { std::ptr::copy_nonoverlapping(encoded.as_ptr(), data, fmt.itemsize) };
    Some(())
}

/// CPython pack_single translates numeric protocol type/value/range failures, but
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
    } else if exception_matches_builtin_name(_py, error, "OverflowError")
        || exception_matches_builtin_name(_py, error, "ValueError")
    {
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
    /// C, Fortran and physical-contiguous gathering share this traversal.
    unsafe fn for_each_byte_chunk(
        &self,
        len: usize,
        order: MemoryViewOrder,
        mut copy: impl FnMut(*mut u8, std::ops::Range<usize>),
    ) {
        debug_assert!(len <= self.len);
        if len == 0 {
            return;
        }
        let fortran_contiguous = memoryview_is_contiguous(
            self.shape,
            self.strides,
            self.itemsize,
            MemoryViewOrder::Fortran,
        );
        let order = if order == MemoryViewOrder::Any {
            if fortran_contiguous {
                MemoryViewOrder::Fortran
            } else {
                MemoryViewOrder::C
            }
        } else {
            order
        };
        if (order == MemoryViewOrder::C && self.is_c_contiguous())
            || (order == MemoryViewOrder::Fortran && fortran_contiguous)
        {
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
            for step in 0..indices.len() {
                let axis = if order == MemoryViewOrder::Fortran {
                    step
                } else {
                    indices.len() - 1 - step
                };
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
            && (!memoryview_bounds_fit_base(
                offset,
                bounds.min_offset,
                bounds.max_end_offset,
                bytes.len(),
            ) || bytes.as_ptr().add(offset as usize).cast_mut() != data)
        {
            return Err(BytesLikeSliceError::NotBytesLike);
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

/// Import an ABI descriptor into the ordinary runtime MemoryView. Format text
/// stays a full runtime string; the fixed-size transport format is never the
/// authority for a native memoryview's Python format or C export.
pub(crate) unsafe fn from_native_descriptor(
    py: &PyToken<'_>,
    descriptor: &molt_cpython_abi::hooks::MoltBufferView,
    format: *const std::ffi::c_char,
    lease: Option<molt_cpython_abi::api::memory::MemoryViewLease>,
) -> u64 {
    let rank = descriptor.ndim as usize;
    if rank > MOLT_BUFFER_MAX_NDIM
        || descriptor.itemsize == 0
        || descriptor.itemsize > isize::MAX as u64
        || descriptor.readonly > 1
        || (descriptor.len != 0 && descriptor.data.is_null())
    {
        return raise_exception(py, "BufferError", "invalid memoryview buffer descriptor");
    }
    let bytes = if format.is_null() {
        b"B".as_slice()
    } else {
        unsafe { std::ffi::CStr::from_ptr(format) }.to_bytes()
    };
    let format_ptr = crate::alloc_string(py, bytes);
    if format_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let format_bits = MoltObject::from_ptr(format_ptr).bits();
    let storage = TypedStridedStorage::new(
        descriptor.data,
        descriptor.readonly != 0,
        descriptor.itemsize as usize,
        0,
        0,
        format_bits,
        descriptor.shape[..rank].to_vec(),
        descriptor.strides[..rank].to_vec(),
    );
    let output = match storage {
        Some(mut storage) if storage.len as u64 == descriptor.len => {
            // Offset measures index zero from the lowest addressed backing byte.
            storage.offset = match storage.min_offset.checked_neg() {
                Some(offset) => offset,
                None => {
                    crate::dec_ref_bits(py, format_bits);
                    return raise_exception(py, "BufferError", "memoryview span overflow");
                }
            };
            crate::alloc_memoryview_from_storage(py, storage.with_native_lease(lease))
        }
        _ => std::ptr::null_mut(),
    };
    crate::dec_ref_bits(py, format_bits);
    if output.is_null() {
        if crate::exception_pending(py) {
            return MoltObject::none().bits();
        }
        return raise_exception(py, "BufferError", "invalid memoryview buffer geometry");
    }
    MoltObject::from_ptr(output).bits()
}

#[cfg(test)]
mod split_buffer_contract_tests {
    use super::*;
    use crate::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SCALAR_RELEASE_VIEW: AtomicU64 = AtomicU64::new(0);
    static SCALAR_KIND: AtomicU64 = AtomicU64::new(0);
    static SCALAR_CONVERSION_CALLS: AtomicU64 = AtomicU64::new(0);
    static SCALAR_RESIZE_OWNER: AtomicU64 = AtomicU64::new(0);

    extern "C" fn release_during_scalar_conversion(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            SCALAR_CONVERSION_CALLS.fetch_add(1, Ordering::SeqCst);
            molt_memoryview_release(SCALAR_RELEASE_VIEW.load(Ordering::SeqCst));
            assert!(!exception_pending(py));
            match SCALAR_KIND.load(Ordering::SeqCst) {
                0 => MoltObject::from_int(120).bits(),
                1 => MoltObject::from_bool(true).bits(),
                3 => MoltObject::from_int(300).bits(),
                4 => MoltObject::from_float(65520.0).bits(),
                5 => crate::int_bits_from_i128(py, i128::MAX),
                6 => MoltObject::from_int(-1).bits(),
                7 => {
                    crate::object::ops_bytes::molt_bytearray_append(
                        SCALAR_RESIZE_OWNER.load(Ordering::SeqCst),
                        MoltObject::from_int(99).bits(),
                    );
                    assert!(!exception_pending(py));
                    MoltObject::from_int(0).bits()
                }
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
                (3, "B", b"__index__".as_slice()),
                (1, "?", b"__bool__".as_slice()),
                (2, "f", b"__float__".as_slice()),
                (2, "d", b"__float__".as_slice()),
                (2, "e", b"__float__".as_slice()),
                (4, "e", b"__float__".as_slice()),
                (5, "B", b"__index__".as_slice()),
                (6, "B", b"__index__".as_slice()),
            ] {
                let base = alloc_bytearray(py, &[0; 8]);
                let format = alloc_string(py, code.as_bytes());
                assert!(!base.is_null() && !format.is_null());
                let base_bits = MoltObject::from_ptr(base).bits();
                let format_bits = MoltObject::from_ptr(format).bits();
                let fmt = memoryview_format_from_str(code).unwrap();
                let storage = TypedStridedStorage::one_dim(
                    unsafe { bytes_data(base).cast_mut() },
                    false,
                    8 / fmt.itemsize,
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
                crate::object::ops::molt_store_index(
                    view_bits,
                    MoltObject::from_int(0).bits(),
                    value,
                );
                assert!(exception_pending(py));
                let error = crate::builtins::exceptions::molt_exception_last_pending();
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    error,
                    "ValueError"
                ));
                assert_eq!(
                    crate::builtins::exceptions::format_exception_message(
                        py,
                        obj_from_bits(error).as_ptr().unwrap(),
                    ),
                    if kind == 5 || kind == 6 {
                        "memoryview: invalid value for format 'B'"
                    } else {
                        "operation forbidden on released memoryview object"
                    },
                    "format {code}, callback kind {kind}",
                );
                clear_exception(py);
                dec_ref_bits(py, error);
                assert_eq!(unsafe { bytes_like_slice_checked(base) }.unwrap(), &[0; 8]);
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
            // If the callback executes on this readonly view, it releases it.
            crate::object::ops::molt_store_index(view_bits, MoltObject::from_int(0).bits(), value);
            assert!(!unsafe { memoryview_released(view) });
            assert!(exception_pending(py));
            let error = crate::builtins::exceptions::molt_exception_last_pending();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                error,
                "TypeError"
            ));
            assert_eq!(
                crate::builtins::exceptions::format_exception_message(
                    py,
                    obj_from_bits(error).as_ptr().unwrap()
                ),
                "cannot modify read-only memory"
            );
            clear_exception(py);
            dec_ref_bits(py, error);
            molt_memoryview_release(view_bits);
            // A second invocation must reject the released view before callback.
            crate::object::ops::molt_store_index(view_bits, MoltObject::from_int(0).bits(), value);
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
    fn split_contract_character_store_rejects_key_release_after_owner_resize() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let base = alloc_bytearray(py, b"ABC");
            let format = alloc_string(py, b"c");
            let value = alloc_bytes(py, b"Z");
            assert!(!base.is_null() && !format.is_null() && !value.is_null());
            let base_bits = MoltObject::from_ptr(base).bits();
            let format_bits = MoltObject::from_ptr(format).bits();
            let value_bits = MoltObject::from_ptr(value).bits();
            let storage = TypedStridedStorage::one_dim(
                unsafe { bytes_data(base).cast_mut() },
                false,
                3,
                1,
                1,
                0,
                base_bits,
                format_bits,
            )
            .unwrap();
            let view = crate::object::builders::alloc_memoryview_from_storage(py, storage);
            assert!(!view.is_null());
            let view_bits = MoltObject::from_ptr(view).bits();
            let key = releasing_scalar(py, b"__index__");
            SCALAR_RELEASE_VIEW.store(view_bits, Ordering::SeqCst);
            SCALAR_RESIZE_OWNER.store(base_bits, Ordering::SeqCst);
            SCALAR_KIND.store(7, Ordering::SeqCst);
            SCALAR_CONVERSION_CALLS.store(0, Ordering::SeqCst);
            crate::object::ops::molt_store_index(view_bits, key, value_bits);
            assert_eq!(SCALAR_CONVERSION_CALLS.load(Ordering::SeqCst), 1);
            let error = crate::builtins::exceptions::molt_exception_last_pending();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                error,
                "ValueError"
            ));
            assert_eq!(
                crate::builtins::exceptions::format_exception_message(
                    py,
                    obj_from_bits(error).as_ptr().unwrap()
                ),
                RELEASED_MEMORYVIEW_ERROR
            );
            clear_exception(py);
            dec_ref_bits(py, error);
            assert_eq!(unsafe { bytes_like_slice_raw(base).unwrap() }, b"ABCc");
            for bits in [key, view_bits, value_bits, format_bits, base_bits] {
                dec_ref_bits(py, bits);
            }
            SCALAR_RELEASE_VIEW.store(0, Ordering::SeqCst);
            SCALAR_RESIZE_OWNER.store(0, Ordering::SeqCst);
        });
    }

    #[test]
    fn split_contract_slice_geometry_preserves_inline_format_and_failed_descriptor() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let mut data = [0u8; 8];
            let mut storage = TypedStridedStorage::new(
                data.as_mut_ptr(),
                false,
                2,
                0,
                0,
                0,
                vec![2, 2],
                vec![4, 2],
            )
            .unwrap();
            storage.format = buffer_format_from_bytes(b"h");
            storage.slice_first_axis(py, 1, -1, -1).unwrap();
            assert_eq!(storage.format, buffer_format_from_bytes(b"h"));
            assert_eq!(storage.shape, [2, 2]);
            assert_eq!(storage.strides, [-4, 2]);
            assert_eq!((storage.len, storage.span_len), (8, 8));
            assert_eq!((storage.min_offset, storage.max_end_offset), (-4, 4));
            let before = storage.clone();
            assert!(storage.slice_first_axis(py, 0, isize::MAX, 1).is_none());
            assert!(exception_pending(py));
            clear_exception(py);
            assert_eq!(storage.data, before.data);
            assert_eq!(storage.offset, before.offset);
            assert_eq!(storage.len, before.len);
            assert_eq!(storage.span_len, before.span_len);
            assert_eq!(storage.min_offset, before.min_offset);
            assert_eq!(storage.max_end_offset, before.max_end_offset);
            assert_eq!(storage.shape, before.shape);
            assert_eq!(storage.strides, before.strides);
            assert_eq!(storage.format, before.format);
        });
    }

    #[test]
    fn split_contract_scalar_packing_translates_only_type_value_and_range_errors() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for format in ["B", "f", "d", "?"] {
                let fmt = memoryview_format_from_str(format).unwrap();
                for (input, output, detail) in [
                    ("TypeError", "TypeError", Some("type")),
                    ("OverflowError", "ValueError", Some("value")),
                    ("ValueError", "ValueError", Some("value")),
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
                    let message = crate::builtins::exceptions::format_exception_message(
                        py,
                        obj_from_bits(error).as_ptr().unwrap(),
                    );
                    let expected = detail.map_or_else(
                        || "scalar callback".to_string(),
                        |kind| format!("memoryview: invalid {kind} for format '{format}'"),
                    );
                    assert_eq!(message, expected);
                    clear_exception(py);
                    dec_ref_bits(py, error);
                }
            }
            // Real get/set/iter/tolist consumers share syntax and scalar-code
            // admission. Unsupported native scalar codes are deferred by iter.
            for format in ["<i", "T{x:}", "w", "@w"] {
                for operation in 0..4 {
                    let owner = alloc_bytearray(py, &[0; 4]);
                    let format_ptr = alloc_string(py, format.as_bytes());
                    assert!(!owner.is_null() && !format_ptr.is_null());
                    let owner_bits = MoltObject::from_ptr(owner).bits();
                    let format_bits = MoltObject::from_ptr(format_ptr).bits();
                    let storage = TypedStridedStorage::one_dim(
                        unsafe { bytes_data(owner).cast_mut() },
                        false,
                        4,
                        1,
                        1,
                        0,
                        owner_bits,
                        format_bits,
                    )
                    .unwrap();
                    let view = crate::object::builders::alloc_memoryview_from_storage(py, storage);
                    assert!(!view.is_null());
                    let view_bits = MoltObject::from_ptr(view).bits();
                    let mut iterator = MoltObject::none().bits();
                    let result = match operation {
                        0 => molt_index(view_bits, MoltObject::from_int(0).bits()),
                        1 => molt_store_index(
                            view_bits,
                            MoltObject::from_int(0).bits(),
                            MoltObject::from_int(1).bits(),
                        ),
                        2 => molt_memoryview_tolist(view_bits),
                        _ => {
                            iterator = molt_iter(view_bits);
                            if exception_pending(py) {
                                MoltObject::none().bits()
                            } else {
                                molt_iter_next(iterator)
                            }
                        }
                    };
                    assert!(exception_pending(py), "{format}: consumer {operation}");
                    let error = crate::builtins::exceptions::molt_exception_last_pending();
                    assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                        py,
                        error,
                        "NotImplementedError"
                    ));
                    let message = crate::builtins::exceptions::format_exception_message(
                        py,
                        obj_from_bits(error).as_ptr().unwrap(),
                    );
                    let code = format.strip_prefix('@').unwrap_or(format);
                    let expected = if code.len() == 1 {
                        format!("memoryview: format {code} not supported")
                    } else {
                        format!("memoryview: unsupported format {format}")
                    };
                    assert_eq!(message, expected, "consumer {operation}");
                    clear_exception(py);
                    for bits in [error, result, iterator, view_bits, format_bits, owner_bits] {
                        dec_ref_bits(py, bits);
                    }
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
    fn split_contract_gather_uses_validated_storage_and_selected_order() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for (shape, strides, offset, c, f, a) in [
                (
                    vec![4],
                    vec![1],
                    0,
                    b"abcd".as_slice(),
                    b"abcd".as_slice(),
                    b"abcd".as_slice(),
                ),
                (
                    vec![2],
                    vec![-2],
                    2,
                    b"ca".as_slice(),
                    b"ca".as_slice(),
                    b"ca".as_slice(),
                ),
                (
                    vec![2, 2],
                    vec![1, 2],
                    0,
                    b"acbd".as_slice(),
                    b"abcd".as_slice(),
                    b"abcd".as_slice(),
                ),
                (
                    vec![2, 2],
                    vec![2, 1],
                    0,
                    b"abcd".as_slice(),
                    b"acbd".as_slice(),
                    b"abcd".as_slice(),
                ),
                (
                    vec![2, 2],
                    vec![-2, -1],
                    3,
                    b"dcba".as_slice(),
                    b"dbca".as_slice(),
                    b"dcba".as_slice(),
                ),
                (
                    vec![0, 2],
                    vec![4, 1],
                    0,
                    b"".as_slice(),
                    b"".as_slice(),
                    b"".as_slice(),
                ),
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
                    assert_eq!(memoryview_collect_bytes(view).unwrap(), c);
                    for (order, expected) in [
                        (MemoryViewOrder::C, c),
                        (MemoryViewOrder::Fortran, f),
                        (MemoryViewOrder::Any, a),
                    ] {
                        assert_eq!(
                            memoryview_collect_bytes_in_order(view, order).unwrap(),
                            expected
                        );
                    }
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
