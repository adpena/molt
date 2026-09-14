//! One authority for the owned reference fields in a class payload.
//!
//! Reference replacement publishes the new owner before releasing the old one.
//! Multi-field operations transfer displaced references to the caller so that
//! every field can be published before the first callback-capable release.

use crate::{MoltObject, PyToken, dec_ref_bits, inc_ref_bits};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub(crate) enum ClassReferenceSlot {
    Name = 0,
    Dictionary = 1,
    Bases = 2,
    Mro = 3,
    Annotations = 5,
    Annotate = 6,
    Qualname = 7,
    SlotDeclaration = 10,
    FieldOffsets = 11,
}

impl ClassReferenceSlot {
    pub(crate) const ALL: [Self; 9] = [
        Self::Name,
        Self::Bases,
        Self::Mro,
        Self::Annotations,
        Self::Annotate,
        Self::Qualname,
        Self::Dictionary,
        Self::SlotDeclaration,
        Self::FieldOffsets,
    ];

    /// `ptr` must address a live class payload and its caller must have read
    /// custody. No Rust borrow is held across reference release or callbacks.
    #[inline]
    pub(crate) unsafe fn load(self, ptr: *mut u8) -> u64 {
        unsafe { *ptr.cast::<u64>().add(self as usize) }
    }

    /// Initialize an unpublished field, consuming one owned reference. Its
    /// storage must not already contain an owned value.
    #[inline]
    pub(crate) unsafe fn initialize_owned(self, ptr: *mut u8, bits: u64) {
        unsafe { ptr.cast::<u64>().add(self as usize).write(bits) };
    }

