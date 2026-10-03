use crate::builtins::attr::lookup_special_method_bits;
use crate::object::HEADER_FLAG_COROUTINE;
use crate::*;

/// Shared admission for the public tp_new wrapper and native allocation.
/// The display label affects diagnostics only; real MRO ancestry owns subtype
/// admission and never dispatches an overridable __subclasscheck__ hook.
pub(crate) fn native_constructor_receiver(
    py: &PyToken<'_>,
    declaring: u64,
    receiver: Option<u64>,
    label: &str,
) -> Option<(u64, *mut u8)> {
    let Some(class) = receiver else {
        return raise_exception(
            py,
            "TypeError",
            &format!("{label}.__new__(): not enough arguments"),
        );
    };
    let Some(ptr) = obj_from_bits(class)
        .as_ptr()
        .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_TYPE })
    else {
        return raise_exception(
            py,
            "TypeError",
            &format!(
                "{label}.__new__(X): X is not a type object ({})",
                type_name(py, obj_from_bits(class)),
            ),
        );
    };
    if !unsafe { crate::object::class_layout::is_real_subtype(py, class, declaring) } {
        let name = class_name_for_error(class);
        return raise_exception(
            py,
            "TypeError",
            &format!("{label}.__new__({name}): {name} is not a subtype of {label}"),
        );
    }
    let builtins = builtin_classes(py);
    if declaring == builtins.int && class == builtins.bool {
        return raise_exception(
            py,
            "TypeError",
            "int.__new__(bool) is not safe, use bool.__new__()",
        );
    }
    Some((class, ptr))
}

#[derive(Clone, Copy)]
pub(crate) enum ClassInfoProtocol {
    Instance,
    Subclass,
}

fn classinfo_type_error_message(protocol: ClassInfoProtocol) -> &'static str {
    match protocol {
        ClassInfoProtocol::Instance => {
            "isinstance() arg 2 must be a type, a tuple of types, or a union"
        }
        ClassInfoProtocol::Subclass => {
            "issubclass() arg 2 must be a class, a tuple of classes, or a union"
        }
    }
}

fn classinfo_protocol_name_bits(py: &PyToken<'_>, protocol: ClassInfoProtocol) -> u64 {
    match protocol {
        ClassInfoProtocol::Instance => intern_static_name(
            py,
            &runtime_state(py).interned.instancecheck_name,
            b"__instancecheck__",
        ),
        ClassInfoProtocol::Subclass => intern_static_name(
            py,
            &runtime_state(py).interned.subclasscheck_name,
            b"__subclasscheck__",
        ),
    }
}

fn real_class(py: &PyToken<'_>, bits: u64) -> Option<bool> {
    if obj_from_bits(bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TYPE })
    {
        return Some(true);
    }
    match unsafe { crate::object::class_layout::real_class_view(bits) } {
        Ok(class) => Some(class.is_some()),
        Err(()) => {
            crate::cpython_abi_hooks::propagate_native_failure(py, "class projection");
            None
        }
    }
}

fn structural_classinfo_match(
    py: &PyToken<'_>,
    value: u64,
    class: u64,
    protocol: ClassInfoProtocol,
) -> Option<bool> {
    if !real_class(py, class)? {
        return raise_exception(py, "TypeError", classinfo_type_error_message(protocol));
    }
    let result = unsafe {
        match protocol {
            ClassInfoProtocol::Instance => {
                crate::object::class_layout::try_is_real_instance(py, value, class)
            }
            ClassInfoProtocol::Subclass => {
                if !real_class(py, value)? {
                    return raise_exception(py, "TypeError", "issubclass() arg 1 must be a class");
                }
                crate::object::class_layout::try_is_real_subtype(py, value, class)
            }
        }
    };
    match result {
        Ok(matched) => Some(matched),
        Err(()) => {
            crate::cpython_abi_hooks::propagate_native_failure(py, "class relation");
            None
        }
    }
}

/// Tuple storage for both public classinfo and observable __bases__. Keep the
/// tuple and each projected element owned across arbitrary callbacks, without
/// invoking a tuple subclass's iteration, indexing or length overrides.
struct ClassInfoTuple<'a, 'py> {
    owner: crate::builtins::exceptions::ExceptionValue<'a, 'py>,
    storage: ClassInfoTupleStorage,
    len: usize,
}

#[derive(Clone, Copy)]
enum ClassInfoTupleStorage {
    Managed(*mut u8),
    Native(*mut molt_cpython_abi::abi_types::PyObject),
}

