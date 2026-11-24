//! The runtime entries the walker's kernels call, as machine-callable stubs
//! over the model tests' runtime model (`scalar::tests::RuntimeModel`), for
//! kernels a backend compiles and runs (`differential::machine`). Each stub
//! has its entry's attached physical signature: the execution context first,
//! which it ignores.

use std::cell::RefCell;

use molt_codegen_abi::inline_refcount::RefcountOperation;
use molt_ir::runtime_fn_generated::RuntimeFn;

use super::scalar::tests::{
    Memory, RuntimeModel, compiled_result, native_result, program_slot_word, set_exception_hint,
};

/// What the stubs saw while the kernel ran on this thread.
#[derive(Default)]
pub(crate) struct Stubs {
    pub(crate) model: RuntimeModel,
    pub(crate) calls: Vec<RuntimeFn>,
    pub(crate) refcounts: Vec<(RefcountOperation, u64)>,
    /// The thread record the kernel was lent, whose exception hint a
    /// raising entry sets.
    thread_record: Option<u64>,
}

thread_local! {
    static STUBS: RefCell<Stubs> = RefCell::default();
}

/// The process memory the kernel's pointer arguments name.
struct ProcessMemory;

impl Memory for ProcessMemory {
    fn bytes(&self, address: u64, len: usize) -> Vec<u8> {
        // SAFETY: the kernel passed `len` readable bytes at `address`.
        unsafe { std::slice::from_raw_parts(address as *const u8, len) }.to_vec()
    }
    fn word(&self, address: u64) -> u64 {
        // SAFETY: the kernel passed an aligned word at `address`.
        unsafe { *(address as *const u64) }
    }
    fn store(&mut self, address: u64, word: u64) {
        // SAFETY: the kernel passed a writable aligned word at `address`.
        unsafe { *(address as *mut u64) = word }
    }
}

fn runtime(function: RuntimeFn, args: &[u64]) -> u64 {
    STUBS.with_borrow_mut(|stubs| {
        stubs.calls.push(function);
        let raised_before = stubs.model.pending_exception;
        let result = stubs
            .model
            .run(function, args, &mut ProcessMemory)
            .unwrap_or(0);
        if stubs.model.pending_exception
            && !raised_before
            && let Some(record) = stubs.thread_record
        {
            // SAFETY: the record is the kernel's lent thread record, alive
            // and aligned for the whole run.
            set_exception_hint(unsafe { &mut *(record as *mut u64) });
        }
        result
    })
}

fn refcount(operation: RefcountOperation, word: u64) {
    STUBS.with_borrow_mut(|stubs| stubs.refcounts.push((operation, word)));
}

