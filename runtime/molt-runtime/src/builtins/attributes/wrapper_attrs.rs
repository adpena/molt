//! Wrapper members are ordinary native member/getset descriptors published in
//! the canonical builtin namespaces. There is no wrapper-specific lookup lane.
use super::*;
use crate::builtins::types::{
    NativeDescriptorFlavor, NativeDescriptorSpec, alloc_native_descriptor,
};
use crate::object::layout::{self, WrapperKind};

#[derive(Clone, Copy)]
enum MemberOperation {
    Reference(usize),
    Name,
    Abstract,
    Dictionary,
    LazyMetadata,
}

struct WrapperMember {
    name: &'static str,
    owners: &'static [WrapperKind],
    flavor: NativeDescriptorFlavor,
    writable: bool,
    minimum_minor: i64,
    operation: MemberOperation,
}

const FUNCTION_WRAPPERS: &[WrapperKind] = &[WrapperKind::Staticmethod, WrapperKind::Classmethod];
const PROPERTY_WRAPPER: &[WrapperKind] = &[WrapperKind::Property];

// One authority for publication, mutability, target gating and callback meaning.
const WRAPPER_MEMBERS: &[WrapperMember] = &[
    WrapperMember {
        name: "__func__",
        owners: FUNCTION_WRAPPERS,
        flavor: NativeDescriptorFlavor::Member,
        writable: false,
        minimum_minor: 0,
        operation: MemberOperation::Reference(0),
    },
    WrapperMember {
        name: "__wrapped__",
        owners: FUNCTION_WRAPPERS,
        flavor: NativeDescriptorFlavor::Member,
        writable: false,
        minimum_minor: 0,
        operation: MemberOperation::Reference(0),
    },
    WrapperMember {
        name: "__dict__",
        owners: FUNCTION_WRAPPERS,
        flavor: NativeDescriptorFlavor::GetSet,
        writable: true,
        minimum_minor: 0,
        operation: MemberOperation::Dictionary,
    },
    WrapperMember {
        name: "__isabstractmethod__",
        owners: FUNCTION_WRAPPERS,
        flavor: NativeDescriptorFlavor::GetSet,
        writable: false,
        minimum_minor: 0,
        operation: MemberOperation::Abstract,
    },
    WrapperMember {
        name: "__annotations__",
        owners: FUNCTION_WRAPPERS,
        flavor: NativeDescriptorFlavor::GetSet,
        writable: true,
        minimum_minor: 14,
        operation: MemberOperation::LazyMetadata,
    },
    WrapperMember {
        name: "__annotate__",
        owners: FUNCTION_WRAPPERS,
        flavor: NativeDescriptorFlavor::GetSet,
        writable: true,
        minimum_minor: 14,
        operation: MemberOperation::LazyMetadata,
    },
    WrapperMember {
        name: "fget",
        owners: PROPERTY_WRAPPER,
        flavor: NativeDescriptorFlavor::Member,
        writable: false,
        minimum_minor: 0,
        operation: MemberOperation::Reference(0),
    },
    WrapperMember {
        name: "fset",
        owners: PROPERTY_WRAPPER,
        flavor: NativeDescriptorFlavor::Member,
        writable: false,
        minimum_minor: 0,
        operation: MemberOperation::Reference(1),
    },
    WrapperMember {
        name: "fdel",
        owners: PROPERTY_WRAPPER,
        flavor: NativeDescriptorFlavor::Member,
        writable: false,
        minimum_minor: 0,
        operation: MemberOperation::Reference(2),
    },
    WrapperMember {
        name: "__doc__",
        owners: PROPERTY_WRAPPER,
        flavor: NativeDescriptorFlavor::Member,
        writable: true,
        minimum_minor: 0,
        operation: MemberOperation::Reference(3),
    },
    WrapperMember {
        name: "__name__",
        owners: PROPERTY_WRAPPER,
        flavor: NativeDescriptorFlavor::GetSet,
        writable: true,
        minimum_minor: 13,
        operation: MemberOperation::Name,
    },
    WrapperMember {
        name: "__isabstractmethod__",
        owners: PROPERTY_WRAPPER,
        flavor: NativeDescriptorFlavor::GetSet,
        writable: false,
        minimum_minor: 0,
        operation: MemberOperation::Abstract,
    },
];

impl WrapperMember {
    fn enabled(&self, major: i64, minor: i64) -> bool {
        self.minimum_minor == 0 || (major, minor) >= (3, self.minimum_minor)
    }
}

/// A prepared class-namespace transaction. Preparation performs all allocation
/// and metadata lookup; commit cannot call Python or fail. Until commit, live
/// dictionaries, class versions, and the publication receipt remain untouched.
pub(crate) struct WrapperMembersPublication {
    version_key: u64,
    staged_owners: Vec<PtrDropGuard>,
    publications: Vec<(*mut u8, *mut u8, *mut u8)>,
}