impl<'a, 'py> ClassInfoTuple<'a, 'py> {
    /// Outer None is failure; Some(None) is a non-tuple without an error.
    fn from_bits(py: &'a PyToken<'py>, bits: u64) -> Option<Option<Self>> {
        use crate::builtins::exceptions::ExceptionValue;
        let Some(pointer) = obj_from_bits(bits).as_ptr() else {
            return Some(None);
        };
        let storage = match unsafe { object_type_id(pointer) } {
            TYPE_ID_TUPLE => ClassInfoTupleStorage::Managed(pointer),
            TYPE_ID_FOREIGN => unsafe {
                let native = std::ptr::with_exposed_provenance_mut(
                    crate::object::foreign::foreign_ptr_from_obj(pointer),
                );
                if molt_cpython_abi::api::sequences::PyTuple_Check(native) == 0 {
                    return Some(None);
                }
                ClassInfoTupleStorage::Native(native)
            },
            _ => return Some(None),
        };
        let owner = ExceptionValue::pin(py, bits);
        let len = match storage {
            ClassInfoTupleStorage::Managed(pointer) => unsafe {
                crate::object::seq_access::len(pointer)
            },
            ClassInfoTupleStorage::Native(pointer) => unsafe {
                let len = molt_cpython_abi::api::sequences::PyTuple_Size(pointer);
                if len < 0 {
                    crate::cpython_abi_hooks::propagate_native_failure(py, "classinfo tuple size");
                    return None;
                }
                len as usize
            },
        };
        Some(Some(Self {
            owner,
            storage,
            len,
        }))
    }

    fn item(&self, index: usize) -> Option<crate::builtins::exceptions::ExceptionValue<'a, 'py>> {
        use crate::builtins::exceptions::ExceptionValue;
        let py = self.owner.py;
        match self.storage {
            ClassInfoTupleStorage::Managed(pointer) => {
                let bits = unsafe { crate::object::seq_access::item(pointer, index) }?;
                Some(ExceptionValue::pin(py, bits))
            }
            ClassInfoTupleStorage::Native(pointer) => unsafe {
                let item =
                    molt_cpython_abi::api::sequences::PyTuple_GetItem(pointer, index as isize);
                if item.is_null() {
                    crate::cpython_abi_hooks::propagate_native_failure(py, "classinfo tuple item");
                    return None;
                }
                let Some(bits) = molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(item)
                else {
                    crate::cpython_abi_hooks::propagate_native_failure(
                        py,
                        "classinfo tuple projection",
                    );
                    return None;
                };
                Some(ExceptionValue::adopt(py, bits))
            },
        }
    }

    fn any(&self, mut matches: impl FnMut(u64) -> Option<bool>) -> Option<bool> {
        for index in 0..self.len {
            let item = self.item(index)?;
            if matches(item.bits())? {
                return Some(true);
            }
        }
        Some(false)
    }
}

/// Optional ordinary lookup masks only AttributeError. Presence is distinct
/// from failure, including for a native wrapper's tp_getattro callback.
fn classinfo_attribute<'a, 'py>(
    py: &'a PyToken<'py>,
    value: u64,
    name: &[u8],
) -> Option<Option<crate::builtins::exceptions::ExceptionValue<'a, 'py>>> {
    use crate::builtins::exceptions::ExceptionValue;
    let Some(pointer) = obj_from_bits(value).as_ptr() else {
        // Immediate builtin values cannot override attributes. Their __class__
        // still participates in abstract isinstance; they have no __bases__.
        return if name == b"__class__" {
            match unsafe { crate::object::class_layout::real_type_bits(py, value) } {
                Ok(class) => Some(Some(ExceptionValue::adopt(py, class))),
                Err(()) => {
                    crate::cpython_abi_hooks::propagate_native_failure(py, "instance type");
                    None
                }
            }
        } else {
            Some(None)
        };
    };
    let name = ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, name)?);
    let result = unsafe { attr_lookup_ptr_allow_missing(py, pointer, name.bits()) }
        .map(|bits| ExceptionValue::adopt(py, bits));
    if exception_pending(py) {
        None
    } else {
        Some(result)
    }
}

fn abstract_bases<'a, 'py>(
    py: &'a PyToken<'py>,
    class: u64,
) -> Option<Option<ClassInfoTuple<'a, 'py>>> {
    let Some(bases) = classinfo_attribute(py, class, b"__bases__")? else {
        return Some(None);
    };
    ClassInfoTuple::from_bits(py, bases.bits())
}

fn check_abstract_class(py: &PyToken<'_>, class: u64, message: &str) -> Option<()> {
    if abstract_bases(py, class)?.is_some() {
        Some(())
    } else {
        raise_exception(py, "TypeError", message)
    }
}

