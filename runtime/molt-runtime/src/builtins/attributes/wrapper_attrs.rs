//! One staged publication authority for versioned builtin namespace members.
//! Native members use the ordinary descriptor lookup lane.
use super::*;
use crate::builtins::functions::native_callable::NativeCallableSpec;
use crate::builtins::types::{
    NativeDescriptorFlavor, NativeDescriptorSpec, alloc_native_descriptor,
};
use crate::object::layout::{self, WrapperKind};

#[derive(Clone, Copy, Eq, PartialEq)]
enum MemberOwner {
    Staticmethod,
    Classmethod,
    Property,
    Type,
    Super,
    Coroutine,
}

#[derive(Clone, Copy)]
enum MemberOperation {
    Reference(usize),
    Name,
    Abstract,
    Dictionary,
    LazyMetadata,
    SuperReference(usize),
    CoroutineRunning,
    CoroutineFrame,
    CoroutineCode,
    CoroutineAwait,
    RootMetadata(molt_cpython_abi::api::typeobj::TypeAttributeField),
}

impl MemberOperation {
    const fn tag(self) -> u32 {
        match self {
            Self::Reference(index) => index as u32 + 1,
            Self::Name => 5,
            Self::Abstract => 6,
            Self::Dictionary => 7,
            Self::LazyMetadata => 8,
            Self::SuperReference(index) => 9 + index as u32,
            Self::CoroutineRunning => 12,
            Self::CoroutineFrame => 13,
            Self::CoroutineCode => 14,
            Self::CoroutineAwait => 15,
            Self::RootMetadata(field) => field as u32,
        }
    }

    fn from_tag(tag: u32) -> Option<Self> {
        Some(match tag {
            1..=4 => Self::Reference((tag - 1) as usize),
            5 => Self::Name,
            6 => Self::Abstract,
            7 => Self::Dictionary,
            8 => Self::LazyMetadata,
            9..=11 => Self::SuperReference((tag - 9) as usize),
            12 => Self::CoroutineRunning,
            13 => Self::CoroutineFrame,
            14 => Self::CoroutineCode,
            15 => Self::CoroutineAwait,
            _ => return None,
        })
    }
}

struct WrapperMember {
    name: &'static str,
    owners: &'static [MemberOwner],
    flavor: NativeDescriptorFlavor,
    writable: bool,
    minimum_minor: i64,
    operation: MemberOperation,
}

const FUNCTION_WRAPPERS: &[MemberOwner] = &[MemberOwner::Staticmethod, MemberOwner::Classmethod];
const PROPERTY_WRAPPER: &[MemberOwner] = &[MemberOwner::Property];