impl WrapperMembersPublication {
    pub(crate) fn commit(self, py: &PyToken<'_>) {
        unsafe {
            crate::gil_assert();
            for &(_, live, staged) in &self.publications {
                crate::object::ops::dict_publish_staged(py, live, staged);
            }
            for &(class, _, _) in &self.publications {
                class_bump_layout_version(class);
            }
            runtime_state(py)
                .attributes
                .wrapper_members_version
                .store(self.version_key, Ordering::Release);
            // This is the first release of displaced values; all namespaces,
            // derived class versions and target-publication state are coherent.
            drop(self.staged_owners);
        }
    }
}

/// Prepare the projection for a candidate target before the caller publishes
/// its sys.version_info/sys.version pair. No version parser lives here.
pub(crate) fn prepare_wrapper_members(
    py: &PyToken<'_>,
    major: i64,
    minor: i64,
) -> Option<WrapperMembersPublication> {
    let result = (|| unsafe {
        crate::gil_assert();
        if exception_pending(py) {
            return None;
        }
        // Bootstrap itself publishes the initial descriptor family. Resolve it
        // before reading the receipt so an outer first-use call cannot restage
        // already-published ungated identities.
        let builtins = builtin_classes(py);
        let attributes = &runtime_state(py).attributes;
        // The receipt fingerprints the table's enabled projection, not an
        // independently parsed/capped Python version. Every gate comes from the
        // same rows, so even unusually large version integers cannot collide.
        let version_key = WRAPPER_MEMBERS
            .iter()
            .enumerate()
            .fold(1u64, |key, (index, member)| {
                key | ((member.enabled(major, minor) as u64) << (index + 1))
            });
        let previous_version = attributes.wrapper_members_version.load(Ordering::Acquire);
        if previous_version == version_key {
            return Some(WrapperMembersPublication {
                version_key,
                staged_owners: Vec::new(),
                publications: Vec::new(),
            });
        }
        let mut callbacks = [0; 3];
        for (index, (slot, symbol, arity)) in [
            (
                &attributes.wrapper_member_get,
                fn_addr!(molt_wrapper_member_get),
                2,
            ),
            (
                &attributes.wrapper_member_set,
                fn_addr!(molt_wrapper_member_set),
                3,
            ),
            (
                &attributes.wrapper_member_delete,
                fn_addr!(molt_wrapper_member_delete),
                2,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let bits = crate::builtins::types::builtin_func_bits(py, slot, symbol, arity);
            if exception_pending(py) || obj_from_bits(bits).as_ptr().is_none() {
                if !exception_pending(py) {
                    raise_exception::<()>(
                        py,
                        "MemoryError",
                        "wrapper member callback allocation failed",
                    );
                }
                return None;
            }
            callbacks[index] = bits;
        }
        let [getter, setter, deleter] = callbacks;
        let mut staged_owners = Vec::new();
        let mut publications = Vec::new();
        for (kind, class_bits) in [
            (WrapperKind::Staticmethod, builtins.staticmethod),
            (WrapperKind::Classmethod, builtins.classmethod),
            (WrapperKind::Property, builtins.property),
        ] {
            let class = obj_from_bits(class_bits).as_ptr().unwrap();
            let dictionary = class_dict_bits(class);
            let live = obj_from_bits(dictionary).as_ptr().unwrap();
            let staged_bits = crate::object::ops_dict::molt_dict_copy(dictionary);
            let Some(staged) = obj_from_bits(staged_bits).as_ptr() else {
                return None;
            };
            staged_owners.push(PtrDropGuard::new(staged));
            if exception_pending(py) {
                return None;
            }
            for member in WRAPPER_MEMBERS
                .iter()
                .filter(|member| member.owners.contains(&kind))
            {
                // Ungated descriptors retain canonical identity across a target
                // version change; only their gated siblings need synchronization.
                if previous_version != 0 && member.minimum_minor == 0 {
                    continue;
                }
                let Some(name_bits) = attr_name_bits_from_bytes(py, member.name.as_bytes()) else {
                    return None;
                };
                let _name_owner = PtrDropGuard::new(obj_from_bits(name_bits).as_ptr().unwrap());
                let enabled = member.enabled(major, minor);
                if !enabled {
                    if let Some(existing) = dict_get_in_place(py, staged, name_bits)
                        && let Some(ptr) = obj_from_bits(existing).as_ptr()
                        && layout::native_descriptor_flavor(ptr).is_some()
                        && layout::native_descriptor_owner_bits(ptr) == class_bits
                        && layout::native_descriptor_getter_bits(ptr) == getter
                    {
                        dict_del_in_place(py, staged, name_bits);
                    }
                    if exception_pending(py) {
                        return None;
                    }
                    continue;
                }
                // Preserve identity for already-present version-gated members
                // whose admission did not change between the two versions.
                if previous_version != 0
                    && let Some(existing) = dict_get_in_place(py, staged, name_bits)
                    && let Some(ptr) = obj_from_bits(existing).as_ptr()
                    && layout::native_descriptor_flavor(ptr).is_some()
                    && layout::native_descriptor_owner_bits(ptr) == class_bits
                    && layout::native_descriptor_getter_bits(ptr) == getter
                {
                    continue;
                }
                if exception_pending(py) {
                    return None;
                }
                let descriptor = alloc_native_descriptor(
                    py,
                    NativeDescriptorSpec {
                        flavor: member.flavor,
                        owner: class_bits,
                        name: name_bits,
                        doc: MoltObject::none().bits(),
                        getter,
                        setter: if member.writable {
                            setter
                        } else {
                            MoltObject::none().bits()
                        },
                        deleter: if member.writable {
                            deleter
                        } else {
                            MoltObject::none().bits()
                        },
                    },
                );
                let Some(descriptor_ptr) = obj_from_bits(descriptor).as_ptr() else {
                    return None;
                };
                let _descriptor_owner = PtrDropGuard::new(descriptor_ptr);
                if exception_pending(py) {
                    return None;
                }
                dict_set_in_place(py, staged, name_bits, descriptor);
                if exception_pending(py) {
                    return None;
                }
            }
            publications.push((class, live, staged));
        }

        Some(WrapperMembersPublication {
            version_key,
            staged_owners,
            publications,
        })
    })();
    if result.is_none() && !exception_pending(py) {
        raise_exception::<()>(
            py,
            "MemoryError",
            "wrapper member publication preparation failed",
        );
    }
    result
}

/// Publish the current runtime target, or resynchronize after its admission.
/// Version setters use prepare_wrapper_members before changing their state.
#[must_use]
pub(crate) fn wrapper_publish_members(py: &PyToken<'_>) -> bool {
    let info = crate::object::ops_sys::runtime_target_python_info(runtime_state(py));
    let Some(publication) = prepare_wrapper_members(py, info.major, info.minor) else {
        return false;
    };
    publication.commit(py);
    !exception_pending(py)
}

/// The generic descriptor protocol validates the retained owner before this
/// callback. Resolve the operation from that same publication table; no MRO or
/// instance-storage precedence policy belongs here.
unsafe fn callback_member(
    descriptor: u64,
    instance: u64,
) -> Option<(&'static WrapperMember, *mut u8, u64)> {
    unsafe {
        let descriptor = obj_from_bits(descriptor).as_ptr()?;
        layout::native_descriptor_flavor(descriptor)?;
        let instance = obj_from_bits(instance).as_ptr()?;
        let kind = WrapperKind::from_type_id(object_type_id(instance))?;
        let name_bits = layout::native_descriptor_name_bits(descriptor);
        let name = string_obj_to_owned(obj_from_bits(name_bits))?;
        let member = WRAPPER_MEMBERS
            .iter()
            .find(|member| member.name == name && member.owners.contains(&kind))?;
        Some((member, instance, name_bits))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_wrapper_member_get(descriptor: u64, instance: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let Some((member, object, name)) = callback_member(descriptor, instance) else {
                return raise_exception::<u64>(py, "TypeError", "invalid wrapper member receiver");
            };
            let result = match member.operation {
                MemberOperation::Reference(index) => {
                    let bits = layout::wrapper_reference_bits(object, index);
                    let bits = if is_missing_bits(py, bits) {
                        MoltObject::none().bits()
                    } else {
                        bits
                    };
                    inc_ref_bits(py, bits);
                    Some(bits)
                }
                MemberOperation::Name => property_name_value(py, object),
                MemberOperation::Abstract => abstract_member(
                    py,
                    object,
                    WrapperKind::from_type_id(object_type_id(object)).unwrap(),
                ),
                MemberOperation::Dictionary => {
                    crate::object::field_storage::materialize(py, object).map(|bits| {
                        inc_ref_bits(py, bits);
                        bits
                    })
                }
                MemberOperation::LazyMetadata => lazy_wrapped_attribute(py, object, name),
            };
            result.unwrap_or_else(|| {
                if !exception_pending(py) {
                    raise_exception::<()>(
                        py,
                        "AttributeError",
                        &format!(
                            "'{}' object has no attribute '{}'",
                            class_name_for_error(object_class_bits(object)),
                            member.name
                        ),
                    );
                }
                MoltObject::none().bits()
            })
        }
    })
}