    /// Transfer `bits` into the field and return its displaced owned reference.
    /// The caller must hold the GIL and release the returned edge, including
    /// when old and new identities are equal (they are two ownership claims).
    #[inline]
    pub(crate) unsafe fn exchange_owned(self, ptr: *mut u8, bits: u64) -> u64 {
        crate::gil_assert();
        unsafe { ptr.cast::<u64>().add(self as usize).replace(bits) }
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

/// Detach all class payload references before returning any for release. GC
/// and terminal destruction use exactly the same slot family as visitation.
pub(crate) unsafe fn detach_class_references(ptr: *mut u8) -> [u64; 9] {
    ClassReferenceSlot::ALL.map(|slot| unsafe { slot.take(ptr) })
}

/// Clear callback-bearing class contents while preserving its name, metaclass,
/// hierarchy and physical layout. Declaration/cache facts commit before any
/// displaced value can reenter Python. A callback may repopulate the class;
/// the owning retirement transaction must reach quiescence before identity
/// detachment, not assume a single pass is terminal.
pub(crate) unsafe fn clear_class_runtime_contents(py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        let annotations = ClassReferenceSlot::Annotations.exchange_owned(ptr, 0);
        let annotate = ClassReferenceSlot::Annotate.exchange_owned(ptr, 0);
        let dictionary = crate::obj_from_bits(ClassReferenceSlot::Dictionary.load(ptr))
            .as_ptr()
            .map(|dict| {
                assert_eq!(crate::object_type_id(dict), crate::TYPE_ID_DICT);
                crate::object::ops::dict_clear_deferred(py, dict)
                    .expect("runtime class namespace must be mutable storage")
            });
        super::class_refresh_declared_finalizer_flag(py, ptr);
        super::layout::class_bump_layout_version(ptr);
        drop(dictionary);
        dec_ref_bits(py, annotations);
        dec_ref_bits(py, annotate);
    }
}

pub(crate) unsafe fn class_runtime_contents_empty(ptr: *mut u8) -> bool {
    unsafe {
        [
            ClassReferenceSlot::Annotations,
            ClassReferenceSlot::Annotate,
        ]
        .into_iter()
        .all(|slot| {
            let bits = slot.load(ptr);
            bits == 0 || crate::obj_from_bits(bits).is_none()
        }) && crate::obj_from_bits(ClassReferenceSlot::Dictionary.load(ptr))
            .as_ptr()
            .is_none_or(|dict| crate::dict_order(dict).is_empty())
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
                && (crate::is_builtin_class_bits(py, bits) || super::class_is_immutable(py, ptr))
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
                "noncanonical class entered runtime retirement"
            );
            let ptr = crate::obj_from_bits(bits)
                .as_ptr()
                .expect("canonical class pointer");
            assert!(
                !unsafe { super::object_class_has_finalizer(py, ptr) },
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
                        ClassReferenceSlot::Name | ClassReferenceSlot::Qualname => {
                            self.assert_exact_metadata(py, bits, crate::TYPE_ID_STRING);
                        }
                        ClassReferenceSlot::Dictionary => {
                            let dict = self.assert_exact_metadata(py, bits, crate::TYPE_ID_DICT);
                            assert!(crate::dict_order(dict).is_empty());
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
                            let tuple = self.assert_exact_metadata(py, bits, crate::TYPE_ID_TUPLE);
                            super::seq_access::with_borrowed(tuple, |names| {
                                for &name in names {
                                    self.assert_exact_metadata(py, name, crate::TYPE_ID_STRING);
                                }
                            });
                        }
                        ClassReferenceSlot::FieldOffsets => {
                            let dict = self.assert_exact_metadata(py, bits, crate::TYPE_ID_DICT);
                            let entries = crate::dict_order(dict);
                            assert_eq!(entries.len() % 2, 0);
                            for pair in entries.chunks_exact(2) {
                                self.assert_exact_metadata(py, pair[0], crate::TYPE_ID_STRING);
                                assert!(
                                    crate::obj_from_bits(pair[1]).as_int().is_some(),
                                    "retiring field offset is not an exact immediate integer"
                                );
                            }
                        }
                        ClassReferenceSlot::Annotations | ClassReferenceSlot::Annotate => {
                            unreachable!("callback-bearing class metadata survived retirement");
                        }
                    }
                }
            }
        }
        for class in &self.classes {
            if let Some(view) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .retire_runtime_type_view_deferred(class.bits)
            {
                // C type pointers have interpreter lifetime, not process
                // lifetime. Detach their bridge identity even if C retained a
                // direct reference; that pointer is invalid after finalization.
                // Dropping the exact Type view clears HAS_ABI_VIEW without
                // releasing callback-bearing projection items. Then release
                // its stable runtime hold under our still-live cohort pin.
                drop(view);
                dec_ref_bits(py, class.bits);
            }
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
                !super::object_class_has_finalizer(py, ptr),
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

    extern "C" fn inspect_replacement(_self_bits: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let owner = crate::obj_from_bits(OBSERVED_CLASS.load(Ordering::SeqCst))
                .as_ptr()
                .expect("test class remains pinned");
            let expected = EXPECTED_ANNOTATIONS.load(Ordering::SeqCst);
            let published = unsafe { ClassReferenceSlot::Annotations.load(owner) };
            let name = unsafe { ClassReferenceSlot::Name.load(owner) };
            let metaclass = unsafe { crate::object_class_bits(owner) };
            if published == expected
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
            unsafe { ClassReferenceSlot::Annotations.replace_borrowed(py, owner_ptr, old) };
            dec_ref_bits(py, old);
            dec_ref_bits(py, observer);
            dec_ref_bits(py, incoming); // now borrowed only through the outgoing dictionary
            OBSERVED_CLASS.store(owner, Ordering::SeqCst);
            EXPECTED_ANNOTATIONS.store(incoming, Ordering::SeqCst);
            CALLBACK_OBSERVATIONS.store(0, Ordering::SeqCst);
            unsafe { ClassReferenceSlot::Annotations.replace_borrowed(py, owner_ptr, incoming) };
            assert_eq!(CALLBACK_OBSERVATIONS.load(Ordering::SeqCst), 1);
            assert_eq!(
                unsafe { ClassReferenceSlot::Annotations.load(owner_ptr) },
                incoming
            );
            assert_eq!(
                unsafe { (*crate::header_from_obj_ptr(incoming_ptr)).ref_count_snapshot() },
                1
            );
            unsafe { ClassReferenceSlot::Annotations.replace_borrowed(py, owner_ptr, 0) };
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
            unsafe { ClassReferenceSlot::Annotations.replace_borrowed(py, owner_ptr, annotation) };
            dec_ref_bits(py, annotation);
            dec_ref_bits(py, namespace);
            dec_ref_bits(py, key);
            OBSERVED_CLASS.store(owner, Ordering::SeqCst);
            EXPECTED_ANNOTATIONS.store(0, Ordering::SeqCst);
            CALLBACK_OBSERVATIONS.store(0, Ordering::SeqCst);
            unsafe { clear_class_runtime_contents(py, owner_ptr) };
            assert_eq!(CALLBACK_OBSERVATIONS.load(Ordering::SeqCst), 2);
            assert!(unsafe { class_runtime_contents_empty(owner_ptr) });
            assert!(!crate::exception_pending(py));
            dec_ref_bits(py, owner);
            dec_ref_bits(py, observer_class);
            OBSERVED_CLASS.store(0, Ordering::SeqCst);
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
            let old = unsafe { detach_class_references(ptr) };
            for (index, slot) in ClassReferenceSlot::ALL.into_iter().enumerate() {
                assert_eq!(old[index], MoltObject::from_int(index as i64).bits());
                assert_eq!(unsafe { slot.load(ptr) }, MoltObject::none().bits());
            }
            for index in [4, 8, 9] {
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
