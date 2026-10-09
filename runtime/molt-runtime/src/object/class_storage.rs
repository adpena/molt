//! Class-owned declarations and reference storage.
//!
//! Reference replacement publishes the new owner before releasing the old one.
//! Multi-field operations transfer displaced references to the caller so that
//! every field can be published before the first callback-capable release.

use crate::{MoltObject, PyToken, dec_ref_bits, inc_ref_bits};

/// Private, monotonic semantic declarations owned by the actual class.
/// These are not Python attributes or global class-registration edges.
#[derive(Clone, Copy)]
#[repr(u64)]
pub(crate) enum ClassDeclaration {
    /// Instances require extension artifact admission, including when their
    /// spec uses a nonstandard extension suffix. This grants no capability.
    ExtensionLoader = 1,
    /// This class introduces payload storage not distinguished by native kind,
    /// object shape, or exception layout root. Inheritance derives its owner.
    IntrinsicLayout = 1 << 1,
    /// A tagged scalar value word at the canonical scalar subclass position.
    IntValue = 1 << 2,
    FloatValue = 1 << 3,
    /// Native class construction supplies its exact baseline capabilities.
    /// Unlike an ordinary heap class, absence of __slots__ adds no policy.
    NativeSlotLayout = 1 << 4,
    VariableSizedInstance = 1 << 5,
    InstanceDictionary = 1 << 6,
    InstanceWeakrefs = 1 << 7,
    /// This native namespace has exposed its full declaration family once.
    /// Subsequent C dictionary edits own missing/replaced values; lookup may
    /// no longer recreate a member from the declaration table.
    NativeNamespacePublished = 1 << 8,
    /// Semantic CPython static origin, independent of physical Molt allocation.
    /// This exact-class declaration is never inherited by a heap subclass.
    StaticType = 1 << 10,
    /// This exact class is constructed from the canonical exception schema.
    /// Renaming it never changes identity; subclasses do not inherit this fact.
    BuiltinException = 1 << 11,
    /// Constructor policy is exact-class state sealed once before projection.
    SemanticPolicySealed = 1 << 12,
    // Bits 13..=23 are the typed NativeProtocolSlot inventory below.
}

/// Type semantics are constructor facts, independent of native instance layout,
/// cache membership, and namespace spelling. Ordinary class allocation defaults
/// to a mutable heap basetype; native factories seal explicit facts before
/// publication. Static origin is deliberately NOT inherited through the MRO.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClassOrigin {
    Heap,
    Static,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ClassSemanticPolicy {
    origin: ClassOrigin,
    immutable: bool,
    basetype: bool,
    abstract_type: bool,
}

impl ClassSemanticPolicy {
    #[cfg(test)]
    pub(crate) const fn origin(self) -> ClassOrigin {
        self.origin
    }

    #[cfg(test)]
    pub(crate) const fn immutable(self) -> bool {
        self.immutable
    }

    pub(crate) const fn heap(immutable: bool, basetype: bool) -> Self {
        Self {
            origin: ClassOrigin::Heap,
            immutable,
            basetype,
            abstract_type: false,
        }
    }

    pub(crate) const fn static_type(basetype: bool) -> Self {
        Self {
            origin: ClassOrigin::Static,
            immutable: true,
            basetype,
            abstract_type: false,
        }
    }

    pub(crate) fn for_builtin_exception(
        spec: &'static molt_obj_model::BuiltinExceptionSpec,
    ) -> Self {
        if spec.is_heap_type() {
            Self::heap(false, true)
        } else {
            Self::static_type(true)
        }
    }

    /// Apply only to the unpublished constructor's class, never a subclass or
    /// a type selected from mutable metadata. No additional storage is needed.
    pub(crate) unsafe fn apply(self, py: &PyToken<'_>, class: *mut u8) -> bool {
        unsafe {
            if class.is_null() || crate::object_type_id(class) != crate::TYPE_ID_TYPE {
                return false;
            }
            let bits = MoltObject::from_ptr(class).bits();
            assert!(
                !class_declares(class, ClassDeclaration::SemanticPolicySealed),
                "class semantics may only be sealed once"
            );
            assert!(
                !molt_cpython_abi::bridge::GLOBAL_BRIDGE.type_has_projection(bits),
                "class semantics must be sealed before C projection"
            );
            if self.origin == ClassOrigin::Static {
                for &base in crate::builtins::type_ops::class_mro_view(py, class).iter() {
                    if base != bits {
                        assert!(
                            crate::obj_from_bits(base).as_ptr().is_some_and(|base| {
                                class_declares(base, ClassDeclaration::StaticType)
                            }),
                            "static type cannot inherit a heap type"
                        );
                    }
                }
                class_declare(class, ClassDeclaration::StaticType);
            }
            if (!self.immutable || super::class_set_immutable(py, class))
                && (self.basetype || super::class_set_not_base(py, class))
            {
                class_declare(class, ClassDeclaration::SemanticPolicySealed);
                true
            } else {
                false
            }
        }
    }

    pub(crate) unsafe fn of(py: &PyToken<'_>, class: *mut u8) -> Self {
        unsafe {
            assert_eq!(crate::object_type_id(class), crate::TYPE_ID_TYPE);
            Self {
                origin: if class_declares(class, ClassDeclaration::StaticType) {
                    ClassOrigin::Static
                } else {
                    ClassOrigin::Heap
                },
                immutable: super::class_is_immutable(py, class),
                basetype: !super::class_is_not_base(py, class),
                abstract_type: class_is_abstract(class),
            }
        }
    }

