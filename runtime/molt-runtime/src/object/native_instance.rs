//! One native-subtype representation: the physical payload followed by aligned
//! declared slots and the ordinary layout tail. FileHandle keeps its dictionary
//! in native backing; its reserved tail word remains empty. The sealed class owns the
//! physical kind and extent; immutable payload length derives the field base.
//! No per-object offset or tuple-only dictionary representation is retained.

use crate::*;
use std::mem::size_of;

const WORD: usize = size_of::<u64>();

#[cfg(test)]
#[path = "native_instance_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativePayload {
    Tuple,
    String,
    Bytes,
    Bytearray,
    Set,
    Frozenset,
    Complex,
    FileHandle,
}

impl NativePayload {
    pub(crate) fn owner(self, py: &PyToken<'_>) -> Option<u64> {
        // FileHandle has several concrete native roots. Their sealed physical
        // kind and non-inherited NativeSlotLayout declaration own admission.
        if self == Self::FileHandle {
            return None;
        }
        let classes = builtin_classes(py);
        Some(match self {
            Self::Tuple => classes.tuple,
            Self::String => classes.str,
            Self::Bytes => classes.bytes,
            Self::Bytearray => classes.bytearray,
            Self::Set => classes.set,
            Self::Frozenset => classes.frozenset,
            Self::Complex => classes.complex,
            Self::FileHandle => unreachable!(),
        })
    }

    pub(crate) fn type_id(self) -> u32 {
        match self {
            Self::Tuple => TYPE_ID_TUPLE,
            Self::String => TYPE_ID_STRING,
            Self::Bytes => TYPE_ID_BYTES,
            Self::Bytearray => TYPE_ID_BYTEARRAY,
            Self::Set => TYPE_ID_SET,
            Self::Frozenset => TYPE_ID_FROZENSET,
            Self::Complex => TYPE_ID_COMPLEX,
            Self::FileHandle => TYPE_ID_FILE_HANDLE,
        }
    }

    pub(crate) fn from_type_id(type_id: u32) -> Option<Self> {
        Some(match type_id {
            TYPE_ID_TUPLE => Self::Tuple,
            TYPE_ID_STRING => Self::String,
            TYPE_ID_BYTES => Self::Bytes,
            TYPE_ID_BYTEARRAY => Self::Bytearray,
            TYPE_ID_SET => Self::Set,
            TYPE_ID_FROZENSET => Self::Frozenset,
            TYPE_ID_COMPLEX => Self::Complex,
            TYPE_ID_FILE_HANDLE => Self::FileHandle,
            _ => return None,
        })
    }

    /// Length is used only by immutable inline values. Mutable native payloads
    /// own stable Vec pointers, so their changing content cannot move the tail.
    fn native_bytes(self, length: usize) -> Option<usize> {
        match self {
            Self::Tuple => super::layout::TupleStorage::object_size(length)?
                .checked_sub(size_of::<MoltHeader>()),
            Self::String | Self::Bytes => super::layout::InlineBytesStorage::payload_size(length),
            Self::Bytearray => Some(size_of::<*mut Vec<u8>>() + size_of::<u64>()),
            Self::Set | Self::Frozenset => Some(size_of::<HashStorage<SetEntry>>()),
            Self::Complex => Some(size_of::<crate::builtins::numbers::ComplexParts>()),
            Self::FileHandle => Some(size_of::<*mut MoltFileHandle>()),
        }
    }

    unsafe fn fields_offset(self, object: *mut u8) -> usize {
        let length = unsafe {
            match self {
                Self::Tuple => super::layout::tuple_storage_len(object),
                Self::String | Self::Bytes => super::layout::InlineBytesStorage::len(object),
                _ => 0,
            }
        };
        align(
            self.native_bytes(length)
                .expect("valid native payload length"),
        )
        .expect("valid aligned native payload extent")
    }