/// CPython abstract_issubclass: compare identity before looking up __bases__,
/// retain the old tuple until the next lookup completes, and tail-traverse a
/// single base without consuming recursive-call budget.
fn abstract_subclass(py: &PyToken<'_>, derived: u64, class: u64) -> Option<bool> {
    if derived == class {
        return Some(true);
    }
    let Some(mut bases) = abstract_bases(py, derived)? else {
        return Some(false);
    };
    loop {
        match bases.len {
            0 => return Some(false),
            1 => {
                let derived = bases.item(0)?;
                if derived.bits() == class {
                    return Some(true);
                }
                let next = abstract_bases(py, derived.bits());
                // The old tuple still owns the raw base. Release a projected
                // element pin before replacing that tuple, matching CPython
                // XSETREF cleanup order on success, absence and failure.
                drop(derived);
                let Some(next) = next? else {
                    return Some(false);
                };
                bases = next;
            }
            _ => {
                let _recursion = crate::state::recursion::RecursionGuard::enter_with_message(
                    py,
                    "maximum recursion depth exceeded in __issubclass__",
                )?;
                return bases.any(|base| abstract_subclass(py, base, class));
            }
        }
    }
}

/// The default type.__instancecheck__ observes __class__ after a failed real
/// relation. Abstract classinfo instead validates __bases__ and follows the
/// apparent class's abstract ancestry. Physical layout admission stays in
/// class_layout and must never use this callback-bearing public protocol.
pub(crate) fn instancecheck_default(py: &PyToken<'_>, value: u64, class: u64) -> Option<bool> {
    if real_class(py, class)? {
        if structural_classinfo_match(py, value, class, ClassInfoProtocol::Instance)? {
            return Some(true);
        }
        let Some(apparent) = classinfo_attribute(py, value, b"__class__")? else {
            return Some(false);
        };
        let actual = match unsafe { crate::object::class_layout::real_type_bits(py, value) } {
            Ok(actual) => crate::builtins::exceptions::ExceptionValue::adopt(py, actual),
            Err(()) => {
                crate::cpython_abi_hooks::propagate_native_failure(py, "instance type");
                return None;
            }
        };
        if apparent.bits() == actual.bits() || !real_class(py, apparent.bits())? {
            return Some(false);
        }
        return structural_classinfo_match(py, apparent.bits(), class, ClassInfoProtocol::Subclass);
    }
    check_abstract_class(
        py,
        class,
        classinfo_type_error_message(ClassInfoProtocol::Instance),
    )?;
    let Some(apparent) = classinfo_attribute(py, value, b"__class__")? else {
        return Some(false);
    };
    abstract_subclass(py, apparent.bits(), class)
}

pub(crate) fn subclasscheck_default(py: &PyToken<'_>, value: u64, class: u64) -> Option<bool> {
    if real_class(py, class)? && real_class(py, value)? {
        return structural_classinfo_match(py, value, class, ClassInfoProtocol::Subclass);
    }
    // A mixed pair validates derived first, even if class is a real type.
    // Neither validation may turn an attribute failure into a new TypeError.
    check_abstract_class(py, value, "issubclass() arg 1 must be a class")?;
    let is_union = obj_from_bits(class)
        .as_ptr()
        .is_some_and(|pointer| unsafe { object_type_id(pointer) == TYPE_ID_UNION });
    if !is_union {
        check_abstract_class(
            py,
            class,
            classinfo_type_error_message(ClassInfoProtocol::Subclass),
        )?;
    }
    abstract_subclass(py, value, class)
}

fn classinfo_default(
    py: &PyToken<'_>,
    value: u64,
    class: u64,
    protocol: ClassInfoProtocol,
) -> Option<bool> {
    match protocol {
        ClassInfoProtocol::Instance => instancecheck_default(py, value, class),
        ClassInfoProtocol::Subclass => subclasscheck_default(py, value, class),
    }
}

