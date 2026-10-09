//! Runtime hooks vtable — pluggable object allocators from `molt-lang-runtime`.
//!
//! `molt-lang-cpython-abi` cannot depend on `molt-lang-runtime` (that would
//! create a circular dependency). Instead, the runtime registers concrete
//! implementations at startup via [`try_set_runtime_hooks`].
//!
//! Every hook function uses `extern "C"` with primitive types so the
//! registration call works across crate boundaries without monomorphisation.
//!
//! ## Handle encoding
//!
//! Value handles carry raw `MoltObject` bit patterns (QNAN-boxed), including
//! `0` for float +0.0. Typed handle results distinguish success, absence, and
//! failure by status, never by successful payload bits. Individual pointer-only
//! allocation hooks and optional container arguments document their own null
//! sentinels; those contracts do not apply to arbitrary value results.

use std::sync::OnceLock;

/// Callbacks copied only from a validated public PyModuleDef. The generic
/// libmolt module registration API accepts an opaque definition identity and
/// therefore supplies the empty callback set instead of dereferencing it.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ModuleGcCallbacks {
    pub traverse: Option<
        unsafe extern "C" fn(
            *mut crate::abi_types::PyObject,
            *mut std::ffi::c_void,
            *mut std::ffi::c_void,
        ) -> std::os::raw::c_int,
    >,
    pub clear: Option<unsafe extern "C" fn(*mut crate::abi_types::PyObject) -> std::os::raw::c_int>,
    pub free: Option<unsafe extern "C" fn(*mut std::ffi::c_void) -> i32>,
}

/// Error policy selected by the public C API boundary, never inferred from
/// an attribute name. PySysGetObject reports lookup errors as unraisable
/// from Python 3.13; its ABI wrapper preserves/suppresses the indicator.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SysLookupPolicy {
    Propagate = 0,
    PySysGetObject = 1,
}

/// Dictionary lookup chooses its hash source explicitly. Supplied hashes are
/// signed Py_hash_t values widened to i64; no value is reserved as a sentinel.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DictHashSource {
    Compute = 0,
    Supplied = 1,
}

/// Normal lookup invokes user overrides; Generic selects descriptor/storage
/// lookup directly. Mutation has its own protocol below.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttributeAccess {
    Normal = 0,
    Generic = 1,
}

/// Mutation has a separate default type protocol: it bypasses metaclass
/// overrides while retaining namespace/cache publication. Generic is only the
/// physical descriptor/dictionary operation, including for class receivers.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttributeMutation {
    Normal = 0,
    Generic = 1,
    TypeDefault = 2,
}

/// Live descriptor slots of a managed value's Python type. Error is distinct
/// from absence, and data-descriptor precedence does not require a get slot.
#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DescriptorProtocol {
    Error = -1,
    None = 0,
    Get = 1,
    Set = 2,
    GetSet = 3,
}

impl DescriptorProtocol {
    pub const fn from_slots(get: bool, set: bool) -> Self {
        match (get, set) {
            (false, false) => Self::None,
            (true, false) => Self::Get,
            (false, true) => Self::Set,
            (true, true) => Self::GetSet,
        }
    }

    pub const fn has_get(self) -> bool {
        matches!(self, Self::Get | Self::GetSet)
    }

    pub const fn is_data(self) -> bool {
        matches!(self, Self::Set | Self::GetSet)
    }
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DescriptorMutationStatus {
    Error = -1,
    Missing = 0,
    Applied = 1,
}

/// Structural type fields, read without metaclass hooks or descriptor binding.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeMetadataField {
    Name = 0,
    QualName = 1,
    Base = 2,
    Bases = 3,
    Mro = 4,
    SolidOwner = 5,
    /// TYPE_SEMANTIC_FLAGS_MASK from exact-class declarations and latched abstract state.
    SemanticFlags = 6,
    /// Exact immutable creation-time documentation, never mutable __doc__.
    /// Missing means captured absence. An owned string result exposes stable
    /// NUL-terminated str_data bytes; the class's existing terminal-lifetime
    /// edge anchors those bytes after the temporary result owner is released.
    CreationDoc = 7,
    /// Exact native class's own sequence/mapping declaration capabilities.
    /// Inherited methods retain the resolved declaring type's physical slots.
    /// Missing means ordinary Python class policy, not an inherited native mask.
    NativeProtocolSlots = 8,
}

/// Native protocol presence is independent of Python method spelling. In
/// particular __len__ may expose sq_length, mp_length, or both. These typed
/// bits are construction facts shared with the runtime class declaration word.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeProtocolSlot {
    SequenceLength,
    SequenceConcat,
    SequenceRepeat,
    SequenceItem,
    SequenceAssignItem,
    SequenceContains,
    SequenceInPlaceConcat,
    SequenceInPlaceRepeat,
    MappingLength,
    MappingSubscript,
    MappingAssignSubscript,
}

impl NativeProtocolSlot {
    pub const SEQUENCE_MASK: u64 = (Self::SequenceInPlaceRepeat.bit() << 1) - 1;
    pub const ALL_MASK: u64 = (Self::MappingAssignSubscript.bit() << 1) - 1;

    pub const fn bit(self) -> u64 {
        1 << self as u8
    }
}

/// Complete runtime-owned CPython type-flag domain. Physical readiness,
/// protocol and GC flags remain owned by each admitted C view.
pub const TYPE_SEMANTIC_FLAGS_MASK: std::os::raw::c_ulong = crate::abi_types::Py_TPFLAGS_HEAPTYPE
    | crate::abi_types::Py_TPFLAGS_IMMUTABLETYPE
    | crate::abi_types::Py_TPFLAGS_BASETYPE
    | crate::abi_types::Py_TPFLAGS_IS_ABSTRACT;

/// Python's generic class-info protocols, distinct from physical subtype tests.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClassInfoOperation {
    Instance = 0,
    Subclass = 1,
}

/// Non-consuming raised-class metadata. An emergency MemoryError deliberately
/// has no heap exception or class-bootstrap requirement. Managed class handles
/// and native class pointers borrow from the live raised instance and remain
/// valid while that state is pending; a native query needs no wrapper allocation.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingExceptionClass {
    None,
    Class(u64),
    NativeClass(*mut crate::abi_types::PyTypeObject),
    EmergencyMemoryError,
}

/// Immutable fields borrowed from the canonical runtime method owner.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MethodPart {
    Function,
    Receiver,
}

pub const HANDLE_RESULT_ERROR: i32 = -1;
pub const HANDLE_RESULT_MISSING: i32 = 0;
pub const HANDLE_RESULT_OK: i32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OwnedHandleResult {
    status: i32,
    _reserved: u32,
    bits: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BorrowedHandleResult {
    status: i32,
    _reserved: u32,
    bits: u64,
}

pub const EXCEPTION_SNAPSHOT_DICT: u32 = 1 << 0;
pub const EXCEPTION_SNAPSHOT_ARGS: u32 = 1 << 1;
pub const EXCEPTION_SNAPSHOT_NOTES: u32 = 1 << 2;
pub const EXCEPTION_SNAPSHOT_TRACEBACK: u32 = 1 << 3;
pub const EXCEPTION_SNAPSHOT_CONTEXT: u32 = 1 << 4;
pub const EXCEPTION_SNAPSHOT_CAUSE: u32 = 1 << 5;
pub const EXCEPTION_TYPED_MAX_FIELDS: usize = molt_lang_obj_model::MAX_EXCEPTION_TYPED_FIELDS;
pub const EXCEPTION_BASE_FIELD_MASKS: [u32; 6] = [
    EXCEPTION_SNAPSHOT_DICT,
    EXCEPTION_SNAPSHOT_ARGS,
    EXCEPTION_SNAPSHOT_NOTES,
    EXCEPTION_SNAPSHOT_TRACEBACK,
    EXCEPTION_SNAPSHOT_CONTEXT,
    EXCEPTION_SNAPSHOT_CAUSE,
];

/// One atomic runtime/ABI transaction for the complete physical exception
/// layout. Base handles use `present_mask`; typed handles use field-order bits
/// in `typed_present_mask` and `typed_handles`. A capture owns one reference
/// to each present handle; a commit borrows every handle. Scalar typed fields
/// travel separately. Presence is determined only by the masks: a present
/// object handle may be zero (float +0.0). Absent payload slots must be zero.
/// `os_error_written == -1` is CPython's missing
/// `BlockingIOError.characters_written` sentinel.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ExceptionSnapshot {
    pub present_mask: u32,
    pub typed_present_mask: u32,
    pub layout_kind: u8,
    pub _reserved: [u8; 3],
    pub suppress_context: u32,
    pub dict: u64,
    pub args: u64,
    pub notes: u64,
    pub traceback: u64,
    pub context: u64,
    pub cause: u64,
    pub typed_handles: [u64; EXCEPTION_TYPED_MAX_FIELDS],
    pub unicode_start: isize,
    pub unicode_end: isize,
    pub os_error_written: isize,
}

impl Default for ExceptionSnapshot {
    fn default() -> Self {
        Self {
            present_mask: 0,
            typed_present_mask: 0,
            layout_kind: molt_lang_obj_model::ExceptionLayoutKind::Base as u8,
            _reserved: [0; 3],
            suppress_context: 0,
            dict: 0,
            args: 0,
            notes: 0,
            traceback: 0,
            context: 0,
            cause: 0,
            typed_handles: [0; EXCEPTION_TYPED_MAX_FIELDS],
            unicode_start: 0,
            unicode_end: 0,
            os_error_written: -1,
        }
    }
}

impl ExceptionSnapshot {
    fn base_payloads(&self) -> [u64; 6] {
        [
            self.dict,
            self.args,
            self.notes,
            self.traceback,
            self.context,
            self.cause,
        ]
    }

    pub fn base_fields(&self) -> [Option<u64>; 6] {
        let payloads = self.base_payloads();
        std::array::from_fn(|index| {
            (self.present_mask & EXCEPTION_BASE_FIELD_MASKS[index] != 0).then_some(payloads[index])
        })
    }

    pub fn typed_fields(&self) -> [Option<u64>; EXCEPTION_TYPED_MAX_FIELDS] {
        std::array::from_fn(|index| {
            (self.typed_present_mask & (1u32 << index) != 0).then_some(self.typed_handles[index])
        })
    }

    /// One occurrence per owned edge, including aliases and boxed zero values.
    pub fn present_handles(&self) -> impl Iterator<Item = u64> {
        self.base_fields()
            .into_iter()
            .chain(self.typed_fields())
            .flatten()
    }

    /// Shared structural admission for both directions of the ABI transaction.
    /// Runtime object types and the receiving exception's identity are checked
    /// by the runtime before mutation; no payload bits imply object presence.
    pub fn validated_layout(
        &self,
        expected: Option<molt_lang_obj_model::ExceptionLayoutKind>,
    ) -> Option<molt_lang_obj_model::ExceptionLayoutKind> {
        use molt_lang_obj_model::{ExceptionFieldStorage, ExceptionLayoutKind};

        let kind = ExceptionLayoutKind::from_u8(self.layout_kind)?;
        if expected.is_some_and(|expected| expected != kind)
            || self._reserved != [0; 3]
            || self.suppress_context > 1
        {
            return None;
        }
        let known_base = EXCEPTION_BASE_FIELD_MASKS.into_iter().fold(0, |a, b| a | b);
        if self.present_mask & !known_base != 0
            || self.present_mask & EXCEPTION_SNAPSHOT_ARGS == 0
            || self
                .base_payloads()
                .into_iter()
                .zip(EXCEPTION_BASE_FIELD_MASKS)
                .any(|(bits, mask)| self.present_mask & mask == 0 && bits != 0)
        {
            return None;
        }
        let policies = kind.field_policies();
        let known_typed = if policies.len() == u32::BITS as usize {
            u32::MAX
        } else {
            (1u32 << policies.len()) - 1
        };
        if self.typed_present_mask & !known_typed != 0
            || self.typed_handles[policies.len()..]
                .iter()
                .any(|bits| *bits != 0)
        {
            return None;
        }
        for (index, policy) in policies.iter().enumerate() {
            let present = self.typed_present_mask & (1u32 << index) != 0;
            let bits = self.typed_handles[index];
            match policy.storage {
                ExceptionFieldStorage::Object | ExceptionFieldStorage::RuntimeMessage => {
                    if !present && bits != 0 {
                        return None;
                    }
                }
                ExceptionFieldStorage::PySsize if present || bits != 0 => return None,
                ExceptionFieldStorage::PySsize => {}
            }
        }
        match kind {
            ExceptionLayoutKind::Unicode if self.os_error_written == -1 => {}
            ExceptionLayoutKind::OSError if self.unicode_start == 0 && self.unicode_end == 0 => {}
            ExceptionLayoutKind::Unicode | ExceptionLayoutKind::OSError => return None,
            _ if self.unicode_start == 0
                && self.unicode_end == 0
                && self.os_error_written == -1 => {}
            _ => return None,
        }
        Some(kind)
    }
}

pub enum DecodedHandleResult {
    Ok(u64),
    Missing,
    Error,
}

impl OwnedHandleResult {
    pub const fn ok(bits: u64) -> Self {
        Self {
            status: HANDLE_RESULT_OK,
            _reserved: 0,
            bits,
        }
    }
    pub const fn missing() -> Self {
        Self {
            status: HANDLE_RESULT_MISSING,
            _reserved: 0,
            bits: 0,
        }
    }
    pub const fn error() -> Self {
        Self {
            status: HANDLE_RESULT_ERROR,
            _reserved: 0,
            bits: 0,
        }
    }
    pub const fn decode(self) -> DecodedHandleResult {
        decode_handle_result(self.status, self.bits)
    }
}