    /// Only semantic flags: physical readiness, protocol, and GC flags remain
    /// owned by the C view. This never derives origin from immutability.
    pub(crate) fn cpython_flags(self) -> std::os::raw::c_ulong {
        use molt_cpython_abi::abi_types::{
            Py_TPFLAGS_BASETYPE, Py_TPFLAGS_HEAPTYPE, Py_TPFLAGS_IMMUTABLETYPE,
            Py_TPFLAGS_IS_ABSTRACT,
        };
        (if self.origin == ClassOrigin::Heap {
            Py_TPFLAGS_HEAPTYPE
        } else {
            0
        }) | (if self.immutable {
            Py_TPFLAGS_IMMUTABLETYPE
        } else {
            0
        }) | (if self.basetype {
            Py_TPFLAGS_BASETYPE
        } else {
            0
        }) | (if self.abstract_type {
            Py_TPFLAGS_IS_ABSTRACT
        } else {
            0
        })
    }
}

pub(crate) unsafe fn class_is_heap_type(class: *mut u8) -> bool {
    unsafe {
        crate::object_type_id(class) == crate::TYPE_ID_TYPE
            && !class_declares(class, ClassDeclaration::StaticType)
    }
}

/// The sealed result of class slot admission, independent of mutable names.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ClassSlotPolicy {
    pub(crate) allows_dict: bool,
    pub(crate) allows_weakref: bool,
    pub(crate) variable_sized: bool,
}

impl ClassSlotPolicy {
    /// Generated heap policy supplies the baseline of an exact native kind.
    /// Class-specific native capabilities are explicit constructor facts; hot
    /// registration reads only the resulting sealed class admission record.
    pub(crate) fn native(type_id: u32) -> Self {
        Self {
            allows_weakref: matches!(
                super::heap_weakref_policy(type_id),
                Some(super::HeapWeakrefPolicy::Allow)
            ),
            ..Self::default()
        }
    }

    pub(crate) fn encode(self) -> u64 {
        MoltObject::from_int(
            i64::from(self.allows_dict)
                | (i64::from(self.allows_weakref) << 1)
                | (i64::from(self.variable_sized) << 2),
        )
        .bits()
    }

    pub(crate) fn decode(bits: u64) -> Self {
        let flags = crate::obj_from_bits(bits)
            .as_int()
            .expect("sealed slot policy");
        assert!((0..=7).contains(&flags), "invalid sealed slot policy");
        Self {
            allows_dict: flags & 1 != 0,
            allows_weakref: flags & 2 != 0,
            variable_sized: flags & 4 != 0,
        }
    }
}

/// Record baseline facts at the native factory, before admission is captured.
pub(crate) unsafe fn class_declare_native_slots(class: *mut u8, policy: ClassSlotPolicy) {
    unsafe {
        assert_eq!(
            super::layout::class_slot_declaration_bits(class),
            0,
            "native slot facts must precede admission"
        );
        class_declare(class, ClassDeclaration::NativeSlotLayout);
        if policy.allows_dict {
            class_declare(class, ClassDeclaration::InstanceDictionary);
        }
        if policy.allows_weakref {
            class_declare(class, ClassDeclaration::InstanceWeakrefs);
        }
        if policy.variable_sized {
            class_declare(class, ClassDeclaration::VariableSizedInstance);
        }
    }
}

/// These are introductions on the actual native constructor's class handle.
/// Inherited facts live only in the sealed admission record.
pub(crate) unsafe fn class_native_slot_policy(class: *mut u8) -> Option<ClassSlotPolicy> {
    unsafe {
        class_declares(class, ClassDeclaration::NativeSlotLayout).then(|| ClassSlotPolicy {
            allows_dict: class_declares(class, ClassDeclaration::InstanceDictionary),
            allows_weakref: class_declares(class, ClassDeclaration::InstanceWeakrefs),
            variable_sized: class_declares(class, ClassDeclaration::VariableSizedInstance),
        })
    }
}

// Native protocol presence shares the existing exact-class declaration owner.
// It is not inherited as a class fact and never inferred from a method name,
// instance shape, mutable Python metadata, or a second runtime registry.
const NATIVE_PROTOCOL_SHIFT: u32 = 13;

pub(crate) unsafe fn class_declare_native_protocols(
    class: *mut u8,
    slots: &[molt_cpython_abi::hooks::NativeProtocolSlot],
) {
    unsafe {
        assert!(class_declares(class, ClassDeclaration::NativeSlotLayout));
        let mask = slots.iter().fold(0, |mask, slot| mask | slot.bit());
        class_declarations_word(class).fetch_or(
            mask << NATIVE_PROTOCOL_SHIFT,
            super::AtomicOrdering::Release,
        );
    }
}

pub(crate) unsafe fn class_native_protocols(class: *mut u8) -> Option<u64> {
    unsafe {
        class_declares(class, ClassDeclaration::NativeSlotLayout).then(|| {
            (class_declarations_word(class).load(super::AtomicOrdering::Acquire)
                >> NATIVE_PROTOCOL_SHIFT)
                & molt_cpython_abi::hooks::NativeProtocolSlot::ALL_MASK
        })
    }
}

#[inline]
unsafe fn class_declarations_word<'a>(ptr: *mut u8) -> &'a super::MoltAuxWord {
    unsafe {
        &*ptr
            .cast::<super::MoltAuxWord>()
            .add(super::layout::CLASS_DECLARATIONS_WORD)
    }
}

// Mutable semantic state shares the existing declaration word, outside the
// monotonic declaration bits. The packed layout-policy word has no free bits.
const CLASS_STATE_ABSTRACT: u64 = 1 << 63;

pub(crate) unsafe fn class_is_abstract(class: *mut u8) -> bool {
    unsafe {
        class_declarations_word(class).load(super::AtomicOrdering::Acquire) & CLASS_STATE_ABSTRACT
            != 0
    }
}