    /// Called before conversions as well as at the allocation boundary.
    /// Real MRO ancestry and the sealed physical kind both have to agree.
    pub(crate) unsafe fn admit(self, py: &PyToken<'_>, class: u64) -> Option<*mut u8> {
        unsafe {
            let owner = self.owner(py);
            let label = owner.map(class_name_for_error);
            let (_, class_ptr) = crate::builtins::type_ops::native_constructor_receiver(
                py,
                owner.unwrap_or(class),
                Some(class),
                label.as_deref().unwrap_or("IO"),
            )?;
            super::class_finish_definition(py, class_ptr).ok()?;
            if super::class_instance_type_id(class_ptr) != self.type_id()
                || super::class_instance_shape_id(class_ptr) != super::ObjectShapeId::Plain
            {
                raise_exception::<()>(
                    py,
                    "TypeError",
                    "native constructor requires its sealed payload layout",
                );
                return None;
            }
            Some(class_ptr)
        }
    }
}

fn align(bytes: usize) -> Option<usize> {
    bytes.checked_add(WORD - 1).map(|bytes| bytes & !(WORD - 1))
}

/// NativeSlotLayout is a non-inherited declaration of each native root.
/// An exact value carrying its builtin class edge still has no extension.
#[inline]
pub(crate) unsafe fn has_fields(object: *mut u8) -> bool {
    unsafe {
        if NativePayload::from_type_id(object_type_id(object)).is_none() {
            return false;
        }
        let Some(class) = obj_from_bits(object_class_bits(object)).as_ptr() else {
            return false;
        };
        !super::class_storage::class_declares(
            class,
            super::class_storage::ClassDeclaration::NativeSlotLayout,
        )
    }
}

/// Initialization can project through the already-sealed class before its edge
/// is committed. No native owner or finalizer is exposed during allocation.
pub(crate) unsafe fn field_base_for_class(object: *mut u8, class: *mut u8) -> *mut u8 {
    unsafe {
        let Some(kind) = NativePayload::from_type_id(super::class_instance_type_id(class)) else {
            return object;
        };
        assert_eq!(object_type_id(object), kind.type_id());
        let offset = kind.fields_offset(object);
        let size =
            super::layout::class_cached_layout_size(class).expect("sealed native class extent");
        assert!(
            offset
                .checked_add(size)
                .is_some_and(|end| end <= object_payload_size(object))
        );
        object.add(offset)
    }
}

#[inline]
pub(crate) unsafe fn field_base(object: *mut u8) -> *mut u8 {
    unsafe {
        if !has_fields(object) {
            return object;
        }
        field_base_for_class(
            object,
            obj_from_bits(object_class_bits(object)).as_ptr().unwrap(),
        )
    }
}

pub(crate) unsafe fn field_payload_size(object: *mut u8) -> usize {
    unsafe {
        if !has_fields(object) {
            return object_payload_size(object);
        }
        let class = obj_from_bits(object_class_bits(object)).as_ptr().unwrap();
        let _ = field_base_for_class(object, class); // Bounds belong to this authority.
        super::layout::class_cached_layout_size(class).unwrap()
    }
}

/// Whether `class` extends its payload's root. A payload with one canonical
/// owner compares against it, so a native class that declares its own slot
/// layout on top of `tuple` keeps its identity and fields. FileHandle has
/// several concrete native roots, each declaring NativeSlotLayout.
unsafe fn extends_root(
    py: &PyToken<'_>,
    kind: NativePayload,
    class: u64,
    class_ptr: *mut u8,
) -> bool {
    match kind.owner(py) {
        Some(owner) => class != owner,
        None => unsafe {
            !super::class_storage::class_declares(
                class_ptr,
                super::class_storage::ClassDeclaration::NativeSlotLayout,
            )
        },
    }
}