impl BorrowedHandleResult {
    pub const fn ok(bits: u64) -> Self {
        Self {
            status: HANDLE_RESULT_OK,
            _reserved: 0,
            bits,
        }
    }
    pub const fn missing() -> Self {
        Self {
            status: HANDLE_RESULT_MISSING,
            _reserved: 0,
            bits: 0,
        }
    }
    pub const fn error() -> Self {
        Self {
            status: HANDLE_RESULT_ERROR,
            _reserved: 0,
            bits: 0,
        }
    }
    pub const fn decode(self) -> DecodedHandleResult {
        decode_handle_result(self.status, self.bits)
    }
}

const fn decode_handle_result(status: i32, bits: u64) -> DecodedHandleResult {
    match status {
        HANDLE_RESULT_OK => DecodedHandleResult::Ok(bits),
        HANDLE_RESULT_MISSING if bits == 0 => DecodedHandleResult::Missing,
        _ => DecodedHandleResult::Error,
    }
}

#[cfg(test)]
mod handle_result_tests {
    use super::*;

    #[test]
    fn test_owned_result_status_preserves_zero_and_other_value_bits() {
        for bits in [
            0,
            (-0.0f64).to_bits(),
            1.5f64.to_bits(),
            molt_lang_obj_model::MoltObject::none().bits(),
        ] {
            assert!(matches!(
                OwnedHandleResult::ok(bits).decode(),
                DecodedHandleResult::Ok(value) if value == bits
            ));
        }
        assert!(matches!(
            OwnedHandleResult::missing().decode(),
            DecodedHandleResult::Missing
        ));
        assert!(matches!(
            OwnedHandleResult::error().decode(),
            DecodedHandleResult::Error
        ));
    }

    #[test]
    fn test_borrowed_result_status_preserves_zero_and_other_value_bits() {
        for bits in [
            0,
            (-0.0f64).to_bits(),
            1.5f64.to_bits(),
            molt_lang_obj_model::MoltObject::none().bits(),
        ] {
            assert!(matches!(
                BorrowedHandleResult::ok(bits).decode(),
                DecodedHandleResult::Ok(value) if value == bits
            ));
        }
        assert!(matches!(
            BorrowedHandleResult::missing().decode(),
            DecodedHandleResult::Missing
        ));
        assert!(matches!(
            BorrowedHandleResult::error().decode(),
            DecodedHandleResult::Error
        ));
    }

    #[test]
    fn test_handle_result_invalid_status_or_missing_payload_fails_closed() {
        for (status, bits) in [
            (HANDLE_RESULT_MISSING, 1),
            (HANDLE_RESULT_ERROR, 0),
            (HANDLE_RESULT_ERROR, 1),
            (2, 0),
            (2, 1),
        ] {
            let owned = OwnedHandleResult {
                status,
                _reserved: 0,
                bits,
            };
            let borrowed = BorrowedHandleResult {
                status,
                _reserved: 0,
                bits,
            };
            assert!(matches!(owned.decode(), DecodedHandleResult::Error));
            assert!(matches!(borrowed.decode(), DecodedHandleResult::Error));
        }
    }
}

pub const INT_BYTES_OK: std::os::raw::c_int = 0;
pub const INT_BYTES_OVERFLOW: std::os::raw::c_int = 1;
pub const INT_BYTES_NEGATIVE_UNSIGNED: std::os::raw::c_int = 2;
pub const INT_BYTES_INVALID: std::os::raw::c_int = -1;

pub const MOLT_BUFFER_MAX_NDIM: usize = 64;
pub const MOLT_BUFFER_FORMAT_CAP: usize = 16;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MoltBufferView {
    pub data: *mut u8,
    pub len: u64,
    pub backing_capacity: u64,
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
        let mut format = [0; MOLT_BUFFER_FORMAT_CAP];
        format[0] = b'B';
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
            format,
        }
    }
}

/// Explicit controls and queries for a borrowed managed object identity.
/// Membership and finalization remain owned by the runtime collector/header.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedGcAction {
    Track = 0,
    Untrack = 1,
    IsTracked = 2,
    IsFinalized = 3,
}