pub(crate) unsafe fn class_set_abstract(
    class: *mut u8,
    abstract_type: bool,
) -> Result<(), molt_cpython_abi::ErrorIndicatorSet> {
    unsafe {
        let word = class_declarations_word(class);
        if abstract_type {
            word.fetch_or(CLASS_STATE_ABSTRACT, super::AtomicOrdering::AcqRel);
        } else {
            let _ = word.try_update(
                super::AtomicOrdering::AcqRel,
                super::AtomicOrdering::Acquire,
                |old| Some(old & !CLASS_STATE_ABSTRACT),
            );
        }
        // A C projection is an observer of this state, never a reason to create
        // a new projection during managed metadata assignment.
        let flag = molt_cpython_abi::abi_types::Py_TPFLAGS_IS_ABSTRACT;
        let published = molt_cpython_abi::bridge::GLOBAL_BRIDGE.publish_existing_type_flags(
            MoltObject::from_ptr(class).bits(),
            flag,
            if abstract_type { flag } else { 0 },
        );
        // CPython latches after invalidation callbacks. Such a callback may
        // warm an object-constructor shortcut under the old abstract flag.
        super::layout::class_bump_layout_version(class);
        published
    }
}

/// Initialize only while the class payload is unpublished.
pub(crate) unsafe fn initialize_class_declarations(ptr: *mut u8) {
    unsafe {
        ptr.cast::<super::MoltAuxWord>()
            .add(super::layout::CLASS_DECLARATIONS_WORD)
            .write(super::MoltAuxWord::new(0));
    }
}

pub(crate) unsafe fn class_declare(ptr: *mut u8, declaration: ClassDeclaration) {
    unsafe {
        assert_eq!(crate::object_type_id(ptr), crate::TYPE_ID_TYPE);
        assert_eq!(
            declaration as u64 & CLASS_STATE_ABSTRACT,
            0,
            "declaration overlaps mutable class state"
        );
        class_declarations_word(ptr).fetch_or(declaration as u64, super::AtomicOrdering::Release);
    }
}

pub(crate) unsafe fn class_declares(ptr: *mut u8, declaration: ClassDeclaration) -> bool {
    unsafe {
        crate::object_type_id(ptr) == crate::TYPE_ID_TYPE
            && class_declarations_word(ptr).load(super::AtomicOrdering::Acquire)
                & declaration as u64
                != 0
    }
}

