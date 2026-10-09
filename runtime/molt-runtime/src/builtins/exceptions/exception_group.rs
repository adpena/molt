//! The one BaseExceptionGroup semantic authority (CPython 3.12
//! `Objects/exceptions.c`): constructor admission (`BaseExceptionGroup_new`),
//! the default `derive`, `exceptiongroup_subset` and
//! `exceptiongroup_split_recursive`. Managed allocation, native C allocation
//! (`RuntimeHooks::exception_group_admit`), `split`, `subgroup`, `derive` and
//! the `except*` match/combine intrinsics all consume these primitives.

use super::*;
use molt_cpython_abi::hooks::ExceptionGroupRequest;
use molt_cpython_abi::{
    abi_types as cabi,
    api::{refcount as cref, sequences as cseq},
};

use super::storage::{ExceptionStorage, ExceptionValue as Owned, native_owned_value};

fn release(py: &PyToken<'_>, bits: u64) {
    if exception_pending(py) {
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, bits));
    } else {
        dec_ref_bits(py, bits);
    }
}

/// Raise unless a callee already published the failure.
fn fail<T: ExceptionSentinel>(py: &PyToken<'_>, kind: &str, message: &str) -> T {
    if exception_pending(py) {
        T::exception_sentinel()
    } else {
        raise_exception(py, kind, message)
    }
}

fn alloc_owned_str<'a, 'py>(py: &'a PyToken<'py>, text: &[u8]) -> Option<Owned<'a, 'py>> {
    let ptr = alloc_string(py, text);
    if ptr.is_null() {
        return fail(py, "MemoryError", "string allocation failed");
    }
    Some(Owned::adopt(py, MoltObject::from_ptr(ptr).bits()))
}

fn alloc_owned_tuple<'a, 'py>(py: &'a PyToken<'py>, items: &[u64]) -> Option<Owned<'a, 'py>> {
    let ptr = alloc_tuple(py, items);
    if ptr.is_null() {
        return fail(py, "MemoryError", "tuple allocation failed");
    }
    Some(Owned::adopt(py, MoltObject::from_ptr(ptr).bits()))
}

fn alloc_owned_list<'a, 'py>(py: &'a PyToken<'py>) -> Option<Owned<'a, 'py>> {
    let ptr = alloc_list(py, &[]);
    if ptr.is_null() {
        return fail(py, "MemoryError", "list allocation failed");
    }
    Some(Owned::adopt(py, MoltObject::from_ptr(ptr).bits()))
}

fn list_append(py: &PyToken<'_>, list: &Owned<'_, '_>, item: u64) -> Option<()> {
    let Some(ptr) = obj_from_bits(list.bits()).as_ptr() else {
        return fail(py, "SystemError", "exception group partition is not a list");
    };
    if unsafe { crate::object::list_mutation::append(py, ptr, item) } {
        Some(())
    } else {
        fail(py, "MemoryError", "list allocation failed")
    }
}

#[cfg(test)]
pub(crate) fn exception_group_message_bits(_py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    let bits = unsafe { exception_msg_bits(ptr) };
    if exception_field_is_missing(bits) {
        MoltObject::none().bits()
    } else {
        bits
    }
}

#[cfg(test)]
pub(crate) fn exception_group_exceptions_bits(_py: &PyToken<'_>, ptr: *mut u8) -> Option<u64> {
    let bits =
        unsafe { exception_typed_field_raw_bits(ptr, ExceptionTypedField::GroupExceptions)? };
    obj_from_bits(bits).as_ptr().and_then(|tuple_ptr| {
        (unsafe { object_type_id(tuple_ptr) } == TYPE_ID_TUPLE).then_some(bits)
    })
}

/// CPython `PyExceptionInstance_Check` plus real-type `Exception` ancestry.
/// Managed values use their class edge and native C objects their exact
/// native type; instance `__class__` is never consulted.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ExceptionIdentity {
    NotException,
    BaseException,
    Exception,
}

/// Keep the original tuple alive across predicates and derive callbacks.
/// Managed tuples expose their immutable slice; foreign tuples use the same
/// C tuple accessors as extension consumers, without invoking subtype iteration.
struct ExceptionTuple<'a, 'py> {
    tuple: Owned<'a, 'py>,
}

impl ExceptionTuple<'_, '_> {
    fn try_all(&self, mut consume: impl FnMut(u64) -> Option<bool>) -> Option<bool> {
        let py = self.tuple.py;
        let Some(ptr) = obj_from_bits(self.tuple.bits()).as_ptr() else {
            return fail(py, "SystemError", "exception group has no exceptions tuple");
        };
        if unsafe { object_type_id(ptr) } == TYPE_ID_TUPLE {
            let values = unsafe { crate::object::seq_access::pin_tuple(py, ptr) }?;
            for &value in values.iter() {
                if !consume(value)? {
                    return Some(false);
                }
            }
            return Some(true);
        }
        if unsafe { object_type_id(ptr) } != crate::TYPE_ID_FOREIGN {
            return fail(py, "SystemError", "exception group has no exceptions tuple");
        }
        unsafe {
            let tuple = std::ptr::with_exposed_provenance_mut(
                crate::object::foreign::foreign_ptr_from_obj(ptr),
            );
            let len = cseq::PyTuple_Size(tuple);
            if len < 0 {
                crate::cpython_abi_hooks::propagate_native_failure(
                    py,
                    "exception group tuple read",
                );
                return None;
            }
            for index in 0..len {
                let child = cseq::PyTuple_GetItem(tuple, index);
                if child.is_null() {
                    crate::cpython_abi_hooks::propagate_native_failure(
                        py,
                        "exception group tuple item",
                    );
                    return None;
                }
                cref::Py_INCREF(child);
                let child = native_owned_value(py, child)?;
                if !consume(child.bits())? {
                    return Some(false);
                }
            }
        }
        Some(true)
    }
}

fn native_identity(ptr: *mut u8) -> u8 {
    unsafe {
        molt_cpython_abi::api::errors::native_exception_identity(
            core::ptr::with_exposed_provenance_mut(crate::object::foreign::foreign_ptr_from_obj(
                ptr,
            )),
        )
    }
}

fn exception_identity(py: &PyToken<'_>, bits: u64) -> ExceptionIdentity {
    if let Some(ptr) = obj_from_bits(bits).as_ptr()
        && unsafe { object_type_id(ptr) } == crate::TYPE_ID_FOREIGN
    {
        let identity = native_identity(ptr);
        return if identity & molt_cpython_abi::api::errors::NATIVE_EXCEPTION_INSTANCE == 0 {
            ExceptionIdentity::NotException
        } else if identity & molt_cpython_abi::api::errors::NATIVE_EXCEPTION_SUBCLASS != 0 {
            ExceptionIdentity::Exception
        } else {
            ExceptionIdentity::BaseException
        };
    }
    if !exception_is_instance(py, bits) {
        return ExceptionIdentity::NotException;
    }
    let builtins = builtin_classes(py);
    let class = type_of_bits(py, bits);
    if issubclass_bits(class, builtins.exception) {
        ExceptionIdentity::Exception
    } else {
        ExceptionIdentity::BaseException
    }
}