/// Vtable of runtime-provided object-allocation and inspection hooks.
/// All function pointers are `extern "C"` for ABI stability across crate boundaries.
#[derive(Clone, Copy)]
#[allow(dead_code)]
#[repr(C)]
pub struct RuntimeHooks {
    pub abi_magic: u64,
    pub abi_version: u32,
    pub struct_size: u32,
    pub gil_ensure: unsafe extern "C" fn() -> std::os::raw::c_int,
    pub gil_leave: unsafe extern "C" fn(state: std::os::raw::c_int),
    pub gil_release: unsafe extern "C" fn(),
    pub gil_restore: unsafe extern "C" fn(),
    pub gil_check: unsafe extern "C" fn() -> std::os::raw::c_int,
    pub runtime_is_initialized: unsafe extern "C" fn() -> std::os::raw::c_int,
    /// Acquire runtime-lifetime and default-GIL custody for destruction of a
    /// retained CPython thread-state record. This lane must never attach or
    /// create a PyThreadState because it is invoked from that TLS destructor.
    /// Returns an opaque nonzero token consumed exactly once by
    /// `thread_state_drop_leave`.
    pub thread_state_drop_enter: unsafe extern "C" fn() -> u64,
    pub thread_state_drop_leave: unsafe extern "C" fn(token: u64),
    /// Project the runtime's target-specific execution custody into one typed
    /// ABI-stable capability discriminant. Pending calls require a non-detached
    /// projection in addition to process-main identity.
    pub attached_runtime_context: unsafe extern "C" fn() -> u32,
    /// Finalize a pending-call failure at a runtime boundary: preserve an
    /// existing runtime error, otherwise consume the exact C error, otherwise
    /// synthesize the typed failure represented by `reason`.
    pub pending_call_error: unsafe extern "C" fn(reason: u32),
    // ── Allocation ────────────────────────────────────────────────────────────
    /// Allocate a UTF-8 string object. Returns handle bits, 0 on failure.
    pub alloc_str: Option<unsafe extern "C" fn(data: *const u8, len: usize) -> u64>,
    /// Allocate a bytes object. Returns handle bits, 0 on failure.
    pub alloc_bytes: unsafe extern "C" fn(data: *const u8, len: usize) -> u64,
    /// Allocate the canonical managed bytearray backing. Null data requests
    /// zero-initialized storage of len bytes, including an out-of-length NUL.
    pub alloc_bytearray: unsafe extern "C" fn(data: *const u8, len: usize) -> u64,
    /// Allocate an exact heap identity for an immediate int/float. This is a
    /// fresh C-origin boundary, not conversion: no user protocol is invoked.
    /// Returns an owned heap handle or Error; existing heap identities bypass it.
    pub numeric_identity_new: Option<unsafe extern "C" fn(bits: u64) -> OwnedHandleResult>,
    /// Extract the immutable payload of an exact heap float without conversion,
    /// allocation or callbacks. Returns 0 on success, -1 on a wrong kind.
    pub float_payload: unsafe extern "C" fn(bits: u64, out: *mut f64) -> std::os::raw::c_int,
    /// Allocate an int object from a signed 64-bit value. Returns handle bits, 0 on failure.
    pub int_from_i64: unsafe extern "C" fn(value: i64) -> u64,
    /// Allocate an int object from an unsigned 64-bit value. Returns handle bits, 0 on failure.
    pub int_from_u64: unsafe extern "C" fn(value: u64) -> u64,
    /// Allocate an arbitrary-width int from a fixed-width byte string.
    pub int_from_bytes: unsafe extern "C" fn(
        data: *const u8,
        len: usize,
        little_endian: std::os::raw::c_int,
        signed: std::os::raw::c_int,
    ) -> u64,
    /// Encode an arbitrary-width int. Returns `INT_BYTES_*`; overflow still
    /// fills the output with the low bytes.
    pub int_to_bytes: unsafe extern "C" fn(
        bits: u64,
        data: *mut u8,
        len: usize,
        little_endian: std::os::raw::c_int,
        signed: std::os::raw::c_int,
    ) -> std::os::raw::c_int,
    /// Absolute bit length. Returns 0 on success, -1 on invalid input.
    pub int_num_bits: unsafe extern "C" fn(bits: u64, out: *mut usize) -> std::os::raw::c_int,
    /// Current `sys.get_int_max_str_digits()` authority; zero disables it.
    pub int_max_str_digits: unsafe extern "C" fn() -> usize,
    /// Allocate an empty list. Returns handle bits.
    pub alloc_list: unsafe extern "C" fn() -> u64,
    /// Allocate a list with its logical length established in one backing-store
    /// allocation. The ABI bridge owns the uninitialized-slot contract.
    pub alloc_list_presized: unsafe extern "C" fn(len: usize) -> u64,
    /// Append `item_bits` to the list at `list_bits`. `item_ptr` is the exact
    /// originating C object when the append crossed from CPython, or NULL for
    /// runtime-only appends. Preserving this origin makes `lst[-1] is item`
    /// exact and avoids rematerializing scalar carriers.
    pub list_append: unsafe extern "C" fn(
        list_bits: u64,
        item_bits: u64,
        item_ptr: *mut crate::abi_types::PyObject,
    ) -> std::os::raw::c_int,
    /// Return the number of items in a list.
    pub list_len: unsafe extern "C" fn(bits: u64) -> usize,
    /// Return a borrowed item, or `Missing` if out of range.
    pub list_item: unsafe extern "C" fn(bits: u64, i: usize) -> BorrowedHandleResult,
    /// Store `val_bits` at index `i`, returning the previous owned occupant in
    /// `Ok(bits)`, or `Error` for an invalid receiver, index, or failed store.
    /// Backs the indexed `PyList_SetItem`/`SET_ITEM`
    /// store: CPython stores directly (`Py_SETREF`), stealing the new reference
    /// and releasing the old — the ABI releases the old result and honors the steal.
    pub list_set:
        unsafe extern "C" fn(list_bits: u64, i: usize, val_bits: u64) -> OwnedHandleResult,
    /// Insert `item_bits` before (clamped) index `where_` in the list, shifting
    /// subsequent elements right. `item_ptr` preserves the exact originating C
    /// object just like append. Returns 0 on success, -1 on any failed runtime
    /// or physical publication.
    pub list_insert: unsafe extern "C" fn(
        list_bits: u64,
        where_: isize,
        item_bits: u64,
        item_ptr: *mut crate::abi_types::PyObject,
    ) -> std::os::raw::c_int,
    /// Sort the list in place via the runtime comparison authority. Returns 0 on
    /// success, -1 with a pending exception on error (uncomparable elements).
    pub list_sort: unsafe extern "C" fn(list_bits: u64) -> std::os::raw::c_int,
    /// Reverse the list in place. Returns 0 on success, -1 on a non-list.
    pub list_reverse: unsafe extern "C" fn(list_bits: u64) -> std::os::raw::c_int,
    /// Replace `list[ilow:ihigh]` with one already-materialized borrowed slice.
    /// The caller retains its element owners through publication. No user
    /// iteration runs here; the exact C projection describes the same values.
    /// Returns 0 on success, -1 with a pending error on failure.
    pub list_set_slice: unsafe extern "C" fn(
        list_bits: u64,
        ilow: isize,
        ihigh: isize,
        replacement: *const u64,
        replacement_len: usize,
        future_pointers: *const *mut crate::abi_types::PyObject,
        future_len: usize,
    ) -> std::os::raw::c_int,
    /// Allocate a tuple of exactly `n` uninitialized slots containing the
    /// canonical runtime Missing singleton, never a valid float-zero value.
    pub alloc_tuple: Option<unsafe extern "C" fn(n: usize) -> u64>,
    /// Set fixed slot `i` after the physical ABI owner admits its publication.
    /// `exact_pointer` is the stolen physical reference; NULL explicitly stores
    /// an uninitialized slot, and only then `val_bits` is ignored. Every real
    /// value (including float-zero bits) carries a non-NULL physical pointer.
    /// The hook never grows the tuple. `Missing` is the successful transition
    /// from an uninitialized slot; `Ok(bits)` replaces an initialized
    /// slot and transfers its old runtime edge; `Error` is failure.
    pub tuple_set: Option<
        unsafe extern "C" fn(
            bits: u64,
            i: usize,
            val_bits: u64,
            exact_pointer: *mut crate::abi_types::PyObject,
        ) -> OwnedHandleResult,
    >,
    /// Return the number of items in a tuple.
    pub tuple_len: Option<unsafe extern "C" fn(bits: u64) -> usize>,
    /// Return a borrowed item, or `Missing` for an uninitialized/out-of-range slot.
    pub tuple_item: Option<unsafe extern "C" fn(bits: u64, i: usize) -> BorrowedHandleResult>,
    /// Allocate an empty dict. Returns handle bits.
    pub alloc_dict: unsafe extern "C" fn() -> u64,
    /// Construct the canonical runtime mappingproxy; mapping is borrowed.
    pub mappingproxy_new: unsafe extern "C" fn(u64) -> OwnedHandleResult,
    /// Resolve borrowed exact backing storage for dicts and managed dict subtypes.
    /// Missing means not a dict (or, with merge_source=1, overridden __iter__).
    /// Error preserves the original lazy-backing allocation/storage exception.
    pub dict_resolve: unsafe extern "C" fn(bits: u64, merge_source: u8) -> BorrowedHandleResult,
    /// Set/delete through the canonical deferred dictionary transaction. The
    /// optional callback publishes derived native facts after storage commit
    /// and before displaced key/value ownership can run Python finalizers.
    /// Return 0 on success, 1 for absent deletion, or -1 with an exception.
    pub dict_mutate: unsafe extern "C" fn(
        dict_bits: u64,
        key_bits: u64,
        val_bits: u64,
        delete: u8,
        publish: Option<unsafe extern "C" fn(*mut std::ffi::c_void) -> i32>,
        context: *mut std::ffi::c_void,
    ) -> std::os::raw::c_int,
    /// Look up a key in admitted backing storage. Compute invokes its hash
    /// protocol once; Supplied bypasses hashing and hashability checks. Equality
    /// failures retain their original error; an absent key returns Missing.
    pub dict_get: unsafe extern "C" fn(
        dict_bits: u64,
        key_bits: u64,
        hash_source: DictHashSource,
        hash: i64,
    ) -> BorrowedHandleResult,
    /// Remove one key with one lookup; transfer its owned value or Missing.
    pub dict_pop: unsafe extern "C" fn(dict_bits: u64, key_bits: u64) -> OwnedHandleResult,
    /// Return the number of entries in a dict.
    pub dict_len: unsafe extern "C" fn(bits: u64) -> usize,
    /// Advance an insertion-ordered physical cursor past vacant dictionary rows.
    /// On success, return 1 and publish the next position and borrowed key/value
    /// bits to non-null outputs. At exhaustion or for a non-dict, return 0 and
    /// leave every output unchanged. This hook allocates nothing and sets no
    /// exception. A complete walk costs O(physical extent), including holes.
    pub dict_next: unsafe extern "C" fn(
        dict_bits: u64,
        position: *mut usize,
        out_key: *mut u64,
        out_val: *mut u64,
    ) -> std::os::raw::c_int,
    // ── Data access ───────────────────────────────────────────────────────────
    /// Return a pointer to the UTF-8 bytes of a string handle, writing the
    /// length into `*out_len`. Pointer is valid until next GC cycle.
    /// Returns null on error.
    pub str_data: unsafe extern "C" fn(bits: u64, out_len: *mut usize) -> *const u8,
    /// Create exact Unicode construction storage with capacity for all Python
    /// codepoints. The bridge owns the open-construction state until commit.
    pub unicode_new: unsafe extern "C" fn(len: usize, maxchar: u32) -> OwnedHandleResult,
    /// Commit admitted Python text into reserved construction storage once.
    pub unicode_commit:
        unsafe extern "C" fn(bits: u64, data: *const u8, len: usize) -> std::os::raw::c_int,
    /// Canonical runtime codec path, including the requested error policy.
    pub unicode_encode:
        unsafe extern "C" fn(bits: u64, encoding: u64, errors: u64) -> OwnedHandleResult,
    /// Return a pointer to the raw bytes of a bytes handle.
    pub bytes_data: unsafe extern "C" fn(bits: u64, out_len: *mut usize) -> *const u8,
    /// Live mutable bytearray backing, including an initialized trailing NUL.
    /// No copy or lease is created; a resize may invalidate the returned pointer.
    pub bytearray_data: unsafe extern "C" fn(bits: u64, out_len: *mut usize) -> *mut u8,
    /// Resize canonical bytearray storage, rejecting length changes while exported.
    pub bytearray_resize: unsafe extern "C" fn(bits: u64, len: usize) -> std::os::raw::c_int,
    /// Callback-free buffer-slot eligibility. Returns 1 or 0 without acquiring
    /// an export or changing the pending exception.
    pub buffer_supports: unsafe extern "C" fn(bits: u64) -> std::os::raw::c_int,
    /// Acquire a typed strided buffer export owned by the runtime.
    pub buffer_acquire:
        unsafe extern "C" fn(bits: u64, out_view: *mut MoltBufferView) -> std::os::raw::c_int,
    /// Release a typed strided buffer export previously acquired from the runtime.
    pub buffer_release: unsafe extern "C" fn(view: *mut MoltBufferView) -> std::os::raw::c_int,
    /// Return owned obj.name using the selected runtime attribute protocol.
    /// Generic reads may replace only the instance-dictionary tier with the
    /// borrowed handle at `dictionary`; NULL uses the receiver's own storage.
    /// With suppression, AttributeError becomes Missing; other errors survive.
    pub object_get_attr: unsafe extern "C" fn(
        obj_bits: u64,
        name_bits: u64,
        access: AttributeAccess,
        dictionary: *const u64,
        suppress: bool,
    ) -> OwnedHandleResult,
    /// Set or delete obj.name. The deletion flag is separate because a zero
    /// value payload is the valid float +0.0. Returns 0 on success, -1 on error.
    pub object_set_attr: unsafe extern "C" fn(
        obj_bits: u64,
        name_bits: u64,
        value_bits: u64,
        delete: bool,
        access: AttributeMutation,
    ) -> std::os::raw::c_int,
    /// C PyMethod_New policy: preserve func exactly, permit Python None receiver.
    /// Both arguments borrow; success owns one canonical runtime method.
    pub method_new: Option<unsafe extern "C" fn(u64, u64) -> OwnedHandleResult>,
    /// Callback-free immutable field read; borrows while the method is alive.
    pub method_part: unsafe extern "C" fn(u64, MethodPart) -> BorrowedHandleResult,
    /// Probe the managed descriptor's live type without binding or executing
    /// Python hooks. The C carrier's physical slots are not this authority.
    pub descriptor_protocol: unsafe extern "C" fn(bits: u64) -> DescriptorProtocol,
    /// Bind through the runtime's descriptor authority. NULL operands mean
    /// absent receiver/owner; every non-NULL payload, including zero, is a value.
    pub descriptor_get: unsafe extern "C" fn(
        descriptor: u64,
        receiver: *const u64,
        owner: *const u64,
    ) -> OwnedHandleResult,
    /// NULL value requests deletion. Missing means no mutation protocol;
    /// failure retains the descriptor body's exception instead of falling back.
    pub descriptor_set: unsafe extern "C" fn(
        descriptor: u64,
        receiver: u64,
        value: *const u64,
    ) -> DescriptorMutationStatus,
    /// Return owned format(obj, spec), or `Error` with a pending exception.
    pub object_format: unsafe extern "C" fn(obj_bits: u64, spec_bits: u64) -> OwnedHandleResult,
    /// Return owned str(obj) / repr(obj) through the runtime protocol, including
    /// subclass overrides and exceptions. The ABI does not format value bytes.
    pub object_str: unsafe extern "C" fn(obj_bits: u64) -> OwnedHandleResult,
    pub object_repr: unsafe extern "C" fn(obj_bits: u64) -> OwnedHandleResult,
    /// Python truth protocol, including subtype __bool__/__len__ overrides.
    /// Returns 0 or 1, or -1 with a pending exception.
    pub object_is_true: unsafe extern "C" fn(obj_bits: u64) -> std::os::raw::c_int,
    /// Python length protocol, including __index__ conversion and the signed
    /// target-pointer-width bound. Returns a nonnegative length, or -1 with a
    /// pending exception. Physical container hooks do not implement this API.
    pub object_length: unsafe extern "C" fn(obj_bits: u64) -> isize,
    /// Python subscription uses the live runtime class protocol, independently
    /// of the C carrier's physical slots and the receiver's storage tag.
    pub object_get_item: unsafe extern "C" fn(obj_bits: u64, key_bits: u64) -> OwnedHandleResult,
    /// Probe the actual subscription slot without binding a descriptor or
    /// consulting instance attributes. Matches Python's mapping admission.
    pub object_supports_subscript: unsafe extern "C" fn(obj_bits: u64) -> std::os::raw::c_int,
    /// NULL value requests deletion; a non-NULL payload, including zero, is a
    /// Python value. Returns 0 on success or -1 with the original exception.
    pub object_set_item: unsafe extern "C" fn(
        obj_bits: u64,
        key_bits: u64,
        value: *const u64,
    ) -> std::os::raw::c_int,
    /// Python iteration uses live runtime class lookup, never physical C slots.
    pub object_get_iter: unsafe extern "C" fn(obj_bits: u64) -> OwnedHandleResult,
    /// Probe __next__ without binding its descriptor or calling user code.
    pub iter_check: unsafe extern "C" fn(obj_bits: u64) -> std::os::raw::c_int,
    /// Advance the runtime iterator and transfer one owned item or exhaustion
    /// payload. On success `*exhausted` is 0 for an item, 1 for completion.
    /// Error keeps the pending exception. Zero bits is a valid float payload.
    pub iter_next: unsafe extern "C" fn(
        iter_bits: u64,
        exhausted: *mut std::os::raw::c_int,
    ) -> OwnedHandleResult,
    pub sys_get_object_borrowed: unsafe extern "C" fn(
        name_data: *const u8,
        name_len: usize,
        policy: SysLookupPolicy,
    ) -> BorrowedHandleResult,
    /// Resolve the current frame's effective builtins dict, or the interpreter
    /// default when no frame-specific override exists. Returns borrowed.
    pub eval_get_builtins_borrowed: unsafe extern "C" fn() -> BorrowedHandleResult,
    // ── Type classification ───────────────────────────────────────────────────
    /// Classify a heap-pointer handle into a `MoltTypeTag` discriminant (u8).
    /// Used by `classify_handle` to fill in the SIMD type-tag table for heap types.
    pub classify_heap: Option<unsafe extern "C" fn(bits: u64) -> u8>,
    /// Compute the CPython hash for a managed heap object. Returns `-1` only
    /// with a pending exception; every real `-1` hash is normalized to `-2`.
    pub object_hash: unsafe extern "C" fn(bits: u64) -> i64,
    /// Materialize and borrow the canonical dictionary of one runtime type.
    /// The type retains the dictionary; the bridge projects this same object.
    pub type_dict_borrowed: unsafe extern "C" fn(type_bits: u64) -> BorrowedHandleResult,
    /// Return an owned current structural type field. Base may be missing;
    /// names preserve their Python string bytes, including lone surrogates.
    pub type_metadata:
        unsafe extern "C" fn(type_bits: u64, field: TypeMetadataField) -> OwnedHandleResult,
    /// Raw lookup in one declaring namespace, without descriptor binding or
    /// inheritance. Only the requested native member is materialized. C MRO
    /// traversal retains its actual mixed foreign/runtime C3 order.
    pub type_lookup_borrowed: unsafe extern "C" fn(
        type_bits: u64,
        name_bits: u64,
        search_mro: u8,
    ) -> BorrowedHandleResult,
    /// Callback-free native declaration identity. A matching wrapper descriptor
    /// or constructor returns its borrowed declaring class; arbitrary Python
    /// callables, including same-name functions, return Missing. Name selects a
    /// canonical slot declaration, never an executable target registry.
    pub builtin_slot_owner: unsafe extern "C" fn(
        descriptor: u64,
        name: *const u8,
        name_len: usize,
        constructor: bool,
    ) -> BorrowedHandleResult,
    // ── Reference counting ────────────────────────────────────────────────────
    /// Increment the Molt reference count for a heap object.
    pub inc_ref: unsafe extern "C" fn(bits: u64),
    /// Decrement the Molt reference count; deallocate if it reaches zero.
    pub dec_ref: unsafe extern "C" fn(bits: u64),
    /// Return the current runtime strong-reference count for a heap object.
    /// Immortal objects return `molt_codegen_abi::IMMORTAL_REFCOUNT` widened
    /// to usize; this is independent of the target CPython refcount encoding.
    pub ref_count: Option<unsafe extern "C" fn(bits: u64) -> usize>,
    /// Mark or clear the runtime header's canonical ABI-view membership bit.
    /// Runtime refcount and GC hot paths use this as the lock-free negative
    /// test before consulting bridge state.
    /// Publish or retire the canonical ABI-view fact. Publication fails when
    /// terminal deallocation has begun; retirement always succeeds.
    pub try_mark_abi_view:
        unsafe extern "C" fn(bits: u64, present: std::os::raw::c_int) -> std::os::raw::c_int,
    // ── Module / C-extension support ─────────────────────────────────────────
    /// Allocate a new Molt module object whose `__name__` is the UTF-8 string
    /// in `name_data[..name_len]`.  Returns module handle bits, 0 on failure.
    pub alloc_module: unsafe extern "C" fn(name_data: *const u8, name_len: usize) -> u64,
    /// Single-phase PyModule_Create allocation, resolving the currently scoped
    /// initializer's package context exactly once for a matching leaf name.
    pub alloc_extension_module: unsafe extern "C" fn(name_data: *const u8, name_len: usize) -> u64,
    /// Return the runtime-owned module dict handle as a borrowed result.
    pub module_get_dict_borrowed: unsafe extern "C" fn(module_bits: u64) -> BorrowedHandleResult,
    /// Atomically get or create `sys.modules[name]`, replacing an existing
    /// non-module value with a fresh empty module. Returns borrowed.
    pub import_add_module_borrowed:
        unsafe extern "C" fn(name_data: *const u8, name_len: usize) -> BorrowedHandleResult,
    /// Set `module_bits.__dict__[name_data[..name_len]] = value_bits`.
    /// `module_bits` must be a Molt module handle.  Returns 0 on success, -1 on failure.
    pub module_set_attr: unsafe extern "C" fn(
        module_bits: u64,
        name_data: *const u8,
        name_len: usize,
        value_bits: u64,
    ) -> std::os::raw::c_int,
    /// Register C-API module metadata. Multi-phase construction defers state
    /// allocation until execution; single-phase construction allocates now.
    pub module_capi_register: unsafe extern "C" fn(
        module_bits: u64,
        module_def_ptr: usize,
        module_state_size: u64,
        defer_state: bool,
        callbacks: ModuleGcCallbacks,
    ) -> std::os::raw::c_int,
    /// Return the runtime-owned C-API module state pointer for a module.
    pub module_capi_get_state: unsafe extern "C" fn(module_bits: u64) -> *mut u8,
    pub module_capi_get_def: unsafe extern "C" fn(module_bits: u64) -> usize,
    /// Add `def -> module` to the process module-state registry.
    pub module_state_add:
        unsafe extern "C" fn(module_bits: u64, module_def_ptr: usize) -> std::os::raw::c_int,
    /// Find the borrowed module handle registered for a module definition
    /// pointer. The registry retains its own strong reference.
    pub module_state_find: unsafe extern "C" fn(module_def_ptr: usize) -> BorrowedHandleResult,
    /// Remove a module definition pointer from the module-state registry.
    pub module_state_remove: unsafe extern "C" fn(module_def_ptr: usize) -> std::os::raw::c_int,
    /// Allocate state and mark execution before calling arbitrary slots.
    /// 0 starts execution, 1 means already started, -1 is an error.
    pub module_exec_begin:
        unsafe extern "C" fn(module_bits: u64, module_def_ptr: usize) -> std::os::raw::c_int,
    /// Register a `PyCFunction`-style C function pointer (`meth_addr`) as a
    /// callable Molt function.  `flags` follows CPython's `METH_*` bitmask.
    /// `self_bits` and `defining_class_bits` are borrowed; the callable retains
    /// both as traced edges. The defining class is required for `METH_METHOD`
    /// and must be canonical None for every other convention.
    /// `self_is_null` distinguishes an absent C receiver from Python None or
    /// any other legitimate boxed value (including floating-point zero).
    /// `name_data[..name_len]` is the function's `__name__`.  Returns the bits
    /// of the resulting owned Molt callable. Zero without an exception means
    /// the convention is unsupported; zero with an exception is a construction
    /// failure, and consumers must preserve it instead of taking a fallback.
    pub register_c_function: Option<
        unsafe extern "C" fn(
            meth_addr: u64,
            flags: std::os::raw::c_int,
            self_bits: u64,
            self_is_null: bool,
            defining_class_bits: u64,
            name_data: *const u8,
            name_len: usize,
        ) -> u64,
    >,
    /// Import the module named by the UTF-8 dotted path in
    /// `name_data[..name_len]` through the runtime import pipeline (package
    /// custody, static extension registry, sys.modules cache).  Returns an
    /// owned module handle, or 0 on failure with the import error left in
    /// the runtime pending-exception state.
    pub import_module: unsafe extern "C" fn(name_data: *const u8, name_len: usize) -> u64,
    /// Invoke PyInit under a scoped package context, then consume its result
    /// through the canonical create/publish/exec transaction. All handles are
    /// borrowed; the result is one owned module or an error.
    pub initialize_extension: unsafe extern "C" fn(
        init: unsafe extern "C" fn() -> *mut crate::abi_types::PyObject,
        name_bits: u64,
        origin_bits: u64,
        spec_bits: u64,
        create_only: bool,
    ) -> OwnedHandleResult,
    /// Return non-zero when the runtime holds a pending Python exception.
    /// Lets ABI-side fallbacks avoid masking a real runtime error with a
    /// synthetic "without setting an exception" message.
    pub exception_pending: unsafe extern "C" fn() -> std::os::raw::c_int,
    /// Borrow the current raised class without consuming its owner or
    /// materializing an exception instance or lazy traceback.
    pub pending_exception_class: unsafe extern "C" fn() -> PendingExceptionClass,
    /// Generic rich comparison for two observed managed values. `op` follows
    /// obj-model's RichCompareOp ordinals; the result remains an owned arbitrary
    /// Python value. Runtime dispatch owns reflected subtype priority and the
    /// final identity/unsupported fallback. Declaring slots use shared kernels.
    pub object_richcompare:
        unsafe extern "C" fn(op: i32, left_bits: u64, right_bits: u64) -> OwnedHandleResult,
    /// Invoke one builtin declaring comparison slot, identified by its canonical
    /// runtime class identity. This bypasses outer operand overrides and returns
    /// owned NotImplemented for an unsupported peer; it never redispatches the
    /// complete binary operation back into the same declaring C slot.
    pub object_richcompare_builtin: unsafe extern "C" fn(
        declaring_class_bits: u64,
        op: i32,
        left_bits: u64,
        right_bits: u64,
    ) -> OwnedHandleResult,
    // ── Numeric protocol (PyNumber_*) ─────────────────────────────────────────
    //
    // The runtime owns the single numeric authority: arbitrary-precision int
    // promotion, float coercion, operator-overload dispatch, and CPython-shaped
    // exception raising all live in `molt-lang-runtime`. The ABI MUST NOT
    // reimplement arithmetic (that silently wraps at 64 bits and masks the
    // exceptions CPython raises). These hooks route `PyNumber_*` straight to
    // that authority. Each returns an owned value on success or an explicit
    // error status with a pending exception. A successful zero is float +0.0.
    /// Binary numeric op. `op` is a [`NumberBinaryOp`] discriminant. Returns
    /// an owned result, or `Error` with a pending exception.
    pub number_binary_op:
        unsafe extern "C" fn(op: u32, mode: u32, a_bits: u64, b_bits: u64) -> OwnedHandleResult,
    /// Unary numeric op. `op` is a [`NumberUnaryOp`] discriminant. Returns
    /// an owned result, or `Error` with a pending exception.
    pub number_unary_op: unsafe extern "C" fn(op: u32, a_bits: u64) -> OwnedHandleResult,
    /// Ternary power `pow(base, exp, modulus)`. Only canonical None selects
    /// two-argument `base ** exp`; zero bits are the present float +0.0.
    /// Returns an owned result, or `Error` with a pending exception.
    pub number_power: unsafe extern "C" fn(
        mode: u32,
        a_bits: u64,
        b_bits: u64,
        mod_bits: u64,
    ) -> OwnedHandleResult,
    /// Semantic target from the runtime version owner; -1 means unavailable/error.
    /// This is independent of the physical CPython ABI version.
    pub target_python_minor: unsafe extern "C" fn() -> i64,
    // ── Mapping protocol (PyDict_*) ───────────────────────────────────────────
    //
    // The runtime owns dict iteration (copy / keys / values / items). The ABI
    // MUST NOT return an empty dict/list ignoring its argument — that is silent
    // data loss. This hook routes to the runtime dict authority. `op` is a
    // [`DictOp`] discriminant. Returns result bits, or 0 with a pending exception
    // on error.
    pub dict_op: unsafe extern "C" fn(op: u32, dict_bits: u64) -> u64,
    pub set_op: unsafe extern "C" fn(op: u32, set_bits: u64) -> OwnedHandleResult,
    // ── Set protocol (PySet_*) ────────────────────────────────────────────────
    //
    // The runtime owns the single set authority (hash table, dedup, membership,
    // frozenset immutability, CPython-shaped exceptions) in
    // `molt-lang-runtime`. The ABI MUST NOT fake a set with a list (no dedup, no
    // hashed membership) or report every membership test as absent — both are
    // silent-wrong-answer. These hooks route `PySet_*` to that authority.
    /// Allocate a set/frozenset. Missing omits the iterable; Ok carries every
    /// Python value, including float-zero bits. Returns 0 with a pending error
    /// (e.g. a non-iterable argument → TypeError).
    pub set_new: unsafe extern "C" fn(iterable: BorrowedHandleResult, frozen: bool) -> u64,
    /// Return the number of elements in a set/frozenset, or -1 with a pending
    /// exception (SystemError) when `set_bits` is not a set/frozenset.
    pub set_size: unsafe extern "C" fn(set_bits: u64) -> std::os::raw::c_int,
    /// Membership test. Returns 1 (present) / 0 (absent) / -1 with a pending
    /// exception on error (TypeError for an unhashable key, SystemError for a
    /// non-set).
    pub set_contains: unsafe extern "C" fn(set_bits: u64, key_bits: u64) -> std::os::raw::c_int,
    /// Add `key_bits` to the set. Returns 0 on success, -1 with a pending
    /// exception on error (TypeError for an unhashable key, SystemError for a
    /// non-set).
    pub set_add: unsafe extern "C" fn(set_bits: u64, key_bits: u64) -> std::os::raw::c_int,
    /// Remove `key_bits` from the set if present. Returns 1 (found and removed)
    /// / 0 (absent) / -1 with a pending exception on error. Never raises
    /// KeyError (unlike `set.discard`).
    pub set_discard: unsafe extern "C" fn(set_bits: u64, key_bits: u64) -> std::os::raw::c_int,
    // ── Object introspection (PyObject_Dir) ───────────────────────────────────
    //
    // The runtime owns `dir(obj)` (MRO walk, `__dict__`, `__dir__`). The ABI MUST
    // NOT return an empty list ignoring its argument. Returns a list handle, or 0
    // with a pending exception on error.
    pub object_dir: unsafe extern "C" fn(obj_bits: u64) -> OwnedHandleResult,
    // ── Call protocol (PyObject_Call) ─────────────────────────────────────────
    //
    // The runtime owns the single call authority (`molt_call_bind`): compiled
    // functions, types, bound methods, kwargs binding, and CPython-shaped
    // exceptions all live there. Bridge proxies for Molt objects carry no
    // `tp_call`, so `PyObject_Call` on a bridge-managed callable (e.g. numpy's
    // `numpy.dtypes._add_dtype_helper`, a Molt-compiled function fetched via
    // `PyObject_GetAttrString`) MUST route through this hook instead of failing
    // "'<proxy-type>' object is not callable".
    /// Call a Molt callable. `args_bits` is a Molt tuple handle of positional
    /// arguments (0 = no positional args); `kwargs_bits` is a Molt dict handle
    /// (0 = no keyword args). Returns an owned result, or `Error` with the
    /// exception left in the runtime pending-exception state.
    pub object_call: unsafe extern "C" fn(
        callable_bits: u64,
        args_bits: u64,
        kwargs_bits: u64,
    ) -> OwnedHandleResult,
    /// Call with borrowed vector arguments without constructing a keyword dict.
    /// `values` contains positional values followed by keyword values; `names`
    /// contains exactly `keyword_count` handles. Each pointer may be null only
    /// for a zero-length span. Counts must fit a Rust slice, including the
    /// checked positional-plus-keyword sum. The caller pins every input through
    /// return; the runtime owns dispatch, validation and C-API release ordering.
    pub object_vectorcall: unsafe extern "C" fn(
        callable_bits: u64,
        values: *const u64,
        positional_count: usize,
        names: *const u64,
        keyword_count: usize,
    ) -> OwnedHandleResult,
    /// The same non-invoking predicate as Python callable(). A semantic class
    /// view does not carry the runtime's native tp_call layout.
    pub object_is_callable: unsafe extern "C" fn(obj_bits: u64) -> std::os::raw::c_int,
    /// Generic isinstance/issubclass authority, including class-info tuples and
    /// metaclass hooks. Both inputs are borrowed. Returns 0/1 or -1 with the
    /// callback/validation error left pending; it never casts class-info values
    /// into physical PyTypeObject storage.
    pub object_classinfo_match: Option<
        unsafe extern "C" fn(
            operation: ClassInfoOperation,
            value_bits: u64,
            classinfo_bits: u64,
        ) -> std::os::raw::c_int,
    >,
    // ── Foreign-object custody (C-extension objects into Molt) ────────────────
    //
    // When a genuine C-extension `PyObject*` (a numpy static type, an extension
    // instance, a descriptor, …) crosses *into* compiled Python, the bridge
    // wraps it in a first-class Molt heap object (`TYPE_ID_FOREIGN`) so that
    // Molt-side attribute access / calls resolve — the previous synthetic
    // `0xA11C…` identity token was not a valid `MoltObject` bit pattern, so
    // `DType.__name__` and friends failed to decode the handle. This hook
    // allocates the wrapper; the runtime owns the `TYPE_ID_FOREIGN` heap type,
    // its drop custody, and the getattr/setattr/call routing back through the
    // object's own CPython type slots (via `molt-cpython-abi` bridge functions).
    /// Allocate a `TYPE_ID_FOREIGN` wrapper around the C `PyObject*` at address
    /// `c_ptr`. Returns the wrapper handle bits, or 0 on failure. The strong
    /// reference custody (`Py_INCREF` on the C object) is handled by the bridge
    /// caller, not this hook.
    pub foreign_new: unsafe extern "C" fn(c_ptr: usize) -> u64,
    /// Construct one arbitrary-width integer from
    /// validated numeric digits in one owned allocation.
    pub int_from_digits: unsafe extern "C" fn(
        digits: *const u8,
        len: usize,
        base: u32,
        negative: std::os::raw::c_int,
    ) -> u64,
    pub int_from_f64_trunc: unsafe extern "C" fn(value: f64) -> u64,
    pub int_sign: unsafe extern "C" fn(bits: u64) -> std::os::raw::c_int,
    pub complex_parts:
        unsafe extern "C" fn(bits: u64, real: *mut f64, imag: *mut f64) -> std::os::raw::c_int,
    pub complex_from_doubles: unsafe extern "C" fn(real: f64, imag: f64) -> OwnedHandleResult,
    /// Report an already-captured C-API exception through the runtime's sole
    /// unraisable transaction. `message` is UTF-8 and borrowed for this call.
    pub report_unraisable: unsafe extern "C" fn(
        context_bits: u64,
        type_bits: u64,
        value_bits: u64,
        traceback_bits: u64,
        message: *const u8,
        message_len: usize,
        err_msg: *const u8,
        err_msg_len: usize,
        has_err_msg: std::os::raw::c_int,
    ),
    /// Replace one managed runtime exception field. Both handles are borrowed;
    /// `has_value == 0` means C NULL (clear cause/context/traceback). Returns
    /// zero on success and -1 when the exception or field value violates the
    /// field contract. Cause/context C wrappers separately honor their stolen
    /// input-reference contract.
    pub exception_set_field: unsafe extern "C" fn(
        exception_bits: u64,
        field: u32,
        value_bits: u64,
        has_value: std::os::raw::c_int,
    ) -> std::os::raw::c_int,
    /// Return one managed exception field as an owned handle. `Missing` means
    /// a cleared cause/context/traceback; args always returns its tuple.
    pub exception_get_field:
        unsafe extern "C" fn(exception_bits: u64, field: u32) -> OwnedHandleResult,
    /// Borrow the actual runtime class of any managed value. Builtin classes
    /// reuse their bound static type objects; user classes reuse their managed
    /// Type projections. This does not transfer a runtime reference.
    pub runtime_class_borrowed:
        Option<unsafe extern "C" fn(value_bits: u64) -> BorrowedHandleResult>,
    /// Allocation-free physical layout discriminator for an exception instance
    /// or exception class. The bridge calls this before publishing either
    /// skeleton, so it must not allocate or re-enter the bridge. Returns an
    /// `ExceptionLayoutKind` discriminant; non-exception/unknown values return
    /// `u8::MAX` and fail closed.
    pub exception_layout_kind: unsafe extern "C" fn(exception_or_class_bits: u64) -> u8,
    /// Capture and pin the complete runtime exception field state in one GIL
    /// transaction.  On success every present field owns one handle reference.
    pub exception_snapshot: unsafe extern "C" fn(
        exception_bits: u64,
        out: *mut ExceptionSnapshot,
    ) -> std::os::raw::c_int,
    /// Validate then publish the complete C sidecar state in one GIL
    /// transaction.  No runtime field changes if validation fails.
    pub exception_commit_snapshot: unsafe extern "C" fn(
        exception_bits: u64,
        snapshot: *const ExceptionSnapshot,
    ) -> std::os::raw::c_int,
    /// Runtime MRO authority for managed type handles, including multiple
    /// inheritance that cannot be represented by a single C `tp_base` edge.
    pub type_is_subtype:
        unsafe extern "C" fn(subclass_bits: u64, class_bits: u64) -> std::os::raw::c_int,
    /// Detach the exact runtime-pending exception into the C indicator domain.
    /// The result owns the exception instance. Each nonzero `actual_class_bits`
    /// and `traceback_bits` output independently owns one runtime reference,
    /// including on an error return; the consumer must release or transfer all
    /// three owners. Native wrappers do not own their projected managed class
    /// or traceback handles. Zero is the intentional no-traceback sentinel.
    pub take_pending_exception: unsafe extern "C" fn(
        actual_class_bits: *mut u64,
        traceback_bits: *mut u64,
    ) -> OwnedHandleResult,
    /// Clear and release the runtime pending-exception indicator without
    /// projecting it into the C domain.
    pub clear_pending_exception: unsafe extern "C" fn(),
    /// Invoke `callback(context)` exactly once, synchronously, with the runtime
    /// raised state detached on the stack, then restore that exact owner.
    /// Emergency errors need no heap instance; handled state stays live.
    /// The callback must not unwind across C and must drain cleanup errors
    /// before returning. Neither callback nor context may escape this call.
    pub with_preserved_pending_exception: unsafe extern "C" fn(
        callback: unsafe extern "C" fn(context: *mut std::ffi::c_void),
        context: *mut std::ffi::c_void,
    ),
    /// Return the runtime's active handled exception (`sys.exception()`) as an
    /// owned handle. `Missing` means no exception is being handled.
    pub handled_exception_get: unsafe extern "C" fn() -> OwnedHandleResult,
    /// Replace the runtime's active handled exception. Zero clears it; a
    /// non-zero handle is owned by and always consumed by this call.
    pub handled_exception_set:
        unsafe extern "C" fn(owned_exception_bits: u64) -> std::os::raw::c_int,
    /// Publish a newly allocated native GC-capable node into the runtime's
    /// single mixed-node identity/epoch authority before it becomes tracked.
    pub native_gc_allocate: Option<unsafe extern "C" fn(addr: usize) -> std::os::raw::c_int>,
    /// Control a managed view's actual runtime object, never its physical C
    /// projection. Mutations return 0 on success and -1 for invalid tracking;
    /// queries return 0 or 1. Track requires an untracked GC-capable object;
    /// immortal C roots remain untracked even after an explicit request.
    /// Untrack is idempotent. Neither operation changes allocation accounting,
    /// finalizer state, reference ownership, or either pending error channel.
    pub managed_gc_control:
        unsafe extern "C" fn(bits: u64, action: ManagedGcAction) -> std::os::raw::c_int,
    /// Borrowed projection of the runtime's complete owned-edge inventory.
    /// ManagedHandle edges may contain inline values; NativePointer edges retain
    /// physical C identity. No callback runs while an owner storage lock is held.
    pub managed_gc_traverse: unsafe extern "C" fn(
        bits: u64,
        visit: crate::api::memory::NativeGcVisitProc,
        context: *mut std::ffi::c_void,
    ) -> std::os::raw::c_int,
    /// Run module clear, reserve and detach the canonical mutable subset, then
    /// release its owners. Preserve a nonzero callback result; allocation failure
    /// returns -1 with an error before local payload mutation.
    pub managed_gc_clear: unsafe extern "C" fn(bits: u64) -> std::os::raw::c_int,
    /// Make an allocated native node a trial-deletion candidate. Managed bridge
    /// views are never sent through this lane.
    pub native_gc_track: unsafe extern "C" fn(addr: usize) -> std::os::raw::c_int,
    /// Remove a native node from candidate traversal while retaining its
    /// runtime-owned allocation identity until deallocation.
    pub native_gc_untrack: unsafe extern "C" fn(addr: usize),
    /// Retire the native node identity immediately before its allocator frees
    /// the address, preventing address-reuse/embedded-reinit ABA.
    pub native_gc_deallocate: unsafe extern "C" fn(addr: usize),
    pub native_gc_is_tracked: unsafe extern "C" fn(addr: usize) -> std::os::raw::c_int,
    pub native_gc_is_finalized: unsafe extern "C" fn(addr: usize) -> std::os::raw::c_int,
    /// Atomically claim a native node's one-shot finalizer. Returns 1 to the
    /// caller that must run tp_finalize, 0 when already finalized/no longer
    /// eligible, and -1 for an unknown native identity.
    pub native_gc_claim_finalizer: unsafe extern "C" fn(addr: usize) -> std::os::raw::c_int,
    /// Process both managed and native candidates in the runtime's one
    /// deterministic, epoch-pinned trial-deletion graph.
    pub gc_collect: unsafe extern "C" fn() -> isize,
    pub gc_enable: unsafe extern "C" fn() -> std::os::raw::c_int,
    pub gc_disable: unsafe extern "C" fn() -> std::os::raw::c_int,
    pub gc_is_enabled: unsafe extern "C" fn() -> std::os::raw::c_int,
    /// CPython `PyErr_CheckSignals`: run pending Python signal handlers when
    /// called on the registered main thread. Returns 0, or -1 with the
    /// handler's exception pending in the runtime.
    pub check_signals: unsafe extern "C" fn() -> std::os::raw::c_int,
    /// CPython `PyErr_SetInterruptEx`: simulate a delivery of `signum`.
    /// Async-signal-safe and callable without an attached thread state.
    /// Returns -1 for an out-of-range signal number, otherwise 0.
    pub set_interrupt: unsafe extern "C" fn(signum: std::os::raw::c_int) -> std::os::raw::c_int,
    /// CPython `PyOS_InterruptOccurred`: on the registered main thread, consume
    /// a recorded SIGINT delivery. Returns 1 when one was recorded.
    pub interrupt_occurred: unsafe extern "C" fn() -> std::os::raw::c_int,
    /// Wake the active process-main park after publishing a C pending call.
    /// Callable without the GIL or attached thread state; must not take a
    /// runtime mutex, allocate, execute a callback, or change errno.
    pub notify_pending_calls: unsafe extern "C" fn(),
    /// CPython sequence admission through the runtime's raw type protocol.
    /// Returns 1 for a sequence, 0 otherwise, and -1 with a pending error.
    pub sequence_check: unsafe extern "C" fn(bits: u64) -> std::os::raw::c_int,
    /// Indexed sequence read, distinct from mapping subscription.
    pub sequence_item: unsafe extern "C" fn(bits: u64, index: isize) -> OwnedHandleResult,
    /// CPython length hint, including the supplied default and error sentinel.
    pub object_length_hint: unsafe extern "C" fn(bits: u64, default: isize) -> isize,
    /// Shared target-version policy for tuple materialization's hint lookup.
    pub tuple_uses_length_hint: unsafe extern "C" fn() -> bool,
    /// CPython `BaseExceptionGroup_new` admission for a native allocation:
    /// `(message, exceptions)` argument shape, sequence admission,
    /// `PySequence_Tuple`, real item identity and the class decision for the
    /// [`ExceptionGroupRequest`] of the exact native type named `type_name`
    /// (`tp_name`). `args_bits` is a borrowed runtime tuple. Returns 0 to keep
    /// the requested type or 1 to narrow to `ExceptionGroup`, with owned
    /// message and exceptions-tuple handles written to the out-params; -1
    /// leaves the runtime exception pending and writes nothing.
    pub exception_group_admit: unsafe extern "C" fn(
        request: u32,
        type_name: *const std::os::raw::c_char,
        args_bits: u64,
        message_bits: *mut u64,
        exceptions_bits: *mut u64,
    ) -> std::os::raw::c_int,
    /// PyObject_Bytes protocol, sharing runtime byte construction without count semantics.
    pub object_bytes: unsafe extern "C" fn(obj_bits: u64, special: bool) -> OwnedHandleResult,
    pub memoryview_new: unsafe extern "C" fn(u64) -> OwnedHandleResult,
    pub memoryview_release: unsafe extern "C" fn(u64) -> OwnedHandleResult,
    pub memoryview_from_buffer: unsafe extern "C" fn(
        *const MoltBufferView,
        *const std::ffi::c_char,
        *const std::ffi::c_void,
        bool,
    ) -> OwnedHandleResult,
    /// 0 = live, 1 = released, -1 = invalid. No callback or owned edge transfer.
    pub memoryview_snapshot: unsafe extern "C" fn(
        u64,
        *mut MoltBufferView,
        *mut *mut crate::abi_types::PyObject,
        *mut *const u8,
        *mut usize,
    ) -> i32,
    /// Read-only membership in the existing source-buffer pointer registry.
    /// These pointers have private storage, never a CPython PyObject prefix.
    /// The callback releases all locks before returning and invokes no Python.
    pub private_c_heap_contains: unsafe extern "C" fn(pointer: usize) -> std::os::raw::c_int,
    /// Construct the exact runtime slice from three borrowed, opaque values.
    /// No __index__ call, coercion, or bounds normalization occurs here.
    pub slice_new: unsafe extern "C" fn(start: u64, stop: u64, step: u64) -> OwnedHandleResult,
    /// Borrow an exact slice field: 0=start, 1=stop, 2=step. Invalid input is
    /// Error with an exception; zero bits remains a valid float-zero field.
    pub slice_item: unsafe extern "C" fn(bits: u64, field: usize) -> BorrowedHandleResult,
    /// Python containment protocol on borrowed operands: 1/0, or -1 with an
    /// exception. The runtime owns special lookup and builtin membership.
    pub object_contains: unsafe extern "C" fn(container: u64, needle: u64) -> std::os::raw::c_int,
    /// Context objects and bindings have one runtime owner. Optional values use
    /// an explicit presence flag; float +0.0 is a valid successful value.
    /// Admit a known public Context static shell through its lazy class owner.
    /// Zero means canonically bound; -1 means failure with a pending error.
    pub context_type_admit: unsafe extern "C" fn(usize) -> i32,
    pub context_new: unsafe extern "C" fn() -> OwnedHandleResult,
    pub context_copy_current: unsafe extern "C" fn() -> OwnedHandleResult,
    pub context_copy: unsafe extern "C" fn(u64) -> OwnedHandleResult,
    pub context_enter: unsafe extern "C" fn(u64) -> i32,
    pub context_exit: unsafe extern "C" fn(u64) -> i32,
    pub context_var_new: unsafe extern "C" fn(u64, u64, i32) -> OwnedHandleResult,
    pub context_var_get: unsafe extern "C" fn(u64, u64, i32) -> OwnedHandleResult,
    pub context_var_set: unsafe extern "C" fn(u64, u64) -> OwnedHandleResult,
    pub context_var_reset: unsafe extern "C" fn(u64, u64) -> i32,
}