/// Allocate only zeroed physical storage. The caller fills all native owners
/// before publish installs the class edge, admits GC membership, and publishes.
pub(crate) unsafe fn alloc_unpublished(
    py: &PyToken<'_>,
    class: u64,
    kind: NativePayload,
    length: usize,
) -> *mut u8 {
    unsafe {
        let Some(class_ptr) = kind.admit(py, class) else {
            return std::ptr::null_mut();
        };
        let subtype = extends_root(py, kind, class, class_ptr);
        let native = kind.native_bytes(length);
        let payload = if subtype {
            native.and_then(align).and_then(|offset| {
                offset.checked_add(super::layout::class_cached_layout_size(class_ptr).unwrap())
            })
        } else {
            native
        };
        let Some(total) = payload.and_then(super::checked_object_total_size) else {
            record_memory_error_without_allocation(py);
            return std::ptr::null_mut();
        };
        let aux = if !subtype {
            if kind == NativePayload::FileHandle {
                // Every concrete IO root retains its actual class identity.
                super::ObjectAuxPreselection::ClassInline
            } else if kind == NativePayload::Frozenset {
                super::ObjectAuxPreselection::StateInline
            } else {
                super::ObjectAuxPreselection::Default
            }
        } else if matches!(
            kind,
            NativePayload::String | NativePayload::Bytes | NativePayload::Frozenset
        ) {
            super::ObjectAuxPreselection::Sidecar // Hash state and class share existing sidecar.
        } else {
            super::ObjectAuxPreselection::ClassInline
        };
        super::alloc_object_zeroed_unpublished_with_aux(py, total, kind.type_id(), aux)
    }
}

/// Prepare fields and class ownership without publishing the allocation. The
/// caller has already installed every native owner, so failure can retire the
/// complete payload through the ordinary lifecycle authority. FileHandle uses
/// this phase before initializing its public dictionary members.
/// Consumes the unpublished allocation on failure; success remains unpublished.
pub(crate) unsafe fn prepare_publication(py: &PyToken<'_>, object: *mut u8, class: u64) -> *mut u8 {
    unsafe {
        if object.is_null() {
            return object;
        }
        let kind = NativePayload::from_type_id(object_type_id(object)).expect("native allocation");
        let class_ptr = obj_from_bits(class).as_ptr().unwrap();
        let subtype = extends_root(py, kind, class, class_ptr);
        if subtype {
            let size = super::layout::class_cached_layout_size(class_ptr).unwrap();
            if super::field_storage::initialize_fields(py, object, class_ptr, size).is_err() {
                dec_ref_bits(py, MoltObject::from_ptr(object).bits());
                return std::ptr::null_mut();
            }
        }
        if (subtype || kind == NativePayload::FileHandle)
            && !super::object_init_class_edge_unpublished(
                py,
                object,
                class,
                super::ClassEdgeOwnership::Owned,
            )
        {
            dec_ref_bits(py, MoltObject::from_ptr(object).bits());
            raise_exception::<()>(py, "SystemError", "native class edge initialization failed");
            return std::ptr::null_mut();
        }
        if subtype {
            // Native classes never own inferred inline rows. The sticky guard
            // also forces any compiled immediate store through field projection.
            (*header_from_obj_ptr(object))
                .fetch_or_flags(super::HEADER_FLAG_HAS_PTRS | super::HEADER_FLAG_CONTAINS_REFS);
            // Always-tracked kinds (including FileHandle) were admitted by the
            // allocator. Only a class-dependent projection needs promotion.
            if super::heap_track_projection(kind.type_id())
                == Some(super::HeapTrackProjection::NativeSubtype)
            {
                super::gc::gc_track_if_cyclic(py, object, kind.type_id());
            }
        }
        object
    }
}

/// Consume the unpublished allocation on both success and failure. Native Vecs,
/// tuple items and inline bytes are complete before any owned class edge exists.
pub(crate) unsafe fn publish(py: &PyToken<'_>, object: *mut u8, class: u64) -> *mut u8 {
    unsafe {
        let object = prepare_publication(py, object, class);
        if !object.is_null() {
            super::gc::gc_publish_initialized(py, object);
        }
        object
    }
}