fn exception_group_storage(py: &PyToken<'_>, bits: u64) -> Option<ExceptionStorage> {
    let storage = ExceptionStorage::for_exception(py, bits)?;
    let is_group = match storage {
        ExceptionStorage::Managed(ptr) => issubclass_bits(
            unsafe { object_class_bits(ptr) },
            builtin_classes(py).base_exception_group,
        ),
        ExceptionStorage::Native(ptr) => unsafe {
            molt_cpython_abi::api::typeobj::PyType_IsSubtype(
                ptr.as_ref()?.ob_type,
                &raw mut cabi::PyExc_BaseExceptionGroup,
            ) != 0
        },
    };
    is_group.then_some(storage)
}

/// Requested constructor identity. Each allocator derives it from its own
/// canonical class authority: the runtime class edge, or the exact native type
/// object and its native subtype ancestry.
pub(crate) enum ExceptionGroupClass<'a> {
    Runtime(u64),
    Native {
        request: ExceptionGroupRequest,
        name: &'a [u8],
    },
}

impl ExceptionGroupClass<'_> {
    fn request(&self, py: &PyToken<'_>) -> ExceptionGroupRequest {
        match *self {
            Self::Native { request, .. } => request,
            Self::Runtime(class) => {
                let builtins = builtin_classes(py);
                if class == builtins.base_exception_group {
                    ExceptionGroupRequest::BaseExceptionGroup
                } else if class == builtins.exception_group {
                    ExceptionGroupRequest::ExceptionGroup
                } else if issubclass_bits(class, builtins.exception) {
                    ExceptionGroupRequest::ExceptionSubclass
                } else {
                    ExceptionGroupRequest::BaseExceptionSubclass
                }
            }
        }
    }

    /// The requested type's `tp_name`, formatted as CPython's `%.200s`.
    fn c_name(&self) -> String {
        let name = match *self {
            Self::Native { name, .. } => name.to_vec(),
            Self::Runtime(class) => crate::object::ops_format::format_class_name_bytes(class),
        };
        String::from_utf8_lossy(&name[..name.len().min(200)]).into_owned()
    }
}

/// Successful CPython `BaseExceptionGroup_new` admission.
struct Admission<'a, 'py> {
    message: Owned<'a, 'py>,
    /// The canonical `PySequence_Tuple` result.
    exceptions: Owned<'a, 'py>,
    /// The exact BaseExceptionGroup request allocates ExceptionGroup instead.
    narrow: bool,
}

/// CPython 3.12 `BaseExceptionGroup_new` admission (CPy:697-794): argument
/// shape, sequence admission, `PySequence_Tuple`, item identity and the class
/// decision. `args` is a positional tuple the caller keeps alive.
fn exception_group_admit<'a, 'py>(
    py: &'a PyToken<'py>,
    class: &ExceptionGroupClass<'_>,
    args: u64,
) -> Option<Admission<'a, 'py>> {
    let Some(args) = obj_from_bits(args)
        .as_ptr()
        .and_then(|ptr| unsafe { crate::object::seq_access::pin_tuple(py, ptr) })
    else {
        return fail(
            py,
            "SystemError",
            "exception group arguments must be a tuple",
        );
    };
    // PyArg_ParseTuple(args, "UO:BaseExceptionGroup.__new__", ...)
    if args.len() != 2 {
        let message = format!(
            "BaseExceptionGroup.__new__() takes exactly 2 arguments ({} given)",
            args.len()
        );
        return fail(py, "TypeError", &message);
    }
    let (message, exceptions) = (args[0], args[1]);
    if !issubclass_bits(type_of_bits(py, message), builtin_classes(py).str) {
        let label = if obj_from_bits(message).is_none() {
            std::borrow::Cow::Borrowed("None")
        } else {
            type_name(py, obj_from_bits(message))
        };
        let message = format!("BaseExceptionGroup.__new__() argument 1 must be str, not {label}");
        return fail(py, "TypeError", &message);
    }
    let message = Owned::pin(py, message);
    if crate::object::ops::sequence_check_bits(py, exceptions) != 1 {
        return fail(
            py,
            "TypeError",
            "second argument (exceptions) must be a sequence",
        );
    }
    // PySequence_Tuple: an exact tuple keeps its identity, an exact list is
    // copied, and every subtype iterates under the target's hint policy.
    let Some(exceptions) = (unsafe { tuple_from_iter_bits(py, exceptions) }) else {
        return fail(py, "MemoryError", "tuple allocation failed");
    };
    let exceptions = Owned::adopt(py, exceptions);
    let Some(tuple) = obj_from_bits(exceptions.bits()).as_ptr() else {
        return fail(
            py,
            "SystemError",
            "PySequence_Tuple did not produce a tuple",
        );
    };
    let verdict = unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(tuple, |items| {
            if items.is_empty() {
                return Err(None);
            }
            let mut nested_base = false;
            for (index, &item) in items.iter().enumerate() {
                match exception_identity(py, item) {
                    ExceptionIdentity::NotException => return Err(Some(index)),
                    ExceptionIdentity::BaseException => nested_base = true,
                    ExceptionIdentity::Exception => {}
                }
            }
            Ok(nested_base)
        })
    };
    let nested_base = match verdict {
        Some(Ok(nested_base)) => nested_base,
        Some(Err(None)) => {
            return fail(
                py,
                "ValueError",
                "second argument (exceptions) must be a non-empty sequence",
            );
        }
        Some(Err(Some(index))) => {
            let message =
                format!("Item {index} of second argument (exceptions) is not an exception");
            return fail(py, "ValueError", &message);
        }
        None => {
            return fail(
                py,
                "SystemError",
                "PySequence_Tuple did not produce a tuple",
            );
        }
    };
    let narrow = match class.request(py) {
        ExceptionGroupRequest::BaseExceptionGroup => !nested_base,
        ExceptionGroupRequest::ExceptionGroup if nested_base => {
            return fail(
                py,
                "TypeError",
                "Cannot nest BaseExceptions in an ExceptionGroup",
            );
        }
        ExceptionGroupRequest::ExceptionSubclass if nested_base => {
            let message = format!("Cannot nest BaseExceptions in '{}'", class.c_name());
            return fail(py, "TypeError", &message);
        }
        _ => false,
    };
    Some(Admission {
        message,
        exceptions,
        narrow,
    })
}