pub const RUNTIME_HOOKS_ABI_MAGIC: u64 = 0x4d4f_4c54_484f_4f4b;
// Version 60 removes obsolete integer-read hooks; physical PyLong digits own reads.
// Version 59 adds borrowed vector call ingress without dictionary transport.
// Version 58 replaces ordinal dictionary reads with a physical cursor pointer.
// Version 57 carries numeric operation mode and the runtime semantic target.
// Version 56 added the NativeProtocolSlots type-metadata domain. Callback
// domains are ABI even when the pointer-sized table layout is unchanged.
// Version 61 adds the sole Context owner and cold static-shell admission.
pub const RUNTIME_HOOKS_ABI_VERSION: u32 = 61;

#[inline]
fn runtime_hooks_layout_matches(abi_magic: u64, abi_version: u32, struct_size: u32) -> bool {
    abi_magic == RUNTIME_HOOKS_ABI_MAGIC
        && abi_version == RUNTIME_HOOKS_ABI_VERSION
        && struct_size as usize == std::mem::size_of::<RuntimeHooks>()
}

/// Target projection used to construct an attached runtime-context capability.
/// This is deliberately not a boolean: native GIL, free-threaded attachment,
/// and wasm single-thread custody have different future synchronization rules.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachedRuntimeContextKind {
    Detached = 0,
    NativeGil = 1,
    NativeFreeThreaded = 2,
    WasmSingleThread = 3,
}

