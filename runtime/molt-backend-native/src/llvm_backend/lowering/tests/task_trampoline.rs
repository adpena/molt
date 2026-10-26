use super::*;

mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}
mod native_object_execution {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/native_object_execution.rs"
    ));
}

fn task_factory(
    name: &str,
    poll: &str,
    kind: &str,
    arity: usize,
    has_closure: bool,
    extent: i64,
) -> TirFunction {
    // Every exported factory accepts one borrowed word so the harness can use
    // one signature. Only closure-bearing constructors capture that word.
    let mut func = TirFunction::new(
        name.into(),
        vec![TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: if has_closure {
            vec![ValueId(0)]
        } else {
            vec![]
        },
        results: vec![result],
        attrs: AttrDict::from([
            (
                "_original_kind".into(),
                AttrValue::Str(
                    if has_closure {
                        "func_new_closure"
                    } else {
                        "func_new"
                    }
                    .into(),
                ),
            ),
            ("s_value".into(), AttrValue::Str(poll.into())),
            ("value".into(), AttrValue::Int(arity as i64)),
            ("task_kind".into(), AttrValue::Str(kind.into())),
            ("task_closure_size".into(), AttrValue::Int(extent)),
        ]),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    func
}

/// Independent native ABI oracle: generators reserve six control words;
/// coroutines start their captured payload at word zero. The allocator lends
/// guard-filled storage, so checking the whole buffer detects shifted stores,
/// overwritten control words, tail writes and allocation-failure writes.
/// Refcounts model the real borrowing/retaining protocol, while RETAIN/RELEASE
/// count calls from generated code, separately from provider-internal custody.
const PROVIDER: &str = r#"#![no_std]
use core::sync::atomic::{AtomicU64, Ordering::SeqCst};

#[export_name = "@ABI@"]
pub static ABI: u8 = 0;
const NONE: u64 = @NONE@;
const PTR_TAG: u64 = @PTR@;
const CLOSURE: u64 = 0x101;
const FIRST: u64 = 0x102;
const SECOND: u64 = 0x103;
const FUNCTION: u64 = 0x301;
const WRAPPER: u64 = 0x302;
const TOKEN: u64 = 0x401;
const GUARD: u64 = 0xa55a_55aa_5aa5_aa55;
const ALLOC_ERROR: u64 = 0xa110c;
const WRAP_ERROR: u64 = 0xa51c;
const RESUMED: u64 = 0xf00d;
const FACTORIES: usize = 0;
const ALLOCS: usize = 1;
const RETAINS: usize = 2;
const RELEASES: usize = 3;
const TOKENS: usize = 4;
const REGISTRATIONS: usize = 5;
const WRAPS: usize = 6;
const POLLS: usize = 7;
const FUNCTIONS: usize = 8;
const TASKS: usize = 9;
const WRAPPERS: usize = 10;
const ERROR: usize = 11;
static COUNTS: [AtomicU64; 12] = [const { AtomicU64::new(0) }; 12];
static INPUTS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
// kind (generator=0, coroutine=1, async generator=2), closure, arity, extent,
// expected poll address, failure (none=0, task allocation=1, wrapper=2).
static EXPECT: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
static ENTRY: AtomicU64 = AtomicU64::new(0);
static CAPTURE: AtomicU64 = AtomicU64::new(0);
static mut FRAME: [u64; 16] = [0; 16];

fn count(index: usize) -> u64 { COUNTS[index].load(SeqCst) }
fn expected(index: usize) -> u64 { EXPECT[index].load(SeqCst) }
fn record(index: usize) { COUNTS[index].fetch_add(1, SeqCst); }
fn raw_frame() -> u64 { core::ptr::addr_of_mut!(FRAME) as u64 }
fn task() -> u64 { raw_frame() | PTR_TAG }
fn base() -> usize { if expected(0) == 1 { 0 } else { 6 } }
fn slots() -> usize { (expected(1) + expected(2)) as usize }
fn payload(index: usize) -> u64 {
    if expected(1) == 1 && index == 0 { return CLOSURE; }
    [FIRST, FIRST, SECOND][index - expected(1) as usize]
}
fn frame_word(index: usize) -> u64 {
    unsafe { (raw_frame() as *const u64).add(index).read() }
}
fn retain(value: u64) {
    let refs = match value {
        CLOSURE..=SECOND => &INPUTS[(value - CLOSURE) as usize],
        FUNCTION => &COUNTS[FUNCTIONS],
        WRAPPER => &COUNTS[WRAPPERS],
        word if word == task() => &COUNTS[TASKS],
        NONE | 0 => return,
        _ => panic!("retaining an unknown owner"),
    };
    assert!(refs.fetch_add(1, SeqCst) > 0, "retain after free");
}
fn release(value: u64) {
    let refs = match value {
        CLOSURE..=SECOND => &INPUTS[(value - CLOSURE) as usize],
        FUNCTION => &COUNTS[FUNCTIONS],
        WRAPPER => &COUNTS[WRAPPERS],
        word if word == task() => &COUNTS[TASKS],
        NONE | 0 => return,
        _ => panic!("releasing an unknown owner"),
    };
    let old = refs.fetch_sub(1, SeqCst);
    assert!(old > 0, "duplicate release");
    if old == 1 {
        if value == FUNCTION { release(CAPTURE.load(SeqCst)); }
        if value == WRAPPER { release(task()); }
        if value == task() {
            for index in 0..slots() { release(frame_word(base() + index)); }
        }
    }
}
#[no_mangle]
pub extern "C" fn molt_inc_ref_obj(value: u64) { record(RETAINS); retain(value); }
#[no_mangle]
pub extern "C" fn molt_dec_ref_obj(value: u64) { record(RELEASES); release(value); }
#[no_mangle]
pub extern "C" fn molt_exception_pending() -> u64 { u64::from(count(ERROR) != 0) }

fn callable(poll: u64, entry: u64, arity: u64, closure: u64, custody: u64) -> u64 {
    assert_eq!(poll, expected(4), "the callable points at the raw-one-argument poll");
    assert_ne!(entry, 0);
    assert_ne!(entry, poll, "construction and resumption are distinct entry points");
    assert_eq!(arity, expected(2), "Python arity is independent of poll arity");
    assert_eq!(custody, 0, "task constructors borrow every Python argument");
    assert_eq!(closure, if expected(1) == 1 { CLOSURE } else { 0 });
    assert_eq!(count(FACTORIES), 0);
    record(FACTORIES);
    COUNTS[FUNCTIONS].store(1, SeqCst);
    ENTRY.store(entry, SeqCst);
    CAPTURE.store(closure, SeqCst);
    retain(closure);
    FUNCTION
}
#[no_mangle]
pub extern "C" fn molt_func_new(poll: u64, entry: u64, arity: u64, custody: u64) -> u64 {
    assert_eq!(expected(1), 0);
    callable(poll, entry, arity, 0, custody)
}
#[no_mangle]
pub extern "C" fn molt_func_new_closure(
    poll: u64, entry: u64, arity: u64, closure: u64, custody: u64,
) -> u64 {
    assert_eq!(expected(1), 1);
    callable(poll, entry, arity, closure, custody)
}
#[no_mangle]
pub extern "C" fn molt_task_new(poll: u64, extent: u64, kind: u64) -> u64 {
    assert_eq!(poll, expected(4));
    assert_eq!(extent, expected(3), "preserve the declared frame extent, including spare tail");
    assert_eq!(kind, if expected(0) == 1 { 2 } else { 1 });
    assert_eq!(count(ALLOCS), 0);
    for index in RETAINS..=POLLS { assert_eq!(count(index), 0); }
    record(ALLOCS);
    if expected(5) == 1 {
        COUNTS[ERROR].store(ALLOC_ERROR, SeqCst);
        return NONE;
    }
    COUNTS[TASKS].store(1, SeqCst);
    task()
}
#[no_mangle]
pub extern "C" fn fixed_verify_memory(allocated: u64) {
    for index in 0..16 {
        let word = if allocated != 0 && (base()..base() + slots()).contains(&index) {
            payload(index - base())
        } else { GUARD };
        assert_eq!(frame_word(index), word, "unexpected frame word at {index}");
    }
}
#[no_mangle]
pub extern "C" fn molt_cancel_token_get_current() -> u64 {
    assert_eq!(expected(0), 1);
    assert_eq!(count(TASKS), 1);
    assert_eq!(count(ERROR), 0);
    fixed_verify_memory(1);
    record(TOKENS);
    TOKEN
}
#[no_mangle]
pub extern "C" fn molt_task_register_execution(object: u64, token: u64, inherited: u64) -> u64 {
    assert_eq!((object, token, inherited), (task(), TOKEN, NONE));
    assert_eq!((count(TOKENS), count(REGISTRATIONS)), (1, 0));
    fixed_verify_memory(1);
    record(REGISTRATIONS);
    NONE
}
#[no_mangle]
pub extern "C" fn molt_asyncgen_new(object: u64) -> u64 {
    assert_eq!(expected(0), 2);
    assert_eq!(object, task());
    assert_eq!(count(TASKS), 1);
    fixed_verify_memory(1);
    record(WRAPS);
    if expected(5) == 2 {
        COUNTS[ERROR].store(WRAP_ERROR, SeqCst);
        return NONE;
    }
    retain(object);
    COUNTS[WRAPPERS].store(1, SeqCst);
    WRAPPER
}
fn poll(frame: u64) -> u64 {
    assert_eq!(frame, raw_frame(), "compiled polls accept exactly the raw frame");
    assert_eq!(count(TASKS), 1);
    assert_eq!(count(ERROR), 0);
    fixed_verify_memory(1);
    record(POLLS);
    RESUMED
}
#[no_mangle]
pub extern "C" fn fixed_reset(kind: u64, closure: u64, arity: u64, extent: u64, poll: u64, failure: u64) {
    assert_eq!(fixed_live(), 0, "the previous case released every owner");
    for count in &COUNTS { count.store(0, SeqCst); }
    for refs in &INPUTS { refs.store(1, SeqCst); }
    for (slot, word) in EXPECT.iter().zip([kind, closure, arity, extent, poll, failure]) {
        slot.store(word, SeqCst);
    }
    ENTRY.store(0, SeqCst);
    CAPTURE.store(0, SeqCst);
    for index in 0..16 {
        unsafe { (raw_frame() as *mut u64).add(index).write(GUARD); }
    }
}
#[no_mangle]
pub extern "C" fn fixed_count(index: usize) -> u64 { count(index) }
#[no_mangle]
pub extern "C" fn fixed_input(index: usize) -> u64 { INPUTS[index].load(SeqCst) }
#[no_mangle]
pub extern "C" fn fixed_entry() -> u64 { ENTRY.load(SeqCst) }
#[no_mangle]
pub extern "C" fn fixed_closure() -> u64 { CAPTURE.load(SeqCst) }
#[no_mangle]
pub extern "C" fn fixed_raw_task() -> u64 { assert_eq!(count(TASKS), 1); raw_frame() }
#[no_mangle]
pub extern "C" fn fixed_task() -> u64 { task() }
#[no_mangle]
pub extern "C" fn fixed_live() -> u64 {
    (FUNCTIONS..=WRAPPERS).map(count).sum::<u64>()
        + INPUTS.iter().map(|refs| refs.load(SeqCst)).sum::<u64>()
}
"#;

const HARNESS: &str = r#"
const NONE: u64 = @NONE@;
const CLOSURE: u64 = 0x101;
const FIRST: u64 = 0x102;
const SECOND: u64 = 0x103;
const FUNCTION: u64 = 0x301;
const WRAPPER: u64 = 0x302;
extern "C" {
    fn fixed_reset(kind: u64, closure: u64, arity: u64, extent: u64, poll: u64, failure: u64);
    fn fixed_count(index: usize) -> u64;
    fn fixed_input(index: usize) -> u64;
    fn fixed_entry() -> u64;
    fn fixed_closure() -> u64;
    fn fixed_raw_task() -> u64;
    fn fixed_task() -> u64;
    fn fixed_live() -> u64;
    fn fixed_verify_memory(allocated: u64);
    fn molt_dec_ref_obj(value: u64);
    @DECLARATIONS@
}
struct Case {
    name: &'static str,
    factory: unsafe extern "C" fn(u64) -> u64,
    poll: unsafe extern "C" fn(u64) -> u64,
    kind: u64,
    closure: u64,
    arity: u64,
    extent: u64,
}
fn counts(expected: [u64; 8], case: &str, failure: u64) {
    for (index, value) in expected.into_iter().enumerate() {
        assert_eq!(unsafe { fixed_count(index) }, value, "{case}/{failure}: counter {index}");
    }
}
fn run(case: &Case, failure: u64) {
    unsafe {
        fixed_reset(case.kind, case.closure, case.arity, case.extent, case.poll as usize as u64, failure);
        assert_eq!((case.factory)(CLOSURE), FUNCTION, "{}", case.name);
        counts([1, 0, 0, 0, 0, 0, 0, 0], case.name, failure);
        assert_eq!(fixed_count(8), 1);
        assert_eq!(fixed_count(9) + fixed_count(10), 0);
        fixed_verify_memory(0);
        let invoke: unsafe extern "C" fn(u64, u64, u64) -> u64 = core::mem::transmute(fixed_entry());
        let arguments = [FIRST, FIRST, SECOND];
        // A failed task allocation must not even read argv. Zero arguments
        // likewise need no readable range, on either success or failure.
        let argv = if failure == 1 { 1 } else if case.arity == 0 { 0 } else { arguments.as_ptr() as u64 };
        let result = invoke(fixed_closure(), argv, case.arity);
        let allocated = u64::from(failure != 1);
        let live_task = u64::from(failure == 0);
        let coroutine = u64::from(case.kind == 1) * allocated;
        let asyncgen = u64::from(case.kind == 2) * allocated;
        let retains = (case.closure + case.arity) * allocated;
        counts([1, 1, retains, asyncgen, coroutine, coroutine, asyncgen, 0], case.name, failure);
        assert_eq!(result, if failure != 0 { NONE } else if case.kind == 2 { WRAPPER } else { fixed_task() });
        assert_eq!(fixed_count(9), live_task, "{}: temporary task custody", case.name);
        assert_eq!(fixed_count(10), u64::from(case.kind == 2) * live_task);
        let captured = [case.closure, case.arity.min(2), u64::from(case.arity == 3)];
        for (index, payload_refs) in captured.into_iter().enumerate() {
            let factory_refs = if index == 0 { case.closure } else { 0 };
            assert_eq!(fixed_input(index), 1 + factory_refs + payload_refs * live_task,
                "{}/{failure}: input {index}", case.name);
        }
        let error = match failure { 0 => 0, 1 => 0xa110c, 2 => 0xa51c, _ => unreachable!() };
        assert_eq!(fixed_count(11), error, "preserve the original exception identity");
        fixed_verify_memory(allocated);
        if failure == 0 {
            assert_eq!((case.poll)(fixed_raw_task()), 0xf00d);
            counts([1, 1, retains, asyncgen, coroutine, coroutine, asyncgen, 1], case.name, failure);
            molt_dec_ref_obj(result);
        }
        molt_dec_ref_obj(FUNCTION);
        for input in [CLOSURE, FIRST, SECOND] { molt_dec_ref_obj(input); }
        assert_eq!(fixed_live(), 0, "{}/{failure}: every owner released", case.name);
        assert_eq!(fixed_count(11), error, "cleanup preserves the original exception");
    }
}
fn main() {
    for case in [@CASES@] {
        run(&case, 0);
        run(&case, 1);
        if case.kind == 2 { run(&case, 2); }
    }
}
"#;

#[test]
fn task_trampolines_execute_lazy_construction_payload_custody_and_failures() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    let mut provider = PROVIDER.to_owned();
    let mut declarations = String::new();
    let mut cases = String::new();
    for (kind, kind_id) in [("generator", 0), ("coroutine", 1), ("async_generator", 2)] {
        for has_closure in [false, true] {
            for arity in [0, 1, 3] {
                let name = format!("task_{kind}_{}_args_{arity}", usize::from(has_closure));
                let poll = format!("{name}_resume");
                // Empty frames use their minimum extent (including a zero
                // byte coroutine frame). Others preserve three spare words.
                // The lowering's layout helper cannot supply this oracle.
                let extent = if kind_id == 1 { 0 } else { 48 }
                    + 8 * (arity + usize::from(has_closure)) as i64
                    + if arity == 0 && !has_closure { 0 } else { 24 };
                let mut abi = test_native_linkage_abi(vec![TirType::I64], Some(TirType::DynBox));
                abi.parameter_custody = vec![crate::ir::ParameterCustody::Borrowed];
                backend.function_linkage_abis.insert(poll.clone(), abi);
                lower_tir_to_llvm(
                    &task_factory(&name, &poll, kind, arity, has_closure, extent),
                    &backend,
                );
                provider.push_str(&format!(
                    "\n#[no_mangle]\npub extern \"C\" fn {poll}(frame: u64) -> u64 {{ poll(frame) }}\n"
                ));
                declarations.push_str(&format!(
                    "fn {name}(closure: u64) -> u64;\nfn {poll}(frame: u64) -> u64;\n"
                ));
                cases.push_str(&format!(
                    "Case {{ name: \"{name}\", factory: {name}, poll: {poll}, kind: {kind_id}, closure: {}, arity: {arity}, extent: {extent} }},\n",
                    usize::from(has_closure),
                ));
            }
        }
    }
    backend
        .module
        .verify()
        .expect("task factory execution module must verify");
    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("llvm-task-trampoline-object")
        .expect("create the LLVM task object within Cargo image custody");
    let object = artifacts.path().join("llvm_task_trampolines.o");
    backend
        .emit_object(&object, crate::llvm_backend::MoltOptLevel::None)
        .expect("emit the LLVM task object");
    let none = (nanbox::QNAN | nanbox::TAG_NONE).to_string();
    let provider = provider
        .replace("@ABI@", molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL)
        .replace("@NONE@", &none)
        .replace("@PTR@", &(nanbox::QNAN | nanbox::TAG_PTR).to_string());
    let harness = HARNESS
        .replace("@NONE@", &none)
        .replace("@DECLARATIONS@", &declarations)
        .replace("@CASES@", &cases);
    native_object_execution::link_and_run_native_object(
        &native_object_execution::rustc_path(),
        "llvm-task-trampoline-execution",
        std::fs::read(object).expect("read the LLVM task object"),
        &provider,
        &harness,
        "LLVM lazy task construction and frame custody",
    );
}

#[test]
fn task_constructor_metadata_rejects_partial_unknown_negative_and_undersized_frames() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    for (kind, size, diagnostic) in [
        (Some("generator"), None, "task_kind"),
        (None, Some(80), "task_kind"),
        (Some("future"), Some(80), "task_kind"),
        (
            Some("generator"),
            Some(-1),
            "closure size must be non-negative",
        ),
        (Some("generator"), Some(79), "closure size too small"),
    ] {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        backend.function_linkage_abis.insert(
            "invalid_task_resume".into(),
            test_native_linkage_abi(vec![TirType::I64], Some(TirType::DynBox)),
        );
        // Six generator control words plus closure and three arguments need
        // exactly 80 bytes, independently of the implementation's layout API.
        let mut func = task_factory(
            "invalid_task_factory",
            "invalid_task_resume",
            "generator",
            3,
            true,
            80,
        );
        let attrs = &mut func.blocks.get_mut(&func.entry_block).unwrap().ops[0].attrs;
        attrs.remove("task_kind");
        attrs.remove("task_closure_size");
        if let Some(kind) = kind {
            attrs.insert("task_kind".into(), AttrValue::Str(kind.into()));
        }
        if let Some(size) = size {
            attrs.insert("task_closure_size".into(), AttrValue::Int(size));
        }
        // Both a structured lowering error and the shared layout authority's
        // panic reject this invalid input. An unrelated panic does not pass.
        let message =
            match catch_unwind(AssertUnwindSafe(|| try_lower_tir_to_llvm(&func, &backend))) {
                Ok(Ok(_)) => panic!("invalid task metadata was accepted: {kind:?}/{size:?}"),
                Ok(Err(error)) => error.to_string(),
                Err(payload) => payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|text| (*text).into()))
                    .expect("task metadata rejection must explain the violated contract"),
            };
        assert!(message.contains(diagnostic), "{kind:?}/{size:?}: {message}");
    }
}