unsafe fn mutate_member(
    py: &PyToken<'_>,
    descriptor: u64,
    instance: u64,
    value: Option<u64>,
) -> u64 {
    unsafe {
        let Some((member, object, name)) = callback_member(descriptor, instance) else {
            return raise_exception::<u64>(py, "TypeError", "invalid wrapper member receiver");
        };
        if !member.writable {
            return raise_exception::<u64>(py, "AttributeError", "readonly attribute");
        }
        match member.operation {
            MemberOperation::Reference(3) => {
                if !layout::property_replace_doc_bits(
                    py,
                    object,
                    value.unwrap_or(MoltObject::none().bits()),
                ) && !exception_pending(py)
                {
                    raise_exception::<()>(py, "SystemError", "invalid property doc replacement");
                }
            }
            MemberOperation::Name => {
                if !layout::property_replace_name_bits(
                    py,
                    object,
                    value.unwrap_or_else(|| missing_bits(py)),
                ) && !exception_pending(py)
                {
                    raise_exception::<()>(py, "SystemError", "invalid property name replacement");
                }
            }
            MemberOperation::Dictionary => {
                let Some(value) = value else {
                    return raise_exception::<u64>(py, "TypeError", "cannot delete __dict__");
                };
                crate::object::field_storage::replace_dictionary(py, object, Some(value));
            }
            MemberOperation::LazyMetadata => {
                let Some(dictionary) = crate::object::field_storage::materialize(py, object) else {
                    return MoltObject::none().bits();
                };
                inc_ref_bits(py, dictionary);
                let dictionary = obj_from_bits(dictionary).as_ptr().unwrap();
                let _dictionary_owner = PtrDropGuard::new(dictionary);
                match value {
                    Some(value) => dict_set_in_place(py, dictionary, name, value),
                    None => {
                        if !dict_del_in_place(py, dictionary, name) && !exception_pending(py) {
                            raise_exception::<()>(
                                py,
                                "AttributeError",
                                &format!(
                                    "'{}' object has no attribute '{}'",
                                    class_name_for_error(object_class_bits(object)),
                                    member.name
                                ),
                            );
                        }
                    }
                }
            }
            _ => {
                return raise_exception::<u64>(
                    py,
                    "SystemError",
                    "invalid writable wrapper member",
                );
            }
        }
        MoltObject::none().bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_wrapper_member_set(descriptor: u64, instance: u64, value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe { mutate_member(py, descriptor, instance, Some(value)) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_wrapper_member_delete(descriptor: u64, instance: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe { mutate_member(py, descriptor, instance, None) }
    })
}

/// Ordinary Python lookup, with only AttributeError treated as absent. Pins the
/// target through callbacks; returned values are owned.
unsafe fn optional_attribute(py: &PyToken<'_>, target: u64, name: u64) -> Option<u64> {
    inc_ref_bits(py, target);
    let value = molt_get_attr_name(target, name);
    dec_ref_bits(py, target);
    if exception_pending(py) {
        dec_ref_bits(py, value);
        clear_attribute_error_if_pending(py);
        None
    } else {
        Some(value)
    }
}

/// Native property name query for both __name__ and descriptor diagnostics.
/// Does not invoke a property subclass's override. Some is an owned value,
/// including explicit None; None means absent or a pending callback exception.
pub(crate) unsafe fn property_name_value(py: &PyToken<'_>, object: *mut u8) -> Option<u64> {
    unsafe {
        let name = layout::property_name_bits(object);
        if !is_missing_bits(py, name) {
            inc_ref_bits(py, name);
            return Some(name);
        }
        let getter = property_get_bits(object);
        if obj_from_bits(getter).is_none() {
            return None;
        }
        let name = attr_name_bits_from_bytes(py, b"__name__")?;
        let result = optional_attribute(py, getter, name);
        dec_ref_bits(py, name);
        result
    }
}

unsafe fn abstract_member(py: &PyToken<'_>, object: *mut u8, kind: WrapperKind) -> Option<u64> {
    unsafe {
        let name = attr_name_bits_from_bytes(py, b"__isabstractmethod__")?;
        let count = if kind == WrapperKind::Property { 3 } else { 1 };
        let result = (|| {
            for index in 0..count {
                // Read each accessor after the prior callback, matching the
                // native descriptor's reentrant observation order.
                let target = layout::wrapper_reference_bits(object, index);
                if is_missing_bits(py, target) || obj_from_bits(target).is_none() {
                    continue;
                }
                if let Some(value) = optional_attribute(py, target, name) {
                    let truth = is_truthy(py, obj_from_bits(value));
                    dec_ref_bits(py, value);
                    if exception_pending(py) {
                        return None;
                    }
                    if truth {
                        return Some(MoltObject::from_bool(true).bits());
                    }
                } else if exception_pending(py) {
                    return None;
                }
            }
            Some(MoltObject::from_bool(false).bits())
        })();
        dec_ref_bits(py, name);
        result
    }
}

/// Materialize and pin the selected dictionary before calling into the target.
/// If the callback replaces __dict__, the in-flight cache still belongs to the
/// selected dictionary, not to a subsequently installed one.
unsafe fn lazy_wrapped_attribute(py: &PyToken<'_>, object: *mut u8, name: u64) -> Option<u64> {
    unsafe {
        let target = layout::wrapper_reference_bits(object, 0);
        inc_ref_bits(py, target);
        let result = (|| {
            let dictionary = crate::object::field_storage::materialize(py, object)?;
            inc_ref_bits(py, dictionary);
            let dictionary_ptr = obj_from_bits(dictionary).as_ptr().unwrap();
            let _dictionary_owner = PtrDropGuard::new(dictionary_ptr);
            if let Some(value) = dict_get_in_place(py, dictionary_ptr, name) {
                inc_ref_bits(py, value);
                return Some(value);
            }
            if exception_pending(py) {
                return None;
            }
            if is_missing_bits(py, target) {
                raise_exception::<()>(
                    py,
                    "RuntimeError",
                    "uninitialized descriptor wrapper object",
                );
                return None;
            }
            let value = molt_get_attr_name(target, name);
            if exception_pending(py) {
                dec_ref_bits(py, value);
                return None;
            }
            dict_set_in_place(py, dictionary_ptr, name, value);
            if exception_pending(py) {
                dec_ref_bits(py, value);
                return None;
            }
            Some(value)
        })();
        dec_ref_bits(py, target);
        result
    }
}

/// Constructor/reinitializer metadata policy for both function wrappers. Keep
/// existing dictionary entries not overwritten by the wrapped object's actual
/// attributes. Every read and write honors Python overrides and owns callbacks.
#[must_use]
pub(crate) unsafe fn wrapper_copy_metadata(py: &PyToken<'_>, object: u64, target: u64) -> bool {
    inc_ref_bits(py, object);
    inc_ref_bits(py, target);
    let result = (|| unsafe {
        let names: &[&[u8]] = if pep649_enabled(py) {
            &[b"__module__", b"__name__", b"__qualname__", b"__doc__"]
        } else {
            &[
                b"__module__",
                b"__name__",
                b"__qualname__",
                b"__doc__",
                b"__annotations__",
            ]
        };
        for name in names {
            let Some(name_bits) = attr_name_bits_from_bytes(py, name) else {
                return false;
            };
            if let Some(value) = optional_attribute(py, target, name_bits) {
                let result = molt_set_attr_name(object, name_bits, value);
                crate::call::discard_owned_call_result(py, result);
                dec_ref_bits(py, value);
            }
            dec_ref_bits(py, name_bits);
            if exception_pending(py) {
                return false;
            }
        }
        true
    })();
    dec_ref_bits(py, target);
    dec_ref_bits(py, object);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_is_staged_versioned_and_preserves_namespace_identity() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                prepare_wrapper_members(py, 3, 12).unwrap().commit(py);
                let classes = builtin_classes(py);
                let class = obj_from_bits(classes.staticmethod).as_ptr().unwrap();
                let dictionary = class_dict_bits(class);
                let pointer = obj_from_bits(dictionary).as_ptr().unwrap();
                let func_name = attr_name_bits_from_bytes(py, b"__func__").unwrap();
                let annotations_name = attr_name_bits_from_bytes(py, b"__annotations__").unwrap();
                let func = dict_get_in_place(py, pointer, func_name).unwrap();
                let object_class_name = attr_name_bits_from_bytes(py, b"__objclass__").unwrap();
                let member_name = attr_name_bits_from_bytes(py, b"__name__").unwrap();
                let read_name = molt_get_attr_name(func, member_name);
                let read_owner = molt_get_attr_name(func, object_class_name);
                assert!(!exception_pending(py));
                assert_eq!(read_name, func_name);
                assert_eq!(read_owner, classes.staticmethod);
                dec_ref_bits(py, read_name);
                dec_ref_bits(py, read_owner);
                molt_set_attr_name(func, member_name, MoltObject::none().bits());
                assert!(clear_attribute_error_if_pending(py));
                let class_read = crate::builtins::types::native_descriptor_get(
                    py,
                    func,
                    None,
                    Some(classes.staticmethod),
                );
                assert_eq!(class_read, func);
                dec_ref_bits(py, class_read);
                let wrapper = alloc_staticmethod_obj(py, MoltObject::from_int(43).bits());
                let direct_read = crate::builtins::types::native_descriptor_get(
                    py,
                    func,
                    Some(MoltObject::from_ptr(wrapper).bits()),
                    Some(classes.staticmethod),
                );
                assert_eq!(direct_read, MoltObject::from_int(43).bits());
                dec_ref_bits(py, direct_read);
                dec_ref_bits(py, MoltObject::from_ptr(wrapper).bits());
                dec_ref_bits(py, member_name);
                dec_ref_bits(py, object_class_name);
                let original_version = class_layout_version_bits(class);
                let staged = prepare_wrapper_members(py, 3, 14).unwrap();
                assert_eq!(class_dict_bits(class), dictionary);
                assert_eq!(class_layout_version_bits(class), original_version);
                assert!(dict_get_in_place(py, pointer, annotations_name).is_none());
                drop(staged);
                assert_eq!(class_layout_version_bits(class), original_version);
                for minor in [14, 13, 12, 14] {
                    prepare_wrapper_members(py, 3, minor).unwrap().commit(py);
                    assert_eq!(class_dict_bits(class), dictionary);
                    assert_eq!(dict_get_in_place(py, pointer, func_name), Some(func));
                    for (kind, owner) in [
                        (WrapperKind::Staticmethod, classes.staticmethod),
                        (WrapperKind::Classmethod, classes.classmethod),
                        (WrapperKind::Property, classes.property),
                    ] {
                        let namespace =
                            obj_from_bits(class_dict_bits(obj_from_bits(owner).as_ptr().unwrap()))
                                .as_ptr()
                                .unwrap();
                        for member in WRAPPER_MEMBERS
                            .iter()
                            .filter(|member| member.owners.contains(&kind))
                        {
                            let name =
                                attr_name_bits_from_bytes(py, member.name.as_bytes()).unwrap();
                            let descriptor = dict_get_in_place(py, namespace, name);
                            if member.enabled(3, minor) {
                                let descriptor =
                                    obj_from_bits(descriptor.unwrap()).as_ptr().unwrap();
                                assert_eq!(
                                    layout::native_descriptor_flavor(descriptor),
                                    Some(member.flavor)
                                );
                                assert_eq!(layout::native_descriptor_owner_bits(descriptor), owner);
                                assert_eq!(layout::native_descriptor_name_bits(descriptor), name);
                                assert_eq!(
                                    !obj_from_bits(layout::native_descriptor_setter_bits(
                                        descriptor
                                    ))
                                    .is_none(),
                                    member.writable
                                );
                            } else {
                                assert!(descriptor.is_none());
                            }
                            dec_ref_bits(py, name);
                        }
                    }
                    let version = class_layout_version_bits(class);
                    prepare_wrapper_members(py, 3, minor).unwrap().commit(py);
                    assert_eq!(class_layout_version_bits(class), version);
                }
                dec_ref_bits(py, annotations_name);
                dec_ref_bits(py, func_name);
                assert!(!exception_pending(py));
                assert!(wrapper_publish_members(py));
            }
        });
    }

    fn test_function(py: &PyToken<'_>, name: &'static str, target: *const (), arity: u64) -> u64 {
        let pointer = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(name, target),
            arity,
        );
        assert!(!pointer.is_null());
        MoltObject::from_ptr(pointer).bits()
    }

    unsafe fn test_class(py: &PyToken<'_>, base: u64, attrs: &[(&[u8], u64)]) -> u64 {
        unsafe {
            let name = attr_name_bits_from_bytes(py, b"WrapperFallbackProbe").unwrap();
            let namespace = alloc_dict_with_pairs(py, &[]);
            assert!(!namespace.is_null());
            let namespace_bits = MoltObject::from_ptr(namespace).bits();
            for &(key, value) in attrs {
                let key = attr_name_bits_from_bytes(py, key).unwrap();
                dict_set_in_place(py, namespace, key, value);
                dec_ref_bits(py, key);
            }
            let bases = alloc_tuple(py, &[base]);
            assert!(!bases.is_null());
            let bases_bits = MoltObject::from_ptr(bases).bits();
            let result = crate::builtins::types::molt_type_new(
                builtin_classes(py).type_obj,
                name,
                bases_bits,
                namespace_bits,
                MoltObject::none().bits(),
            );
            dec_ref_bits(py, bases_bits);
            dec_ref_bits(py, namespace_bits);
            dec_ref_bits(py, name);
            assert!(!exception_pending(py));
            assert!(obj_from_bits(result).as_ptr().is_some());
            result
        }
    }

    extern "C" fn fallback_returns_name(_receiver: u64, name: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            inc_ref_bits(py, name);
            name
        })
    }

    extern "C" fn getter_replaces_fallback_then_misses(receiver: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            let name = attr_name_bits_from_bytes(py, b"__getattr__").unwrap();
            molt_set_attr_name(type_of_bits(py, receiver), name, MoltObject::none().bits());
            dec_ref_bits(py, name);
            raise_exception::<u64>(
                py,
                "AttributeError",
                "property miss after fallback replacement",
            )
        })
    }

    extern "C" fn getter_runtime_error(_receiver: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            raise_exception::<u64>(py, "RuntimeError", "non-attribute descriptor failure")
        })
    }

    #[test]
    fn normal_descriptor_errors_share_captured_fallback_and_raw_bypasses_it() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                assert!(wrapper_publish_members(py));
                let fallback = test_function(
                    py,
                    "wrapper_fallback_returns_name",
                    fallback_returns_name as *const (),
                    2,
                );
                let getter = test_function(
                    py,
                    "wrapper_getter_replaces_fallback",
                    getter_replaces_fallback_then_misses as *const (),
                    1,
                );
                let failing = test_function(
                    py,
                    "wrapper_getter_runtime_error",
                    getter_runtime_error as *const (),
                    1,
                );
                let none = MoltObject::none().bits();
                let property =
                    MoltObject::from_ptr(alloc_property_obj(py, getter, none, none)).bits();
                let failure_property =
                    MoltObject::from_ptr(alloc_property_obj(py, failing, none, none)).bits();
                let class = test_class(
                    py,
                    builtin_classes(py).object,
                    &[
                        (b"probe", property),
                        (b"failure", failure_property),
                        (b"__getattr__", fallback),
                    ],
                );
                let instance =
                    crate::alloc_instance_for_class(py, obj_from_bits(class).as_ptr().unwrap());
                let name = attr_name_bits_from_bytes(py, b"probe").unwrap();
                let result = molt_get_attr_name(instance, name);
                assert!(!exception_pending(py));
                assert_eq!(result, name);
                dec_ref_bits(py, result);
                crate::molt_object_getattribute(instance, name);
                assert!(clear_attribute_error_if_pending(py));
                let hook_name = attr_name_bits_from_bytes(py, b"__getattr__").unwrap();
                molt_set_attr_name(class, hook_name, fallback);
                let failure_name = attr_name_bits_from_bytes(py, b"failure").unwrap();
                molt_get_attr_name(instance, failure_name);
                assert!(exception_pending(py));
                let error = molt_exception_last_pending();
                assert!(exception_matches_builtin_name(py, error, "RuntimeError"));
                molt_exception_clear();
                dec_ref_bits(py, error);
                for bits in [
                    failure_name,
                    hook_name,
                    name,
                    instance,
                    class,
                    failure_property,
                    property,
                    failing,
                    getter,
                    fallback,
                ] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn dictless_default_lookup_finishes_before_fallback_and_preserves_errors() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let fallback = test_function(
                    py,
                    "dictless_fallback_returns_name",
                    fallback_returns_name as *const (),
                    2,
                );
                let getter = test_function(
                    py,
                    "dictless_getter_runtime_error",
                    getter_runtime_error as *const (),
                    1,
                );
                let none = MoltObject::none().bits();
                let property =
                    MoltObject::from_ptr(alloc_property_obj(py, getter, none, none)).bits();
                let slots_ptr = alloc_tuple(py, &[]);
                assert!(!slots_ptr.is_null());
                let slots = MoltObject::from_ptr(slots_ptr).bits();
                let class = test_class(
                    py,
                    builtin_classes(py).list,
                    &[
                        (b"append", property),
                        (b"__getattr__", fallback),
                        (b"__slots__", slots),
                    ],
                );
                dec_ref_bits(py, slots);
                // Exercise the dictless native-receiver lookup contract, not
                // generic subclass construction (which uses class-shaped
                // object storage rather than a list's native backing).
                let instance_ptr = alloc_list(py, &[]);
                assert!(!instance_ptr.is_null());
                assert!(crate::object::object_replace_class_edge(
                    py,
                    instance_ptr,
                    class,
                    crate::object::ClassEdgeOwnership::Owned,
                ));
                let instance = MoltObject::from_ptr(instance_ptr).bits();
                assert!(!exception_pending(py));
                assert_eq!(
                    object_type_id(obj_from_bits(instance).as_ptr().unwrap()),
                    TYPE_ID_LIST
                );
                assert_eq!(object_class_bits(instance_ptr), class);
                let missing = attr_name_bits_from_bytes(py, b"probe").unwrap();
                let result = molt_get_attr_name(instance, missing);
                assert!(!exception_pending(py));
                assert_eq!(result, missing);
                dec_ref_bits(py, result);
                crate::molt_object_getattribute(instance, missing);
                assert!(clear_attribute_error_if_pending(py));
                let append = attr_name_bits_from_bytes(py, b"append").unwrap();
                molt_get_attr_name(instance, append);
                assert!(exception_pending(py));
                let error = molt_exception_last_pending();
                assert!(exception_matches_builtin_name(py, error, "RuntimeError"));
                molt_exception_clear();
                dec_ref_bits(py, error);
                // An unshadowed builtin member is found before __getattr__.
                let clear = attr_name_bits_from_bytes(py, b"clear").unwrap();
                let result = molt_get_attr_name(instance, clear);
                assert!(!exception_pending(py));
                assert_ne!(result, clear);
                dec_ref_bits(py, result);
                for bits in [
                    clear, append, missing, instance, class, property, getter, fallback,
                ] {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn property_missing_name_uses_the_same_default_lookup_fallback() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                prepare_wrapper_members(py, 3, 14).unwrap().commit(py);
                let fallback = test_function(
                    py,
                    "wrapper_name_fallback",
                    fallback_returns_name as *const (),
                    2,
                );
                let class = test_class(
                    py,
                    builtin_classes(py).property,
                    &[(b"__getattr__", fallback)],
                );
                let instance =
                    crate::alloc_instance_for_class(py, obj_from_bits(class).as_ptr().unwrap());
                let name = attr_name_bits_from_bytes(py, b"__name__").unwrap();
                let result = molt_get_attr_name(instance, name);
                assert!(!exception_pending(py));
                assert_eq!(result, name);
                dec_ref_bits(py, result);
                crate::molt_object_getattribute(instance, name);
                assert!(clear_attribute_error_if_pending(py));
                for bits in [name, instance, class, fallback] {
                    dec_ref_bits(py, bits);
                }
                assert!(wrapper_publish_members(py));
            }
        });
    }

    #[test]
    fn native_property_doc_and_name_are_owned_members() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                assert!(wrapper_publish_members(py));
                let none = MoltObject::none().bits();
                let object = alloc_property_obj(py, none, none, none);
                assert!(!object.is_null());
                let doc = attr_name_bits_from_bytes(py, b"__doc__").unwrap();
                let text = attr_name_bits_from_bytes(py, b"native property documentation").unwrap();
                molt_set_attr_name(MoltObject::from_ptr(object).bits(), doc, text);
                assert!(!exception_pending(py));
                let actual = molt_get_attr_name(MoltObject::from_ptr(object).bits(), doc);
                assert_eq!(actual, text);
                dec_ref_bits(py, actual);
                // Assigning None is an actual replacement, never a request to
                // fall back to a getter's subsequently changed documentation.
                molt_set_attr_name(MoltObject::from_ptr(object).bits(), doc, none);
                assert_eq!(layout::property_doc_bits(object), none);
                assert!(layout::property_replace_name_bits(py, object, none));
                assert_eq!(property_name_value(py, object), Some(none));
                assert!(layout::property_replace_name_bits(
                    py,
                    object,
                    missing_bits(py)
                ));
                assert_eq!(property_name_value(py, object), None);
                assert!(!exception_pending(py));
                dec_ref_bits(py, text);
                dec_ref_bits(py, doc);
                dec_ref_bits(py, MoltObject::from_ptr(object).bits());
            }
        });
    }

    #[test]
    fn function_wrapper_members_precede_dictionary_and_reject_mutation() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                assert!(wrapper_publish_members(py));
                let target = MoltObject::from_int(17).bits();
                for object in [
                    alloc_staticmethod_obj(py, target),
                    alloc_classmethod_obj(py, target),
                ] {
                    assert!(!object.is_null());
                    let dictionary = crate::object::field_storage::materialize(py, object).unwrap();
                    let dictionary_ptr = obj_from_bits(dictionary).as_ptr().unwrap();
                    for name in [b"__func__".as_slice(), b"__wrapped__".as_slice()] {
                        let name_bits = attr_name_bits_from_bytes(py, name).unwrap();
                        dict_set_in_place(py, dictionary_ptr, name_bits, MoltObject::none().bits());
                        let actual =
                            molt_get_attr_name(MoltObject::from_ptr(object).bits(), name_bits);
                        assert_eq!(actual, target);
                        dec_ref_bits(py, actual);
                        for value in [Some(MoltObject::none().bits()), None] {
                            match value {
                                Some(value) => {
                                    molt_set_attr_name(
                                        MoltObject::from_ptr(object).bits(),
                                        name_bits,
                                        value,
                                    );
                                }
                                None => {
                                    molt_del_attr_name(
                                        MoltObject::from_ptr(object).bits(),
                                        name_bits,
                                    );
                                }
                            }
                            assert!(exception_pending(py));
                            assert!(clear_attribute_error_if_pending(py));
                            assert_eq!(layout::wrapper_reference_bits(object, 0), target);
                        }
                        dec_ref_bits(py, name_bits);
                    }
                    dec_ref_bits(py, MoltObject::from_ptr(object).bits());
                }
            }
        });
    }
}