impl AttachedRuntimeContextKind {
    #[inline]
    pub fn from_abi(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Detached),
            1 => Some(Self::NativeGil),
            2 => Some(Self::NativeFreeThreaded),
            3 => Some(Self::WasmSingleThread),
            _ => None,
        }
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingCallErrorKind {
    CallbackFailedWithoutException = 1,
    RuntimeContextDetached = 2,
}

impl PendingCallErrorKind {
    #[inline]
    pub fn from_abi(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::CallbackFailedWithoutException),
            2 => Some(Self::RuntimeContextDetached),
            _ => None,
        }
    }

    pub fn message(self) -> &'static std::ffi::CStr {
        match self {
            Self::CallbackFailedWithoutException => {
                c"pending-call callback failed without setting an exception"
            }
            Self::RuntimeContextDetached => {
                c"pending-call execution requires an attached main-thread runtime context"
            }
        }
    }
}

/// Discriminants for [`RuntimeHooks::dict_op`]. Kept in sync with the match in
/// the runtime hook implementation (`hook_dict_op`).
#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum DictOp {
    Copy = 0,
    Keys = 1,
    Values = 2,
    Items = 3,
    Clear = 4,
    MappingKeys = 5,
    MappingValues = 6,
    MappingItems = 7,
}

#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum SetOp {
    Pop = 1,
    Clear = 2,
}