// One authority for publication, mutability, target gating and callback meaning.
const WRAPPER_MEMBERS: &[WrapperMember] = &[
    WrapperMember {
        name: "__thisclass__",
        owners: &[MemberOwner::Super],
        flavor: NativeDescriptorFlavor::Member,
        writable: false,
        minimum_minor: 0,
        operation: MemberOperation::SuperReference(0),
    },
    WrapperMember {
        name: "__self__",
        owners: &[MemberOwner::Super],
        flavor: NativeDescriptorFlavor::Member,
        writable: false,
        minimum_minor: 0,
        operation: MemberOperation::SuperReference(1),
    },
    WrapperMember {
        name: "__self_class__",
        owners: &[MemberOwner::Super],
        flavor: NativeDescriptorFlavor::Member,
        writable: false,
        minimum_minor: 0,
        operation: MemberOperation::SuperReference(2),
    },
    WrapperMember {
        name: "cr_running",
        owners: &[MemberOwner::Coroutine],
        flavor: NativeDescriptorFlavor::GetSet,
        writable: false,
        minimum_minor: 0,
        operation: MemberOperation::CoroutineRunning,
    },
    WrapperMember {
        name: "cr_frame",
        owners: &[MemberOwner::Coroutine],
        flavor: NativeDescriptorFlavor::GetSet,
        writable: false,
        minimum_minor: 0,
        operation: MemberOperation::CoroutineFrame,
    },
    WrapperMember {
        name: "cr_code",
        owners: &[MemberOwner::Coroutine],
        flavor: NativeDescriptorFlavor::Member,
        writable: false,
        minimum_minor: 0,
        operation: MemberOperation::CoroutineCode,
    },
    WrapperMember {
        name: "cr_await",
        owners: &[MemberOwner::Coroutine],
        flavor: NativeDescriptorFlavor::GetSet,
        writable: false,
        minimum_minor: 0,
        operation: MemberOperation::CoroutineAwait,
    },
    WrapperMember {
        name: "__annotate__",
        owners: &[MemberOwner::Type],
        flavor: NativeDescriptorFlavor::RootMetadata,
        writable: true,
        minimum_minor: 14,
        operation: MemberOperation::RootMetadata(
            molt_cpython_abi::api::typeobj::TypeAttributeField::Annotate,
        ),
    },
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
/// and metadata lookup; commit cannot fail. Native watchers run only after all
/// namespaces, runtime epochs and native tags are coherent. Until commit, live
/// dictionaries, class versions, and the publication receipt remain untouched.
pub(crate) struct WrapperMembersPublication {
    version_key: u64,
    staged_owners: Vec<PtrDropGuard>,
    publications: Vec<(Option<*mut u8>, *mut u8, *mut u8)>,
    native_invalidation: molt_cpython_abi::api::typeobj::TypeNamespaceInvalidation,
}

impl WrapperMembersPublication {
    pub(crate) fn commit(self, py: &PyToken<'_>) {
        unsafe {
            crate::gil_assert();
            for &(_, live, staged) in &self.publications {
                crate::object::ops::dict_publish_staged(py, live, staged);
            }
            for &(class, _, _) in &self.publications {
                if let Some(class) = class {
                    class_bump_layout_version(class);
                }
            }
            runtime_state(py)
                .attributes
                .wrapper_members_version
                .store(self.version_key, Ordering::Release);
            // All runtime state is coherent before a native watcher can run.
            crate::object::foreign::retire_native_reference(|| self.native_invalidation.commit());
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
        use molt_cpython_abi::api::typeobj::{self, BuiltinSlotNamespacePolicy};
        const SLOT_POLICY_BIT: u32 = WRAPPER_MEMBERS.len() as u32 + 1;
        let slot_policy = BuiltinSlotNamespacePolicy::for_minor(minor);
        let version_key = WRAPPER_MEMBERS
            .iter()
            .enumerate()
            .fold(1u64, |key, (index, member)| {
                key | ((member.enabled(major, minor) as u64) << (index + 1))
            })
            | (slot_policy.receipt_bit() << SLOT_POLICY_BIT);
        let previous_version = attributes.wrapper_members_version.load(Ordering::Acquire);
        if previous_version == version_key {
            return Some(WrapperMembersPublication {
                version_key,
                staged_owners: Vec::new(),
                publications: Vec::new(),
                native_invalidation: typeobj::TypeNamespaceInvalidation::prepare(&[]),
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
            let bits = crate::builtins::types::builtin_func_bits(
                py,
                NativeCallableSpec::function(slot),
                symbol,
                arity,
            );
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
            (MemberOwner::Staticmethod, builtins.staticmethod),
            (MemberOwner::Classmethod, builtins.classmethod),
            (MemberOwner::Property, builtins.property),
            (MemberOwner::Type, builtins.type_obj),
            (MemberOwner::Super, builtins.super_type),
            (MemberOwner::Coroutine, builtins.coroutine),
        ] {
            let class = obj_from_bits(class_bits).as_ptr().unwrap();
            let dictionary = class_dict_bits(class);
            let live = obj_from_bits(dictionary).as_ptr().unwrap();
            let staged_bits = crate::object::ops_dict::molt_dict_copy(dictionary);
            let staged = obj_from_bits(staged_bits).as_ptr()?;
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
                let name_bits = attr_name_bits_from_bytes(py, member.name.as_bytes())?;
                let _name_owner = PtrDropGuard::new(obj_from_bits(name_bits).as_ptr().unwrap());
                let (getter, setter, deleter) =
                    if matches!(member.operation, MemberOperation::RootMetadata(_)) {
                        let none = MoltObject::none().bits();
                        (none, none, none)
                    } else {
                        (getter, setter, deleter)
                    };
                let enabled = member.enabled(major, minor);
                if !enabled {
                    if let Some(existing) = match dict_get_in_place(py, staged, name_bits) {
                        Ok(value) => value,
                        Err(()) => return None,
                    } && let Some(ptr) = obj_from_bits(existing).as_ptr()
                        && layout::native_descriptor_flavor(ptr) == Some(member.flavor)
                        && layout::native_descriptor_operation(ptr) == Some(member.operation.tag())
                        && layout::native_descriptor_owner_bits(ptr) == class_bits
                        && layout::native_descriptor_getter_bits(ptr) == getter
                    {
                        if dict_del_in_place(py, staged, name_bits).is_err() {
                            return None;
                        }
                    }
                    if exception_pending(py) {
                        return None;
                    }
                    continue;
                }
                // Preserve identity for already-present version-gated members
                // whose admission did not change between the two versions.
                if previous_version != 0
                    && let Some(existing) = match dict_get_in_place(py, staged, name_bits) {
                        Ok(value) => value,
                        Err(()) => return None,
                    }
                    && let Some(ptr) = obj_from_bits(existing).as_ptr()
                    && layout::native_descriptor_flavor(ptr) == Some(member.flavor)
                    && layout::native_descriptor_operation(ptr) == Some(member.operation.tag())
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
                        operation: member.operation.tag(),
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
                let descriptor_ptr = obj_from_bits(descriptor).as_ptr()?;
                let _descriptor_owner = PtrDropGuard::new(descriptor_ptr);
                if exception_pending(py) {
                    return None;
                }
                if !dict_set_in_place(py, staged, name_bits, descriptor) {
                    return None;
                }
            }
            publications.push((Some(class), live, staged));
        }

        // Hook installation readies static types before the compiled entry can
        // publish its target. Reconcile only changed slot policy, in the same
        // private copies as other versioned namespace members.
        let previous_policy = if previous_version & (1 << SLOT_POLICY_BIT) == 0 {
            BuiltinSlotNamespacePolicy::ManualInheritance
        } else {
            BuiltinSlotNamespacePolicy::PairedInheritance
        };
        let mut native_roots = Vec::new();
        if previous_version != 0 && previous_policy != slot_policy {
            use molt_cpython_abi::abi_types;
            use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
            for entry in abi_types::builtin_static_type_runtime_cohort()
                .filter(|entry| entry.namespace == abi_types::StaticTypeNamespace::StaticBuiltin)
            {
                let native = entry.type_ptr;
                let object = native.cast::<abi_types::PyObject>();
                match typeobj::builtin_slot_namespace_needs_update(
                    native,
                    previous_policy,
                    slot_policy,
                ) {
                    Ok(false) => continue,
                    Ok(true) => {}
                    Err(_) => {
                        crate::cpython_abi_hooks::propagate_native_failure(
                            py,
                            "static namespace policy observation",
                        );
                        return None;
                    }
                }
                // Runtime-created builtin namespaces are exact managed dicts;
                // observe their existing binding without creating a second one.
                let dictionary = GLOBAL_BRIDGE
                    .molt_handle_for_pyobj((*native).tp_dict)
                    .map(|handle| handle.bits());
                let Some(dictionary) = dictionary else {
                    raise_exception::<()>(
                        py,
                        "SystemError",
                        "builtin namespace has no runtime dictionary owner",
                    );
                    return None;
                };
                let Some(live) = obj_from_bits(dictionary).as_ptr() else {
                    raise_exception::<()>(
                        py,
                        "SystemError",
                        "builtin namespace has invalid dictionary storage",
                    );
                    return None;
                };
                if object_type_id(live) != TYPE_ID_DICT {
                    raise_exception::<()>(
                        py,
                        "SystemError",
                        "builtin namespace requires exact dictionary storage",
                    );
                    return None;
                }
                let staged = if let Some((_, _, staged)) = publications
                    .iter()
                    .find(|(_, candidate, _)| *candidate == live)
                {
                    *staged
                } else {
                    let staged_bits = crate::object::ops_dict::molt_dict_copy(dictionary);
                    let staged = obj_from_bits(staged_bits).as_ptr()?;
                    staged_owners.push(PtrDropGuard::new(staged));
                    if exception_pending(py) {
                        return None;
                    }
                    let class = GLOBAL_BRIDGE
                        .molt_handle_for_pyobj(object)
                        .and_then(|handle| obj_from_bits(handle.bits()).as_ptr())
                        .filter(|&ptr| object_type_id(ptr) == TYPE_ID_TYPE);
                    publications.push((class, live, staged));
                    staged
                };
                let staged_view = crate::object::foreign::NativeObjectOwner::from_owned(
                    GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(MoltObject::from_ptr(staged).bits()),
                );
                if staged_view.as_ptr().is_null() {
                    crate::cpython_abi_hooks::propagate_native_failure(
                        py,
                        "static namespace staging projection",
                    );
                    return None;
                }
                match typeobj::stage_builtin_slot_namespace(
                    native,
                    staged_view.as_ptr(),
                    previous_policy,
                    slot_policy,
                ) {
                    Ok(true) => native_roots.push(native),
                    Ok(false) => {}
                    Err(_) => {
                        crate::cpython_abi_hooks::propagate_native_failure(
                            py,
                            "static namespace target staging",
                        );
                        return None;
                    }
                }
            }
        }
        // Preparation records lifetime-qualified identities only. It changes no
        // namespace or cache flag and performs all traversal allocation now.
        let native_invalidation = typeobj::TypeNamespaceInvalidation::prepare(&native_roots);
        Some(WrapperMembersPublication {
            version_key,
            staged_owners,
            publications,
            native_invalidation,
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

/// Publication seals the typed operation into the descriptor's control word.
/// Its name remains metadata or the key of a lazy attribute, never dispatch.
unsafe fn callback_member(
    py: &PyToken<'_>,
    descriptor: u64,
    instance: u64,
) -> Option<(MemberOperation, *mut u8, u64)> {
    unsafe {
        let descriptor = obj_from_bits(descriptor).as_ptr()?;
        layout::native_descriptor_flavor(descriptor)?;
        let operation =
            MemberOperation::from_tag(layout::native_descriptor_operation(descriptor)?)?;
        let getter = obj_from_bits(layout::native_descriptor_getter_bits(descriptor)).as_ptr()?;
        if object_type_id(getter) != TYPE_ID_FUNCTION
            || function_fn_ptr(getter)
                != crate::builtins::functions::runtime_fn_key(
                    "molt_wrapper_member_get",
                    molt_wrapper_member_get as *const (),
                )
        {
            return None;
        }
        let object = obj_from_bits(instance).as_ptr()?;
        let valid_shape = match operation {
            MemberOperation::SuperReference(_) => object_type_id(object) == TYPE_ID_SUPER,
            MemberOperation::CoroutineRunning
            | MemberOperation::CoroutineFrame
            | MemberOperation::CoroutineCode
            | MemberOperation::CoroutineAwait => {
                crate::async_rt::generators::is_native_coroutine_bits(instance)
            }
            _ => WrapperKind::from_type_id(object_type_id(object)).is_some(),
        };
        if !valid_shape {
            return None;
        }
        if !crate::object::class_layout::is_real_subtype(
            py,
            type_of_bits(py, instance),
            layout::native_descriptor_owner_bits(descriptor),
        ) {
            return None;
        }
        Some((
            operation,
            object,
            layout::native_descriptor_name_bits(descriptor),
        ))
    }
}

unsafe fn coroutine_member_value(
    py: &PyToken<'_>,
    object: *mut u8,
    operation: MemberOperation,
) -> Option<u64> {
    unsafe {
        let header = header_from_obj_ptr(object);
        match operation {
            MemberOperation::CoroutineRunning => {
                let running = ((*header).load_synchronized_flags() & HEADER_FLAG_GEN_RUNNING) != 0;
                Some(MoltObject::from_bool(running).bits())
            }
            MemberOperation::CoroutineFrame => {
                if crate::object::object_poll_fn(object) == 0
                    || ((*header).load_synchronized_flags() & HEADER_FLAG_TASK_DONE) != 0
                {
                    return Some(MoltObject::none().bits());
                }
                let lasti = if crate::object::object_state(object) == 0 {
                    -1
                } else {
                    0
                };
                Some(suspended_frame_bits(py, object, lasti))
            }
            MemberOperation::CoroutineCode => {
                let code_bits = crate::object::aux_header::object_frame_code_bits(object);
                if code_bits != 0 {
                    inc_ref_bits(py, code_bits);
                    return Some(code_bits);
                }
                Some(MoltObject::none().bits())
            }
            MemberOperation::CoroutineAwait => {
                if (*header).load_synchronized_flags()
                    & (HEADER_FLAG_GEN_RUNNING | HEADER_FLAG_TASK_DONE)
                    != 0
                {
                    return Some(MoltObject::none().bits());
                }
                let bits = crate::object::aux_header::object_frame_awaited_bits(object);
                if bits != 0 {
                    return Some(crate::async_rt::awaitable::python_awaited_bits(py, bits));
                }
                Some(MoltObject::none().bits())
            }
            _ => unreachable!("non-coroutine member operation"),
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_wrapper_member_get(descriptor: u64, instance: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let Some((operation, object, name)) = callback_member(py, descriptor, instance) else {
                return raise_exception::<u64>(py, "TypeError", "invalid wrapper member receiver");
            };
            let result = match operation {
                MemberOperation::SuperReference(index) => {
                    let value = match index {
                        0 => super_type_bits(object),
                        1 => super_obj_bits(object),
                        2 => layout::super_receiver_class_bits(object),
                        _ => unreachable!("invalid super member selector"),
                    };
                    inc_ref_bits(py, value);
                    Ok(Some(value))
                }
                MemberOperation::CoroutineRunning
                | MemberOperation::CoroutineFrame
                | MemberOperation::CoroutineCode
                | MemberOperation::CoroutineAwait => coroutine_member_value(py, object, operation)
                    .map(Some)
                    .ok_or(()),
                MemberOperation::Reference(index) => {
                    let bits = layout::wrapper_reference_bits(object, index);
                    let bits = if is_missing_bits(py, bits) {
                        MoltObject::none().bits()
                    } else {
                        bits
                    };
                    inc_ref_bits(py, bits);
                    Ok(Some(bits))
                }
                MemberOperation::Name => property_name_value(py, object),
                MemberOperation::Abstract => abstract_member(
                    py,
                    object,
                    WrapperKind::from_type_id(object_type_id(object)).unwrap(),
                )
                .map(Some),
                MemberOperation::Dictionary => {
                    crate::object::field_storage::materialize(py, object)
                        .inspect(|&bits| {
                            inc_ref_bits(py, bits);
                        })
                        .map(Some)
                        .ok_or(())
                }
                MemberOperation::LazyMetadata => lazy_wrapped_attribute(py, object, name).map(Some),
                MemberOperation::RootMetadata(_) => {
                    unreachable!("root metadata has no wrapper callback")
                }
            };
            match result {
                Ok(Some(value)) => value,
                Ok(None) => raise_exception::<u64>(
                    py,
                    "AttributeError",
                    &format!(
                        "'{}' object has no attribute '{}'",
                        class_name_for_error(object_class_bits(object)),
                        string_obj_to_owned(obj_from_bits(name)).unwrap_or_default(),
                    ),
                ),
                Err(()) => MoltObject::none().bits(),
            }
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
        let Some((operation, object, name)) = callback_member(py, descriptor, instance) else {
            return raise_exception::<u64>(py, "TypeError", "invalid wrapper member receiver");
        };
        let descriptor = obj_from_bits(descriptor).as_ptr().unwrap();
        let callback = if value.is_some() {
            layout::native_descriptor_setter_bits(descriptor)
        } else {
            layout::native_descriptor_deleter_bits(descriptor)
        };
        if obj_from_bits(callback).is_none() {
            return raise_exception::<u64>(py, "AttributeError", "readonly attribute");
        }
        match operation {
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
                if crate::object::field_storage::replace_dictionary(py, object, Some(value))
                    .is_err()
                {
                    return MoltObject::none().bits();
                }
            }
            MemberOperation::LazyMetadata => {
                let Some(dictionary) = crate::object::field_storage::materialize(py, object) else {
                    return MoltObject::none().bits();
                };
                inc_ref_bits(py, dictionary);
                let dictionary = obj_from_bits(dictionary).as_ptr().unwrap();
                let _dictionary_owner = PtrDropGuard::new(dictionary);
                match value {
                    Some(value) => {
                        if !dict_set_in_place(py, dictionary, name, value) {
                            return MoltObject::none().bits();
                        }
                    }
                    None => {
                        let deleted = match dict_del_in_place(py, dictionary, name) {
                            Ok(deleted) => deleted,
                            Err(()) => return MoltObject::none().bits(),
                        };
                        if !deleted {
                            raise_exception::<()>(
                                py,
                                "AttributeError",
                                &format!(
                                    "'{}' object has no attribute '{}'",
                                    class_name_for_error(object_class_bits(object)),
                                    string_obj_to_owned(obj_from_bits(name)).unwrap_or_default()
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
unsafe fn optional_attribute(py: &PyToken<'_>, target: u64, name: u64) -> Result<Option<u64>, ()> {
    inc_ref_bits(py, target);
    let value = super::get_attr_name(py, target, name);
    dec_ref_bits(py, target);
    match value {
        Ok(value) => Ok(Some(value)),
        Err(()) => {
            if clear_attribute_error_if_pending(py) {
                Ok(None)
            } else {
                Err(())
            }
        }
    }
}

/// Native property name query for both __name__ and descriptor diagnostics.
/// Does not invoke a property subclass's override. Some is an owned value,
/// including explicit None; None means absent or a pending callback exception.
pub(crate) unsafe fn property_name_value(
    py: &PyToken<'_>,
    object: *mut u8,
) -> Result<Option<u64>, ()> {
    unsafe {
        let name = layout::property_name_bits(object);
        if !is_missing_bits(py, name) {
            inc_ref_bits(py, name);
            return Ok(Some(name));
        }
        let getter = property_get_bits(object);
        if obj_from_bits(getter).is_none() {
            return Ok(None);
        }
        let name = attr_name_bits_from_bytes(py, b"__name__").ok_or(())?;
        let result = optional_attribute(py, getter, name);
        dec_ref_bits(py, name);
        result
    }
}

unsafe fn abstract_member(py: &PyToken<'_>, object: *mut u8, kind: WrapperKind) -> Result<u64, ()> {
    unsafe {
        let name = attr_name_bits_from_bytes(py, b"__isabstractmethod__").ok_or(())?;
        let count = if kind == WrapperKind::Property { 3 } else { 1 };
        let result = (|| {
            for index in 0..count {
                // Read each accessor after the prior callback, matching the
                // native descriptor's reentrant observation order.
                let target = layout::wrapper_reference_bits(object, index);
                if is_missing_bits(py, target) || obj_from_bits(target).is_none() {
                    continue;
                }
                if let Some(value) = optional_attribute(py, target, name)? {
                    let truth = crate::object::ops::truth_value(py, obj_from_bits(value));
                    dec_ref_bits(py, value);
                    if truth? {
                        return Ok(MoltObject::from_bool(true).bits());
                    }
                }
            }
            Ok(MoltObject::from_bool(false).bits())
        })();
        dec_ref_bits(py, name);
        result
    }
}

/// Materialize and pin the selected dictionary before calling into the target.
/// If the callback replaces __dict__, the in-flight cache still belongs to the
/// selected dictionary, not to a subsequently installed one.
unsafe fn lazy_wrapped_attribute(py: &PyToken<'_>, object: *mut u8, name: u64) -> Result<u64, ()> {
    unsafe {
        let target = layout::wrapper_reference_bits(object, 0);
        inc_ref_bits(py, target);
        let result = (|| {
            let dictionary = crate::object::field_storage::materialize(py, object).ok_or(())?;
            inc_ref_bits(py, dictionary);
            let dictionary_ptr = obj_from_bits(dictionary).as_ptr().unwrap();
            let _dictionary_owner = PtrDropGuard::new(dictionary_ptr);
            if let Some(value) = dict_get_in_place(py, dictionary_ptr, name)? {
                inc_ref_bits(py, value);
                return Ok(value);
            }
            if is_missing_bits(py, target) {
                raise_exception::<()>(
                    py,
                    "RuntimeError",
                    "uninitialized descriptor wrapper object",
                );
                return Err(());
            }
            let value = super::get_attr_name(py, target, name)?;
            if !dict_set_in_place(py, dictionary_ptr, name, value) {
                dec_ref_bits(py, value);
                return Err(());
            }
            Ok(value)
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
            let completed = match optional_attribute(py, target, name_bits) {
                Ok(Some(value)) => {
                    let result = super::mutate_attr_name(
                        py,
                        object,
                        name_bits,
                        crate::builtins::attr::DescriptorMutation::Set(value),
                        molt_cpython_abi::hooks::AttributeMutation::Normal,
                    );
                    dec_ref_bits(py, value);
                    result.is_ok()
                }
                Ok(None) => true,
                Err(()) => false,
            };
            dec_ref_bits(py, name_bits);
            if !completed {
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
    fn compiler_version_publication_admits_type_annotate_without_environment() {
        struct RestoreEnvironment(Vec<(&'static str, Option<std::ffi::OsString>)>);
        impl Drop for RestoreEnvironment {
            fn drop(&mut self) {
                for (key, value) in &self.0 {
                    unsafe {
                        match value {
                            Some(value) => std::env::set_var(key, value),
                            None => std::env::remove_var(key),
                        }
                    }
                }
            }
        }
        static WATCH_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        static WATCH_COHERENT: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(true);
        unsafe extern "C" fn observe_publication(
            _kind: *mut molt_cpython_abi::abi_types::PyObject,
        ) -> std::ffi::c_int {
            use molt_cpython_abi::abi_types::{PyList_Type, PyMemoryView_Type};
            unsafe {
                let coherent = (molt_cpython_abi::hooks::hooks_or_stubs().target_python_minor)()
                    == 14
                    && PyList_Type.tp_version_tag == 0
                    && PyMemoryView_Type.tp_version_tag == 0;
                WATCH_COHERENT.fetch_and(coherent, Ordering::Relaxed);
                WATCH_CALLS.fetch_add(1, Ordering::Relaxed);
            }
            0
        }
        struct Watcher(std::ffi::c_int);
        impl Drop for Watcher {
            fn drop(&mut self) {
                unsafe {
                    for kind in [
                        &raw mut molt_cpython_abi::abi_types::PyList_Type,
                        &raw mut molt_cpython_abi::abi_types::PyMemoryView_Type,
                    ] {
                        molt_cpython_abi::api::typeobj::PyType_Unwatch(self.0, kind.cast());
                    }
                    molt_cpython_abi::api::typeobj::PyType_ClearWatcher(self.0);
                }
            }
        }
        struct RestoreTracker;
        impl Drop for RestoreTracker {
            fn drop(&mut self) {
                crate::resource::set_tracker(Box::new(crate::resource::UnlimitedTracker));
            }
        }
        // Every epoch starts cold, so both receipt reset and the real compiler
        // startup order (builtins first, compiled version second) are exercised.
        for minor in [12, 14, 12, 14] {
            crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
                let _environment = RestoreEnvironment(
                    [
                        "MOLT_PYTHON_VERSION",
                        "MOLT_SYS_VERSION_INFO",
                        "MOLT_SYS_VERSION",
                    ]
                    .into_iter()
                    .map(|key| (key, std::env::var_os(key)))
                    .collect(),
                );
                for (key, _) in &_environment.0 {
                    unsafe {
                        std::env::remove_var(key);
                    }
                }
                assert_eq!(crate::state::runtime_state::molt_runtime_init(), 1);
                crate::with_gil_entry_nopanic!(py, {
                    unsafe {
                        let classes = builtin_classes(py);
                        let root = obj_from_bits(classes.type_obj).as_ptr().unwrap();
                        let namespace = class_dict_bits(root);
                        let dictionary = obj_from_bits(namespace).as_ptr().unwrap();
                        let name = attr_name_bits_from_bytes(py, b"__annotate__").unwrap();
                        let _name_owner =
                            crate::builtins::exceptions::ExceptionValue::adopt(py, name);
                        let release = attr_name_bits_from_bytes(py, b"final").unwrap();
                        let _release_owner =
                            crate::builtins::exceptions::ExceptionValue::adopt(py, release);
                        let version =
                            attr_name_bits_from_bytes(py, format!("3.{minor}.0").as_bytes())
                                .unwrap();
                        let _version_owner =
                            crate::builtins::exceptions::ExceptionValue::adopt(py, version);
                        assert!(
                            dict_get_in_place(py, dictionary, name)
                                .expect("dictionary lookup succeeds")
                                .is_none()
                        );
                        use molt_cpython_abi::abi_types::{PyFloat_Type, PyList_Type};
                        use molt_cpython_abi::api::{errors, mapping, refcount, typeobj};
                        let list = &raw mut PyList_Type;
                        let float = &raw mut PyFloat_Type;
                        assert_eq!(typeobj::PyType_Ready(list), 0);
                        assert_eq!(typeobj::PyType_Ready(float), 0);
                        let own_member =
                            |kind: *mut molt_cpython_abi::abi_types::PyTypeObject,
                             member: &std::ffi::CStr| {
                                let value = mapping::_PyDict_GetItemStringWithError(
                                    (*kind).tp_dict,
                                    member.as_ptr(),
                                );
                                assert!(errors::PyErr_Occurred().is_null());
                                assert!(!exception_pending(py));
                                value
                            };
                        // Source-pinned own-dictionary oracle, not inherited
                        // lookup: renderer ownership and declaration differ.
                        let exception_str_declarations = |target_minor: i64| {
                            use molt_cpython_abi::abi_types::*;
                            for (kind, declared) in [
                                (&raw mut PyExc_AttributeError, target_minor < 14),
                                (&raw mut PyExc_NameError, target_minor < 14),
                                (&raw mut PyExc_Exception, false),
                                (
                                    molt_cpython_abi::abi_types::exc_singleton_for_builtin_name(
                                        "ExceptionGroup",
                                    )
                                    .unwrap()
                                    .cast(),
                                    false,
                                ),
                                (&raw mut PyExc_ImportError, true),
                                (&raw mut PyExc_BaseException, true),
                            ] {
                                assert_eq!(typeobj::PyType_Ready(kind), 0);
                                let member = own_member(kind, c"__str__");
                                assert_eq!(!member.is_null(), declared);
                                if declared {
                                    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
                                    if GLOBAL_BRIDGE.molt_handle_for_pyobj(member).is_some() {
                                        // Managed exception declarations own semantic
                                        // wrappers; their C carriers do not have the
                                        // physical PyWrapperDescrObject layout.
                                        let value = GLOBAL_BRIDGE
                                            .observed_handle_for_pyobj(member)
                                            .expect("managed declaration remains observable");
                                        assert_eq!(
                                            type_of_bits(py, value.bits()),
                                            classes.wrapper_descriptor
                                        );
                                        let owner = refcount::OwnedPyObject::from_owned(
                                            molt_cpython_abi::api::object::PyObject_GetAttrString(
                                                member,
                                                c"__objclass__".as_ptr(),
                                            ),
                                        );
                                        assert_eq!(owner.as_ptr(), kind.cast());
                                    } else {
                                        assert_eq!((*member).ob_type, &raw mut PyWrapperDescr_Type);
                                        assert_eq!(
                                            (*member.cast::<PyWrapperDescrObject>()).d_common.d_type,
                                            kind
                                        );
                                    }
                                }
                            }
                        };
                        let group = molt_cpython_abi::abi_types::exc_singleton_for_builtin_name(
                            "ExceptionGroup",
                        )
                        .unwrap()
                        .cast();
                        for member in [c"__init__", c"__repr__", c"__str__", c"__getattribute__"] {
                            assert!(own_member(group, member).is_null());
                        }
                        exception_str_declarations(12);
                        // Independent CP 3.12 own-namespace declarations before
                        // the compiled target is published after bootstrap.
                        let initial_getattribute = own_member(list, c"__getattribute__");
                        assert!(!initial_getattribute.is_null());
                        assert!(!own_member(list, c"__init__").is_null());
                        let initial_repr = refcount::OwnedPyObject::from_owned(
                            molt_cpython_abi::api::object::Py_NewRef(own_member(
                                float,
                                c"__repr__",
                            )),
                        );
                        assert!(!initial_repr.as_ptr().is_null());
                        WATCH_CALLS.store(0, Ordering::Relaxed);
                        WATCH_COHERENT.store(true, Ordering::Relaxed);
                        let _watcher = if minor >= 14 {
                            let memoryview =
                                &raw mut molt_cpython_abi::abi_types::PyMemoryView_Type;
                            assert_eq!(typeobj::PyType_Ready(memoryview), 0);
                            assert!(!own_member(memoryview, c"__getattribute__").is_null());
                            let id = typeobj::PyType_AddWatcher(Some(observe_publication));
                            assert!(id >= 0);
                            let owner = Watcher(id);
                            for kind in [list, memoryview] {
                                assert_eq!(typeobj::PyType_Watch(id, kind.cast()), 0);
                            }
                            Some(owner)
                        } else {
                            None
                        };
                        assert_eq!(typeobj::PyUnstable_Type_AssignVersionTag(list), 1);
                        let initial_tag = (*list).tp_version_tag;
                        let initial_epoch = class_layout_version_bits(root);
                        let initial_receipt = runtime_state(py)
                            .attributes
                            .wrapper_members_version
                            .load(Ordering::Acquire);
                        let publish = || {
                            crate::object::ops_sys::molt_sys_set_version_info(
                                MoltObject::from_int(3).bits(),
                                MoltObject::from_int(minor).bits(),
                                MoltObject::from_int(0).bits(),
                                release,
                                MoltObject::from_int(0).bits(),
                                version,
                            )
                        };
                        if minor >= 14 {
                            // Real allocation denial cannot partially publish
                            // target facts, namespaces, epochs or cache tags.
                            let tracker = RestoreTracker;
                            crate::resource::set_tracker(Box::new(
                                crate::resource::LimitedTracker::new(
                                    &crate::resource::ResourceLimits {
                                        max_memory: Some(0),
                                        ..Default::default()
                                    },
                                ),
                            ));
                            publish();
                            assert_eq!(
                                errors::PyErr_Occurred(),
                                (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).cast()
                            );
                            drop(tracker);
                            errors::PyErr_Clear();
                            molt_exception_clear();
                            assert_eq!(
                                crate::object::ops_sys::runtime_target_python_info(runtime_state(
                                    py
                                ))
                                .minor,
                                12
                            );
                            assert_eq!(class_dict_bits(root), namespace);
                            assert_eq!(class_layout_version_bits(root), initial_epoch);
                            assert_eq!((*list).tp_version_tag, initial_tag);
                            assert_eq!(own_member(list, c"__getattribute__"), initial_getattribute);
                            assert_eq!(
                                runtime_state(py)
                                    .attributes
                                    .wrapper_members_version
                                    .load(Ordering::Acquire),
                                initial_receipt
                            );
                            assert_eq!(WATCH_CALLS.load(Ordering::Relaxed), 0);
                            exception_str_declarations(12);
                        }
                        publish();
                        assert!(!exception_pending(py));
                        assert!(errors::PyErr_Occurred().is_null());
                        assert_eq!(own_member(list, c"__getattribute__").is_null(), minor >= 14);
                        assert_eq!(own_member(float, c"__repr__"), initial_repr.as_ptr());
                        exception_str_declarations(minor);
                        for member in [c"__init__", c"__repr__", c"__str__", c"__getattribute__"] {
                            assert!(own_member(group, member).is_null());
                        }
                        if minor >= 14 {
                            assert_eq!((*list).tp_version_tag, 0);
                            assert!(WATCH_CALLS.load(Ordering::Relaxed) >= 2);
                            assert!(WATCH_COHERENT.load(Ordering::Relaxed));
                        } else {
                            assert_eq!((*list).tp_version_tag, initial_tag);
                        }
                        // Stage the reverse projection on a private dictionary:
                        // public startup rejects conflicting target versions.
                        // Existing user/extension replacements must survive.
                        let copy = refcount::OwnedPyObject::from_owned(mapping::PyDict_Copy(
                            (*list).tp_dict,
                        ));
                        assert!(!copy.as_ptr().is_null());
                        use typeobj::BuiltinSlotNamespacePolicy::{
                            ManualInheritance, PairedInheritance,
                        };
                        if minor >= 14 {
                            assert!(
                                typeobj::stage_builtin_slot_namespace(
                                    list,
                                    copy.as_ptr(),
                                    PairedInheritance,
                                    ManualInheritance
                                )
                                .unwrap()
                            );
                            assert!(
                                !mapping::_PyDict_GetItemStringWithError(
                                    copy.as_ptr(),
                                    c"__getattribute__".as_ptr()
                                )
                                .is_null()
                            );
                        }
                        assert_eq!(
                            mapping::PyDict_SetItemString(
                                copy.as_ptr(),
                                c"__getattribute__".as_ptr(),
                                &raw mut molt_cpython_abi::abi_types::Py_None
                            ),
                            0
                        );
                        typeobj::stage_builtin_slot_namespace(
                            list,
                            copy.as_ptr(),
                            ManualInheritance,
                            PairedInheritance,
                        )
                        .unwrap();
                        assert_eq!(
                            mapping::_PyDict_GetItemStringWithError(
                                copy.as_ptr(),
                                c"__getattribute__".as_ptr()
                            ),
                            &raw mut molt_cpython_abi::abi_types::Py_None
                        );
                        assert!(errors::PyErr_Occurred().is_null());
                        assert!(!exception_pending(py));
                        let descriptor = dict_get_in_place(py, dictionary, name)
                            .expect("dictionary lookup succeeds");
                        assert_eq!(descriptor.is_some(), minor >= 14);
                        assert_eq!(
                            crate::object::ops_sys::runtime_target_python_info(runtime_state(py))
                                .minor,
                            minor
                        );
                        let epoch = class_layout_version_bits(root);
                        let receipt = runtime_state(py)
                            .attributes
                            .wrapper_members_version
                            .load(Ordering::Acquire);
                        publish();
                        assert!(!exception_pending(py));
                        assert_eq!(class_dict_bits(root), namespace);
                        assert_eq!(class_layout_version_bits(root), epoch);
                        assert_eq!(
                            dict_get_in_place(py, dictionary, name)
                                .expect("dictionary lookup succeeds"),
                            descriptor
                        );
                        assert_eq!(
                            runtime_state(py)
                                .attributes
                                .wrapper_members_version
                                .load(Ordering::Acquire),
                            receipt
                        );
                        // A rejected startup update cannot publish staged root
                        // descriptors or advance the version receipt.
                        let conflicting =
                            attr_name_bits_from_bytes(py, b"conflicting version").unwrap();
                        let _conflicting_owner =
                            crate::builtins::exceptions::ExceptionValue::adopt(py, conflicting);
                        crate::object::ops_sys::molt_sys_set_version_info(
                            MoltObject::from_int(3).bits(),
                            MoltObject::from_int(13).bits(),
                            MoltObject::from_int(0).bits(),
                            release,
                            MoltObject::from_int(0).bits(),
                            conflicting,
                        );
                        assert!(exception_pending(py));
                        molt_exception_clear();
                        assert_eq!(class_dict_bits(root), namespace);
                        assert_eq!(class_layout_version_bits(root), epoch);
                        assert_eq!(
                            dict_get_in_place(py, dictionary, name)
                                .expect("dictionary lookup succeeds"),
                            descriptor
                        );
                        assert_eq!(
                            runtime_state(py)
                                .attributes
                                .wrapper_members_version
                                .load(Ordering::Acquire),
                            receipt
                        );
                    }
                });
            });
        }
    }

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
                let func = dict_get_in_place(py, pointer, func_name)
                    .expect("dictionary lookup succeeds")
                    .unwrap();
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
                )
                .expect("operation succeeds");
                assert_eq!(class_read, func);
                dec_ref_bits(py, class_read);
                let wrapper = alloc_staticmethod_obj(py, MoltObject::from_int(43).bits());
                let direct_read = crate::builtins::types::native_descriptor_get(
                    py,
                    func,
                    Some(MoltObject::from_ptr(wrapper).bits()),
                    Some(classes.staticmethod),
                )
                .expect("operation succeeds");
                assert_eq!(direct_read, MoltObject::from_int(43).bits());
                dec_ref_bits(py, direct_read);
                dec_ref_bits(py, MoltObject::from_ptr(wrapper).bits());
                dec_ref_bits(py, member_name);
                dec_ref_bits(py, object_class_name);
                let root = obj_from_bits(classes.type_obj).as_ptr().unwrap();
                let root_namespace = class_dict_bits(root);
                let root_dictionary = obj_from_bits(root_namespace).as_ptr().unwrap();
                let root_name = attr_name_bits_from_bytes(py, b"__annotate__").unwrap();
                let root_version = class_layout_version_bits(root);
                let original_version = class_layout_version_bits(class);
                let staged = prepare_wrapper_members(py, 3, 14).unwrap();
                assert_eq!(class_dict_bits(class), dictionary);
                assert_eq!(class_layout_version_bits(class), original_version);
                assert!(
                    dict_get_in_place(py, pointer, annotations_name)
                        .expect("dictionary lookup succeeds")
                        .is_none()
                );
                assert!(
                    dict_get_in_place(py, root_dictionary, root_name)
                        .expect("dictionary lookup succeeds")
                        .is_none()
                );
                assert_eq!(class_layout_version_bits(root), root_version);
                drop(staged);
                assert_eq!(class_layout_version_bits(root), root_version);
                assert_eq!(class_dict_bits(root), root_namespace);
                assert_eq!(class_layout_version_bits(class), original_version);
                for minor in [14, 13, 12, 14] {
                    prepare_wrapper_members(py, 3, minor).unwrap().commit(py);
                    assert_eq!(class_dict_bits(class), dictionary);
                    assert_eq!(
                        dict_get_in_place(py, pointer, func_name)
                            .expect("dictionary lookup succeeds"),
                        Some(func)
                    );
                    for (kind, owner) in [
                        (MemberOwner::Staticmethod, classes.staticmethod),
                        (MemberOwner::Classmethod, classes.classmethod),
                        (MemberOwner::Property, classes.property),
                        (MemberOwner::Type, classes.type_obj),
                        (MemberOwner::Super, classes.super_type),
                        (MemberOwner::Coroutine, classes.coroutine),
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
                            let descriptor = dict_get_in_place(py, namespace, name)
                                .expect("dictionary lookup succeeds");
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
                                        && member.flavor != NativeDescriptorFlavor::RootMetadata
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
                dec_ref_bits(py, root_name);
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
                assert_eq!(property_name_value(py, object), Ok(Some(none)));
                assert!(layout::property_replace_name_bits(
                    py,
                    object,
                    missing_bits(py)
                ));
                assert_eq!(property_name_value(py, object), Ok(None));
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