/// Allocate one managed group after admission. The original positional tuple
/// is the stored `args` (`BaseException_new`).
fn exception_group_allocate(
    py: &PyToken<'_>,
    class: u64,
    args: u64,
    admission: Admission<'_, '_>,
) -> Option<u64> {
    let ptr = alloc_exception_obj(
        py,
        class,
        admission.message.bits(),
        args,
        MoltObject::none().bits(),
    );
    if ptr.is_null() {
        return fail(py, "MemoryError", "exception group allocation failed");
    }
    let group = Owned::adopt(py, MoltObject::from_ptr(ptr).bits());
    if let Err(message) = exception_typed_field_replace_internal(
        py,
        group.bits(),
        ExceptionTypedField::GroupExceptions,
        admission.exceptions.bits(),
    ) {
        return fail(py, "SystemError", message);
    }
    Some(group.into_bits())
}

/// Managed `BaseExceptionGroup.__new__`. Consumes `args_bits`.
pub(crate) fn alloc_exception_group_from_class_bits(
    py: &PyToken<'_>,
    class_bits: u64,
    args_bits: u64,
) -> *mut u8 {
    let args = Owned::adopt(py, args_bits);
    let Some(admission) =
        exception_group_admit(py, &ExceptionGroupClass::Runtime(class_bits), args.bits())
    else {
        return std::ptr::null_mut();
    };
    let class = if admission.narrow {
        builtin_classes(py).exception_group
    } else {
        class_bits
    };
    exception_group_allocate(py, class, args.bits(), admission)
        .and_then(|bits| obj_from_bits(bits).as_ptr())
        .unwrap_or(std::ptr::null_mut())
}

/// Native C allocation (`RuntimeHooks::exception_group_admit`): the same
/// admission with the requested identity of the exact native type. Returns
/// owned message and exceptions handles plus the narrowing decision.
pub(crate) fn exception_group_admit_native(
    py: &PyToken<'_>,
    request: ExceptionGroupRequest,
    name: &[u8],
    args: u64,
) -> Option<(u64, u64, bool)> {
    let admission =
        exception_group_admit(py, &ExceptionGroupClass::Native { request, name }, args)?;
    let narrow = admission.narrow;
    Some((
        admission.message.into_bits(),
        admission.exceptions.into_bits(),
        narrow,
    ))
}

/// `PyObject_CallObject(PyExc_BaseExceptionGroup, (message, exceptions))`:
/// the canonical class-call authority, including exact-class narrowing.
fn exception_group_construct(py: &PyToken<'_>, message: u64, exceptions: u64) -> Option<u64> {
    let Some(class) = obj_from_bits(builtin_classes(py).base_exception_group).as_ptr() else {
        return fail(py, "SystemError", "BaseExceptionGroup is not initialized");
    };
    let result = unsafe {
        crate::call::bind::call_bind_capi(
            py,
            MoltObject::from_ptr(class).bits(),
            None,
            &[message, exceptions],
            MoltObject::none().bits(),
        )
    };
    if exception_pending(py) {
        release(py, result);
        return None;
    }
    Some(result)
}

/// CPython `_exceptiongroup_split_matcher_type` for `split`, `subgroup` and
/// the `except*` handler. Values are borrowed from the caller's arguments.
enum GroupMatcher {
    /// `PyErr_GivenExceptionMatches` against one exception class.
    Class(u64),
    /// `PyErr_GivenExceptionMatches` against each class of a tuple.
    Classes(u64),
    /// A predicate called exactly once per node.
    Predicate(u64),
}

impl GroupMatcher {
    /// CPython `get_matcher_type`. CPython 3.12 (CPy:989-1021) admits a
    /// function, an exception class or an exact tuple of exception classes,
    /// including the empty tuple; 3.13+ admits any callable that is not a
    /// class in place of the function.
    fn parse(py: &PyToken<'_>, value: u64) -> Option<Self> {
        let builtins = builtin_classes(py);
        let any_callable = crate::object::ops_sys::runtime_target_at_least(py, 3, 13);
        let pointer = obj_from_bits(value).as_ptr();
        let is_class = pointer.is_some_and(|ptr| unsafe {
            object_type_id(ptr) == TYPE_ID_TYPE
                || (object_type_id(ptr) == crate::TYPE_ID_FOREIGN
                    && molt_cpython_abi::api::typeobj::PyType_Check(
                        std::ptr::with_exposed_provenance_mut(
                            crate::object::foreign::foreign_ptr_from_obj(ptr),
                        ),
                    ) != 0)
        });
        let predicate = if any_callable {
            !is_class && crate::builtins::callable::is_callable_impl(py, value)
        } else {
            type_of_bits(py, value) == builtins.function
        };
        if predicate {
            return Some(Self::Predicate(value));
        }
        if exception_is_class(py, value) {
            return Some(Self::Class(value));
        }
        let exact_tuple = pointer.is_some_and(|ptr| unsafe {
            (object_type_id(ptr) == TYPE_ID_TUPLE && type_of_bits(py, value) == builtins.tuple)
                || (object_type_id(ptr) == crate::TYPE_ID_FOREIGN
                    && cseq::PyTuple_CheckExact(std::ptr::with_exposed_provenance_mut(
                        crate::object::foreign::foreign_ptr_from_obj(ptr),
                    )) != 0)
        });
        if exact_tuple
            && (ExceptionTuple {
                tuple: Owned::pin(py, value),
            })
            .try_all(|class| {
                let valid = exception_is_class(py, class);
                (!exception_pending(py)).then_some(valid)
            })?
        {
            return Some(Self::Classes(value));
        }
        let message = if any_callable {
            "expected an exception type, a tuple of exception types, or a callable (other than a class)"
        } else {
            "expected a function, exception type or tuple of exception types"
        };
        fail(py, "TypeError", message)
    }

    /// An `except*` handler already validated as a class or tuple of classes.
    fn handler(value: u64) -> Self {
        let tuple = obj_from_bits(value).as_ptr().is_some_and(|ptr| unsafe {
            object_type_id(ptr) == TYPE_ID_TUPLE
                || (object_type_id(ptr) == crate::TYPE_ID_FOREIGN
                    && cseq::PyTuple_Check(std::ptr::with_exposed_provenance_mut(
                        crate::object::foreign::foreign_ptr_from_obj(ptr),
                    )) != 0)
        });
        if tuple {
            Self::Classes(value)
        } else {
            Self::Class(value)
        }
    }