/// Traverse tuple/union members in observation order. Exact instance identity
/// and exact-type defaults precede special lookup; metaclass overrides precede
/// the abstract fallback. Only recursive paths consume recursion budget.
fn classinfo_match(
    py: &PyToken<'_>,
    value: u64,
    classinfo: u64,
    protocol: ClassInfoProtocol,
    hooks: bool,
) -> Option<bool> {
    use crate::builtins::exceptions::ExceptionValue;
    let owner = ExceptionValue::pin(py, classinfo);
    if hooks {
        if matches!(protocol, ClassInfoProtocol::Instance) {
            let actual = match unsafe { crate::object::class_layout::real_type_bits(py, value) } {
                Ok(actual) => ExceptionValue::adopt(py, actual),
                Err(()) => {
                    crate::cpython_abi_hooks::propagate_native_failure(py, "instance type");
                    return None;
                }
            };
            if actual.bits() == classinfo {
                return Some(true);
            }
        }
        if real_class(py, classinfo)? {
            let meta = match unsafe { crate::object::class_layout::real_type_bits(py, classinfo) } {
                Ok(meta) => ExceptionValue::adopt(py, meta),
                Err(()) => {
                    crate::cpython_abi_hooks::propagate_native_failure(py, "class metatype");
                    return None;
                }
            };
            if meta.bits() == builtin_classes(py).type_obj {
                return classinfo_default(py, value, classinfo, protocol);
            }
        }
    }
    let tuple = obj_from_bits(classinfo)
        .as_ptr()
        .filter(|&pointer| unsafe { object_type_id(pointer) == TYPE_ID_UNION })
        .map_or(classinfo, |pointer| unsafe {
            union_type_args_bits(pointer)
        });
    if let Some(items) = ClassInfoTuple::from_bits(py, tuple)? {
        let message = match protocol {
            ClassInfoProtocol::Instance => "maximum recursion depth exceeded in __instancecheck__",
            ClassInfoProtocol::Subclass => "maximum recursion depth exceeded in __subclasscheck__",
        };
        let _recursion = crate::state::recursion::RecursionGuard::enter_with_message(py, message)?;
        return items.any(|item| classinfo_match(py, value, item, protocol, hooks));
    }
    if !hooks {
        return structural_classinfo_match(py, value, classinfo, protocol);
    }
    let name = classinfo_protocol_name_bits(py, protocol);
    let method = unsafe { lookup_special_method_bits(py, owner.bits(), name) }
        .map(|method| ExceptionValue::adopt(py, method));
    if exception_pending(py) {
        return None;
    }
    if let Some(method) = method {
        let result = {
            let _recursion = crate::state::recursion::RecursionGuard::enter(py)?;
            unsafe { call_callable1(py, method.bits(), value) }
        };
        let result = ExceptionValue::adopt(py, result);
        drop(method);
        if exception_pending(py) {
            return None;
        }
        let matched = is_truthy(py, obj_from_bits(result.bits()));
        return if exception_pending(py) {
            None
        } else {
            Some(matched)
        };
    }
    classinfo_default(py, value, classinfo, protocol)
}

fn runtime_classinfo_match(
    py: &PyToken<'_>,
    value: u64,
    classinfo: u64,
    protocol: ClassInfoProtocol,
) -> bool {
    let _value = crate::builtins::exceptions::ExceptionValue::pin(py, value);
    let mut matched = false;
    crate::builtins::exceptions::with_saved_raised_exception(py, || {
        let Some(result) = classinfo_match(py, value, classinfo, protocol, true) else {
            return false;
        };
        matched = result;
        true
    });
    matched
}
pub(crate) unsafe fn class_mro_pinned<'a, 'py>(
    _py: &'a PyToken<'py>,
    class_ptr: *mut u8,
) -> Option<crate::object::seq_access::PinnedTuple<'a, 'py>> {
    unsafe {
        let mro_bits = class_mro_bits(class_ptr);
        let mro_obj = obj_from_bits(mro_bits);
        let mro_ptr = mro_obj.as_ptr()?;
        if object_type_id(mro_ptr) != TYPE_ID_TUPLE {
            return None;
        }
        crate::object::seq_access::pin_tuple(_py, mro_ptr)
    }
}

pub(crate) enum ClassMroView<'a, 'py> {
    Pinned(crate::object::seq_access::PinnedTuple<'a, 'py>),
    Owned(Vec<u64>),
}

impl std::ops::Deref for ClassMroView<'_, '_> {
    type Target = [u64];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Pinned(mro) => mro,
            Self::Owned(mro) => mro,
        }
    }
}

pub(crate) unsafe fn class_mro_view<'a, 'py>(
    _py: &'a PyToken<'py>,
    class_ptr: *mut u8,
) -> ClassMroView<'a, 'py> {
    if let Some(mro) = unsafe { class_mro_pinned(_py, class_ptr) } {
        ClassMroView::Pinned(mro)
    } else {
        ClassMroView::Owned(class_mro_vec(MoltObject::from_ptr(class_ptr).bits()))
    }
}

pub(crate) fn class_mro_vec(class_bits: u64) -> Vec<u64> {
    let obj = obj_from_bits(class_bits);
    let Some(ptr) = obj.as_ptr() else {
        return vec![class_bits];
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TYPE {
            return vec![class_bits];
        }
        let mro_bits = class_mro_bits(ptr);
        if let Some(mro_ptr) = obj_from_bits(mro_bits).as_ptr()
            && object_type_id(mro_ptr) == TYPE_ID_TUPLE
        {
            return crate::object::seq_access::with_immutable_tuple_slice(mro_ptr, |mro| {
                mro.to_vec()
            })
            .unwrap_or_default();
        }
        let mut out = vec![class_bits];
        let bases_bits = class_bases_bits(ptr);
        let bases = class_bases_vec(bases_bits);
        for base in bases {
            out.extend(class_mro_vec(base));
        }
        out
    }
}

pub(crate) fn class_bases_vec(bits: u64) -> Vec<u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() || bits == 0 {
        return Vec::new();
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            match object_type_id(ptr) {
                TYPE_ID_TYPE => return vec![bits],
                TYPE_ID_TUPLE => {
                    return crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| {
                        items.to_vec()
                    })
                    .unwrap_or_default();
                }
                _ => {}
            }
        }
    }
    Vec::new()
}