/// Follow the real class edge and current MRO, without Python attribute or
/// descriptor callbacks. Inherited declarations are never copied to children:
/// changing __bases__ changes their projection without a descendant registry.
pub(crate) fn object_class_declares(
    py: &PyToken<'_>,
    bits: u64,
    declaration: ClassDeclaration,
) -> bool {
    let Some(ptr) = crate::obj_from_bits(bits).as_ptr() else {
        return false;
    };
    unsafe {
        let Some(class) = crate::obj_from_bits(crate::object_class_bits(ptr)).as_ptr() else {
            return false;
        };
        if crate::object_type_id(class) != crate::TYPE_ID_TYPE {
            return false;
        }
        if class_declares(class, declaration) {
            return true;
        }
        crate::class_mro_view(py, class)
            .iter()
            .copied()
            .any(|base| {
                crate::obj_from_bits(base)
                    .as_ptr()
                    .is_some_and(|base| class_declares(base, declaration))
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub(crate) enum ClassReferenceSlot {
    Name = 0,
    Dictionary = 1,
    Bases = 2,
    Mro = 3,
    Qualname = 7,
    SlotDeclaration = 5,
    FieldLayout = 6,
    /// Static types have generic object attributes separate from their type
    /// namespace. Heap types alias Dictionary and leave this owner empty.
    InstanceDictionary = 11,
    /// Exact immutable creation-time documentation, independent of __doc__.
    CreationDoc = 12,
}

/// Capture the internal doc once at class birth. Store an exact string, not
/// the possibly callback-bearing str subclass from the public namespace. A NUL
/// terminates CPython's copied tp_doc; absence is a captured None, never a later
/// invitation to read mutable __doc__ again.
pub(crate) unsafe fn class_capture_creation_doc(py: &PyToken<'_>, class: *mut u8) -> bool {
    unsafe {
        if ClassReferenceSlot::CreationDoc.load(class) != 0 {
            return true;
        }
        let Some(dictionary) = crate::obj_from_bits(crate::class_dict_bits(class)).as_ptr() else {
            return false;
        };
        let doc = crate::object::ops::dict_get_str_bytes_borrowed(py, dictionary, b"__doc__");
        if crate::exception_pending(py) {
            return false;
        }
        let captured = if let Some(doc) = doc
            .and_then(|bits| crate::obj_from_bits(bits).as_ptr())
            .filter(|ptr| crate::object_type_id(*ptr) == crate::TYPE_ID_STRING)
        {
            if !crate::object::ops_string::require_strict_utf8(py, MoltObject::from_ptr(doc).bits())
            {
                return false;
            }
            let bytes =
                std::slice::from_raw_parts(crate::string_bytes(doc), crate::string_len(doc));
            let end = bytes
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(bytes.len());
            let copy = crate::alloc_string(py, &bytes[..end]);
            if copy.is_null() {
                return false;
            }
            MoltObject::from_ptr(copy).bits()
        } else {
            MoltObject::none().bits()
        };
        ClassReferenceSlot::CreationDoc.initialize_owned(class, captured);
        true
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum ClassReferenceRelease {
    Cycle,
    Terminal,
}

impl ClassReferenceSlot {
    pub(crate) const ALL: [Self; 9] = [
        Self::Name,
        Self::Bases,
        Self::Mro,
        Self::Qualname,
        Self::Dictionary,
        Self::SlotDeclaration,
        Self::FieldLayout,
        Self::InstanceDictionary,
        Self::CreationDoc,
    ];

    const DICTIONARIES: [Self; 2] = [Self::Dictionary, Self::InstanceDictionary];

    /// A class may be cleared before an instance in the same cyclic isolate.
    /// Keep physical storage and non-cyclic identity alive until the last
    /// instance releases its class edge. MRO owns the class itself; mutable
    /// namespaces and annotation callbacks can close arbitrary cycles.
    const fn released_by(self, phase: ClassReferenceRelease) -> bool {
        match self {
            Self::Mro | Self::Dictionary | Self::InstanceDictionary => true,
            Self::Name
            | Self::Qualname
            | Self::Bases
            | Self::SlotDeclaration
            | Self::FieldLayout
            | Self::CreationDoc => {
                matches!(phase, ClassReferenceRelease::Terminal)
            }
        }
    }

    #[inline]
    unsafe fn pointer(self, ptr: *mut u8) -> *mut u64 {
        unsafe { ptr.cast::<u64>().add(self as usize) }
    }

    /// `ptr` must address a live class payload and its caller must have read
    /// custody. No Rust borrow is held across reference release or callbacks.
    #[inline]
    pub(crate) unsafe fn load(self, ptr: *mut u8) -> u64 {
        unsafe { *self.pointer(ptr) }
    }

    /// Initialize an unpublished field, consuming one owned reference. Its
    /// storage must not already contain an owned value.
    #[inline]
    pub(crate) unsafe fn initialize_owned(self, ptr: *mut u8, bits: u64) {
        unsafe { self.pointer(ptr).write(bits) };
    }

    /// Transfer `bits` into the field and return its displaced owned reference.
    /// The caller must hold the GIL and release the returned edge, including
    /// when old and new identities are equal (they are two ownership claims).
    #[inline]
    pub(crate) unsafe fn exchange_owned(self, ptr: *mut u8, bits: u64) -> u64 {
        crate::gil_assert();
        unsafe { self.pointer(ptr).replace(bits) }
    }

    #[inline]
    pub(crate) unsafe fn replace_borrowed(self, py: &PyToken<'_>, ptr: *mut u8, bits: u64) {
        crate::gil_assert();
        if unsafe { self.load(ptr) } == bits {
            return;
        }
        inc_ref_bits(py, bits);
        let old = unsafe { self.exchange_owned(ptr, bits) };
        dec_ref_bits(py, old);
    }

    #[inline]
    pub(crate) unsafe fn take(self, ptr: *mut u8) -> u64 {
        unsafe { self.exchange_owned(ptr, MoltObject::none().bits()) }
    }
}

/// Physical dictionary for the generic object attribute primitive. Type lookup
/// and tp_dict continue to use Dictionary regardless of semantic origin. No
/// allocation, namespace publication, or cache invalidation occurs here.
pub(crate) unsafe fn class_generic_dict_bits_ptr(class: *mut u8) -> *mut u64 {
    unsafe {
        assert_eq!(crate::object_type_id(class), crate::TYPE_ID_TYPE);
        let slot = if class_is_heap_type(class) {
            ClassReferenceSlot::Dictionary
        } else {
            ClassReferenceSlot::InstanceDictionary
        };
        slot.pointer(class)
    }
}

/// Publish the phase's complete empty state before returning any owned edge.
/// Traversal, GC clear and terminal destruction share the same slot authority;
/// cycle collection must not retire metadata needed to destroy live instances.
pub(crate) unsafe fn detach_class_references(
    ptr: *mut u8,
    phase: ClassReferenceRelease,
) -> [u64; ClassReferenceSlot::ALL.len()] {
    let detached = ClassReferenceSlot::ALL.map(|slot| {
        if slot.released_by(phase) {
            unsafe { slot.take(ptr) }
        } else {
            MoltObject::none().bits()
        }
    });
    // Retired methods must not remain callable through class/attribute caches.
    // No callbacks can run until the caller releases the detached references.
    if detached
        .iter()
        .any(|&bits| bits != 0 && !crate::obj_from_bits(bits).is_none())
    {
        unsafe { super::layout::class_bump_layout_version(ptr) };
    }
    detached
}

/// Clear callback-bearing class contents while preserving its name, metaclass,
/// hierarchy and physical layout. Declaration/cache facts commit before any
/// displaced value can reenter Python. A callback may repopulate the class;
/// the owning retirement transaction must reach quiescence before identity
/// detachment, not assume a single pass is terminal.
pub(crate) unsafe fn clear_class_runtime_contents(py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        // Empty every physical dictionary before releasing any displaced
        // entry: namespace and generic-dictionary callbacks may observe or
        // repopulate either owner during the same retirement transaction.
        let dictionaries = ClassReferenceSlot::DICTIONARIES.map(|slot| {
            crate::obj_from_bits(slot.load(ptr)).as_ptr().map(|dict| {
                assert_eq!(crate::object_type_id(dict), crate::TYPE_ID_DICT);
                crate::object::ops::dict_clear_deferred(py, dict)
                    .expect("runtime class dictionary must be mutable storage")
            })
        });
        super::class_refresh_declared_finalizer_flag(py, ptr);
        super::layout::class_bump_layout_version(ptr);
        drop(dictionaries);
    }
}

pub(crate) unsafe fn class_runtime_contents_empty(ptr: *mut u8) -> bool {
    unsafe {
        ClassReferenceSlot::DICTIONARIES.into_iter().all(|slot| {
            crate::obj_from_bits(slot.load(ptr))
                .as_ptr()
                .is_none_or(|dict| crate::dict_len(dict) == 0)
        })
    }
}

/// Eligibility is used only on declared runtime-owned class anchor surfaces,
/// never as a reason to retire arbitrary immutable classes found in the heap.
pub(crate) fn is_canonical_runtime_class(py: &PyToken<'_>, bits: u64) -> bool {
    crate::obj_from_bits(bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe {
            crate::object_type_id(ptr) == crate::TYPE_ID_TYPE
                && crate::object_class_bits(ptr) == crate::builtin_classes(py).type_obj
                && (crate::is_builtin_class_bits(py, bits)
                    || class_declares(ptr, ClassDeclaration::BuiltinException)
                    || super::class_is_immutable(py, ptr))
        })
}

struct RetiringClass {
    bits: u64,
    owner: crate::PtrDropGuard,
}

/// Pins the complete canonical class cohort across callback-capable teardown.
/// The ordinary RC/GC lifecycle of user classes is deliberately not intercepted.
pub(crate) struct RuntimeClassRetirement {
    classes: Vec<RetiringClass>,
}

impl RuntimeClassRetirement {
    pub(crate) fn new() -> Self {
        Self {
            classes: Vec::new(),
        }
    }

    pub(crate) fn include(
        &mut self,
        py: &PyToken<'_>,
        roots: impl IntoIterator<Item = u64>,
    ) -> bool {
        let mut added = Vec::new();
        for bits in roots {
            if bits == 0
                || crate::obj_from_bits(bits).is_none()
                || self.classes.iter().any(|class| class.bits == bits)
            {
                continue;
            }
            assert!(
                is_canonical_runtime_class(py, bits),
                "noncanonical class entered runtime retirement: bits={bits:#x}, \
                 builtin={}, object_header(type_id, metaclass, immutable)={:?}",
                crate::is_builtin_class_bits(py, bits),
                crate::obj_from_bits(bits).as_ptr().map(|ptr| unsafe {
                    (
                        crate::object_type_id(ptr),
                        crate::object_class_bits(ptr),
                        super::class_is_immutable(py, ptr),
                    )
                })
            );
            let ptr = crate::obj_from_bits(bits)
                .as_ptr()
                .expect("canonical class pointer");
            assert!(
                !unsafe { super::object_has_finalizer(py, ptr) },
                "canonical runtime class acquired a metaclass finalizer"
            );
            // Allocate before claiming the new owner. Existing pins are RAII
            // roots even if a later validation or callback fails.
            self.classes.reserve(1);
            added.reserve(1);
            inc_ref_bits(py, bits);
            self.classes.push(RetiringClass {
                bits,
                owner: crate::PtrDropGuard::new(ptr),
            });
            unsafe { super::class_begin_runtime_retirement(ptr) };
            added.push(crate::PtrSlot(ptr));
        }
        // Admission is closed for the whole added cohort before any callback.
        // The canonical weakref primitive nulls all target links before invoking
        // surviving callbacks, without holding registry locks across Python.
        let changed = !added.is_empty();
        if changed {
            super::weakref::weakref_handle_cycle_unreachable(py, &added, |_| false);
        }
        changed
    }

    pub(crate) fn clear_contents(&self, py: &PyToken<'_>) -> bool {
        let mut changed = false;
        for class in &self.classes {
            let ptr = crate::obj_from_bits(class.bits).as_ptr().unwrap();
            if !unsafe { class_runtime_contents_empty(ptr) } {
                changed = true;
                unsafe { clear_class_runtime_contents(py, ptr) };
            }
        }
        changed
    }

    /// Retire C aliases only after both thread-state owner domains have drained.
    /// Native owners and saved C errors cannot be indexed through ManagedView;
    /// their ordinary release must finish before any C identity is invalidated.
    pub(crate) fn retire_projections(&self) -> bool {
        if crate::object::gc::gc_has_live_native_nodes()
            || molt_cpython_abi::api::object::runtime_retained_thread_state_count() != 0
        {
            return false;
        }
        let roots: Vec<_> = self.classes.iter().map(|class| class.bits).collect();
        molt_cpython_abi::bridge::GLOBAL_BRIDGE.retire_runtime_type_views(&roots)
    }

    pub(crate) fn contents_empty(&self) -> bool {
        self.classes.iter().all(|class| unsafe {
            class_runtime_contents_empty(crate::obj_from_bits(class.bits).as_ptr().unwrap())
        })
    }

    pub(crate) fn detach_identities(&self, py: &PyToken<'_>) {
        assert!(
            self.contents_empty(),
            "class contents survived final callback drain"
        );
        // Validate the whole remaining graph before removing even one identity.
        // Only exact non-callback metadata and pinned cohort edges may reach
        // this tail; immutability by itself says nothing about owned referents.
        for class in &self.classes {
            let ptr = crate::obj_from_bits(class.bits).as_ptr().unwrap();
            let metaclass = unsafe { crate::object_class_bits(ptr) };
            assert!(
                self.contains(metaclass),
                "retiring metaclass is outside the pinned cohort"
            );
            for slot in ClassReferenceSlot::ALL {
                let bits = unsafe { slot.load(ptr) };
                if bits == 0 || crate::obj_from_bits(bits).is_none() {
                    continue;
                }
                unsafe {
                    match slot {
                        ClassReferenceSlot::Name
                        | ClassReferenceSlot::Qualname
                        | ClassReferenceSlot::CreationDoc => {
                            self.assert_exact_metadata(py, bits, crate::TYPE_ID_STRING);
                        }
                        ClassReferenceSlot::Dictionary | ClassReferenceSlot::InstanceDictionary => {
                            let dict = self.assert_exact_metadata(py, bits, crate::TYPE_ID_DICT);
                            assert!(crate::dict_len(dict) == 0);
                        }
                        ClassReferenceSlot::Bases | ClassReferenceSlot::Mro => {
                            let tuple = self.assert_exact_metadata(py, bits, crate::TYPE_ID_TUPLE);
                            super::seq_access::with_borrowed(tuple, |bases| {
                                for &base in bases {
                                    assert!(
                                        self.contains(base),
                                        "retiring hierarchy escapes pinned cohort"
                                    );
                                }
                            });
                        }
                        ClassReferenceSlot::SlotDeclaration => {
                            self.assert_exact_metadata(py, bits, crate::TYPE_ID_TUPLE);
                            let (names, _) = super::layout::slot_record_parts(bits);
                            if !crate::obj_from_bits(names).is_none() {
                                let names =
                                    self.assert_exact_metadata(py, names, crate::TYPE_ID_TUPLE);
                                super::seq_access::with_immutable_tuple_slice(names, |names| {
                                    for &name in names {
                                        self.assert_exact_metadata(py, name, crate::TYPE_ID_STRING);
                                    }
                                })
                                .expect("retiring slot names must be an exact tuple");
                            }
                        }
                        ClassReferenceSlot::FieldLayout => {
                            self.assert_exact_metadata(py, bits, crate::TYPE_ID_TUPLE);
                            let (map, rows) = super::class_layout::projection_parts(bits);
                            if !crate::obj_from_bits(map).is_none() {
                                let dict = self.assert_exact_metadata(py, map, crate::TYPE_ID_DICT);
                                for row in crate::dict_live_entries(dict) {
                                    let pair = [row.key, row.value];
                                    self.assert_exact_metadata(py, pair[0], crate::TYPE_ID_STRING);
                                    assert!(crate::obj_from_bits(pair[1]).as_int().is_some());
                                }
                            }
                            let rows = self.assert_exact_metadata(py, rows, crate::TYPE_ID_TUPLE);
                            super::seq_access::with_immutable_tuple_slice(rows, |rows| {
                                assert_eq!(rows.len() % 3, 0);
                                for row in rows.as_chunks::<3>().0 {
                                    let field = super::class_layout::decode_row(row);
                                    if field.kind.is_intrinsic() {
                                        assert!(crate::obj_from_bits(field.name).is_none());
                                    } else {
                                        self.assert_exact_metadata(
                                            py,
                                            field.name,
                                            crate::TYPE_ID_STRING,
                                        );
                                    }
                                }
                            })
                            .expect("retiring physical rows must be a tuple");
                        }
                    }
                }
            }
        }
        // Managed views and all incoming projection aliases were retired in
        // the callback-capable fixed point, before reaching this sealed tail.
        assert!(
            !crate::object::gc::gc_has_live_native_nodes(),
            "native owners survived the last callback drain before class retirement"
        );
        assert_eq!(
            molt_cpython_abi::api::object::runtime_retained_thread_state_count(),
            0,
            "C thread-state owners survived the last callback drain before class retirement"
        );
        for class in &self.classes {
            assert!(
                !molt_cpython_abi::bridge::GLOBAL_BRIDGE.has_managed_type_view(class.bits),
                "managed class projection survived the shutdown callback drain"
            );
        }
        for class in &self.classes {
            crate::class_break_cycles(py, class.bits);
        }
    }

    fn contains(&self, bits: u64) -> bool {
        self.classes.iter().any(|class| class.bits == bits)
    }

    unsafe fn assert_exact_metadata(&self, py: &PyToken<'_>, bits: u64, type_id: u32) -> *mut u8 {
        let ptr = crate::obj_from_bits(bits)
            .as_ptr()
            .expect("runtime class metadata must be heap-backed");
        unsafe {
            assert_eq!(
                crate::object_type_id(ptr),
                type_id,
                "unexpected runtime class metadata representation"
            );
            let class = crate::object_class_bits(ptr);
            let builtins = crate::builtin_classes(py);
            let expected = match type_id {
                crate::TYPE_ID_STRING => builtins.str,
                crate::TYPE_ID_TUPLE => builtins.tuple,
                crate::TYPE_ID_DICT => builtins.dict,
                _ => unreachable!("unknown class metadata representation"),
            };
            // Bootstrap-owned exact primitives can have no explicit class edge.
            assert!(
                class == 0 || class == expected,
                "runtime class metadata acquired a callback-capable subclass"
            );
            assert!(
                !super::object_has_finalizer(py, ptr),
                "runtime class metadata acquired a finalizer"
            );
        }
        ptr
    }

    pub(crate) fn release_pins(mut self, py: &PyToken<'_>) {
        for class in &mut self.classes {
            // The callback-free tail already owns the GIL; do not reenter a
            // public ABI decref wrapper and acquire a fresh thread-state record.
            class.owner.release();
            dec_ref_bits(py, class.bits);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::functions::{alloc_runtime_function_obj, runtime_fn_addr};
    use std::sync::atomic::{AtomicU64, Ordering};

    static OBSERVED_CLASS: AtomicU64 = AtomicU64::new(0);
    static EXPECTED_ANNOTATIONS: AtomicU64 = AtomicU64::new(0);
    static CALLBACK_OBSERVATIONS: AtomicU64 = AtomicU64::new(0);

    // Observe storage without materializing missing annotations during retirement.
    fn namespace_annotations(py: &PyToken<'_>, class: *mut u8) -> u64 {
        unsafe {
            let dict = crate::obj_from_bits(crate::class_dict_bits(class))
                .as_ptr()
                .unwrap();
            crate::object::ops::dict_get_str_bytes_borrowed(py, dict, b"__annotations__")
                .or_else(|| {
                    crate::object::ops::dict_get_str_bytes_borrowed(
                        py,
                        dict,
                        b"__annotations_cache__",
                    )
                })
                .unwrap_or(0)
        }
    }

    fn set_annotations(py: &PyToken<'_>, class: u64, value: Option<u64>) {
        let name = crate::attr_name_bits_from_bytes(py, b"__annotations__").unwrap();
        match value {
            Some(value) => {
                crate::molt_set_attr_name(class, name, value);
            }
            None => {
                crate::molt_del_attr_name(class, name);
            }
        }
        dec_ref_bits(py, name);
        assert!(!crate::exception_pending(py));
    }

    extern "C" fn inspect_replacement(_self_bits: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let owner = crate::obj_from_bits(OBSERVED_CLASS.load(Ordering::SeqCst))
                .as_ptr()
                .expect("test class remains pinned");
            let expected = EXPECTED_ANNOTATIONS.load(Ordering::SeqCst);
            let published = namespace_annotations(py, owner);
            let name = unsafe { ClassReferenceSlot::Name.load(owner) };
            let metaclass = unsafe { crate::object_class_bits(owner) };
            if published == expected
                && (expected != 0 || unsafe { class_runtime_contents_empty(owner) })
                && name != 0
                && !crate::obj_from_bits(name).is_none()
                && metaclass == crate::builtin_classes(py).type_obj
                && unsafe { ClassReferenceSlot::Mro.load(owner) } != MoltObject::none().bits()
            {
                CALLBACK_OBSERVATIONS.fetch_add(1, Ordering::SeqCst);
            }
            MoltObject::none().bits()
        })
    }

    fn user_class(py: &PyToken<'_>, name: &[u8]) -> u64 {
        let name = crate::attr_name_bits_from_bytes(py, name).unwrap();
        let class = crate::molt_class_new(name);
        crate::molt_class_set_base(class, crate::builtin_classes(py).object);
        let ptr = crate::obj_from_bits(class).as_ptr().unwrap();
        unsafe { crate::object::class_finish_definition(py, ptr) }.expect("seal test class");
        dec_ref_bits(py, name);
        assert!(!crate::exception_pending(py));
        class
    }

    fn callback_class(py: &PyToken<'_>) -> u64 {
        let class = user_class(py, b"ClassReferenceReleaseObserver");
        let key = crate::attr_name_bits_from_bytes(py, b"__del__").unwrap();
        let function = alloc_runtime_function_obj(
            py,
            runtime_fn_addr(
                "class_reference_release_observer",
                inspect_replacement as *const (),
            ),
            1,
        );
        assert!(!function.is_null());
        let function = MoltObject::from_ptr(function).bits();
        crate::molt_set_attr_name(class, key, function);
        dec_ref_bits(py, function);
        dec_ref_bits(py, key);
        assert!(!crate::exception_pending(py));
        class
    }

    fn callback_instance(py: &PyToken<'_>, class: u64) -> u64 {
        let class_ptr = crate::obj_from_bits(class)
            .as_ptr()
            .expect("callback class");
        let instance = unsafe { crate::alloc_instance_for_class(py, class_ptr) };
        assert!(crate::obj_from_bits(instance).as_ptr().is_some());
        instance
    }

    #[test]
    fn class_replacement_retains_incoming_alias_before_displaced_dictionary_finalizes() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let owner = user_class(py, b"ClassReferenceReplacementOwner");
            let owner_ptr = crate::obj_from_bits(owner).as_ptr().unwrap();
            let observer_class = callback_class(py);
            let observer = callback_instance(py, observer_class);
            let incoming_ptr = crate::alloc_dict_with_pairs(py, &[]);
            assert!(!incoming_ptr.is_null());
            let incoming = MoltObject::from_ptr(incoming_ptr).bits();
            let key = MoltObject::from_int(1).bits();
            let old_ptr = crate::alloc_dict_with_pairs(
                py,
                &[key, incoming, MoltObject::from_int(2).bits(), observer],
            );
            assert!(!old_ptr.is_null());
            let old = MoltObject::from_ptr(old_ptr).bits();
            set_annotations(py, owner, Some(old));
            dec_ref_bits(py, old);
            dec_ref_bits(py, observer);
            dec_ref_bits(py, incoming); // now borrowed only through the outgoing dictionary
            OBSERVED_CLASS.store(owner, Ordering::SeqCst);
            EXPECTED_ANNOTATIONS.store(incoming, Ordering::SeqCst);
            CALLBACK_OBSERVATIONS.store(0, Ordering::SeqCst);
            set_annotations(py, owner, Some(incoming));
            assert_eq!(CALLBACK_OBSERVATIONS.load(Ordering::SeqCst), 1);
            assert_eq!(namespace_annotations(py, owner_ptr), incoming);
            assert_eq!(
                unsafe { (*crate::header_from_obj_ptr(incoming_ptr)).ref_count_snapshot() },
                1
            );
            set_annotations(py, owner, None);
            dec_ref_bits(py, owner);
            dec_ref_bits(py, observer_class);
            OBSERVED_CLASS.store(0, Ordering::SeqCst);
        });
    }

    #[test]
    fn class_content_retirement_keeps_identity_live_for_namespace_and_annotation_callbacks() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let owner = user_class(py, b"ClassContentRetirementOwner");
            let owner_ptr = crate::obj_from_bits(owner).as_ptr().unwrap();
            let observer_class = callback_class(py);
            let annotation = callback_instance(py, observer_class);
            let namespace = callback_instance(py, observer_class);
            let key = crate::attr_name_bits_from_bytes(py, b"payload").unwrap();
            crate::molt_set_attr_name(owner, key, namespace);
            set_annotations(py, owner, Some(annotation));
            dec_ref_bits(py, annotation);
            dec_ref_bits(py, namespace);
            // Static generic storage must empty with the namespace before
            // either dictionary releases its callback-bearing entries.
            assert!(unsafe { ClassSemanticPolicy::static_type(true).apply(py, owner_ptr) });
            let generic = callback_instance(py, observer_class);
            let retired = unsafe {
                crate::object::field_storage::set_item_deferred(py, owner_ptr, key, generic)
            }
            .expect("static generic dictionary insertion");
            drop(retired);
            dec_ref_bits(py, generic);
            dec_ref_bits(py, key);
            OBSERVED_CLASS.store(owner, Ordering::SeqCst);
            EXPECTED_ANNOTATIONS.store(0, Ordering::SeqCst);
            CALLBACK_OBSERVATIONS.store(0, Ordering::SeqCst);
            unsafe { clear_class_runtime_contents(py, owner_ptr) };
            assert_eq!(CALLBACK_OBSERVATIONS.load(Ordering::SeqCst), 3);
            assert!(unsafe { class_runtime_contents_empty(owner_ptr) });
            assert!(!crate::exception_pending(py));
            dec_ref_bits(py, owner);
            dec_ref_bits(py, observer_class);
            OBSERVED_CLASS.store(0, Ordering::SeqCst);
        });
    }

    #[test]
    fn class_generic_dictionary_aliases_only_heap_namespace_and_owns_each_edge_once() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let key = crate::attr_name_bits_from_bytes(py, b"generic_payload").unwrap();
                for is_static in [false, true] {
                    let owner = user_class(py, b"GenericDictionaryOwner");
                    let ptr = crate::obj_from_bits(owner).as_ptr().unwrap();
                    if is_static {
                        assert!(ClassSemanticPolicy::static_type(true).apply(py, ptr));
                    }
                    let namespace = ClassReferenceSlot::Dictionary.load(ptr);
                    assert_eq!(ClassReferenceSlot::InstanceDictionary.load(ptr), 0);
                    assert_eq!(
                        class_generic_dict_bits_ptr(ptr),
                        if is_static {
                            ClassReferenceSlot::InstanceDictionary.pointer(ptr)
                        } else {
                            ClassReferenceSlot::Dictionary.pointer(ptr)
                        }
                    );
                    assert_eq!(
                        crate::object::instance_dict_bits_ptr(ptr),
                        class_generic_dict_bits_ptr(ptr)
                    );
                    let version = crate::class_layout_version_bits(ptr);
                    let value = MoltObject::from_int(71).bits();
                    let retired =
                        crate::object::field_storage::set_item_deferred(py, ptr, key, value)
                            .expect("generic class storage insertion");
                    drop(retired);
                    assert_eq!(crate::class_layout_version_bits(ptr), version);
                    let dictionary = crate::object::instance_dict_bits(ptr);
                    assert_eq!(dictionary == namespace, !is_static);
                    let namespace_ptr = crate::obj_from_bits(namespace).as_ptr().unwrap();
                    assert_eq!(
                        crate::dict_get_in_place(py, namespace_ptr, key),
                        (!is_static).then_some(value)
                    );
                    let result = crate::molt_object_getattribute(owner, key);
                    assert_eq!(result, value);
                    dec_ref_bits(py, result);
                    assert!(!crate::exception_pending(py));

                    let dictionary_ptr = crate::obj_from_bits(dictionary).as_ptr().unwrap();
                    let mut edges: Vec<*mut u8> = Vec::new();
                    crate::object::heap_lifecycle::visit_owned_edges(py, ptr, &mut |child| {
                        edges.push(child)
                    });
                    assert_eq!(
                        edges
                            .iter()
                            .filter(|&&child| child == namespace_ptr)
                            .count(),
                        1
                    );
                    assert_eq!(
                        edges
                            .iter()
                            .filter(|&&child| child == dictionary_ptr)
                            .count(),
                        1
                    );
                    inc_ref_bits(py, dictionary);
                    let count = (*crate::header_from_obj_ptr(dictionary_ptr)).ref_count_snapshot();
                    assert_eq!(
                        crate::object::heap_lifecycle::try_clear_cycle_edges(py, ptr),
                        0
                    );
                    assert_eq!(
                        (*crate::header_from_obj_ptr(dictionary_ptr)).ref_count_snapshot(),
                        count - 1
                    );
                    assert_eq!(crate::object::instance_dict_bits(ptr), 0);
                    assert_eq!(
                        crate::object::heap_lifecycle::try_clear_cycle_edges(py, ptr),
                        0
                    );
                    assert_eq!(
                        (*crate::header_from_obj_ptr(dictionary_ptr)).ref_count_snapshot(),
                        count - 1
                    );
                    dec_ref_bits(py, owner);
                    assert_eq!(
                        (*crate::header_from_obj_ptr(dictionary_ptr)).ref_count_snapshot(),
                        count - 1
                    );
                    dec_ref_bits(py, dictionary);
                }
                dec_ref_bits(py, key);
                assert!(!crate::exception_pending(py));
            }
        });
    }

    #[test]
    fn class_reference_family_detaches_every_owned_slot_without_touching_policy() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let mut words = [0xA55A_u64; super::super::layout::CLASS_PAYLOAD_WORDS];
            for (index, slot) in ClassReferenceSlot::ALL.into_iter().enumerate() {
                words[slot as usize] = MoltObject::from_int(index as i64).bits();
            }
            let ptr = words.as_mut_ptr().cast::<u8>();
            let old = unsafe { detach_class_references(ptr, ClassReferenceRelease::Terminal) };
            for (index, slot) in ClassReferenceSlot::ALL.into_iter().enumerate() {
                assert_eq!(old[index], MoltObject::from_int(index as i64).bits());
                assert_eq!(unsafe { slot.load(ptr) }, MoltObject::none().bits());
            }
            assert_eq!(words[4], 0xA55A + 1);
            for index in [8, 9, super::super::layout::CLASS_DECLARATIONS_WORD] {
                assert_eq!(words[index], 0xA55A);
            }
        });
    }

    #[test]
    fn class_reference_replacement_preserves_self_alias_and_transfers_distinct_owners() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            // Exact class payload storage isolates edge ownership from the
            // semantic validation performed by each public class attribute.
            let mut words = [0_u64; super::super::layout::CLASS_PAYLOAD_WORDS];
            let ptr = words.as_mut_ptr().cast::<u8>();
            let value_ptr = crate::alloc_dict_with_pairs(py, &[]);
            assert!(!value_ptr.is_null());
            let value = MoltObject::from_ptr(value_ptr).bits();
            let count = || unsafe { (*crate::header_from_obj_ptr(value_ptr)).ref_count_snapshot() };
            let initial = count();
            for slot in ClassReferenceSlot::ALL {
                unsafe { slot.replace_borrowed(py, ptr, value) };
                assert_eq!(count(), initial + 1);
                unsafe { slot.replace_borrowed(py, ptr, value) };
                assert_eq!(count(), initial + 1, "same-identity replacement is neutral");
                inc_ref_bits(py, value);
                let displaced = unsafe { slot.exchange_owned(ptr, value) };
                dec_ref_bits(py, displaced);
                assert_eq!(
                    count(),
                    initial + 1,
                    "owned exchange consumes its input owner"
                );
                unsafe { slot.replace_borrowed(py, ptr, 0) };
                assert_eq!(count(), initial);
            }
            dec_ref_bits(py, value);
        });
    }
}