    /// CPython `exceptiongroup_split_check_match` (CPy:1023-1059); classes
    /// use the `except` clause authority (`PyErr_GivenExceptionMatches`).
    fn matches(&self, py: &PyToken<'_>, exc: u64) -> Option<bool> {
        match *self {
            Self::Class(class) => {
                let matched = exception_matches_type(py, exc, class);
                (!exception_pending(py)).then_some(matched)
            }
            Self::Classes(classes) => (ExceptionTuple {
                tuple: Owned::pin(py, classes),
            })
            .try_all(|class| {
                let matched = exception_matches_type(py, exc, class);
                (!exception_pending(py)).then_some(!matched)
            })
            .map(|unmatched| !unmatched),
            Self::Predicate(predicate) => {
                let result = unsafe { call_callable1(py, predicate, exc) };
                if exception_pending(py) {
                    release(py, result);
                    return None;
                }
                let result = Owned::adopt(py, result);
                let truth = is_truthy(py, obj_from_bits(result.bits()));
                drop(result);
                (!exception_pending(py)).then_some(truth)
            }
        }
    }
}

fn exception_group_parse_except_star_matcher(py: &PyToken<'_>, matcher_bits: u64) -> Option<u64> {
    if !super::validate_exception_handler(py, matcher_bits) {
        return None;
    }
    let base_group = builtin_classes(py).base_exception_group;
    let catches_group = match GroupMatcher::handler(matcher_bits) {
        GroupMatcher::Classes(classes) => !(ExceptionTuple {
            tuple: Owned::pin(py, classes),
        })
        .try_all(|class| {
            let matches = exception_class_is_subtype(py, class, base_group);
            (!exception_pending(py)).then_some(!matches)
        })?,
        GroupMatcher::Class(class) => exception_class_is_subtype(py, class, base_group),
        GroupMatcher::Predicate(_) => unreachable!("validated except* handler"),
    };
    if exception_pending(py) {
        return None;
    }
    if catches_group {
        let _ = raise_exception::<u64>(
            py,
            "TypeError",
            "catching ExceptionGroup with except* is not allowed. Use except instead.",
        );
        return None;
    }
    Some(matcher_bits)
}

struct GroupSplit<'a, 'py> {
    matched: Option<Owned<'a, 'py>>,
    rest: Option<Owned<'a, 'py>>,
}

/// CPython `exceptiongroup_split_recursive` (CPy:1066-1175). `exc` is borrowed
/// from a caller-pinned owner; the matcher runs once per node and the rest
/// partition is built only when requested.
unsafe fn exception_group_split<'a, 'py>(
    py: &'a PyToken<'py>,
    exc: u64,
    matcher: &GroupMatcher,
    construct_rest: bool,
) -> Option<GroupSplit<'a, 'py>> {
    if matcher.matches(py, exc)? {
        return Some(GroupSplit {
            matched: Some(Owned::pin(py, exc)),
            rest: None,
        });
    }
    let Some(group) = exception_group_storage(py, exc) else {
        return Some(GroupSplit {
            matched: None,
            rest: construct_rest.then(|| Owned::pin(py, exc)),
        });
    };
    // Callbacks below may replace the group's physical exceptions edge; the
    // pinned tuple keeps every borrowed child alive for the whole partition.
    let children = ExceptionTuple {
        tuple: group.typed_field(py, ExceptionTypedField::GroupExceptions)?,
    };
    let matched = alloc_owned_list(py)?;
    let rest = if construct_rest {
        Some(alloc_owned_list(py)?)
    } else {
        None
    };
    children.try_all(|child| {
        let part = {
            let _depth = crate::state::recursion::RecursionGuard::enter_with_message(
                py,
                "maximum recursion depth exceeded in exceptiongroup_split_recursive",
            )?;
            unsafe { exception_group_split(py, child, matcher, construct_rest) }?
        };
        if let Some(item) = part.matched {
            list_append(py, &matched, item.bits())?;
        }
        if let (Some(rest), Some(item)) = (rest.as_ref(), part.rest) {
            list_append(py, rest, item.bits())?;
        }
        Some(true)
    })?;
    let matched = unsafe { exception_group_subset(py, exc, group, &matched) }?;
    let rest = match rest {
        Some(rest) => unsafe { exception_group_subset(py, exc, group, &rest) }?,
        None => None,
    };
    Some(GroupSplit { matched, rest })
}

/// CPython `exceptiongroup_subset` (CPy:896-975): the Python-visible `derive`
/// builds the part, whose exact type must be a BaseExceptionGroup; the
/// original's traceback, context, cause and notes are then transferred.
unsafe fn exception_group_subset<'a, 'py>(
    py: &'a PyToken<'py>,
    orig: u64,
    original: ExceptionStorage,
    excs: &Owned<'a, 'py>,
) -> Option<Option<Owned<'a, 'py>>> {
    let Some(excs_ptr) = obj_from_bits(excs.bits()).as_ptr() else {
        return fail(py, "SystemError", "exception group partition is not a list");
    };
    if unsafe { crate::object::seq_access::len(excs_ptr) } == 0 {
        return Some(None);
    }
    let Some(name) = attr_name_bits_from_bytes(py, b"derive") else {
        return fail(py, "MemoryError", "attribute name allocation failed");
    };
    let name = Owned::adopt(py, name);
    let derive = crate::molt_get_attr_name(orig, name.bits());
    if exception_pending(py) {
        release(py, derive);
        return None;
    }
    let derive = Owned::adopt(py, derive);
    let derived = unsafe { call_callable1(py, derive.bits(), excs.bits()) };
    if exception_pending(py) {
        release(py, derived);
        return None;
    }
    let derived = Owned::adopt(py, derived);
    let Some(destination) = exception_group_storage(py, derived.bits()) else {
        return fail(
            py,
            "TypeError",
            "derive must return an instance of BaseExceptionGroup",
        );
    };
    unsafe { exception_group_copy_metadata(py, derived.bits(), destination, orig, original) }?;
    Some(Some(derived))
}

unsafe fn publish_slot(
    py: &PyToken<'_>,
    exception: *mut u8,
    field: ExceptionFieldSlot,
    value: u64,
) -> Option<()> {
    if unsafe { exception_publish_field_slot(py, exception, field, value) } {
        Some(())
    } else {
        fail(
            py,
            "SystemError",
            "exception ABI sidecar synchronization failed",
        )
    }
}

