//! Shared ownership-bearing dictionary/string runtime for ABI fixture binaries.
//! The real runtime remains the semantic oracle; this model supplies the hook
//! capabilities and terminal foreign-owner release needed by native C fixtures.
#![allow(dead_code)]

use molt_cpython_abi::abi_types::{self, MoltTypeTag, PyTypeObject};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_cpython_abi::hooks::{
    BorrowedHandleResult, DictHashSource, OwnedHandleResult, RuntimeHooks,
};
use molt_lang_obj_model::MoltObject;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

mod sequences;

enum Value {
    Opaque { supports_subscript: bool },
    Class,
    CFunction { method: bool },
    String(Vec<u8>),
    Dict(Vec<(u64, u64)>),
    List(Vec<u64>),
    Tuple(Vec<Option<u64>>),
    Iterator { source: u64, index: usize },
    Module(u64),
    Foreign(usize),
}
struct Entry {
    refs: usize,
    value: Value,
    on_retire: Option<fn(u64)>,
}
static NEXT: AtomicU64 = AtomicU64::new(0x6400_0000);
static VALUES: LazyLock<Mutex<HashMap<u64, Entry>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn allocate(value: Value) -> u64 {
    let address = NEXT.fetch_add(0x10, Ordering::Relaxed) as usize;
    let bits = MoltObject::from_ptr(std::ptr::with_exposed_provenance_mut(address)).bits();
    VALUES.lock().unwrap().insert(
        bits,
        Entry {
            refs: 1,
            value,
            on_retire: None,
        },
    );
    bits
}
pub fn fresh_handle() -> u64 {
    allocate(Value::Opaque {
        supports_subscript: false,
    })
}
/// A logical subscript capability independent of the physical object shell.
pub fn subscriptable_handle() -> u64 {
    allocate(Value::Opaque {
        supports_subscript: true,
    })
}
pub fn contains(bits: u64) -> bool {
    VALUES.lock().unwrap().contains_key(&bits)
}
/// Attach fixture observations to the one owner registry. The observer runs
/// once, after terminal value release, even when a container retires its child.
pub fn observe_retirement(bits: u64, observer: fn(u64)) {
    let mut values = VALUES.lock().unwrap();
    let entry = values.get_mut(&bits).expect("observe a live fixture owner");
    assert!(entry.on_retire.replace(observer).is_none());
}
pub unsafe extern "C" fn register_c_function(
    _: u64,
    flags: std::os::raw::c_int,
    _: u64,
    _: bool,
    _: u64,
    _: *const u8,
    _: usize,
) -> u64 {
    allocate(Value::CFunction {
        method: flags & abi_types::METH_METHOD != 0,
    })
}