#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ExceptionField {
    Cause = 0,
    Context = 1,
    Traceback = 2,
    Args = 3,
}

/// Requested constructor identity for [`RuntimeHooks::exception_group_admit`],
/// derived from the exact native type object: the two builtin group types,
/// or a subtype classified by its real `Exception` ancestry.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExceptionGroupRequest {
    BaseExceptionGroup = 0,
    ExceptionGroup = 1,
    ExceptionSubclass = 2,
    BaseExceptionSubclass = 3,
}

impl ExceptionGroupRequest {
    #[inline]
    pub fn from_abi(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::BaseExceptionGroup),
            1 => Some(Self::ExceptionGroup),
            2 => Some(Self::ExceptionSubclass),
            3 => Some(Self::BaseExceptionSubclass),
            _ => None,
        }
    }
}

/// Numeric protocol phase carried across the strict runtime hook boundary.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NumberOperationMode {
    Normal = 0,
    InPlace = 1,
}

impl NumberOperationMode {
    pub fn from_abi(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Normal),
            1 => Some(Self::InPlace),
            _ => None,
        }
    }
}

/// Discriminants for [`RuntimeHooks::number_binary_op`]. Kept in sync with the
/// match in the runtime hook implementation (`hook_number_binary_op`).
#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum NumberBinaryOp {
    Add = 0,
    Subtract = 1,
    Multiply = 2,
    TrueDivide = 3,
    FloorDivide = 4,
    Remainder = 5,
    Lshift = 6,
    Rshift = 7,
    And = 8,
    Or = 9,
    Xor = 10,
    MatrixMultiply = 11,
}

/// Discriminants for [`RuntimeHooks::number_unary_op`]. Kept in sync with the
/// match in the runtime hook implementation (`hook_number_unary_op`).
#[repr(u32)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum NumberUnaryOp {
    Negative = 0,
    Positive = 1,
    Absolute = 2,
    Invert = 3,
    /// PyNumber_Float / float(): exact identity, numeric slots, then text.
    Float = 4,
    /// PyFloat_AsDouble: float payload, then numeric slots, never text.
    FloatAsDouble = 5,
    /// Private _PyNumber_Index: validated owned integer; preserve subtypes.
    Index = 6,
    /// PyNumber_Long / int(): exact identity, numeric protocols, then text.
    Long = 7,
}

/// Global hook table, set once by `molt-lang-runtime` at init time.
static RUNTIME_HOOKS: OnceLock<RuntimeHooks> = OnceLock::new();

/// Register the exact runtime hook vtable without panicking on host input.
///
/// Returns `true` if this call installed the hooks, or `false` if a prior
/// registration was already in effect or the table is incompatible. The
/// passed-in table is dropped in either failure case.
///
/// # Safety
/// Every function pointer in `hooks` must remain valid for the lifetime of the
/// process.
pub unsafe fn try_set_runtime_hooks(hooks: RuntimeHooks) -> bool {
    if !runtime_hooks_layout_matches(hooks.abi_magic, hooks.abi_version, hooks.struct_size) {
        return false;
    }
    RUNTIME_HOOKS.set(hooks).is_ok()
}

/// C-callable registration entry point for `molt-lang-runtime`.
///
/// # Safety
/// A non-null pointer must contain the readable ABI prefix through `struct_size`.
/// If that prefix declares the current layout, it must contain a complete valid
/// `RuntimeHooks` whose function pointers remain valid for the process lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_register_hooks(hooks: *const RuntimeHooks) -> i32 {
    if hooks.is_null() {
        return -1;
    }
    // Read only the integer prefix until the producer's layout is admitted.
    // Copying the current table first would overread a shorter old allocation
    // and could materialize invalid function pointers before rejecting it.
    let (abi_magic, abi_version, struct_size) = unsafe {
        (
            std::ptr::read_unaligned(std::ptr::addr_of!((*hooks).abi_magic)),
            std::ptr::read_unaligned(std::ptr::addr_of!((*hooks).abi_version)),
            std::ptr::read_unaligned(std::ptr::addr_of!((*hooks).struct_size)),
        )
    };
    // Registration accepts exactly the current table. There is no shorter
    // append-only/legacy ABI lane: producer and consumer are rebuilt together.
    if !runtime_hooks_layout_matches(abi_magic, abi_version, struct_size) {
        return -1;
    }
    let hooks = unsafe { std::ptr::read_unaligned(hooks) };
    if !unsafe { try_set_runtime_hooks(hooks) } {
        return -1;
    }
    // The C registration entrypoint is itself a production bootstrap
    // boundary. Generic Rust fixtures use try_set_runtime_hooks directly and
    // therefore never acquire runtime-owned type dictionaries as a side
    // effect.
    if unsafe { crate::abi_types::ready_exception_singleton_types() } < 0 {
        -1
    } else {
        0
    }
}

#[cfg(test)]
#[path = "hooks_registration_tests.rs"]
mod registration_tests;

/// Access the runtime hooks. Returns `None` if hooks have not been registered
/// (pre-init or test contexts). Callers must degrade gracefully (return None/0).
#[inline]
pub fn hooks() -> Option<&'static RuntimeHooks> {
    RUNTIME_HOOKS.get()
}

/// Numeric semantic admission requires a real identity producer. Physical
/// standalone construction/extraction does not require this capability.
#[inline]
pub(crate) fn numeric_identity_available() -> bool {
    hooks().is_some_and(|runtime| runtime.numeric_identity_new.is_some())
}

/// Select callable ownership before construction. Producer failure never
/// licenses an unrelated physical fallback.
pub(crate) fn method_construction_available() -> bool {
    hooks().is_some_and(|runtime| runtime.method_new.is_some())
}

pub(crate) fn cfunction_registration_available() -> bool {
    hooks().is_some_and(|runtime| runtime.register_c_function.is_some())
}

/// Whether the registered runtime owns managed tuple construction. Before
/// runtime initialization (and in intentionally partial ABI fixtures), exact
/// C tuples use their native `PyTupleObject` allocation authority instead.
#[inline]
pub(crate) fn managed_tuple_construction_available() -> bool {
    hooks().is_some_and(|runtime| {
        runtime.alloc_tuple.is_some()
            && runtime.tuple_set.is_some()
            && runtime.tuple_len.is_some()
            && runtime.tuple_item.is_some()
            && runtime.ref_count.is_some()
            && runtime.classify_heap.is_some()
    })
}

/// Physical exception argument tuples and native exception instances require
/// the runtime's GC allocation authority. Registration alone is insufficient:
/// intentionally partial hook tables may omit that capability.
#[inline]
pub(crate) fn native_gc_allocation_available() -> bool {
    hooks().is_some_and(|runtime| runtime.native_gc_allocate.is_some())
}

/// A partial table cannot answer Python's generic class-info protocol through
/// the physical subtype predicate. Select the actual producer before calling.
#[inline]
pub(crate) fn classinfo_match_available() -> bool {
    hooks().is_some_and(|runtime| runtime.object_classinfo_match.is_some())
}

// Operation dispatch derives only from the optional callback field. None uses
// its canonical failure implementation before invocation; a Some callback's
// result is never reinterpreted as absence or retried through another owner.
impl RuntimeHooks {
    #[inline]
    #[expect(
        clippy::too_many_arguments,
        reason = "Forwards the fixed C-function producer ABI with its receiver"
    )]
    pub unsafe fn register_c_function(
        &self,
        meth_addr: u64,
        flags: std::os::raw::c_int,
        self_bits: u64,
        self_is_null: bool,
        defining_class_bits: u64,
        name_data: *const u8,
        name_len: usize,
    ) -> u64 {
        unsafe {
            (self.register_c_function.unwrap_or(stub_register_c_function))(
                meth_addr,
                flags,
                self_bits,
                self_is_null,
                defining_class_bits,
                name_data,
                name_len,
            )
        }
    }
    #[inline]
    pub unsafe fn alloc_tuple(&self, n: usize) -> u64 {
        unsafe { (self.alloc_tuple.unwrap_or(stub_alloc_tuple))(n) }
    }
    #[inline]
    pub unsafe fn tuple_set(
        &self,
        bits: u64,
        i: usize,
        val_bits: u64,
        exact_pointer: *mut crate::abi_types::PyObject,
    ) -> OwnedHandleResult {
        unsafe { (self.tuple_set.unwrap_or(stub_tuple_set))(bits, i, val_bits, exact_pointer) }
    }
    #[inline]
    pub unsafe fn tuple_len(&self, bits: u64) -> usize {
        unsafe { (self.tuple_len.unwrap_or(stub_tuple_len))(bits) }
    }
    #[inline]
    pub unsafe fn tuple_item(&self, bits: u64, i: usize) -> BorrowedHandleResult {
        unsafe { (self.tuple_item.unwrap_or(stub_tuple_item))(bits, i) }
    }
    #[inline]
    pub unsafe fn ref_count(&self, bits: u64) -> usize {
        unsafe { (self.ref_count.unwrap_or(stub_ref_count))(bits) }
    }
    #[inline]
    pub unsafe fn classify_heap(&self, bits: u64) -> u8 {
        unsafe { (self.classify_heap.unwrap_or(stub_classify_heap))(bits) }
    }
    #[inline]
    pub unsafe fn native_gc_allocate(&self, addr: usize) -> std::os::raw::c_int {
        unsafe { (self.native_gc_allocate.unwrap_or(stub_native_gc_allocate))(addr) }
    }
    #[inline]
    pub unsafe fn object_classinfo_match(
        &self,
        operation: ClassInfoOperation,
        value_bits: u64,
        classinfo_bits: u64,
    ) -> std::os::raw::c_int {
        unsafe {
            (self
                .object_classinfo_match
                .unwrap_or(stub_object_classinfo_match))(
                operation, value_bits, classinfo_bits
            )
        }
    }
    #[inline]
    pub unsafe fn alloc_str(&self, data: *const u8, len: usize) -> u64 {
        unsafe { (self.alloc_str.unwrap_or(stub_alloc_str))(data, len) }
    }
    #[inline]
    pub unsafe fn runtime_class_borrowed(&self, value_bits: u64) -> BorrowedHandleResult {
        unsafe {
            (self
                .runtime_class_borrowed
                .unwrap_or(stub_runtime_class_borrowed))(value_bits)
        }
    }
    #[inline]
    pub unsafe fn numeric_identity_new(&self, bits: u64) -> OwnedHandleResult {
        unsafe {
            (self
                .numeric_identity_new
                .unwrap_or(stub_numeric_identity_new))(bits)
        }
    }
    #[inline]
    pub unsafe fn method_new(&self, function: u64, receiver: u64) -> OwnedHandleResult {
        unsafe { (self.method_new.unwrap_or(stub_method_new))(function, receiver) }
    }
}

// ─── No-op stubs for pre-init or test use ────────────────────────────────────