// Each runtime entry is attached: its first parameter is the execution
// context, which the stubs ignore.
extern "C" fn exception_pending(_: u64) -> u64 {
    runtime(RuntimeFn::ExceptionPending, &[])
}
extern "C" fn async_work_poll_and_exception_pending(_: u64) -> u64 {
    runtime(RuntimeFn::AsyncWorkPollAndExceptionPending, &[])
}
extern "C" fn exception_frame_origin(_: u64, homes: u64) -> u64 {
    runtime(RuntimeFn::ExceptionFrameOrigin, &[homes])
}
extern "C" fn async_work_poll_and_exception_last_pending(_: u64) -> u64 {
    runtime(RuntimeFn::AsyncWorkPollAndExceptionLastPending, &[])
}
extern "C" fn exception_last_pending(_: u64) -> u64 {
    runtime(RuntimeFn::ExceptionLastPending, &[])
}
extern "C" fn frame_home_load(_: u64, home: u64) -> u64 {
    runtime(RuntimeFn::FrameHomeLoad, &[home])
}
extern "C" fn int_from_i64(_: u64, raw: u64) -> u64 {
    runtime(RuntimeFn::IntFromI64, &[raw])
}
extern "C" fn int_as_i64(_: u64, word: u64) -> u64 {
    runtime(RuntimeFn::IntAsI64, &[word])
}
extern "C" fn is_truthy(_: u64, word: u64) -> u64 {
    runtime(RuntimeFn::IsTruthy, &[word])
}
extern "C" fn div(_: u64, left: u64, right: u64) -> u64 {
    runtime(RuntimeFn::Div, &[left, right])
}
extern "C" fn floordiv(_: u64, left: u64, right: u64) -> u64 {
    runtime(RuntimeFn::Floordiv, &[left, right])
}
extern "C" fn modulo(_: u64, left: u64, right: u64) -> u64 {
    runtime(RuntimeFn::Mod, &[left, right])
}
extern "C" fn call_inputs_release(_: u64, callable: u64, args: u64, count: u64) {
    runtime(RuntimeFn::CallInputsRelease, &[callable, args, count]);
}
extern "C" fn string_from_bytes(_: u64, data: u64, len: u64, out: u64) -> u32 {
    runtime(RuntimeFn::StringFromBytes, &[data, len, out]) as u32
}
extern "C" fn bytes_from_bytes(_: u64, data: u64, len: u64, out: u64) -> u32 {
    runtime(RuntimeFn::BytesFromBytes, &[data, len, out]) as u32
}
extern "C" fn bigint_from_bytes(_: u64, data: u64, len: u64) -> u64 {
    runtime(RuntimeFn::BigintFromBytes, &[data, len])
}
extern "C" fn literal_cell_fill(_: u64, cell: u64, data: u64, kind: u64, len: u64) -> u64 {
    runtime(RuntimeFn::LiteralCellFill, &[cell, data, kind, len])
}
extern "C" fn exception_last(_: u64) -> u64 {
    runtime(RuntimeFn::ExceptionLast, &[])
}
extern "C" fn ellipsis(_: u64) -> u64 {
    runtime(RuntimeFn::Ellipsis, &[])
}
extern "C" fn generator_send(_: u64, a: u64, b: u64) -> u64 {
    runtime(RuntimeFn::GeneratorSend, &[a, b])
}
extern "C" fn type_of(_: u64, a: u64) -> u64 {
    runtime(RuntimeFn::TypeOf, &[a])
}
extern "C" fn two(function: RuntimeFn, a: u64, b: u64) -> u64 {
    runtime(function, &[a, b])
}
extern "C" fn get_attr_name(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::GetAttrName, a, b)
}
extern "C" fn del_attr_name(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::DelAttrName, a, b)
}
extern "C" fn callargs_new(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::CallargsNew, a, b)
}
extern "C" fn callargs_push_pos(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::CallargsPushPos, a, b)
}
extern "C" fn call_bind(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::CallBind, a, b)
}
extern "C" fn call_bind_ic(_: u64, a: u64, b: u64, c: u64) -> u64 {
    runtime(RuntimeFn::CallBindIc, &[a, b, c])
}
extern "C" fn call_builtin(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::CallBuiltin, a, b)
}
extern "C" fn load_super_method(_: u64, a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> u64 {
    runtime(RuntimeFn::LoadSuperMethod, &[a, b, c, d, e, f])
}
extern "C" fn alloc(_: u64, a: u64) -> u64 {
    runtime(RuntimeFn::Alloc, &[a])
}
extern "C" fn object_publish_initialized(_: u64, a: u64) -> u64 {
    runtime(RuntimeFn::ObjectPublishInitialized, &[a])
}
extern "C" fn ord_at(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::OrdAt, a, b)
}
extern "C" fn function_closure_bits(_: u64, a: u64) -> u64 {
    runtime(RuntimeFn::FunctionClosureBits, &[a])
}
extern "C" fn call_func_fast0(_: u64, a: u64) -> u64 {
    runtime(RuntimeFn::CallFuncFast0, &[a])
}
extern "C" fn call_func_fast1(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::CallFuncFast1, a, b)
}
extern "C" fn call_func_fast2(_: u64, a: u64, b: u64, c: u64) -> u64 {
    runtime(RuntimeFn::CallFuncFast2, &[a, b, c])
}
extern "C" fn call_func_fast3(_: u64, a: u64, b: u64, c: u64, d: u64) -> u64 {
    runtime(RuntimeFn::CallFuncFast3, &[a, b, c, d])
}
extern "C" fn is_function_obj(_: u64, a: u64) -> u64 {
    runtime(RuntimeFn::IsFunctionObj, &[a])
}
extern "C" fn call_func_owned(_: u64, a: u64, b: u64, c: u64, d: u64) -> u64 {
    runtime(RuntimeFn::CallFuncOwned, &[a, b, c, d])
}
extern "C" fn contains(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::Contains, a, b)
}
extern "C" fn add(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::Add, a, b)
}
extern "C" fn eq(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::Eq, a, b)
}
extern "C" fn not(_: u64, a: u64) -> u64 {
    runtime(RuntimeFn::Not, &[a])
}
extern "C" fn getitem_method(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::GetitemMethod, a, b)
}
extern "C" fn getitem_unchecked(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::GetitemUnchecked, a, b)
}
extern "C" fn delitem_method(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::DelitemMethod, a, b)
}
extern "C" fn set_attr_name(_: u64, a: u64, b: u64, c: u64) -> u64 {
    runtime(RuntimeFn::SetAttrName, &[a, b, c])
}
extern "C" fn setitem_method(_: u64, a: u64, b: u64, c: u64) -> u64 {
    runtime(RuntimeFn::SetitemMethod, &[a, b, c])
}
extern "C" fn get_attr_object_ic(_: u64, a: u64, b: u64, c: u64, d: u64) -> u64 {
    runtime(RuntimeFn::GetAttrObjectIc, &[a, b, c, d])
}
extern "C" fn set_attr_object_ic(_: u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> u64 {
    runtime(RuntimeFn::SetAttrObjectIc, &[a, b, c, d, e])
}
extern "C" fn guarded_field_get(_: u64, a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> u64 {
    runtime(RuntimeFn::GuardedFieldGet, &[a, b, c, d, e, f])
}
#[allow(clippy::too_many_arguments)]
extern "C" fn guarded_field_set(
    _: u64,
    a: u64,
    b: u64,
    c: u64,
    d: u64,
    e: u64,
    f: u64,
    g: u64,
) -> u64 {
    runtime(RuntimeFn::GuardedFieldSet, &[a, b, c, d, e, f, g])
}
// A site body's misses for an object pointer; the model has no objects,
// so a test never reaches them.
extern "C" fn object_field_get_ptr(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::ObjectFieldGetPtr, a, b)
}
extern "C" fn object_field_set_ptr(_: u64, a: u64, b: u64, c: u64) -> u64 {
    runtime(RuntimeFn::ObjectFieldSetPtr, &[a, b, c])
}
extern "C" fn inc_ref_obj(_: u64, word: u64) {
    refcount(RefcountOperation::Retain, word);
}
extern "C" fn dec_ref_obj(_: u64, word: u64) {
    refcount(RefcountOperation::Release, word);
}

extern "C" fn list_from_values(_: u64, a: u64, b: u64) -> u64 {
    runtime(RuntimeFn::ListFromValues, &[a, b])
}
extern "C" fn tuple_from_values(_: u64, a: u64, b: u64) -> u64 {
    runtime(RuntimeFn::TupleFromValues, &[a, b])
}
extern "C" fn dict_from_pairs(_: u64, a: u64, b: u64, c: u64) -> u64 {
    runtime(RuntimeFn::DictFromPairs, &[a, b, c])
}
extern "C" fn slice_new(_: u64, a: u64, b: u64, c: u64) -> u64 {
    runtime(RuntimeFn::SliceNew, &[a, b, c])
}
#[allow(clippy::too_many_arguments)]
extern "C" fn guarded_class_def(
    _: u64,
    a: u64,
    b: u64,
    c: u64,
    d: u64,
    e: u64,
    f: u64,
    g: u64,
    h: u64,
) -> u64 {
    runtime(RuntimeFn::GuardedClassDef, &[a, b, c, d, e, f, g, h])
}
extern "C" fn iter_next_unboxed(_: u64, a: u64, b: u64) -> u64 {
    runtime(RuntimeFn::IterNextUnboxed, &[a, b])
}
extern "C" fn iter_next_unpack2(_: u64, a: u64, b: u64) -> u64 {
    runtime(RuntimeFn::IterNextUnpack2, &[a, b])
}

extern "C" fn global_slot_base(_: u64, a: u64) -> u64 {
    runtime(RuntimeFn::GlobalSlotBase, &[a])
}

/// The model's identity for code address `code`: the program-slot word of
/// the stub symbol it resolves to, so a constructor handed the wrong target
/// answers a different object.
fn code_identity(code: u64) -> u64 {
    let compiled = [
        "walker_compiled_boxed",
        "walker_compiled_raw",
        "walker_compiled_handoff",
    ]
    .map(molt_ir::runtime_callable_abi_generated::compiled_execution_symbol);
    let runtime = RuntimeFn::ALL
        .iter()
        .map(|function| function.linker_symbol().to_owned());
    compiled
        .into_iter()
        .chain(runtime)
        .find(|symbol| lookup(symbol).is_some_and(|address| address as u64 == code))
        .map(|symbol| program_slot_word(&symbol))
        .unwrap_or_else(|| panic!("code address {code:#x} is no stub"))
}

/// Enter a new function object's trampoline as the runtime does, with
/// `closure` and the inline ints 2, 3, ... as its `arity` arguments, and
/// answer its word. The kernel's attached context is lent through.
fn enter_trampoline(context: u64, trampoline: u64, closure: u64, arity: u64) -> u64 {
    let argv: Vec<u64> = (0..arity)
        .map(|index| (0x7ff8 << 48) | (1 << 48) | (2 + index))
        .collect();
    // SAFETY: the constructor was handed a callable trampoline, whose
    // signature is `(context, closure, argv, argc) -> word`.
    let trampoline: extern "C" fn(u64, u64, u64, u64) -> u64 =
        unsafe { std::mem::transmute(trampoline as *const u8) };
    trampoline(context, closure, argv.as_ptr() as u64, arity)
}

// The function constructors enter the trampoline first, then answer an
// object keyed by the target's identity, the trampoline's answer and the
// raw words.
extern "C" fn func_new(context: u64, code: u64, trampoline: u64, arity: u64, custody: u64) -> u64 {
    let entered = enter_trampoline(context, trampoline, 0, arity);
    runtime(
        RuntimeFn::FuncNew,
        &[code_identity(code), entered, arity, custody],
    )
}
extern "C" fn func_new_closure(
    context: u64,
    code: u64,
    trampoline: u64,
    arity: u64,
    closure: u64,
    custody: u64,
) -> u64 {
    let entered = enter_trampoline(context, trampoline, closure, arity);
    runtime(
        RuntimeFn::FuncNewClosure,
        &[code_identity(code), entered, arity, closure, custody],
    )
}
extern "C" fn func_new_builtin(
    context: u64,
    key: u64,
    code: u64,
    trampoline: u64,
    arity: u64,
) -> u64 {
    let entered = enter_trampoline(context, trampoline, 0, arity);
    runtime(
        RuntimeFn::FuncNewBuiltin,
        &[key, code_identity(code), entered, arity],
    )
}

extern "C" fn iter(_: u64, a: u64) -> u64 {
    runtime(RuntimeFn::IterChecked, &[a])
}
extern "C" fn iter_next(_: u64, a: u64) -> u64 {
    runtime(RuntimeFn::IterNext, &[a])
}
extern "C" fn len(_: u64, a: u64) -> u64 {
    runtime(RuntimeFn::Len, &[a])
}
extern "C" fn len_list(_: u64, a: u64) -> u64 {
    runtime(RuntimeFn::LenList, &[a])
}
extern "C" fn locals_builtin(_: u64) -> u64 {
    runtime(RuntimeFn::LocalsBuiltin, &[])
}
extern "C" fn frame_locals_set(_: u64, a: u64) -> u64 {
    runtime(RuntimeFn::FrameLocalsSet, &[a])
}
extern "C" fn unpack_sequence(_: u64, a: u64, b: u64, c: u64) -> u64 {
    runtime(RuntimeFn::UnpackSequence, &[a, b, c])
}
extern "C" fn object_new_bound(_: u64, a: u64) -> u64 {
    runtime(RuntimeFn::ObjectNewBound, &[a])
}
extern "C" fn object_new(_: u64) -> u64 {
    runtime(RuntimeFn::ObjectNew, &[])
}
extern "C" fn builtin_type(_: u64, a: u64) -> u64 {
    runtime(RuntimeFn::BuiltinType, &[a])
}
// The model has no allocator: a lifecycle allocation takes its miss, the
// runtime constructor.
extern "C" fn heap_alloc(_: u64, _bytes: u64) -> u64 {
    0
}
// The model's objects carry no lifecycle plan, so no release frees one
// inline; reaching this is a defect in the terminal-release admission.
extern "C" fn heap_free(_: u64, object: u64) {
    panic!("the model's object {object:#x} cannot be freed inline")
}
extern "C" fn module_get_attr(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::ModuleGetAttr, a, b)
}
// A closure store's miss; inline words never reach it.
extern "C" fn closure_store(_: u64, a: u64, b: u64, c: u64) -> u64 {
    runtime(RuntimeFn::ClosureStore, &[a, b, c])
}
extern "C" fn set_new(_: u64, a: u64) -> u64 {
    runtime(RuntimeFn::SetNew, &[a])
}
extern "C" fn frozenset_new(_: u64, a: u64) -> u64 {
    runtime(RuntimeFn::FrozensetNew, &[a])
}
extern "C" fn set_add(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::SetAdd, a, b)
}
extern "C" fn frozenset_add(_: u64, a: u64, b: u64) -> u64 {
    two(RuntimeFn::FrozensetAdd, a, b)
}