pub unsafe extern "C" fn inc_ref(bits: u64) {
    // Inline scalars have no runtime heap owner. A heap handle must belong to
    // this registry; accepting an unknown handle would hide a split allocator.
    if !MoltObject::from_bits(bits).is_ptr() {
        return;
    }
    let retain = || {
        let mut values = VALUES.lock().unwrap();
        let value = values
            .get_mut(&bits)
            .expect("retain unknown fixture heap owner");
        let previous = value.refs;
        value.refs += 1;
        Some(u32::try_from(previous).unwrap())
    };
    if GLOBAL_BRIDGE
        .transition_runtime_owner_add(bits, false, 0, retain)
        .is_none()
    {
        let _ = retain();
    }
}
pub unsafe extern "C" fn dec_ref(bits: u64) {
    if !MoltObject::from_bits(bits).is_ptr() {
        return;
    }
    let release = || {
        let mut values = VALUES.lock().unwrap();
        let value = values
            .get_mut(&bits)
            .expect("release unknown fixture heap owner");
        let previous = value.refs;
        value.refs = previous
            .checked_sub(1)
            .expect("fixture runtime ownership underflow");
        u32::try_from(previous).unwrap()
    };
    let restore = || {
        VALUES.lock().unwrap().get_mut(&bits).unwrap().refs += 1;
    };
    let terminal = match GLOBAL_BRIDGE.transition_runtime_owner_release(bits, 0, release, restore) {
        Some(outcome) => {
            if outcome.should_finalize() {
                drop(GLOBAL_BRIDGE.retire_runtime_object_deferred(bits));
                assert_eq!(release(), 1);
            }
            outcome.should_finalize()
        }
        None => release() == 1,
    };
    if !terminal {
        return;
    }
    let Entry {
        value, on_retire, ..
    } = VALUES.lock().unwrap().remove(&bits).unwrap();
    match value {
        Value::Dict(entries) => {
            for (key, value) in entries {
                unsafe {
                    dec_ref(key);
                    dec_ref(value);
                }
            }
        }
        Value::Foreign(address) => unsafe {
            molt_cpython_abi::bridge::molt_foreign_object_release(address)
        },
        Value::Module(dict) => unsafe { dec_ref(dict) },
        Value::List(items) => {
            for item in items {
                unsafe { dec_ref(item) };
            }
        }
        Value::Tuple(items) => {
            for item in items.into_iter().flatten() {
                unsafe { dec_ref(item) };
            }
        }
        Value::Iterator { source, .. } => unsafe { dec_ref(source) },
        Value::String(_) | Value::Opaque { .. } | Value::Class | Value::CFunction { .. } => {}
    }
    if let Some(observer) = on_retire {
        observer(bits);
    }
}
pub unsafe extern "C" fn ref_count(bits: u64) -> usize {
    VALUES
        .lock()
        .unwrap()
        .get(&bits)
        .map_or(0, |value| value.refs)
}
pub unsafe extern "C" fn foreign_new(address: usize) -> u64 {
    allocate(Value::Foreign(address))
}
pub unsafe extern "C" fn alloc_dict() -> u64 {
    allocate(Value::Dict(Vec::new()))
}
pub unsafe extern "C" fn alloc_module(name: *const u8, len: usize) -> u64 {
    let dict = unsafe { alloc_dict() };
    let key = unsafe { alloc_str(b"__name__".as_ptr(), 8) };
    let value = unsafe { alloc_str(name, len) };
    assert_eq!(
        unsafe { dict_mutate(dict, key, value, 0, None, std::ptr::null_mut()) },
        0
    );
    unsafe {
        dec_ref(key);
        dec_ref(value);
    }
    allocate(Value::Module(dict))
}
pub unsafe extern "C" fn module_get_dict(module: u64) -> BorrowedHandleResult {
    match VALUES
        .lock()
        .unwrap()
        .get(&module)
        .map(|entry| &entry.value)
    {
        Some(Value::Module(dict)) => BorrowedHandleResult::ok(*dict),
        _ => BorrowedHandleResult::error(),
    }
}
pub unsafe extern "C" fn dict_resolve(bits: u64, _: u8) -> BorrowedHandleResult {
    if VALUES
        .lock()
        .unwrap()
        .get(&bits)
        .is_some_and(|entry| matches!(entry.value, Value::Dict(_)))
    {
        BorrowedHandleResult::ok(bits)
    } else {
        BorrowedHandleResult::missing()
    }
}
fn key_equal(values: &HashMap<u64, Entry>, left: u64, right: u64) -> bool {
    if left == right {
        return true;
    }
    match (values.get(&left), values.get(&right)) {
        (
            Some(Entry {
                value: Value::String(left),
                ..
            }),
            Some(Entry {
                value: Value::String(right),
                ..
            }),
        ) => left == right,
        _ => false,
    }
}
fn dict_index(values: &HashMap<u64, Entry>, dict: u64, key: u64) -> Option<usize> {
    let Value::Dict(entries) = &values.get(&dict)?.value else {
        return None;
    };
    entries
        .iter()
        .position(|(stored, _)| key_equal(values, *stored, key))
}
// The fixture follows the hook transaction: storage commit, publication, then
// displaced-owner retirement. No callback or finalizer runs under VALUES.
pub unsafe extern "C" fn dict_mutate(
    dict: u64,
    key: u64,
    value: u64,
    delete: u8,
    publish: Option<unsafe extern "C" fn(*mut std::ffi::c_void) -> i32>,
    context: *mut std::ffi::c_void,
) -> i32 {
    if !matches!(
        unsafe { dict_resolve(dict, 0) }.decode(),
        molt_cpython_abi::hooks::DecodedHandleResult::Ok(_)
    ) {
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut abi_types::PyExc_TypeError).cast(),
                c"fixture mutation requires a dictionary".as_ptr(),
            );
        }
        return -1;
    }
    if delete == 0 {
        unsafe {
            inc_ref(key);
            inc_ref(value);
        }
    }
    let displaced = {
        let mut values = VALUES.lock().unwrap();
        let index = dict_index(&values, dict, key);
        let Value::Dict(entries) = &mut values.get_mut(&dict).unwrap().value else {
            unreachable!("dictionary admitted above");
        };
        if delete != 0 {
            let Some(index) = index else {
                return 1;
            };
            Some(entries.remove(index))
        } else if let Some(index) = index {
            // Preserve the original equal key and its position. The incoming
            // extra key owner retires alongside the displaced value.
            Some((key, std::mem::replace(&mut entries[index].1, value)))
        } else {
            entries.push((key, value));
            None
        }
    };
    let status = publish.map_or(0, |publish| unsafe { publish(context) });
    molt_cpython_abi::api::errors::with_preserved_error(|| {
        if let Some((key, value)) = displaced {
            unsafe {
                dec_ref(key);
                dec_ref(value);
            }
        }
    });
    status
}
pub unsafe extern "C" fn dict_pop(dict: u64, key: u64) -> OwnedHandleResult {
    let removed = {
        let mut values = VALUES.lock().unwrap();
        let index = dict_index(&values, dict, key);
        let Some(Entry {
            value: Value::Dict(entries),
            ..
        }) = values.get_mut(&dict)
        else {
            drop(values);
            unsafe {
                molt_cpython_abi::api::errors::PyErr_SetString(
                    (&raw mut abi_types::PyExc_TypeError).cast(),
                    c"fixture pop requires a dictionary".as_ptr(),
                );
            }
            return OwnedHandleResult::error();
        };
        index.map(|index| entries.remove(index))
    };
    match removed {
        Some((key, value)) => {
            molt_cpython_abi::api::errors::with_preserved_error(|| unsafe { dec_ref(key) });
            OwnedHandleResult::ok(value)
        }
        None => OwnedHandleResult::missing(),
    }
}
pub unsafe extern "C" fn dict_get(
    dict: u64,
    key: u64,
    _: DictHashSource,
    _: i64,
) -> BorrowedHandleResult {
    let values = VALUES.lock().unwrap();
    let Some(Entry {
        value: Value::Dict(entries),
        ..
    }) = values.get(&dict)
    else {
        return BorrowedHandleResult::error();
    };
    dict_index(&values, dict, key)
        .map(|index| entries[index].1)
        .map_or_else(BorrowedHandleResult::missing, BorrowedHandleResult::ok)
}
pub unsafe extern "C" fn dict_len(dict: u64) -> usize {
    let values = VALUES.lock().unwrap();
    let Some(Entry {
        value: Value::Dict(entries),
        ..
    }) = values.get(&dict)
    else {
        return 0;
    };
    entries.len()
}
pub unsafe extern "C" fn dict_entry(
    dict: u64,
    index: usize,
    out_key: *mut u64,
    out_value: *mut u64,
) -> i32 {
    let values = VALUES.lock().unwrap();
    let Some(Entry {
        value: Value::Dict(entries),
        ..
    }) = values.get(&dict)
    else {
        return 0;
    };
    let Some(&(key, value)) = entries.get(index) else {
        return 0;
    };
    unsafe {
        if !out_key.is_null() {
            *out_key = key;
        }
        if !out_value.is_null() {
            *out_value = value;
        }
    }
    1
}
pub unsafe extern "C" fn dict_op(op: u32, dict: u64) -> u64 {
    assert_eq!(op, molt_cpython_abi::DictOp::Clear as u32);
    let entries = {
        let mut values = VALUES.lock().unwrap();
        let Some(Entry {
            value: Value::Dict(entries),
            ..
        }) = values.get_mut(&dict)
        else {
            return 0;
        };
        std::mem::take(entries)
    };
    molt_cpython_abi::api::errors::with_preserved_error(|| {
        for (key, value) in entries {
            unsafe {
                dec_ref(key);
                dec_ref(value);
            }
        }
    });
    MoltObject::none().bits()
}
pub unsafe extern "C" fn alloc_str(data: *const u8, len: usize) -> u64 {
    let mut bytes = if data.is_null() || len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }.to_vec()
    };
    bytes.push(0);
    allocate(Value::String(bytes))
}
pub unsafe extern "C" fn str_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let values = VALUES.lock().unwrap();
    if let Some(Entry {
        value: Value::String(bytes),
        ..
    }) = values.get(&bits)
    {
        if !out_len.is_null() {
            unsafe {
                *out_len = bytes.len() - 1;
            }
        }
        return bytes.as_ptr();
    }
    std::ptr::null()
}
pub unsafe extern "C" fn classify_heap(bits: u64) -> u8 {
    let values = VALUES.lock().unwrap();
    match values.get(&bits).map(|entry| &entry.value) {
        Some(Value::Class) => MoltTypeTag::Type as u8,
        Some(Value::CFunction { .. }) => MoltTypeTag::BuiltinCallable as u8,
        Some(Value::String(_)) => MoltTypeTag::Str as u8,
        Some(Value::Dict(_)) => MoltTypeTag::Dict as u8,
        Some(Value::List(_)) => MoltTypeTag::List as u8,
        Some(Value::Tuple(_)) => MoltTypeTag::Tuple as u8,
        Some(Value::Module(_)) => MoltTypeTag::Module as u8,
        _ => MoltTypeTag::Other as u8,
    }
}
// Storage classification does not supply Python class identity. Public type
// predicates and diagnostic text admission use the runtime class hook, exactly
// as the production provider does. Each immortal fixture class anchor resolves
// through the existing static-binding authority, never a second PyType shell.
struct Classes {
    type_class: u64,
    string: u64,
    dict: u64,
    module: u64,
    list: u64,
    function: u64,
    method: u64,
    opaque: u64,
}
// Only a fixture that supplies tuple storage needs this binding. Keep the
// native-only tuple capability of other shared-runtime consumers unchanged.
static TUPLE_CLASS: LazyLock<u64> = LazyLock::new(|| {
    let bits = allocate(Value::Class);
    unsafe {
        GLOBAL_BRIDGE
            .bind_static_pyobj_to_runtime_handle(
                (&raw mut abi_types::PyTuple_Type).cast(),
                bits,
                true,
            )
            .expect("bind fixture tuple class");
    }
    bits
});
static CLASSES: LazyLock<Classes> = LazyLock::new(|| {
    let bind = |class: *mut PyTypeObject| {
        let bits = allocate(Value::Class);
        unsafe {
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(class.cast(), bits, true)
                .expect("bind fixture class to its canonical static type");
        }
        bits
    };
    Classes {
        type_class: bind(&raw mut abi_types::PyType_Type),
        string: bind(&raw mut abi_types::PyUnicode_Type),
        dict: bind(&raw mut abi_types::PyDict_Type),
        module: bind(&raw mut abi_types::PyModule_Type),
        list: bind(&raw mut abi_types::PyList_Type),
        function: bind(&raw mut abi_types::PyCFunction_Type),
        method: bind(&raw mut abi_types::PyCMethod_Type),
        opaque: bind(&raw mut abi_types::MoltManaged_Type),
    }
});
pub unsafe extern "C" fn runtime_class_borrowed(bits: u64) -> BorrowedHandleResult {
    let classes = &*CLASSES;
    let callable_class = {
        let values = VALUES.lock().unwrap();
        match values.get(&bits).map(|entry| &entry.value) {
            Some(Value::CFunction { method: true }) => Some(classes.method),
            Some(Value::CFunction { method: false }) => Some(classes.function),
            _ => None,
        }
    };
    if let Some(class) = callable_class {
        return BorrowedHandleResult::ok(class);
    }
    // Specialized inline-list fixtures keep their explicit observation registries.
    // Consult their installed classifier without holding the shared value lock.
    let tag = unsafe { (molt_cpython_abi::hooks::hooks_or_stubs().classify_heap)(bits) };
    let class = match tag {
        tag if tag == MoltTypeTag::Type as u8 => classes.type_class,
        tag if tag == MoltTypeTag::Str as u8 => classes.string,
        tag if tag == MoltTypeTag::Dict as u8 => classes.dict,
        tag if tag == MoltTypeTag::Module as u8 => classes.module,
        tag if tag == MoltTypeTag::List as u8 => classes.list,
        tag if tag == MoltTypeTag::Tuple as u8 => *TUPLE_CLASS,
        _ => classes.opaque,
    };
    BorrowedHandleResult::ok(class)
}
unsafe extern "C" fn type_is_subtype(subclass: u64, class: u64) -> i32 {
    // This fixture cohort has no user-defined managed classes. Its only
    // non-reflexive relation between bound classes is CMethod -> CFunction.
    let classes = &*CLASSES;
    i32::from(subclass == class || (subclass == classes.method && class == classes.function))
}
// Formatting is intentionally limited to the fixture's strings and inline
// scalars. The result always allocates or retains in this same owner registry.
unsafe fn stringify(bits: u64, repr: bool) -> OwnedHandleResult {
    let object = MoltObject::from_bits(bits);
    let text = if object.is_none() {
        "None".to_owned()
    } else if let Some(value) = object.as_bool() {
        if value { "True" } else { "False" }.to_owned()
    } else if let Some(value) = object.as_int() {
        value.to_string()
    } else if let Some(value) = object.as_float() {
        value.to_string()
    } else {
        let values = VALUES.lock().unwrap();
        let Some(Entry {
            value: Value::String(bytes),
            ..
        }) = values.get(&bits)
        else {
            return OwnedHandleResult::error();
        };
        if !repr {
            drop(values);
            unsafe { inc_ref(bits) };
            return OwnedHandleResult::ok(bits);
        }
        format!("'{}'", String::from_utf8_lossy(&bytes[..bytes.len() - 1]))
    };
    OwnedHandleResult::ok(unsafe { alloc_str(text.as_ptr(), text.len()) })
}
pub unsafe extern "C" fn object_str(bits: u64) -> OwnedHandleResult {
    unsafe { stringify(bits, false) }
}
pub unsafe extern "C" fn object_repr(bits: u64) -> OwnedHandleResult {
    unsafe { stringify(bits, true) }
}
pub fn wire(hooks: &mut RuntimeHooks) {
    hooks.register_c_function = register_c_function;
    hooks.alloc_dict = alloc_dict;
    hooks.alloc_module = alloc_module;
    hooks.module_get_dict_borrowed = module_get_dict;
    hooks.dict_mutate = dict_mutate;
    hooks.dict_pop = dict_pop;
    hooks.dict_resolve = dict_resolve;
    hooks.dict_get = dict_get;
    hooks.dict_len = dict_len;
    hooks.dict_entry = dict_entry;
    hooks.dict_op = dict_op;
    hooks.alloc_str = alloc_str;
    hooks.str_data = str_data;
    hooks.classify_heap = classify_heap;
    hooks.runtime_class_borrowed = runtime_class_borrowed;
    hooks.type_is_subtype = type_is_subtype;
    hooks.inc_ref = inc_ref;
    hooks.dec_ref = dec_ref;
    hooks.ref_count = ref_count;
    hooks.foreign_new = foreign_new;
    hooks.object_str = object_str;
    hooks.object_repr = object_repr;
}

/// Enable sequence storage and Python iteration in this same owner registry.
/// Other fixture binaries retain their explicit native or specialized storage
/// capability profiles; no process-wide default or stub contract changes.
pub fn wire_sequences(hooks: &mut RuntimeHooks) {
    wire(hooks);
    sequences::wire(hooks);
}