/// `exceptiongroup_subset` metadata (CPy:932-968): the traceback only when the
/// original has one, then context, then cause (setting `__suppress_context__`),
/// then an independent `__notes__` list through the attribute protocol.
unsafe fn exception_group_copy_metadata(
    py: &PyToken<'_>,
    derived: u64,
    destination: ExceptionStorage,
    original_bits: u64,
    original: ExceptionStorage,
) -> Option<()> {
    for field in [
        ExceptionFieldSlot::Traceback,
        ExceptionFieldSlot::Context,
        ExceptionFieldSlot::Cause,
    ] {
        let value = original.metadata(py, field)?;
        if !matches!(field, ExceptionFieldSlot::Traceback) || !obj_from_bits(value.bits()).is_none()
        {
            destination.publish(py, field, value.bits())?;
        }
    }
    let name = intern_static_name(py, &runtime_state(py).interned.notes_name, b"__notes__");
    if name == 0 {
        return fail(py, "MemoryError", "attribute name allocation failed");
    }
    let notes = unsafe {
        attr_lookup_ptr_allow_missing(py, obj_from_bits(original_bits).as_ptr().unwrap(), name)
    };
    if exception_pending(py) {
        if let Some(notes) = notes {
            release(py, notes);
        }
        return None;
    }
    let Some(notes) = notes else {
        return Some(());
    };
    let notes = Owned::adopt(py, notes);
    // Non-sequence notes are ignored rather than reported here (CPy:961-967).
    if crate::object::ops::sequence_check_bits(py, notes.bits()) != 1 {
        return (!exception_pending(py)).then_some(());
    }
    let Some(copy) = (unsafe { crate::object::ops::list_from_iter_bits(py, notes.bits()) }) else {
        return fail(py, "MemoryError", "list allocation failed");
    };
    let copy = Owned::adopt(py, copy);
    drop(notes);
    let result = crate::builtins::attributes::molt_set_attr_name(derived, name, copy.bits());
    release(py, result);
    (!exception_pending(py)).then_some(())
}

fn exception_group_pair(
    py: &PyToken<'_>,
    matched: Option<Owned<'_, '_>>,
    rest: Option<Owned<'_, '_>>,
) -> u64 {
    let none = MoltObject::none().bits();
    let pair = alloc_tuple(
        py,
        &[
            matched.as_ref().map_or(none, |item| item.bits()),
            rest.as_ref().map_or(none, |item| item.bits()),
        ],
    );
    if pair.is_null() {
        return fail(py, "MemoryError", "tuple allocation failed");
    }
    MoltObject::from_ptr(pair).bits()
}

/// `BaseExceptionGroup` method-descriptor receiver admission.
fn exception_group_receiver(
    py: &PyToken<'_>,
    receiver: u64,
    method: &str,
) -> Option<ExceptionStorage> {
    if let Some(storage) = exception_group_storage(py, receiver) {
        return Some(storage);
    }
    let message = format!(
        "descriptor '{method}' for 'BaseExceptionGroup' objects doesn't apply to a '{}' object",
        type_name(py, obj_from_bits(receiver))
    );
    fail(py, "TypeError", &message)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exceptiongroup_init(self_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(storage) = ExceptionStorage::for_exception(_py, self_bits) else {
            if !obj_from_bits(args_bits).is_none() {
                release(_py, args_bits);
            }
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "exception init expects exception instance",
            );
        };
        let norm_bits = exception_normalize_args(_py, args_bits);
        if obj_from_bits(norm_bits).is_none() {
            if !obj_from_bits(args_bits).is_none() {
                dec_ref_bits(_py, args_bits);
            }
            return MoltObject::none().bits();
        }
        let _ = storage.publish(_py, ExceptionFieldSlot::Args, norm_bits);
        dec_ref_bits(_py, norm_bits);
        if !obj_from_bits(args_bits).is_none() {
            dec_ref_bits(_py, args_bits);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exceptiongroup_subgroup(self_bits: u64, matcher_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none = MoltObject::none().bits();
        if exception_group_receiver(_py, self_bits, "subgroup").is_none() {
            return none;
        }
        let Some(matcher) = GroupMatcher::parse(_py, matcher_bits) else {
            return none;
        };
        match unsafe { exception_group_split(_py, self_bits, &matcher, false) } {
            Some(split) => split.matched.map_or(none, Owned::into_bits),
            None => none,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exceptiongroup_split(self_bits: u64, matcher_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none = MoltObject::none().bits();
        if exception_group_receiver(_py, self_bits, "split").is_none() {
            return none;
        }
        let Some(matcher) = GroupMatcher::parse(_py, matcher_bits) else {
            return none;
        };
        match unsafe { exception_group_split(_py, self_bits, &matcher, true) } {
            Some(split) => exception_group_pair(_py, split.matched, split.rest),
            None => none,
        }
    })
}

/// CPython `BaseExceptionGroup_derive` (CPy:877-893):
/// `BaseExceptionGroup(self.message, excs)` through the canonical constructor.
#[unsafe(no_mangle)]
pub extern "C" fn molt_exceptiongroup_derive(self_bits: u64, exceptions_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none = MoltObject::none().bits();
        let Some(group) = exception_group_receiver(_py, self_bits, "derive") else {
            return none;
        };
        let Some(message) = group.typed_field(_py, ExceptionTypedField::GroupMessage) else {
            return none;
        };
        exception_group_construct(_py, message.bits(), exceptions_bits).unwrap_or(none)
    })
}

/// `except*` matching. A group is partitioned by the shared split authority
/// (calling `derive` for each non-empty part); a matching naked exception is
/// wrapped with `_PyExc_CreateExceptionGroup("", (exc,))`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_exceptiongroup_match(exc_bits: u64, matcher_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none = MoltObject::none().bits();
        if obj_from_bits(exc_bits).is_none() {
            return exception_group_pair(_py, None, None);
        }
        let Some(handler) = exception_group_parse_except_star_matcher(_py, matcher_bits) else {
            return none;
        };
        let Some(exception) = ExceptionStorage::for_exception(_py, exc_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected exception object");
        };
        let matcher = GroupMatcher::handler(handler);
        if exception_group_storage(_py, exc_bits).is_some() {
            return match unsafe { exception_group_split(_py, exc_bits, &matcher, true) } {
                Some(split) => exception_group_pair(_py, split.matched, split.rest),
                None => none,
            };
        }
        match matcher.matches(_py, exc_bits) {
            Some(true) => {}
            Some(false) => {
                return exception_group_pair(_py, None, Some(Owned::pin(_py, exc_bits)));
            }
            None => return none,
        }
        let Some(message) = alloc_owned_str(_py, b"") else {
            return none;
        };
        let Some(items) = alloc_owned_tuple(_py, &[exc_bits]) else {
            return none;
        };
        let Some(group) = exception_group_construct(_py, message.bits(), items.bits()) else {
            return none;
        };
        let group = Owned::adopt(_py, group);
        // The wrapper reports the matched exception's traceback.
        let Some(group_ptr) = obj_from_bits(group.bits()).as_ptr() else {
            return fail(
                _py,
                "SystemError",
                "exception group construction returned no object",
            );
        };
        let Some(traceback) = exception.metadata(_py, ExceptionFieldSlot::Traceback) else {
            return none;
        };
        if unsafe {
            publish_slot(
                _py,
                group_ptr,
                ExceptionFieldSlot::Traceback,
                traceback.bits(),
            )
        }
        .is_none()
        {
            return none;
        }
        exception_group_pair(_py, Some(group), None)
    })
}