// The model's compiled functions (`scalar::tests::compiled_result`); each is
// attached, so its first parameter is the execution context.
extern "C" fn compiled_boxed(_: u64, a: u64, b: u64) -> u64 {
    compiled_result("walker_compiled_boxed", &[a, b])
}
extern "C" fn compiled_raw(_: u64, a: u64, b: u64) -> u64 {
    compiled_result("walker_compiled_raw", &[a, b])
}
/// A frame entry's read of the pending-invocation word, through the attached
/// execution context it was called with.
extern "C" fn compiled_handoff(context: u64, a: u64) -> u64 {
    // SAFETY: the kernel passes its attached execution context, which lends
    // a live thread record.
    let record = unsafe { &*(context as *const molt_codegen_abi::AttachedExecutionContextStorage) }
        .thread_record();
    let offset = molt_codegen_abi::THREAD_RECORD_PENDING_INVOCATION_OFFSET as u64;
    // SAFETY: the record holds an aligned word at the invocation offset.
    let invocation = unsafe { *((record + offset) as *const u64) };
    compiled_result("walker_compiled_handoff", &[a, invocation])
}

// The model's extension symbols (`scalar::tests::native_result`): plain C
// functions that take no execution context, as an extension archive defines.
extern "C" fn native_object_call0() -> u64 {
    native_result("walker_native_object_call0", &[])
}
extern "C" fn native_object_call1(a: u64) -> u64 {
    native_result("walker_native_object_call1", &[a])
}
extern "C" fn native_object_call3(a: u64, b: u64, c: u64) -> u64 {
    native_result("walker_native_object_call3", &[a, b, c])
}
extern "C" fn native_pyinit() -> usize {
    0
}