pub(crate) fn type_of_bits(_py: &PyToken<'_>, val_bits: u64) -> u64 {
    let builtins = builtin_classes(_py);
    let obj = obj_from_bits(val_bits);
    if obj.is_none() {
        return builtins.none_type;
    }
    if val_bits == ellipsis_bits(_py) {
        return builtins.ellipsis_type;
    }
    if obj.is_bool() {
        return builtins.bool;
    }
    if obj.is_int() {
        return builtins.int;
    }
    if obj.is_float() {
        return builtins.float;
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let tid = object_type_id(ptr);
            let class_bits = object_class_bits(ptr);
            if class_bits != 0 {
                return class_bits;
            }
            return match tid {
                TYPE_ID_DATACLASS => builtins.object,
                TYPE_ID_FLOAT => builtins.float,
                TYPE_ID_STRING => builtins.str,
                TYPE_ID_BYTES => builtins.bytes,
                TYPE_ID_BYTEARRAY => builtins.bytearray,
                TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL => builtins.list,
                TYPE_ID_TUPLE => builtins.tuple,
                TYPE_ID_DICT => builtins.dict,
                TYPE_ID_DICT_KEYS_VIEW => builtins.dict_keys,
                TYPE_ID_DICT_ITEMS_VIEW => builtins.dict_items,
                TYPE_ID_DICT_VALUES_VIEW => builtins.dict_values,
                TYPE_ID_SET => builtins.set,
                TYPE_ID_FROZENSET => builtins.frozenset,
                TYPE_ID_BIGINT => builtins.int,
                TYPE_ID_COMPLEX => builtins.complex,
                TYPE_ID_RANGE => builtins.range,
                TYPE_ID_SLICE => builtins.slice,
                TYPE_ID_MEMORYVIEW => builtins.memoryview,
                TYPE_ID_FILE_HANDLE => builtins.file,
                TYPE_ID_NOT_IMPLEMENTED => builtins.not_implemented_type,
                TYPE_ID_ELLIPSIS => builtins.ellipsis_type,
                // Every exception owns an explicit common-header class edge.
                // A missing edge is corrupt state, not permission to synthesize
                // type identity from the display name.
                TYPE_ID_EXCEPTION | crate::TYPE_ID_NATIVE_DESCRIPTOR => 0,
                TYPE_ID_FUNCTION => {
                    let class_bits = object_class_bits(ptr);
                    if class_bits != 0 {
                        class_bits
                    } else {
                        builtins.function
                    }
                }
                TYPE_ID_BOUND_METHOD => {
                    let func_bits = bound_method_func_bits(ptr);
                    let func_obj = obj_from_bits(func_bits);
                    let func_ptr = func_obj.as_ptr();
                    if let Some(func_ptr) = func_ptr {
                        let func_class_bits = object_class_bits(func_ptr);
                        if let Some(kind) = crate::builtins::functions::native_callable::NativeCallableKind::from_class(_py, func_class_bits) {
                            kind.bound_class(_py)
                        } else {
                            crate::builtins::types::method_class(_py)
                        }
                    } else {
                        crate::builtins::types::method_class(_py)
                    }
                }
                TYPE_ID_GENERATOR => builtins.generator,
                TYPE_ID_ASYNC_GENERATOR => builtins.async_generator,
                TYPE_ID_ITER => {
                    // CPython exposes distinct iterator types (e.g. list_iterator,
                    // str_ascii_iterator). Our iterator object stores the target iterable, so
                    // resolve the type from the target at runtime.
                    let target_bits = iter_target_bits(ptr);
                    let target_obj = obj_from_bits(target_bits);
                    if let Some(target_ptr) = target_obj.as_ptr() {
                        match object_type_id(target_ptr) {
                            TYPE_ID_LIST => builtins.list_iterator,
                            TYPE_ID_TUPLE => builtins.tuple_iterator,
                            TYPE_ID_STRING => {
                                let bytes = std::slice::from_raw_parts(
                                    string_bytes(target_ptr),
                                    string_len(target_ptr),
                                );
                                if bytes.is_ascii() {
                                    builtins.str_ascii_iterator
                                } else {
                                    builtins.str_iterator
                                }
                            }
                            TYPE_ID_BYTES => builtins.bytes_iterator,
                            TYPE_ID_BYTEARRAY => builtins.bytearray_iterator,
                            TYPE_ID_DICT | TYPE_ID_DICT_KEYS_VIEW => builtins.dict_keyiterator,
                            TYPE_ID_DICT_VALUES_VIEW => builtins.dict_valueiterator,
                            TYPE_ID_DICT_ITEMS_VIEW => builtins.dict_itemiterator,
                            TYPE_ID_SET | TYPE_ID_FROZENSET => builtins.set_iterator,
                            TYPE_ID_RANGE => {
                                let start_bits = range_start_bits(target_ptr);
                                let stop_bits = range_stop_bits(target_ptr);
                                let step_bits = range_step_bits(target_ptr);
                                if bigint_ptr_from_bits(start_bits).is_some()
                                    || bigint_ptr_from_bits(stop_bits).is_some()
                                    || bigint_ptr_from_bits(step_bits).is_some()
                                {
                                    builtins.longrange_iterator
                                } else {
                                    builtins.range_iterator
                                }
                            }
                            _ => builtins.iterator,
                        }
                    } else {
                        builtins.iterator
                    }
                }
                TYPE_ID_ENUMERATE => builtins.enumerate,
                TYPE_ID_CALL_ITER => builtins.callable_iterator,
                TYPE_ID_REVERSED => {
                    // CPython exposes distinct reverse iterator types for some builtins
                    // (notably list_reverseiterator and the dict reverse iterators). Our
                    // reversed object stores the target, so resolve the public type name from
                    // that target.
                    let target_bits = reversed_target_bits(ptr);
                    let target_obj = obj_from_bits(target_bits);
                    if let Some(target_ptr) = target_obj.as_ptr() {
                        match object_type_id(target_ptr) {
                            TYPE_ID_LIST => builtins.list_reverseiterator,
                            TYPE_ID_DICT | TYPE_ID_DICT_KEYS_VIEW => {
                                builtins.dict_reversekeyiterator
                            }
                            TYPE_ID_DICT_VALUES_VIEW => builtins.dict_reversevalueiterator,
                            TYPE_ID_DICT_ITEMS_VIEW => builtins.dict_reverseitemiterator,
                            TYPE_ID_RANGE => {
                                let start_bits = range_start_bits(target_ptr);
                                let stop_bits = range_stop_bits(target_ptr);
                                let step_bits = range_step_bits(target_ptr);
                                if bigint_ptr_from_bits(start_bits).is_some()
                                    || bigint_ptr_from_bits(stop_bits).is_some()
                                    || bigint_ptr_from_bits(step_bits).is_some()
                                {
                                    builtins.longrange_iterator
                                } else {
                                    builtins.range_iterator
                                }
                            }
                            _ => builtins.reversed,
                        }
                    } else {
                        builtins.reversed
                    }
                }
                TYPE_ID_ZIP => builtins.zip,
                TYPE_ID_MAP => builtins.map,
                TYPE_ID_FILTER => builtins.filter,
                TYPE_ID_CODE => builtins.code,
                crate::TYPE_ID_CELL => crate::builtins::types::cell_class(_py),
                TYPE_ID_MODULE => builtins.module,
                TYPE_ID_TYPE => {
                    let class_bits = object_class_bits(ptr);
                    if class_bits != 0 {
                        class_bits
                    } else {
                        builtins.type_obj
                    }
                }
                TYPE_ID_GENERIC_ALIAS => builtins.generic_alias,
                TYPE_ID_UNION => builtins.union_type,
                TYPE_ID_WEAKREF => builtins.reference_type,
                TYPE_ID_SUPER => builtins.super_type,
                TYPE_ID_CLASSMETHOD => builtins.classmethod,
                TYPE_ID_STATICMETHOD => builtins.staticmethod,
                TYPE_ID_PROPERTY => builtins.property,
                TYPE_ID_OBJECT => {
                    let header = header_from_obj_ptr(ptr);
                    if ((*header).load_metadata_flags() & HEADER_FLAG_COROUTINE) != 0 {
                        return builtins.coroutine;
                    }
                    let class_bits = object_class_bits(ptr);
                    if class_bits != 0 {
                        class_bits
                    } else {
                        builtins.object
                    }
                }
                _ => builtins.object,
            };
        }
    }
    if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
        unsafe {
            let class_bits = object_class_bits(ptr);
            if class_bits != 0 {
                return class_bits;
            }
        }
    }
    builtins.object
}