/// `except*` reraise combination: `_PyExc_CreateExceptionGroup("", raised)`
/// (CPy:810-824) through the canonical constructor.
#[unsafe(no_mangle)]
pub extern "C" fn molt_exceptiongroup_combine(list_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none = MoltObject::none().bits();
        let Some(message) = alloc_owned_str(_py, b"") else {
            return none;
        };
        exception_group_construct(_py, message.bits(), list_bits).unwrap_or(none)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alloc_dict_with_pairs;
    use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
    use std::sync::atomic::{AtomicU64, Ordering};

    static GENERIC_SEQUENCE_ITEMS: AtomicU64 = AtomicU64::new(0);
    static PREDICATE_CALLS: AtomicU64 = AtomicU64::new(0);
    static PREDICATE_TARGET: AtomicU64 = AtomicU64::new(0);

    extern "C" fn generic_sequence_getitem(_self_bits: u64, index_bits: u64) -> u64 {
        crate::with_gil_entry_nopanic!(_py, {
            let Some(index) = to_i64(obj_from_bits(index_bits)) else {
                return raise_exception::<u64>(_py, "TypeError", "index must be int");
            };
            let items_bits = GENERIC_SEQUENCE_ITEMS.load(Ordering::Acquire);
            let Some(items_ptr) = obj_from_bits(items_bits).as_ptr() else {
                return raise_exception::<u64>(_py, "RuntimeError", "test sequence unavailable");
            };
            let Ok(index) = usize::try_from(index) else {
                return raise_exception::<u64>(_py, "IndexError", "test sequence exhausted");
            };
            let mut bits = 0;
            if crate::object::seq_access::read_item_owned(items_ptr, index, &mut bits) == 0 {
                return raise_exception::<u64>(_py, "IndexError", "test sequence exhausted");
            }
            bits
        })
    }

    /// A Python-function predicate that records every node it inspects.
    extern "C" fn counting_predicate(exc_bits: u64) -> u64 {
        PREDICATE_CALLS.fetch_add(1, Ordering::SeqCst);
        MoltObject::from_bool(exc_bits == PREDICATE_TARGET.load(Ordering::SeqCst)).bits()
    }

    fn heap_refcount(bits: u64) -> u32 {
        let ptr = obj_from_bits(bits).as_ptr().expect("heap object");
        unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    fn generic_sequence(_py: &PyToken<'_>, items_bits: u64) -> (u64, u64, u64) {
        GENERIC_SEQUENCE_ITEMS.store(items_bits, Ordering::Release);
        let function_ptr = crate::builtins::functions::alloc_runtime_function_obj(
            _py,
            crate::builtins::functions::runtime_fn_addr(
                "exception_group_test_getitem",
                generic_sequence_getitem as *const (),
            ),
            2,
        );
        assert!(!function_ptr.is_null());
        let function_bits = MoltObject::from_ptr(function_ptr).bits();
        let name_ptr = alloc_string(_py, b"ExceptionGroupGenericSequence");
        let getitem_ptr = alloc_string(_py, b"__getitem__");
        let namespace_ptr = alloc_dict_with_pairs(
            _py,
            &[MoltObject::from_ptr(getitem_ptr).bits(), function_bits],
        );
        assert!(!name_ptr.is_null() && !getitem_ptr.is_null());
        assert!(!namespace_ptr.is_null());
        let namespace_bits = MoltObject::from_ptr(namespace_ptr).bits();
        let class_bits = crate::builtins::types::molt_type_new(
            builtin_classes(_py).type_obj,
            MoltObject::from_ptr(name_ptr).bits(),
            MoltObject::none().bits(),
            namespace_bits,
            MoltObject::none().bits(),
        );
        assert!(!obj_from_bits(class_bits).is_none());
        let class_ptr = obj_from_bits(class_bits).as_ptr().expect("sequence class");
        let sequence_bits = unsafe { crate::alloc_instance_for_class(_py, class_ptr) };
        assert!(!obj_from_bits(sequence_bits).is_none());
        dec_ref_bits(_py, namespace_bits);
        dec_ref_bits(_py, MoltObject::from_ptr(getitem_ptr).bits());
        dec_ref_bits(_py, MoltObject::from_ptr(name_ptr).bits());
        (sequence_bits, class_bits, function_bits)
    }

    fn clear_generic_sequence() {
        GENERIC_SEQUENCE_ITEMS.store(0, Ordering::Release);
    }

    /// Allocate `class(message, children)` through the managed constructor.
    fn group(py: &PyToken<'_>, class: u64, message: &[u8], children: u64) -> u64 {
        let message_ptr = alloc_string(py, message);
        assert!(!message_ptr.is_null());
        let message_bits = MoltObject::from_ptr(message_ptr).bits();
        let args_ptr = alloc_tuple(py, &[message_bits, children]);
        assert!(!args_ptr.is_null());
        dec_ref_bits(py, message_bits);
        let group_ptr =
            alloc_exception_group_from_class_bits(py, class, MoltObject::from_ptr(args_ptr).bits());
        assert!(!group_ptr.is_null(), "group construction failed");
        assert!(!exception_pending(py));
        MoltObject::from_ptr(group_ptr).bits()
    }

    fn tuple_items(bits: u64) -> Vec<u64> {
        let ptr = obj_from_bits(bits).as_ptr().expect("tuple");
        unsafe {
            crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| items.to_vec())
        }
        .expect("exact tuple")
    }

    fn group_children(py: &PyToken<'_>, group_bits: u64) -> Vec<u64> {
        let ptr = obj_from_bits(group_bits).as_ptr().expect("group");
        tuple_items(exception_group_exceptions_bits(py, ptr).expect("canonical exceptions tuple"))
    }

    fn pending_message(py: &PyToken<'_>, kind: &str) -> String {
        let error = crate::exception_last_bits_noinc(py).expect("pending exception");
        assert!(
            super::super::exception_matches_builtin_name(py, error, kind),
            "pending exception is not {kind}"
        );
        let text = crate::builtins::exceptions::format_exception_message(
            py,
            obj_from_bits(error).as_ptr().expect("exception"),
        );
        clear_exception(py);
        text
    }

    #[test]
    fn constructor_generic_sequence_transfers_each_item_once() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let first_ptr = alloc_exception(_py, "ValueError", "first");
            let second_ptr = alloc_exception(_py, "TypeError", "second");
            assert!(!first_ptr.is_null() && !second_ptr.is_null());
            let first_bits = MoltObject::from_ptr(first_ptr).bits();
            let second_bits = MoltObject::from_ptr(second_ptr).bits();
            let items_ptr = alloc_tuple(_py, &[first_bits, second_bits]);
            assert!(!items_ptr.is_null());
            let items_bits = MoltObject::from_ptr(items_ptr).bits();
            let (sequence_bits, class_bits, function_bits) = generic_sequence(_py, items_bits);
            let first_baseline = heap_refcount(first_bits);
            let second_baseline = heap_refcount(second_bits);

            let group_bits = group(
                _py,
                builtin_classes(_py).exception_group,
                b"generic",
                sequence_bits,
            );
            assert_eq!(group_children(_py, group_bits), [first_bits, second_bits]);
            assert_eq!(heap_refcount(first_bits), first_baseline + 1);
            assert_eq!(heap_refcount(second_bits), second_baseline + 1);

            dec_ref_bits(_py, group_bits);
            assert_eq!(heap_refcount(first_bits), first_baseline);
            assert_eq!(heap_refcount(second_bits), second_baseline);
            clear_generic_sequence();
            dec_ref_bits(_py, sequence_bits);
            dec_ref_bits(_py, class_bits);
            dec_ref_bits(_py, function_bits);
            dec_ref_bits(_py, items_bits);
            dec_ref_bits(_py, first_bits);
            dec_ref_bits(_py, second_bits);
        });
    }

    #[test]
    fn exact_tuple_argument_is_the_canonical_exceptions_tuple() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let value_ptr = alloc_exception(_py, "ValueError", "value");
            assert!(!value_ptr.is_null());
            let value_bits = MoltObject::from_ptr(value_ptr).bits();
            let children_ptr = alloc_tuple(_py, &[value_bits]);
            assert!(!children_ptr.is_null());
            let children_bits = MoltObject::from_ptr(children_ptr).bits();

            let group_bits = group(
                _py,
                builtin_classes(_py).exception_group,
                b"identity",
                children_bits,
            );
            let group_ptr = obj_from_bits(group_bits).as_ptr().unwrap();
            assert_eq!(
                exception_group_exceptions_bits(_py, group_ptr),
                Some(children_bits),
                "PySequence_Tuple keeps an exact tuple's identity"
            );

            let list_ptr = alloc_list(_py, &[value_bits]);
            assert!(!list_ptr.is_null());
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            let copied_bits = group(
                _py,
                builtin_classes(_py).exception_group,
                b"copy",
                list_bits,
            );
            let copied_ptr = obj_from_bits(copied_bits).as_ptr().unwrap();
            let copied_tuple = exception_group_exceptions_bits(_py, copied_ptr).unwrap();
            assert_ne!(copied_tuple, list_bits);
            assert_eq!(tuple_items(copied_tuple), [value_bits]);

            for bits in [
                copied_bits,
                list_bits,
                group_bits,
                children_bits,
                value_bits,
            ] {
                dec_ref_bits(_py, bits);
            }
        });
    }

    #[test]
    fn exact_base_exception_group_narrows_only_for_exception_children() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let builtins = builtin_classes(_py);
            let value_ptr = alloc_exception(_py, "ValueError", "value");
            let interrupt_ptr = alloc_exception(_py, "KeyboardInterrupt", "interrupt");
            assert!(!value_ptr.is_null() && !interrupt_ptr.is_null());
            let value_bits = MoltObject::from_ptr(value_ptr).bits();
            let interrupt_bits = MoltObject::from_ptr(interrupt_ptr).bits();
            let exceptions_ptr = alloc_tuple(_py, &[value_bits]);
            let mixed_ptr = alloc_tuple(_py, &[value_bits, interrupt_bits]);
            assert!(!exceptions_ptr.is_null() && !mixed_ptr.is_null());
            let exceptions_bits = MoltObject::from_ptr(exceptions_ptr).bits();
            let mixed_bits = MoltObject::from_ptr(mixed_ptr).bits();

            let narrowed = group(
                _py,
                builtins.base_exception_group,
                b"narrow",
                exceptions_bits,
            );
            assert_eq!(
                unsafe { object_class_bits(obj_from_bits(narrowed).as_ptr().unwrap()) },
                builtins.exception_group
            );
            let kept = group(_py, builtins.base_exception_group, b"keep", mixed_bits);
            assert_eq!(
                unsafe { object_class_bits(obj_from_bits(kept).as_ptr().unwrap()) },
                builtins.base_exception_group
            );

            let message_ptr = alloc_string(_py, b"nest");
            assert!(!message_ptr.is_null());
            let message_bits = MoltObject::from_ptr(message_ptr).bits();
            let args_ptr = alloc_tuple(_py, &[message_bits, mixed_bits]);
            assert!(!args_ptr.is_null());
            let rejected = alloc_exception_group_from_class_bits(
                _py,
                builtins.exception_group,
                MoltObject::from_ptr(args_ptr).bits(),
            );
            assert!(rejected.is_null());
            assert_eq!(
                pending_message(_py, "TypeError"),
                "Cannot nest BaseExceptions in an ExceptionGroup"
            );

            for bits in [
                kept,
                narrowed,
                message_bits,
                mixed_bits,
                exceptions_bits,
                interrupt_bits,
                value_bits,
            ] {
                dec_ref_bits(_py, bits);
            }
        });
    }

    #[test]
    fn invalid_generic_sequence_releases_current_and_prior_items() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let exception_ptr = alloc_exception(_py, "ValueError", "valid first");
            let invalid_ptr = alloc_string(_py, b"not an exception");
            assert!(!exception_ptr.is_null() && !invalid_ptr.is_null());
            let exception_bits = MoltObject::from_ptr(exception_ptr).bits();
            let invalid_bits = MoltObject::from_ptr(invalid_ptr).bits();
            let items_ptr = alloc_tuple(_py, &[exception_bits, invalid_bits]);
            assert!(!items_ptr.is_null());
            let items_bits = MoltObject::from_ptr(items_ptr).bits();
            let (sequence_bits, class_bits, function_bits) = generic_sequence(_py, items_bits);
            let exception_baseline = heap_refcount(exception_bits);
            let invalid_baseline = heap_refcount(invalid_bits);

            let message_ptr = alloc_string(_py, b"invalid");
            assert!(!message_ptr.is_null());
            let message_bits = MoltObject::from_ptr(message_ptr).bits();
            let args_ptr = alloc_tuple(_py, &[message_bits, sequence_bits]);
            assert!(!args_ptr.is_null());
            let group_ptr = alloc_exception_group_from_class_bits(
                _py,
                builtin_classes(_py).exception_group,
                MoltObject::from_ptr(args_ptr).bits(),
            );
            assert!(group_ptr.is_null());
            assert_eq!(
                pending_message(_py, "ValueError"),
                "Item 1 of second argument (exceptions) is not an exception"
            );
            assert_eq!(heap_refcount(exception_bits), exception_baseline);
            assert_eq!(heap_refcount(invalid_bits), invalid_baseline);

            clear_generic_sequence();
            dec_ref_bits(_py, message_bits);
            dec_ref_bits(_py, sequence_bits);
            dec_ref_bits(_py, class_bits);
            dec_ref_bits(_py, function_bits);
            dec_ref_bits(_py, items_bits);
            dec_ref_bits(_py, exception_bits);
            dec_ref_bits(_py, invalid_bits);
        });
    }

    #[test]
    fn admission_oom_releases_every_item() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let exception_ptr = alloc_exception(_py, "ValueError", "owned");
            assert!(!exception_ptr.is_null());
            let exception_bits = MoltObject::from_ptr(exception_ptr).bits();
            let list_ptr = alloc_list(_py, &[exception_bits]);
            let message_ptr = alloc_string(_py, b"oom");
            assert!(!list_ptr.is_null() && !message_ptr.is_null());
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            let message_bits = MoltObject::from_ptr(message_ptr).bits();
            let args_ptr = alloc_tuple(_py, &[message_bits, list_bits]);
            assert!(!args_ptr.is_null());
            let baseline = heap_refcount(exception_bits);
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_memory: Some(0),
                ..Default::default()
            })));
            struct TrackerReset;
            impl Drop for TrackerReset {
                fn drop(&mut self) {
                    set_tracker(Box::new(UnlimitedTracker));
                }
            }
            let reset = TrackerReset;
            let group_ptr = alloc_exception_group_from_class_bits(
                _py,
                builtin_classes(_py).exception_group,
                MoltObject::from_ptr(args_ptr).bits(),
            );
            drop(reset);
            assert!(group_ptr.is_null());
            assert!(exception_pending(_py));
            clear_exception(_py);
            assert_eq!(heap_refcount(exception_bits), baseline);
            for bits in [message_bits, list_bits, exception_bits] {
                dec_ref_bits(_py, bits);
            }
        });
    }

    #[test]
    fn split_and_subgroup_run_the_predicate_once_per_node() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let first_ptr = alloc_exception(_py, "ValueError", "first");
            let second_ptr = alloc_exception(_py, "TypeError", "second");
            assert!(!first_ptr.is_null() && !second_ptr.is_null());
            let first_bits = MoltObject::from_ptr(first_ptr).bits();
            let second_bits = MoltObject::from_ptr(second_ptr).bits();
            let children_ptr = alloc_tuple(_py, &[first_bits, second_bits]);
            assert!(!children_ptr.is_null());
            let children_bits = MoltObject::from_ptr(children_ptr).bits();
            let group_bits = group(
                _py,
                builtin_classes(_py).exception_group,
                b"predicate",
                children_bits,
            );
            let predicate_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                _py,
                crate::builtins::functions::runtime_fn_addr(
                    "exception_group_test_predicate",
                    counting_predicate as *const (),
                ),
                1,
            );
            assert!(!predicate_ptr.is_null());
            let predicate_bits = MoltObject::from_ptr(predicate_ptr).bits();
            PREDICATE_TARGET.store(first_bits, Ordering::SeqCst);

            PREDICATE_CALLS.store(0, Ordering::SeqCst);
            let pair_bits = molt_exceptiongroup_split(group_bits, predicate_bits);
            assert!(!exception_pending(_py));
            assert_eq!(
                PREDICATE_CALLS.load(Ordering::SeqCst),
                3,
                "root, first and second are each inspected once"
            );
            let pair = tuple_items(pair_bits);
            assert_eq!(pair.len(), 2, "split returns a pair");
            let (matched, rest) = (pair[0], pair[1]);
            assert_eq!(group_children(_py, matched), [first_bits]);
            assert_eq!(group_children(_py, rest), [second_bits]);
            assert_eq!(
                unsafe { object_class_bits(obj_from_bits(matched).as_ptr().unwrap()) },
                builtin_classes(_py).exception_group,
                "the default derive constructs through BaseExceptionGroup narrowing"
            );

            PREDICATE_CALLS.store(0, Ordering::SeqCst);
            let subgroup_bits = molt_exceptiongroup_subgroup(group_bits, predicate_bits);
            assert!(!exception_pending(_py));
            assert_eq!(PREDICATE_CALLS.load(Ordering::SeqCst), 3);
            assert_eq!(group_children(_py, subgroup_bits), [first_bits]);

            PREDICATE_TARGET.store(group_bits, Ordering::SeqCst);
            PREDICATE_CALLS.store(0, Ordering::SeqCst);
            let whole_bits = molt_exceptiongroup_subgroup(group_bits, predicate_bits);
            assert_eq!(whole_bits, group_bits, "a matching root is returned itself");
            assert_eq!(PREDICATE_CALLS.load(Ordering::SeqCst), 1);

            PREDICATE_TARGET.store(0, Ordering::SeqCst);
            for bits in [
                whole_bits,
                subgroup_bits,
                pair_bits,
                predicate_bits,
                group_bits,
                children_bits,
                first_bits,
                second_bits,
            ] {
                dec_ref_bits(_py, bits);
            }
        });
    }

    #[test]
    fn default_derive_constructs_base_exception_group_with_narrowing() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let builtins = builtin_classes(_py);
            let interrupt_ptr = alloc_exception(_py, "KeyboardInterrupt", "interrupt");
            let value_ptr = alloc_exception(_py, "ValueError", "value");
            assert!(!interrupt_ptr.is_null() && !value_ptr.is_null());
            let interrupt_bits = MoltObject::from_ptr(interrupt_ptr).bits();
            let value_bits = MoltObject::from_ptr(value_ptr).bits();
            let base_children = alloc_tuple(_py, &[interrupt_bits]);
            assert!(!base_children.is_null());
            let base_children = MoltObject::from_ptr(base_children).bits();
            let base_group = group(_py, builtins.base_exception_group, b"base", base_children);

            let values = alloc_list(_py, &[value_bits]);
            assert!(!values.is_null());
            let values = MoltObject::from_ptr(values).bits();
            let derived = molt_exceptiongroup_derive(base_group, values);
            assert!(!exception_pending(_py));
            let derived_ptr = obj_from_bits(derived).as_ptr().unwrap();
            assert_eq!(
                unsafe { object_class_bits(derived_ptr) },
                builtins.exception_group,
                "derive narrows exactly like BaseExceptionGroup(msg, excs)"
            );
            assert_eq!(group_children(_py, derived), [value_bits]);
            let derived_message = exception_group_message_bits(_py, derived_ptr);
            let base_message =
                exception_group_message_bits(_py, obj_from_bits(base_group).as_ptr().unwrap());
            assert_eq!(derived_message, base_message, "derive reuses self.message");

            for bits in [
                derived,
                values,
                base_group,
                base_children,
                value_bits,
                interrupt_bits,
            ] {
                dec_ref_bits(_py, bits);
            }
        });
    }
}