pub(crate) fn lookup(name: &str) -> Option<*const u8> {
    for (symbol, code) in [
        (
            "walker_native_object_call0",
            native_object_call0 as *const u8,
        ),
        (
            "walker_native_object_call1",
            native_object_call1 as *const u8,
        ),
        (
            "walker_native_object_call3",
            native_object_call3 as *const u8,
        ),
        ("walker_native_pyinit", native_pyinit as *const u8),
    ] {
        if name == symbol {
            return Some(code);
        }
    }
    for (symbol, code) in [
        ("walker_compiled_boxed", compiled_boxed as *const u8),
        ("walker_compiled_raw", compiled_raw as *const u8),
        ("walker_compiled_handoff", compiled_handoff as *const u8),
    ] {
        if name == molt_ir::runtime_callable_abi_generated::compiled_execution_symbol(symbol) {
            return Some(code);
        }
    }
    if name.starts_with("__molt_module_slot_") || name.starts_with("__molt_code_slot_") {
        // The binding the linker would resolve; it lives as long as the
        // code that reads it, which the test never frees before the read.
        let word: &'static u64 = Box::leak(Box::new(program_slot_word(name)));
        return Some(std::ptr::from_ref(word).cast());
    }
    let function = RuntimeFn::ALL
        .iter()
        .copied()
        .find(|function| function.linker_symbol() == name)?;
    Some(match function {
        RuntimeFn::ExceptionPending => exception_pending as *const u8,
        RuntimeFn::AsyncWorkPollAndExceptionPending => {
            async_work_poll_and_exception_pending as *const u8
        }
        RuntimeFn::ExceptionFrameOrigin => exception_frame_origin as *const u8,
        RuntimeFn::AsyncWorkPollAndExceptionLastPending => {
            async_work_poll_and_exception_last_pending as *const u8
        }
        RuntimeFn::ExceptionLastPending => exception_last_pending as *const u8,
        RuntimeFn::ExceptionLast => exception_last as *const u8,
        RuntimeFn::Ellipsis => ellipsis as *const u8,
        RuntimeFn::GeneratorSend => generator_send as *const u8,
        RuntimeFn::TypeOf => type_of as *const u8,
        RuntimeFn::FrameHomeLoad => frame_home_load as *const u8,
        RuntimeFn::IntFromI64 => int_from_i64 as *const u8,
        RuntimeFn::IntAsI64 => int_as_i64 as *const u8,
        RuntimeFn::IsTruthy => is_truthy as *const u8,
        RuntimeFn::Div => div as *const u8,
        RuntimeFn::Floordiv => floordiv as *const u8,
        RuntimeFn::Mod => modulo as *const u8,
        RuntimeFn::CallInputsRelease => call_inputs_release as *const u8,
        RuntimeFn::StringFromBytes => string_from_bytes as *const u8,
        RuntimeFn::BytesFromBytes => bytes_from_bytes as *const u8,
        RuntimeFn::BigintFromBytes => bigint_from_bytes as *const u8,
        RuntimeFn::LiteralCellFill => literal_cell_fill as *const u8,
        RuntimeFn::GetAttrName => get_attr_name as *const u8,
        RuntimeFn::DelAttrName => del_attr_name as *const u8,
        RuntimeFn::SetAttrName => set_attr_name as *const u8,
        RuntimeFn::CallargsNew => callargs_new as *const u8,
        RuntimeFn::CallargsPushPos => callargs_push_pos as *const u8,
        RuntimeFn::CallBind => call_bind as *const u8,
        RuntimeFn::FunctionClosureBits => function_closure_bits as *const u8,
        RuntimeFn::Alloc => alloc as *const u8,
        RuntimeFn::ObjectPublishInitialized => object_publish_initialized as *const u8,
        RuntimeFn::OrdAt => ord_at as *const u8,
        RuntimeFn::CallBindIc => call_bind_ic as *const u8,
        RuntimeFn::CallBuiltin => call_builtin as *const u8,
        RuntimeFn::LoadSuperMethod => load_super_method as *const u8,
        RuntimeFn::CallFuncFast0 => call_func_fast0 as *const u8,
        RuntimeFn::CallFuncFast1 => call_func_fast1 as *const u8,
        RuntimeFn::CallFuncFast2 => call_func_fast2 as *const u8,
        RuntimeFn::CallFuncFast3 => call_func_fast3 as *const u8,
        RuntimeFn::IsFunctionObj => is_function_obj as *const u8,
        RuntimeFn::CallFuncOwned => call_func_owned as *const u8,
        RuntimeFn::Contains => contains as *const u8,
        RuntimeFn::Add => add as *const u8,
        RuntimeFn::Eq => eq as *const u8,
        RuntimeFn::Not => not as *const u8,
        RuntimeFn::GetitemMethod => getitem_method as *const u8,
        RuntimeFn::GetitemUnchecked => getitem_unchecked as *const u8,
        RuntimeFn::DelitemMethod => delitem_method as *const u8,
        RuntimeFn::SetitemMethod => setitem_method as *const u8,
        RuntimeFn::GetAttrObjectIc => get_attr_object_ic as *const u8,
        RuntimeFn::SetAttrObjectIc => set_attr_object_ic as *const u8,
        RuntimeFn::GuardedFieldGet => guarded_field_get as *const u8,
        RuntimeFn::GuardedFieldSet => guarded_field_set as *const u8,
        RuntimeFn::ObjectFieldGetPtr => object_field_get_ptr as *const u8,
        RuntimeFn::ObjectFieldSetPtr => object_field_set_ptr as *const u8,
        RuntimeFn::ListFromValues => list_from_values as *const u8,
        RuntimeFn::TupleFromValues => tuple_from_values as *const u8,
        RuntimeFn::DictFromPairs => dict_from_pairs as *const u8,
        RuntimeFn::SliceNew => slice_new as *const u8,
        RuntimeFn::GuardedClassDef => guarded_class_def as *const u8,
        RuntimeFn::IterNextUnboxed => iter_next_unboxed as *const u8,
        RuntimeFn::IterNextUnpack2 => iter_next_unpack2 as *const u8,
        RuntimeFn::GlobalSlotBase => global_slot_base as *const u8,
        RuntimeFn::FuncNew => func_new as *const u8,
        RuntimeFn::FuncNewClosure => func_new_closure as *const u8,
        RuntimeFn::FuncNewBuiltin => func_new_builtin as *const u8,
        RuntimeFn::IterChecked => iter as *const u8,
        RuntimeFn::IterNext => iter_next as *const u8,
        RuntimeFn::Len => len as *const u8,
        RuntimeFn::LenList => len_list as *const u8,
        RuntimeFn::LocalsBuiltin => locals_builtin as *const u8,
        RuntimeFn::FrameLocalsSet => frame_locals_set as *const u8,
        RuntimeFn::UnpackSequence => unpack_sequence as *const u8,
        RuntimeFn::ObjectNewBound => object_new_bound as *const u8,
        RuntimeFn::ObjectNew => object_new as *const u8,
        RuntimeFn::BuiltinType => builtin_type as *const u8,
        RuntimeFn::HeapAlloc => heap_alloc as *const u8,
        RuntimeFn::HeapFree => heap_free as *const u8,
        RuntimeFn::ModuleGetAttr => module_get_attr as *const u8,
        RuntimeFn::ClosureStore => closure_store as *const u8,
        RuntimeFn::SetNew => set_new as *const u8,
        RuntimeFn::FrozensetNew => frozenset_new as *const u8,
        RuntimeFn::SetAdd => set_add as *const u8,
        RuntimeFn::FrozensetAdd => frozenset_add as *const u8,
        RuntimeFn::IncRefObj => inc_ref_obj as *const u8,
        RuntimeFn::DecRefObj => dec_ref_obj as *const u8,
        _ => return None,
    })
}

/// Reset the stubs for one kernel run on this thread, lent the thread record
/// at `thread_record`.
pub(crate) fn begin(fail_allocation: bool, thread_record: u64) {
    STUBS.with_borrow_mut(|stubs| {
        *stubs = Stubs::default();
        stubs.model.fail_allocation = fail_allocation;
        stubs.thread_record = Some(thread_record);
    });
}

/// What the stubs saw since [`begin`].
pub(crate) fn finish() -> Stubs {
    STUBS.take()
}

/// Hold a current exception from the start of the run when `raised`, as a
/// runtime that raised before the code ran does.
pub(crate) fn raise_when(raised: bool) {
    STUBS.with_borrow_mut(|stubs| stubs.model.pending_exception = raised);
}