unsafe extern "C" fn stub_alloc_str(_data: *const u8, _len: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_alloc_bytes(_data: *const u8, _len: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_numeric_identity_new(_bits: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_float_payload(_bits: u64, _out: *mut f64) -> std::os::raw::c_int {
    -1
}

unsafe extern "C" fn stub_int_from_i64(_value: i64) -> u64 {
    0
}
unsafe extern "C" fn stub_int_from_u64(_value: u64) -> u64 {
    0
}

unsafe extern "C" fn stub_int_from_bytes(
    _data: *const u8,
    _len: usize,
    _little_endian: std::os::raw::c_int,
    _signed: std::os::raw::c_int,
) -> u64 {
    0
}

unsafe extern "C" fn stub_int_from_digits(
    _digits: *const u8,
    _len: usize,
    _base: u32,
    _negative: std::os::raw::c_int,
) -> u64 {
    0
}

unsafe extern "C" fn stub_int_from_f64_trunc(_value: f64) -> u64 {
    0
}

unsafe extern "C" fn stub_int_sign(_bits: u64) -> std::os::raw::c_int {
    0
}

unsafe extern "C" fn stub_int_to_bytes(
    _bits: u64,
    _data: *mut u8,
    _len: usize,
    _little_endian: std::os::raw::c_int,
    _signed: std::os::raw::c_int,
) -> std::os::raw::c_int {
    INT_BYTES_INVALID
}

unsafe extern "C" fn stub_int_num_bits(_bits: u64, _out: *mut usize) -> std::os::raw::c_int {
    -1
}

unsafe extern "C" fn stub_int_max_str_digits() -> usize {
    4300
}

unsafe extern "C" fn stub_complex_parts(_bits: u64, _real: *mut f64, _imag: *mut f64) -> i32 {
    -1
}
unsafe extern "C" fn stub_complex_from_doubles(_real: f64, _imag: f64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_alloc_list() -> u64 {
    0
}
unsafe extern "C" fn stub_alloc_list_presized(_len: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_list_append(
    _list_bits: u64,
    _item_bits: u64,
    _item_ptr: *mut crate::abi_types::PyObject,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_list_len(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn stub_list_item(_bits: u64, _i: usize) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn stub_list_set(
    _list_bits: u64,
    _i: usize,
    _val_bits: u64,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_list_insert(
    _list_bits: u64,
    _where_: isize,
    _item_bits: u64,
    _item_ptr: *mut crate::abi_types::PyObject,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_list_sort(_list_bits: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_list_reverse(_list_bits: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_list_set_slice(
    _list_bits: u64,
    _ilow: isize,
    _ihigh: isize,
    _replacement: *const u64,
    _replacement_len: usize,
    _future_pointers: *const *mut crate::abi_types::PyObject,
    _future_len: usize,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_alloc_tuple(_n: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_tuple_set(
    _bits: u64,
    _i: usize,
    _val: u64,
    _exact_pointer: *mut crate::abi_types::PyObject,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_tuple_len(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn stub_tuple_item(_bits: u64, _i: usize) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn stub_slice_new(_: u64, _: u64, _: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_mappingproxy_new(_: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}

unsafe extern "C" fn stub_alloc_dict() -> u64 {
    0
}
unsafe extern "C" fn stub_dict_resolve(_: u64, _: u8) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn stub_dict_mutate(
    _d: u64,
    _k: u64,
    _v: u64,
    _delete: u8,
    _publish: Option<unsafe extern "C" fn(*mut std::ffi::c_void) -> i32>,
    _context: *mut std::ffi::c_void,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_dict_get(
    _d: u64,
    _k: u64,
    _: DictHashSource,
    _: i64,
) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn stub_dict_pop(_d: u64, _k: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}

unsafe extern "C" fn stub_dict_len(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn stub_dict_next(
    _dict_bits: u64,
    _position: *mut usize,
    _out_key: *mut u64,
    _out_val: *mut u64,
) -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_unicode_new(_len: usize, _maxchar: u32) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_unicode_commit(
    _bits: u64,
    _data: *const u8,
    _len: usize,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_unicode_encode(
    _bits: u64,
    _encoding: u64,
    _errors: u64,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_str_data(_bits: u64, out_len: *mut usize) -> *const u8 {
    if !out_len.is_null() {
        unsafe {
            *out_len = 0;
        }
    }
    c"".as_ptr().cast()
}
unsafe extern "C" fn stub_bytes_data(_bits: u64, out_len: *mut usize) -> *const u8 {
    if !out_len.is_null() {
        unsafe {
            *out_len = 0;
        }
    }
    std::ptr::null()
}
unsafe extern "C" fn stub_bytearray_data(_bits: u64, out_len: *mut usize) -> *mut u8 {
    if !out_len.is_null() {
        unsafe { *out_len = 0 }
    };
    std::ptr::null_mut()
}
unsafe extern "C" fn stub_bytearray_resize(_bits: u64, _len: usize) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_buffer_supports(_bits: u64) -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_descriptor_protocol(_bits: u64) -> DescriptorProtocol {
    DescriptorProtocol::Error
}
unsafe extern "C" fn stub_descriptor_get(
    _descriptor: u64,
    _receiver: *const u64,
    _owner: *const u64,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_descriptor_set(
    _descriptor: u64,
    _receiver: u64,
    _value: *const u64,
) -> DescriptorMutationStatus {
    DescriptorMutationStatus::Error
}
unsafe extern "C" fn stub_buffer_acquire(
    _bits: u64,
    out_view: *mut MoltBufferView,
) -> std::os::raw::c_int {
    if !out_view.is_null() {
        unsafe {
            *out_view = MoltBufferView::default();
        }
    }
    -1
}
unsafe extern "C" fn stub_buffer_release(view: *mut MoltBufferView) -> std::os::raw::c_int {
    if !view.is_null() {
        unsafe {
            *view = MoltBufferView::default();
        }
    }
    0
}
unsafe extern "C" fn stub_type_dict_borrowed(_type: u64) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}

unsafe extern "C" fn stub_type_metadata(
    _type: u64,
    _field: TypeMetadataField,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_type_lookup_borrowed(
    _type: u64,
    _name: u64,
    _mro: u8,
) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn stub_object_get_attr(
    _obj: u64,
    _name: u64,
    _access: AttributeAccess,
    _dictionary: *const u64,
    _suppress: bool,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_object_set_attr(
    _obj: u64,
    _name: u64,
    _value: u64,
    _delete: bool,
    _access: AttributeMutation,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_object_format(_obj: u64, _spec: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_object_stringify(_value: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_object_contains(_container: u64, _needle: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_object_is_true(_value: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_object_length(_value: u64) -> isize {
    -1
}
unsafe extern "C" fn stub_object_get_item(_obj: u64, _key: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_object_set_item(
    _obj: u64,
    _key: u64,
    _value: *const u64,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_sys_get_object_borrowed(
    _data: *const u8,
    _len: usize,
    _policy: SysLookupPolicy,
) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn stub_eval_get_builtins_borrowed() -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn stub_classify_heap(_bits: u64) -> u8 {
    crate::abi_types::MoltTypeTag::Other as u8
}
unsafe extern "C" fn stub_object_hash(_bits: u64) -> i64 {
    -1
}
unsafe extern "C" fn stub_inc_ref(_bits: u64) {}
unsafe extern "C" fn stub_dec_ref(_bits: u64) {}
unsafe extern "C" fn stub_ref_count(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn stub_alloc_module(_data: *const u8, _len: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_alloc_extension_module(data: *const u8, len: usize) -> u64 {
    // Hook-only ABI fixtures have no active initializer scope. The real runtime
    // supplies the scoped resolver; ordinary module allocation stays shared.
    unsafe { (hooks_or_stubs().alloc_module)(data, len) }
}
unsafe extern "C" fn stub_module_get_dict_borrowed(_module_bits: u64) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn stub_import_add_module_borrowed(
    _data: *const u8,
    _len: usize,
) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn stub_module_set_attr(
    _m: u64,
    _data: *const u8,
    _len: usize,
    _v: u64,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_module_capi_register(
    _module_bits: u64,
    _module_def_ptr: usize,
    _module_state_size: u64,
    _defer_state: bool,
    _callbacks: ModuleGcCallbacks,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_module_capi_get_state(_module_bits: u64) -> *mut u8 {
    std::ptr::null_mut()
}
unsafe extern "C" fn stub_module_capi_get_def(_module_bits: u64) -> usize {
    0
}
unsafe extern "C" fn stub_module_state_add(
    _module_bits: u64,
    _module_def_ptr: usize,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_module_state_find(_module_def_ptr: usize) -> BorrowedHandleResult {
    BorrowedHandleResult::missing()
}
unsafe extern "C" fn stub_module_state_remove(_module_def_ptr: usize) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_register_c_function(
    _meth: u64,
    _flags: std::os::raw::c_int,
    _self_bits: u64,
    _self_is_null: bool,
    _defining_class_bits: u64,
    _data: *const u8,
    _len: usize,
) -> u64 {
    0
}
unsafe extern "C" fn stub_import_module(_data: *const u8, _len: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_initialize_extension(
    _init: unsafe extern "C" fn() -> *mut crate::abi_types::PyObject,
    _name: u64,
    _origin: u64,
    _spec: u64,
    _create_only: bool,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_module_exec_begin(_module: u64, _def: usize) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_exception_pending() -> std::os::raw::c_int {
    0
}

unsafe extern "C" fn stub_pending_exception_class() -> PendingExceptionClass {
    PendingExceptionClass::None
}
unsafe extern "C" fn stub_object_richcompare(
    _op: i32,
    _left: u64,
    _right: u64,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_object_richcompare_builtin(
    _owner: u64,
    _op: i32,
    _left: u64,
    _right: u64,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_number_binary_op(
    _op: u32,
    _mode: u32,
    _a: u64,
    _b: u64,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_number_unary_op(_op: u32, _a: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_number_power(
    _mode: u32,
    _a: u64,
    _b: u64,
    _mod_bits: u64,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_target_python_minor() -> i64 {
    -1
}
unsafe extern "C" fn stub_dict_op(_op: u32, _dict: u64) -> u64 {
    0
}
unsafe extern "C" fn stub_set_op(_op: u32, _set: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
// Set stubs fail closed with the CPython error sentinel (0 / -1). Without the
// runtime set authority registered, returning a fake success would silently
// corrupt set semantics; the API wrappers turn these sentinels into NULL / -1
// with an exception set.
unsafe extern "C" fn stub_set_new(_iterable: BorrowedHandleResult, _frozen: bool) -> u64 {
    0
}
unsafe extern "C" fn stub_set_size(_set: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_set_contains(_set: u64, _key: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_set_add(_set: u64, _key: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_set_discard(_set: u64, _key: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_object_dir(_obj: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}

unsafe extern "C" fn stub_memoryview_new(_bits: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_memoryview_from_buffer(
    _view: *const MoltBufferView,
    _format: *const std::ffi::c_char,
    _lease: *const std::ffi::c_void,
    _restricted: bool,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_memoryview_snapshot(
    _bits: u64,
    _view: *mut MoltBufferView,
    _base: *mut *mut crate::abi_types::PyObject,
    _format: *mut *const u8,
    _format_len: *mut usize,
) -> i32 {
    -1
}
unsafe extern "C" fn stub_object_bytes(_obj: u64, _special: bool) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_object_call(
    _callable: u64,
    _args: u64,
    _kwargs: u64,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_method_new(_func: u64, _receiver: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_method_part(_method: u64, _part: MethodPart) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}

unsafe extern "C" fn stub_object_vectorcall(
    _callable: u64,
    _values: *const u64,
    _positional_count: usize,
    _names: *const u64,
    _keyword_count: usize,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_object_is_callable(_obj: u64) -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_object_classinfo_match(
    _operation: ClassInfoOperation,
    _value_bits: u64,
    _classinfo_bits: u64,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_private_c_heap_contains(_pointer: usize) -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_foreign_new(_c_ptr: usize) -> u64 {
    0
}
unsafe extern "C" fn stub_gil_ensure() -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_gil_leave(_state: std::os::raw::c_int) {}
unsafe extern "C" fn stub_gil_release() {}
unsafe extern "C" fn stub_gil_restore() {}
unsafe extern "C" fn stub_gil_check() -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_runtime_is_initialized() -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_thread_state_drop_enter() -> u64 {
    0
}
unsafe extern "C" fn stub_thread_state_drop_leave(_token: u64) {}
unsafe extern "C" fn stub_attached_runtime_context() -> u32 {
    AttachedRuntimeContextKind::Detached as u32
}
unsafe extern "C" fn stub_pending_call_error(_reason: u32) {}
unsafe extern "C" fn stub_try_mark_abi_view(
    _bits: u64,
    _present: std::os::raw::c_int,
) -> std::os::raw::c_int {
    1
}
unsafe extern "C" fn stub_report_unraisable(
    _context_bits: u64,
    _type_bits: u64,
    _value_bits: u64,
    _traceback_bits: u64,
    message: *const u8,
    message_len: usize,
    _err_msg: *const u8,
    _err_msg_len: usize,
    _has_err_msg: std::os::raw::c_int,
) {
    let text = if message.is_null() {
        "<unraisable exception>".into()
    } else {
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(message, message_len) })
            .into_owned()
    };
    eprintln!("[molt-cpython-abi] unraisable exception: {text}");
}
unsafe extern "C" fn stub_exception_set_field(
    _exception_bits: u64,
    _field: u32,
    _value_bits: u64,
    _has_value: std::os::raw::c_int,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_exception_get_field(
    _exception_bits: u64,
    _field: u32,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_runtime_class_borrowed(_value_bits: u64) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn stub_exception_layout_kind(_exception_bits: u64) -> u8 {
    u8::MAX
}
unsafe extern "C" fn stub_exception_snapshot(
    _exception_bits: u64,
    _out: *mut ExceptionSnapshot,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_exception_commit_snapshot(
    _exception_bits: u64,
    _snapshot: *const ExceptionSnapshot,
) -> std::os::raw::c_int {
    -1
}

unsafe extern "C" fn stub_type_is_subtype(
    _subclass_bits: u64,
    _class_bits: u64,
) -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_take_pending_exception(
    _actual_class_bits: *mut u64,
    _traceback_bits: *mut u64,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_handled_exception_get() -> OwnedHandleResult {
    OwnedHandleResult::missing()
}
unsafe extern "C" fn stub_clear_pending_exception() {}
unsafe extern "C" fn stub_with_preserved_pending_exception(
    callback: unsafe extern "C" fn(*mut std::ffi::c_void),
    context: *mut std::ffi::c_void,
) {
    unsafe { callback(context) };
}
unsafe extern "C" fn stub_handled_exception_set(_owned_exception_bits: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_sequence_item(_bits: u64, _index: isize) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_sequence_check(_bits: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_iter_next(
    _bits: u64,
    _exhausted: *mut std::os::raw::c_int,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}

unsafe extern "C" fn stub_object_length_hint(_bits: u64, _default: isize) -> isize {
    -1
}
unsafe extern "C" fn stub_tuple_uses_length_hint() -> bool {
    false
}
unsafe extern "C" fn stub_native_gc_allocate(_addr: usize) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_managed_gc_control(
    _bits: u64,
    action: ManagedGcAction,
) -> std::os::raw::c_int {
    match action {
        ManagedGcAction::Track | ManagedGcAction::Untrack => -1,
        ManagedGcAction::IsTracked | ManagedGcAction::IsFinalized => 0,
    }
}
unsafe extern "C" fn stub_native_gc_track(_addr: usize) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_managed_gc_traverse(
    _bits: u64,
    _visit: crate::api::memory::NativeGcVisitProc,
    _context: *mut std::ffi::c_void,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_managed_gc_clear(_bits: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_native_gc_untrack(_addr: usize) {}
unsafe extern "C" fn stub_native_gc_deallocate(_addr: usize) {}
unsafe extern "C" fn stub_native_gc_is_tracked(_addr: usize) -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_native_gc_is_finalized(_addr: usize) -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_native_gc_claim_finalizer(_addr: usize) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn stub_gc_collect() -> isize {
    0
}
unsafe extern "C" fn stub_gc_enable() -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_gc_disable() -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_gc_is_enabled() -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_check_signals() -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_set_interrupt(signum: std::os::raw::c_int) -> std::os::raw::c_int {
    if signum < 1 { -1 } else { 0 }
}
unsafe extern "C" fn stub_interrupt_occurred() -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn stub_notify_pending_calls() {}
unsafe extern "C" fn stub_exception_group_admit(
    _request: u32,
    _type_name: *const std::os::raw::c_char,
    _args_bits: u64,
    _message_bits: *mut u64,
    _exceptions_bits: *mut u64,
) -> std::os::raw::c_int {
    -1
}

/// A no-op hooks table used when the runtime hasn't registered yet.
unsafe extern "C" fn stub_builtin_slot_owner(
    _descriptor: u64,
    _name: *const u8,
    _name_len: usize,
    _constructor: bool,
) -> BorrowedHandleResult {
    BorrowedHandleResult::missing()
}

// Optional callback fields are the sole authority for capability absence.
unsafe extern "C" fn stub_context_type_admit(_: usize) -> i32 {
    -1
}
unsafe extern "C" fn stub_context_new() -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_context_copy(_: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_context_transition(_: u64) -> i32 {
    -1
}
unsafe extern "C" fn stub_context_optional(_: u64, _: u64, _: i32) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_context_set(_: u64, _: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn stub_context_reset(_: u64, _: u64) -> i32 {
    -1
}

pub const STUB_HOOKS: RuntimeHooks = RuntimeHooks {
    abi_magic: RUNTIME_HOOKS_ABI_MAGIC,
    abi_version: RUNTIME_HOOKS_ABI_VERSION,
    struct_size: std::mem::size_of::<RuntimeHooks>() as u32,
    gil_ensure: stub_gil_ensure,
    gil_leave: stub_gil_leave,
    gil_release: stub_gil_release,
    gil_restore: stub_gil_restore,
    gil_check: stub_gil_check,
    runtime_is_initialized: stub_runtime_is_initialized,
    thread_state_drop_enter: stub_thread_state_drop_enter,
    thread_state_drop_leave: stub_thread_state_drop_leave,
    attached_runtime_context: stub_attached_runtime_context,
    pending_call_error: stub_pending_call_error,
    alloc_str: None,
    alloc_bytes: stub_alloc_bytes,
    alloc_bytearray: stub_alloc_bytes,
    numeric_identity_new: None,
    float_payload: stub_float_payload,
    int_from_i64: stub_int_from_i64,
    int_from_u64: stub_int_from_u64,

    int_from_digits: stub_int_from_digits,
    int_from_f64_trunc: stub_int_from_f64_trunc,
    int_sign: stub_int_sign,

    int_from_bytes: stub_int_from_bytes,
    int_to_bytes: stub_int_to_bytes,
    int_num_bits: stub_int_num_bits,
    int_max_str_digits: stub_int_max_str_digits,
    complex_parts: stub_complex_parts,
    complex_from_doubles: stub_complex_from_doubles,
    alloc_list: stub_alloc_list,
    alloc_list_presized: stub_alloc_list_presized,
    list_append: stub_list_append,
    list_len: stub_list_len,
    list_item: stub_list_item,
    list_set: stub_list_set,
    list_insert: stub_list_insert,
    list_sort: stub_list_sort,
    list_reverse: stub_list_reverse,
    list_set_slice: stub_list_set_slice,
    alloc_tuple: None,
    tuple_set: None,
    tuple_len: None,
    tuple_item: None,
    alloc_dict: stub_alloc_dict,
    mappingproxy_new: stub_mappingproxy_new,
    dict_resolve: stub_dict_resolve,
    dict_mutate: stub_dict_mutate,
    dict_get: stub_dict_get,
    dict_pop: stub_dict_pop,
    dict_len: stub_dict_len,
    dict_next: stub_dict_next,
    str_data: stub_str_data,
    unicode_new: stub_unicode_new,
    unicode_commit: stub_unicode_commit,
    unicode_encode: stub_unicode_encode,
    bytes_data: stub_bytes_data,
    bytearray_data: stub_bytearray_data,
    bytearray_resize: stub_bytearray_resize,
    buffer_supports: stub_buffer_supports,
    buffer_acquire: stub_buffer_acquire,
    buffer_release: stub_buffer_release,
    object_get_attr: stub_object_get_attr,
    type_dict_borrowed: stub_type_dict_borrowed,
    type_metadata: stub_type_metadata,
    builtin_slot_owner: stub_builtin_slot_owner,
    type_lookup_borrowed: stub_type_lookup_borrowed,
    object_set_attr: stub_object_set_attr,
    descriptor_protocol: stub_descriptor_protocol,
    descriptor_get: stub_descriptor_get,
    descriptor_set: stub_descriptor_set,
    object_format: stub_object_format,
    object_str: stub_object_stringify,
    object_repr: stub_object_stringify,
    object_is_true: stub_object_is_true,
    object_length: stub_object_length,
    object_get_item: stub_object_get_item,
    object_supports_subscript: stub_object_is_true,
    object_set_item: stub_object_set_item,
    object_get_iter: stub_object_stringify,
    iter_check: stub_object_is_callable,
    iter_next: stub_iter_next,
    sys_get_object_borrowed: stub_sys_get_object_borrowed,
    eval_get_builtins_borrowed: stub_eval_get_builtins_borrowed,
    classify_heap: None,
    object_hash: stub_object_hash,
    inc_ref: stub_inc_ref,
    dec_ref: stub_dec_ref,
    ref_count: None,
    try_mark_abi_view: stub_try_mark_abi_view,
    alloc_module: stub_alloc_module,
    alloc_extension_module: stub_alloc_extension_module,
    module_get_dict_borrowed: stub_module_get_dict_borrowed,
    import_add_module_borrowed: stub_import_add_module_borrowed,
    module_set_attr: stub_module_set_attr,
    module_capi_register: stub_module_capi_register,
    module_capi_get_state: stub_module_capi_get_state,
    module_capi_get_def: stub_module_capi_get_def,
    module_state_add: stub_module_state_add,
    module_state_find: stub_module_state_find,
    module_state_remove: stub_module_state_remove,
    module_exec_begin: stub_module_exec_begin,
    register_c_function: None,
    import_module: stub_import_module,
    initialize_extension: stub_initialize_extension,
    exception_pending: stub_exception_pending,
    pending_exception_class: stub_pending_exception_class,
    object_richcompare: stub_object_richcompare,
    object_richcompare_builtin: stub_object_richcompare_builtin,
    number_binary_op: stub_number_binary_op,
    number_unary_op: stub_number_unary_op,
    number_power: stub_number_power,
    target_python_minor: stub_target_python_minor,
    dict_op: stub_dict_op,
    set_op: stub_set_op,
    set_new: stub_set_new,
    set_size: stub_set_size,
    set_contains: stub_set_contains,
    set_add: stub_set_add,
    set_discard: stub_set_discard,
    object_dir: stub_object_dir,
    object_call: stub_object_call,
    object_vectorcall: stub_object_vectorcall,
    method_new: None,
    method_part: stub_method_part,
    object_is_callable: stub_object_is_callable,
    object_classinfo_match: None,
    foreign_new: stub_foreign_new,
    report_unraisable: stub_report_unraisable,
    exception_set_field: stub_exception_set_field,
    exception_get_field: stub_exception_get_field,
    runtime_class_borrowed: None,
    exception_layout_kind: stub_exception_layout_kind,
    exception_snapshot: stub_exception_snapshot,
    exception_commit_snapshot: stub_exception_commit_snapshot,
    type_is_subtype: stub_type_is_subtype,
    take_pending_exception: stub_take_pending_exception,
    clear_pending_exception: stub_clear_pending_exception,
    with_preserved_pending_exception: stub_with_preserved_pending_exception,
    handled_exception_get: stub_handled_exception_get,
    handled_exception_set: stub_handled_exception_set,
    native_gc_allocate: None,
    managed_gc_control: stub_managed_gc_control,
    managed_gc_traverse: stub_managed_gc_traverse,
    managed_gc_clear: stub_managed_gc_clear,
    native_gc_track: stub_native_gc_track,
    native_gc_untrack: stub_native_gc_untrack,
    native_gc_deallocate: stub_native_gc_deallocate,
    native_gc_is_tracked: stub_native_gc_is_tracked,
    native_gc_is_finalized: stub_native_gc_is_finalized,
    native_gc_claim_finalizer: stub_native_gc_claim_finalizer,
    gc_collect: stub_gc_collect,
    gc_enable: stub_gc_enable,
    gc_disable: stub_gc_disable,
    gc_is_enabled: stub_gc_is_enabled,
    check_signals: stub_check_signals,
    set_interrupt: stub_set_interrupt,
    interrupt_occurred: stub_interrupt_occurred,
    notify_pending_calls: stub_notify_pending_calls,
    sequence_check: stub_sequence_check,
    sequence_item: stub_sequence_item,
    object_length_hint: stub_object_length_hint,
    tuple_uses_length_hint: stub_tuple_uses_length_hint,
    exception_group_admit: stub_exception_group_admit,
    private_c_heap_contains: stub_private_c_heap_contains,
    object_bytes: stub_object_bytes,
    memoryview_new: stub_memoryview_new,
    memoryview_release: stub_memoryview_new,
    memoryview_from_buffer: stub_memoryview_from_buffer,
    memoryview_snapshot: stub_memoryview_snapshot,
    slice_new: stub_slice_new,
    slice_item: stub_tuple_item,
    object_contains: stub_object_contains,
    context_type_admit: stub_context_type_admit,
    context_new: stub_context_new,
    context_copy_current: stub_context_new,
    context_copy: stub_context_copy,
    context_enter: stub_context_transition,
    context_exit: stub_context_transition,
    context_var_new: stub_context_optional,
    context_var_get: stub_context_optional,
    context_var_set: stub_context_set,
    context_var_reset: stub_context_reset,
};

/// Return the registered hooks or the typed fail-closed bootstrap table.
/// Stubs provide ABI-safe sentinels, never alternate runtime semantics.
#[inline]
pub fn hooks_or_stubs() -> &'static RuntimeHooks {
    RUNTIME_HOOKS.get().unwrap_or(&STUB_HOOKS)
}

/// Pins the managed-runtime GIL for a complete ABI bridge transaction.
///
/// Acquiring once before any bridge shard lock preserves the global lock order
/// (`runtime GIL -> bridge locks`) and avoids the inversion that would result
/// from acquiring inside `try_mark_abi_view` while address/handle shards are
/// held. Calls made from an existing Molt execution frame are a zero-acquire
/// fast path: `gil_check` observes the current owner and Drop does nothing.
pub(crate) struct RuntimeGilGuard {
    state: std::os::raw::c_int,
    acquired: bool,
}

impl RuntimeGilGuard {
    #[inline]
    pub(crate) fn ensure() -> Self {
        let hooks = hooks_or_stubs();
        if unsafe { (hooks.gil_check)() } != 0 {
            Self {
                state: 0,
                acquired: false,
            }
        } else {
            Self {
                state: unsafe { (hooks.gil_ensure)() },
                acquired: true,
            }
        }
    }
}

impl Drop for RuntimeGilGuard {
    #[inline]
    fn drop(&mut self) {
        if self.acquired {
            unsafe { (hooks_or_stubs().gil_leave)(self.state) };
        }
    }
}