pub(crate) fn issubclass_bits(sub_bits: u64, class_bits: u64) -> bool {
    crate::with_gil_entry_nopanic!(py, {
        unsafe { crate::object::class_layout::is_real_subtype(py, sub_bits, class_bits) }
    })
}

pub(crate) fn issubclass_runtime(py: &PyToken<'_>, sub_bits: u64, class_bits: u64) -> bool {
    runtime_classinfo_match(py, sub_bits, class_bits, ClassInfoProtocol::Subclass)
}

pub(crate) fn isinstance_bits(py: &PyToken<'_>, val_bits: u64, class_bits: u64) -> bool {
    classinfo_match(py, val_bits, class_bits, ClassInfoProtocol::Instance, false).unwrap_or(false)
}

pub(crate) fn isinstance_runtime(py: &PyToken<'_>, val_bits: u64, class_bits: u64) -> bool {
    runtime_classinfo_match(py, val_bits, class_bits, ClassInfoProtocol::Instance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::exceptions::ExceptionValue;
    use molt_cpython_abi::api::{refcount, sequences, typeobj};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;

    // Ordinary attributes supply abstract protocol facts; this hook also lets
    // the failure tests distinguish original exceptions from synthetic errors.
    extern "C" fn protocol_attribute(receiver: u64, name: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let pointer = obj_from_bits(receiver).as_ptr().unwrap();
                let dictionary = obj_from_bits(instance_dict_bits(pointer)).as_ptr().unwrap();
                let failure_name =
                    ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, b"failure").unwrap());
                if let Some(failure) = dict_get_in_place(py, dictionary, failure_name.bits()) {
                    return crate::molt_raise(failure);
                }
                let field = match string_obj_to_owned(obj_from_bits(name)).as_deref() {
                    Some("__bases__") => b"bases_result".as_slice(),
                    Some("__class__") => b"class_result".as_slice(),
                    _ => return raise_exception(py, "AttributeError", "fixture attribute missing"),
                };
                let field =
                    ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, field).unwrap());
                match dict_get_in_place(py, dictionary, field.bits()) {
                    Some(value) => {
                        inc_ref_bits(py, value);
                        value
                    }
                    None => raise_exception(py, "AttributeError", "fixture attribute missing"),
                }
            }
        })
    }

    fn protocol_class<'a, 'py>(py: &'a PyToken<'py>) -> ExceptionValue<'a, 'py> {
        let name = ExceptionValue::adopt(
            py,
            attr_name_bits_from_bytes(py, b"AbstractClassInfo").unwrap(),
        );
        let method_name = ExceptionValue::adopt(
            py,
            attr_name_bits_from_bytes(py, b"__getattribute__").unwrap(),
        );
        let function = alloc_function_obj(py, fn_addr!(protocol_attribute), 2);
        assert!(!function.is_null());
        let function = ExceptionValue::adopt(py, MoltObject::from_ptr(function).bits());
        let namespace = ExceptionValue::adopt(py, crate::molt_dict_new(0));
        assert_eq!(
            crate::c_api::molt_mapping_setitem(
                namespace.bits(),
                method_name.bits(),
                function.bits()
            ),
            0
        );
        let class = crate::builtins::types::molt_type_new(
            builtin_classes(py).type_obj,
            name.bits(),
            MoltObject::none().bits(),
            namespace.bits(),
            MoltObject::none().bits(),
        );
        assert!(!exception_pending(py));
        ExceptionValue::adopt(py, class)
    }

    fn protocol_object<'a, 'py>(
        py: &'a PyToken<'py>,
        class: u64,
        field: &[u8],
        value: u64,
    ) -> ExceptionValue<'a, 'py> {
        let object =
            unsafe { alloc_instance_for_class(py, obj_from_bits(class).as_ptr().unwrap()) };
        let object = ExceptionValue::adopt(py, object);
        let field = ExceptionValue::adopt(py, attr_name_bits_from_bytes(py, field).unwrap());
        crate::molt_set_attr_name(object.bits(), field.bits(), value);
        assert!(!exception_pending(py));
        object
    }

    fn tuple<'a, 'py>(
        py: &'a PyToken<'py>,
        values: &[u64],
        native: bool,
    ) -> ExceptionValue<'a, 'py> {
        if !native {
            let tuple = alloc_tuple(py, values);
            assert!(!tuple.is_null());
            return ExceptionValue::adopt(py, MoltObject::from_ptr(tuple).bits());
        }
        unsafe {
            // Public physical allocation deliberately bypasses managed tuple
            // construction hooks, so this exercises genuine foreign storage.
            let tuple = refcount::OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                &raw mut molt_cpython_abi::abi_types::PyTuple_Type,
                values.len() as isize,
            ));
            assert!(!tuple.as_ptr().is_null());
            for (index, &value) in values.iter().enumerate() {
                let item = GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(value);
                assert!(!item.is_null());
                assert_eq!(
                    sequences::PyTuple_SetItem(tuple.as_ptr(), index as isize, item),
                    0
                );
            }
            let bits = GLOBAL_BRIDGE.molt_value_for_pyobj(tuple.as_ptr()).unwrap();
            let value = ExceptionValue::adopt(py, bits);
            assert_eq!(
                object_type_id(obj_from_bits(bits).as_ptr().unwrap()),
                TYPE_ID_FOREIGN
            );
            value
        }
    }

    fn assert_original_error(py: &PyToken<'_>, expected: u64) {
        assert!(exception_pending(py));
        let raised = ExceptionValue::adopt(py, crate::molt_exception_last_pending());
        assert_eq!(raised.bits(), expected);
        clear_exception(py);
    }

    #[test]
    fn abstract_classinfo_shares_managed_and_native_tuple_storage() {
        let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::with_gil_entry_nopanic!(py, {
            let class = protocol_class(py);
            for native in [false, true] {
                let empty = tuple(py, &[], native);
                let root = protocol_object(py, class.bits(), b"bases_result", empty.bits());
                let bases = tuple(py, &[root.bits()], native);
                let derived = protocol_object(py, class.bits(), b"bases_result", bases.bits());
                let value = protocol_object(py, class.bits(), b"class_result", derived.bits());
                let alternatives = tuple(py, &[root.bits(), MoltObject::none().bits()], native);
                assert!(issubclass_runtime(py, derived.bits(), root.bits()));
                assert!(issubclass_runtime(py, root.bits(), root.bits()));
                assert!(isinstance_runtime(py, value.bits(), root.bits()));
                assert!(isinstance_runtime(py, value.bits(), alternatives.bits()));
                assert!(issubclass_runtime(py, derived.bits(), alternatives.bits()));
                assert!(!isinstance_runtime(py, value.bits(), empty.bits()));
                assert!(!issubclass_runtime(
                    py,
                    MoltObject::none().bits(),
                    empty.bits()
                ));
                // Public abstract acceptance never proves physical ancestry.
                assert!(!unsafe {
                    crate::object::class_layout::is_real_subtype(py, derived.bits(), root.bits())
                });
                assert!(!unsafe {
                    crate::object::class_layout::is_real_instance(py, value.bits(), root.bits())
                });
                let malformed = protocol_object(
                    py,
                    class.bits(),
                    b"bases_result",
                    MoltObject::from_int(7).bits(),
                );
                assert_eq!(
                    subclasscheck_default(py, malformed.bits(), root.bits()),
                    None
                );
                let raised = ExceptionValue::adopt(py, crate::molt_exception_last_pending());
                assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                    py,
                    raised.bits(),
                    "TypeError"
                ));
                clear_exception(py);
                let bad_branch = tuple(py, &[malformed.bits()], native);
                let outer = protocol_object(py, class.bits(), b"bases_result", bad_branch.bits());
                assert!(!issubclass_runtime(py, outer.bits(), root.bits()));
                assert!(!exception_pending(py));
            }
        });
    }

    #[test]
    fn abstract_classinfo_preserves_callback_identity_and_default_class_checks() {
        let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
        assert!(crate::cpython_abi_hooks::register_cpython_hooks());
        crate::with_gil_entry_nopanic!(py, {
            let class = protocol_class(py);
            let empty = tuple(py, &[], false);
            let root = protocol_object(py, class.bits(), b"bases_result", empty.bits());
            let failure = crate::builtins::exceptions::alloc_exception(
                py,
                "LookupError",
                "classinfo callback",
            );
            assert!(!failure.is_null());
            let failure = ExceptionValue::adopt(py, MoltObject::from_ptr(failure).bits());
            let failing = protocol_object(py, class.bits(), b"failure", failure.bits());
            let builtins = builtin_classes(py);
            assert_eq!(
                subclasscheck_default(py, failing.bits(), builtins.object),
                None
            );
            assert_original_error(py, failure.bits());
            assert!(!issubclass_runtime(
                py,
                failing.bits(),
                MoltObject::none().bits()
            ));
            assert_original_error(py, failure.bits());
            assert!(!issubclass_runtime(py, root.bits(), failing.bits()));
            assert_original_error(py, failure.bits());
            assert!(!isinstance_runtime(py, root.bits(), failing.bits()));
            assert_original_error(py, failure.bits());
            for target in [builtins.int, root.bits()] {
                assert_eq!(instancecheck_default(py, failing.bits(), target), None);
                assert_original_error(py, failure.bits());
            }
            // Exact actual-type admission must not observe the failing __class__.
            assert!(isinstance_runtime(py, failing.bits(), class.bits()));
            assert_eq!(
                instancecheck_default(py, failing.bits(), class.bits()),
                Some(true)
            );
            for native in [false, true] {
                let matched_first = tuple(py, &[class.bits(), failing.bits()], native);
                assert!(isinstance_runtime(py, failing.bits(), matched_first.bits()));
                let failed_first = tuple(py, &[failing.bits(), class.bits()], native);
                assert!(!isinstance_runtime(py, root.bits(), failed_first.bits()));
                assert_original_error(py, failure.bits());
            }
            let apparent = protocol_object(py, class.bits(), b"class_result", builtins.int);
            assert_eq!(
                instancecheck_default(py, apparent.bits(), builtins.int),
                Some(true)
            );
            let int_bases = tuple(py, &[builtins.int], false);
            let int_derived = protocol_object(py, class.bits(), b"bases_result", int_bases.bits());
            assert_eq!(
                subclasscheck_default(py, int_derived.bits(), builtins.int),
                Some(true)
            );
            let missing = protocol_object(py, class.bits(), b"unused", MoltObject::none().bits());
            assert_eq!(
                instancecheck_default(py, missing.bits(), builtins.int),
                Some(false)
            );
            assert_eq!(
                instancecheck_default(py, missing.bits(), root.bits()),
                Some(false)
            );
            assert!(!exception_pending(py));
        });
    }
}
