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

fn make_constructor_backend(ctx: &Context) -> LlvmBackend<'_> {
    let mut backend = make_backend(ctx);
    // The linked provider below implements this admitted boxed-call symbol.
    backend
        .runtime_callable_symbols
        .insert("molt_slice_new".into());
    backend
}

/// Raw integers outside the inline window: boxing each mints a heap owner.
const FIRST_RAW: i64 = 1 << 62;
const SECOND_RAW: i64 = (1 << 62) + (1 << 31);

#[derive(Clone, Copy, Debug)]
enum OperandConstructor {
    Tuple,
    List,
    PreservedTuple,
    PreservedList,
    Slice,
    PreservedSlice,
    DirectSlice,
    DataclassValues,
    DataclassTuple,
    Class,
}

impl OperandConstructor {
    const ALL: [Self; 10] = [
        Self::Tuple,
        Self::List,
        Self::PreservedTuple,
        Self::PreservedList,
        Self::Slice,
        Self::PreservedSlice,
        Self::DirectSlice,
        Self::DataclassValues,
        Self::DataclassTuple,
        Self::Class,
    ];

    fn symbol(self) -> &'static str {
        match self {
            Self::Tuple | Self::PreservedTuple => "molt_tuple_from_values",
            Self::List | Self::PreservedList => "molt_list_from_values",
            Self::Slice | Self::PreservedSlice | Self::DirectSlice => "molt_slice_new",
            Self::DataclassValues => "molt_dataclass_new_from_values",
            Self::DataclassTuple => "molt_dataclass_new",
            Self::Class => "molt_guarded_class_def",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Slice => "slice",
            Self::PreservedSlice | Self::DirectSlice => "boxed_call",
            Self::DataclassValues | Self::DataclassTuple => "dataclass",
            Self::Class => "class",
            _ => "sequence",
        }
    }

    /// Direct constructors pass their words as arguments; only range
    /// constructors own an entry-block word range.
    fn uses_range(self) -> bool {
        !matches!(
            self,
            Self::Slice | Self::PreservedSlice | Self::DirectSlice | Self::DataclassTuple
        )
    }

    /// The operation whose constructor words are `[first, first, second]`.
    /// Header operands (the dataclass-values name, fields and flags; the
    /// tuple-form dataclass and class names) are None. The class takes `first`
    /// as its base and `(first, second)` as one namespace pair, and the
    /// tuple-form dataclass takes them as its field names, values and flags:
    /// operand roles do not change transport custody.
    fn operation(
        self,
        first: ValueId,
        second: ValueId,
        none: ValueId,
        results: Vec<ValueId>,
    ) -> TirOp {
        let words = vec![first, first, second];
        let preserved =
            |kind: &str| AttrDict::from([("_original_kind".into(), AttrValue::Str(kind.into()))]);
        let (opcode, operands, attrs) = match self {
            Self::Tuple => (OpCode::BuildTuple, words, AttrDict::new()),
            Self::List => (OpCode::BuildList, words, AttrDict::new()),
            Self::PreservedTuple => (OpCode::Copy, words, preserved("tuple_new")),
            Self::PreservedList => (OpCode::Copy, words, preserved("list_new")),
            Self::Slice => (OpCode::BuildSlice, words, AttrDict::new()),
            Self::PreservedSlice => (OpCode::Copy, words, preserved("slice_new")),
            Self::DirectSlice => (
                OpCode::Call,
                words,
                AttrDict::from([
                    ("_original_kind".into(), AttrValue::Str("call".into())),
                    ("s_value".into(), AttrValue::Str("molt_slice_new".into())),
                ]),
            ),
            Self::DataclassValues => (
                OpCode::Copy,
                [vec![none; 3], words].concat(),
                preserved("dataclass_new_values"),
            ),
            Self::DataclassTuple => (
                OpCode::Copy,
                [vec![none], words].concat(),
                preserved("dataclass_new"),
            ),
            Self::Class => {
                let mut attrs = preserved("class_def");
                attrs.insert("s_value".into(), AttrValue::Str("1,1,16,1,0".into()));
                (OpCode::Copy, [vec![none], words].concat(), attrs)
            }
        };
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs,
            source_span: None,
        }
    }
}

fn slice_operation(operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildSlice,
        operands,
        results,
        attrs: AttrDict::new(),
        source_span: None,
    }
}

/// One function over the constants `first`, `second` and None whose single
/// operation returns or discards its constructed owner.
fn function_with(
    name: &str,
    bound: bool,
    operation: impl FnOnce(ValueId, ValueId, ValueId, Vec<ValueId>) -> TirOp,
) -> TirFunction {
    let (return_type, return_abi) = if bound {
        (TirType::DynBox, molt_ir::FunctionReturnAbi::Value)
    } else {
        (TirType::None, molt_ir::FunctionReturnAbi::Void)
    };
    let mut func = TirFunction::new(name.into(), vec![], return_type, return_abi);
    let first = func.fresh_value();
    let second = func.fresh_value();
    let none = func.fresh_value();
    let results = if bound {
        vec![func.fresh_value()]
    } else {
        Vec::new()
    };
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.extend([
        const_int_def(first, FIRST_RAW),
        const_int_def(second, SECOND_RAW),
        const_none_def(none),
        operation(first, second, none, results.clone()),
    ]);
    entry.terminator = Terminator::Return { values: results };
    func
}

fn constructor_function(name: &str, constructor: OperandConstructor, bound: bool) -> TirFunction {
    function_with(name, bound, |first, second, none, results| {
        constructor.operation(first, second, none, results)
    })
}

fn block_ir(block: BasicBlock<'_>) -> String {
    let mut text = String::new();
    let mut instruction = block.get_first_instruction();
    while let Some(current) = instruction {
        text.push_str(&current.print_to_string().to_string());
        text.push('\n');
        instruction = current.get_next_instruction();
    }
    text
}

/// A slot allocated where its operation runs grows the stack on every loop
/// iteration, so every alloca must be a static entry-block slot.
fn assert_entry_only_allocas(llvm_fn: FunctionValue<'_>, ir: &str) {
    for block in llvm_fn.get_basic_blocks().into_iter().skip(1) {
        let mut instruction = block.get_first_instruction();
        while let Some(current) = instruction {
            assert_ne!(
                current.get_opcode(),
                inkwell::values::InstructionOpcode::Alloca,
                "dynamic alloca outside the entry block: {ir}"
            );
            instruction = current.get_next_instruction();
        }
    }
}

#[test]
fn constructor_operands_box_each_distinct_operand_once_and_stop_at_first_failure() {
    let none = nanbox::QNAN | nanbox::TAG_NONE;
    for constructor in OperandConstructor::ALL {
        let ctx = Context::create();
        let backend = make_constructor_backend(&ctx);
        let func = constructor_function("constructor_transaction", constructor, true);
        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        backend
            .module
            .verify()
            .unwrap_or_else(|error| panic!("{constructor:?}: {error}"));
        let ir = llvm_fn.print_to_string().to_string();
        let at = |needle: &str| -> Vec<usize> {
            ir.match_indices(needle).map(|(index, _)| index).collect()
        };
        let label = constructor.label();
        let boxes = at("call i64 @molt_int_from_i64(");
        // A heap box is checked against None, which the runtime returns exactly
        // when its allocation raised; boxing needs no exception poll.
        let checks = at(&format!("label %{label}_abort"));
        let construct = at(&format!("call i64 @{}(", constructor.symbol()));
        let releases = at("call void @molt_dec_ref_obj(");
        assert_eq!(
            boxes.len(),
            2,
            "{constructor:?}: one box per distinct operand keeps the repeated word's identity: {ir}"
        );
        assert_eq!(
            checks.len(),
            2,
            "{constructor:?}: each minted box branches to the failure block: {ir}"
        );
        assert!(
            !ir.contains("@molt_exception_pending("),
            "{constructor:?}: a box reports failure through its word: {ir}"
        );
        assert_eq!(construct.len(), 1, "{constructor:?}: {ir}");
        assert!(
            boxes[0] < checks[0]
                && checks[0] < boxes[1]
                && boxes[1] < checks[1]
                && checks[1] < construct[0],
            "{constructor:?}: a failed box skips every later box and the constructor: {ir}"
        );
        assert_eq!(
            releases.len(),
            2,
            "{constructor:?}: each minted owner is released exactly once on both paths: {ir}"
        );
        assert!(
            releases.iter().all(|&release| release > construct[0]),
            "{constructor:?}: minted owners outlive the constructor's retain: {ir}"
        );
        let constructed = if matches!(
            constructor,
            OperandConstructor::PreservedSlice | OperandConstructor::DirectSlice
        ) {
            "molt_slice_new".to_string()
        } else {
            format!("{label}_constructed")
        };
        assert!(
            ir.contains(&format!("%{label}_result = phi i64 [ %{constructed}, ")),
            "{constructor:?}: {ir}"
        );
        assert!(
            ir.contains(&format!("[ {none}, %{label}_abort")),
            "{constructor:?}: a failed box publishes None with its exception pending: {ir}"
        );
        assert_eq!(
            ir.contains("alloca i64, i64"),
            constructor.uses_range(),
            "{constructor:?}: only range constructors own a word range: {ir}"
        );
        assert_entry_only_allocas(llvm_fn, &ir);
    }
}

#[test]
fn slice_bounds_are_direct_operands_padded_with_none() {
    let none = nanbox::QNAN | nanbox::TAG_NONE;
    let ctx = Context::create();
    let backend = make_constructor_backend(&ctx);
    let start_only = lower_tir_to_llvm(
        &function_with("slice_start_only", true, |first, _, _, results| {
            slice_operation(vec![first], results)
        }),
        &backend,
    );
    let without_bounds = lower_tir_to_llvm(
        &function_with("slice_without_bounds", true, |_, _, _, results| {
            slice_operation(Vec::new(), results)
        }),
        &backend,
    );
    backend
        .module
        .verify()
        .expect("omitted slice bounds must verify");
    let start_ir = start_only.print_to_string().to_string();
    let empty_ir = without_bounds.print_to_string().to_string();
    assert!(
        start_ir.contains(&format!(
            "%slice_constructed = call i64 @molt_slice_new(i64 %boxed_int, i64 {none}, i64 {none})"
        )),
        "the present bound is boxed once and the omitted bounds are None: {start_ir}"
    );
    assert_eq!(
        start_ir.matches("call i64 @molt_int_from_i64(").count(),
        1,
        "{start_ir}"
    );
    assert_eq!(
        start_ir.matches("call void @molt_dec_ref_obj(").count(),
        1,
        "{start_ir}"
    );
    assert!(
        empty_ir.contains(&format!(
            "%slice_result = call i64 @molt_slice_new(i64 {none}, i64 {none}, i64 {none})"
        )),
        "{empty_ir}"
    );
    for needle in [
        "alloca",
        "@molt_int_from_i64(",
        "@molt_exception_pending(",
        "@molt_dec_ref_obj(",
    ] {
        assert!(
            !empty_ir.contains(needle),
            "omitted bounds need no box, owner, check or slot ({needle}): {empty_ir}"
        );
    }
    assert!(
        !start_ir.contains("alloca i64, i64"),
        "direct operands need no range: {start_ir}"
    );
    assert_entry_only_allocas(start_only, &start_ir);
}

#[test]
fn slice_with_more_than_three_bounds_fails_closed() {
    let ctx = Context::create();
    let backend = make_constructor_backend(&ctx);
    let func = function_with("slice_four_bounds", true, |first, second, none, results| {
        slice_operation(vec![first, second, none, none], results)
    });
    let error = try_lower_tir_to_llvm(&func, &backend)
        .expect_err("a fourth slice bound must fail closed instead of being dropped");
    assert_lowering_error_contains(&error, "BuildSlice takes at most start, stop and step");
}

#[test]
fn constructor_ranges_and_out_parameter_slots_are_static_entry_block_allocas() {
    let ctx = Context::create();
    let backend = make_constructor_backend(&ctx);
    let mut func = TirFunction::new(
        "range_boundary_slots".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let first = func.fresh_value();
    let second = func.fresh_value();
    let none = func.fresh_value();
    let text = func.fresh_value();
    let item = func.fresh_value();
    let done = func.fresh_value();
    let unpacked = func.fresh_value();
    let constructed: Vec<ValueId> = OperandConstructor::ALL
        .iter()
        .map(|_| func.fresh_value())
        .collect();
    let body = func.fresh_block();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.extend([
        const_int_def(first, FIRST_RAW),
        const_int_def(second, SECOND_RAW),
        const_none_def(none),
    ]);
    entry.terminator = Terminator::Branch {
        target: body,
        args: vec![],
    };
    // Every operation below runs in a block that may execute repeatedly.
    let mut ops = vec![
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstStr,
            operands: vec![],
            results: vec![text],
            attrs: AttrDict::from([("s_value".into(), AttrValue::Str("slot".into()))]),
            source_span: None,
        },
        TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::IterNextUnboxed,
            operands: vec![none],
            results: vec![item, done],
            attrs: AttrDict::new(),
            source_span: None,
        },
    ];
    for (constructor, &result) in OperandConstructor::ALL.into_iter().zip(&constructed) {
        ops.push(constructor.operation(first, second, none, vec![result]));
    }
    ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![constructed[0]],
        results: vec![unpacked],
        attrs: AttrDict::from([
            (
                "_original_kind".into(),
                AttrValue::Str("unpack_sequence".into()),
            ),
            ("value".into(), AttrValue::Int(1)),
        ]),
        source_span: None,
    });
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops,
            terminator: Terminator::Return {
                values: vec![unpacked],
            },
        },
    );
    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    backend
        .module
        .verify()
        .expect("constructor and out-parameter slots must verify");
    let ir = llvm_fn.print_to_string().to_string();
    assert_entry_only_allocas(llvm_fn, &ir);
    let entry_ir = block_ir(llvm_fn.get_first_basic_block().unwrap());
    for slot in [
        "%str_out = alloca i64",
        "%iter_next_unboxed_value = alloca i64",
        "%sequence_owner = alloca i64",
        "%slice_owner = alloca i64",
        "%sequence_values = alloca i64, i64 3",
        "%dataclass_values = alloca i64, i64 3",
        "%class_bases = alloca i64, i64 1",
        "%class_attrs = alloca i64, i64 2",
        "%unpack_out = alloca i64, i64 1",
    ] {
        assert!(
            entry_ir.contains(slot),
            "{slot} must be a static entry-block slot: {ir}"
        );
    }
}

/// Runtime oracle for the real-link test. Two heap owners (0x101, 0x102) and one
/// constructed object (0x200) are reference counted. Every constructor checks
/// its three words against the harness's expectation, retains them, and the
/// constructed object releases what it retained when it dies.
const PROVIDER: &str = r#"#![no_std]
use core::sync::atomic::{AtomicU64, Ordering::SeqCst};

#[export_name = "@ABI@"]
pub static ABI: u8 = 0;
const NONE: u64 = @NONE@;
static mut PENDING: u8 = 0;
static ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static FAIL: AtomicU64 = AtomicU64::new(0);
static CTOR_FAIL: AtomicU64 = AtomicU64::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);
static RESULT: AtomicU64 = AtomicU64::new(0);
static FIRST: AtomicU64 = AtomicU64::new(0);
static SECOND: AtomicU64 = AtomicU64::new(0);
static EXPECTED: [AtomicU64; 3] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];
static RETAINED: [AtomicU64; 3] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];
static RETAINED_LEN: AtomicU64 = AtomicU64::new(0);

#[no_mangle]
pub extern "C" fn molt_int_from_i64(value: i64) -> u64 {
    let attempt = ATTEMPTS.fetch_add(1, SeqCst) + 1;
    assert_eq!(value, if attempt == 1 { 1_i64 << 62 } else { (1_i64 << 62) + (1_i64 << 31) });
    if FAIL.load(SeqCst) == attempt {
        unsafe { PENDING = 1; }
        return NONE;
    }
    assert!(attempt <= 2, "one box per distinct operand");
    if attempt == 1 { FIRST.store(1, SeqCst) } else { SECOND.store(1, SeqCst) }
    0x100 + attempt
}
#[no_mangle]
pub extern "C" fn molt_exception_pending() -> u64 {
    unsafe { PENDING as u64 }
}
#[no_mangle]
pub extern "C" fn molt_inc_ref_obj(value: u64) {
    let refs = match value { 0x101 => &FIRST, 0x102 => &SECOND, 0x200 => &RESULT, _ => return };
    assert!(refs.fetch_add(1, SeqCst) > 0, "retain after free");
}
#[no_mangle]
pub extern "C" fn molt_dec_ref_obj(value: u64) {
    let refs = match value { 0x101 => &FIRST, 0x102 => &SECOND, 0x200 => &RESULT, _ => return };
    let old = refs.fetch_sub(1, SeqCst);
    assert!(old > 0, "duplicate release");
    if value == 0x200 && old == 1 {
        for slot in &RETAINED[..RETAINED_LEN.load(SeqCst) as usize] {
            molt_dec_ref_obj(slot.load(SeqCst));
        }
    }
}
unsafe fn words<'a>(address: u64, len: u64) -> &'a [u64] {
    unsafe { core::slice::from_raw_parts(address as *const u64, len as usize) }
}
fn construct(values: &[u64]) -> u64 {
    CALLS.fetch_add(1, SeqCst);
    assert_eq!(molt_exception_pending(), 0, "no constructor runs after a failed box");
    let expected = [EXPECTED[0].load(SeqCst), EXPECTED[1].load(SeqCst), EXPECTED[2].load(SeqCst)];
    assert_eq!(values, &expected, "one box per distinct operand, in operand order");
    for (word, refs) in [(0x101, &FIRST), (0x102, &SECOND)] {
        assert_eq!(
            refs.load(SeqCst),
            u64::from(values.contains(&word)),
            "a box is owned only by its transaction until construction"
        );
    }
    if CTOR_FAIL.load(SeqCst) != 0 {
        unsafe { PENDING = 1; }
        return NONE;
    }
    for (slot, &value) in RETAINED.iter().zip(values) {
        molt_inc_ref_obj(value);
        slot.store(value, SeqCst);
    }
    RETAINED_LEN.store(values.len() as u64, SeqCst);
    RESULT.store(1, SeqCst);
    0x200
}
#[no_mangle]
pub unsafe extern "C" fn molt_tuple_from_values(address: u64, len: u64) -> u64 {
    construct(unsafe { words(address, len) })
}
#[no_mangle]
pub unsafe extern "C" fn molt_list_from_values(address: u64, len: u64) -> u64 {
    construct(unsafe { words(address, len) })
}
#[no_mangle]
pub extern "C" fn molt_slice_new(start: u64, stop: u64, step: u64) -> u64 {
    construct(&[start, stop, step])
}
#[no_mangle]
pub unsafe extern "C" fn molt_dataclass_new_from_values(
    name: u64,
    fields: u64,
    address: u64,
    len: u64,
    flags: u64,
) -> u64 {
    assert_eq!([name, fields, flags], [NONE; 3], "header words are direct arguments");
    construct(unsafe { words(address, len) })
}
#[no_mangle]
pub extern "C" fn molt_dataclass_new(name: u64, fields: u64, values: u64, flags: u64) -> u64 {
    assert_eq!(name, NONE, "the class name is a direct argument");
    construct(&[fields, values, flags])
}
#[no_mangle]
pub unsafe extern "C" fn molt_guarded_class_def(
    name: u64,
    bases: u64,
    nbases: u64,
    attrs: u64,
    nattrs: u64,
    layout_size: i64,
    layout_version: i64,
    flags: i64,
) -> u64 {
    assert_eq!((name, nbases, nattrs), (NONE, 1, 1));
    assert_eq!((layout_size, layout_version, flags), (16, 1, 0));
    let (bases, attrs) = unsafe { (words(bases, 1), words(attrs, 2)) };
    construct(&[bases[0], attrs[0], attrs[1]])
}
#[no_mangle]
pub extern "C" fn fixed_reset(failure: u64, ctor_failure: u64) {
    assert_eq!(fixed_live(), 0);
    ATTEMPTS.store(0, SeqCst);
    CALLS.store(0, SeqCst);
    RETAINED_LEN.store(0, SeqCst);
    FAIL.store(failure, SeqCst);
    CTOR_FAIL.store(ctor_failure, SeqCst);
    unsafe { PENDING = 0; }
}
#[no_mangle]
pub extern "C" fn fixed_expect(first: u64, second: u64, third: u64) {
    for (slot, word) in EXPECTED.iter().zip([first, second, third]) {
        slot.store(word, SeqCst);
    }
}
#[no_mangle]
pub extern "C" fn fixed_live() -> u64 {
    FIRST.load(SeqCst) + SECOND.load(SeqCst) + RESULT.load(SeqCst)
}
#[no_mangle]
pub extern "C" fn fixed_attempts() -> u64 {
    ATTEMPTS.load(SeqCst)
}
#[no_mangle]
pub extern "C" fn fixed_calls() -> u64 {
    CALLS.load(SeqCst)
}
"#;

const HARNESS: &str = r#"
const NONE: u64 = @NONE@;
const REPEATED: [u64; 3] = [0x101, 0x101, 0x102];
const EVERY_FAILURE: [(u64, u64); 4] = [(0, 0), (1, 0), (2, 0), (0, 1)];

extern "C" {
    fn fixed_tuple() -> u64;
    fn fixed_list() -> u64;
    fn fixed_dataclass_values() -> u64;
    fn fixed_class() -> u64;
    fn fixed_slice() -> u64;
    fn fixed_preserved_slice() -> u64;
    fn fixed_direct_slice() -> u64;
    fn fixed_dataclass_tuple() -> u64;
    fn fixed_slice_start() -> u64;
    fn fixed_slice_empty() -> u64;
    fn fixed_tuple_discard();
    fn fixed_slice_discard();
    fn fixed_preserved_slice_discard();
    fn fixed_direct_slice_discard();
    fn fixed_reset(failure: u64, ctor_failure: u64);
    fn fixed_expect(first: u64, second: u64, third: u64);
    fn fixed_live() -> u64;
    fn fixed_attempts() -> u64;
    fn fixed_calls() -> u64;
    fn molt_exception_pending() -> u64;
    fn molt_dec_ref_obj(value: u64);
}

fn run(
    name: &str,
    construct: unsafe extern "C" fn() -> u64,
    words: [u64; 3],
    failures: &[(u64, u64)],
) {
    let boxes = [0x101_u64, 0x102].iter().filter(|word| words.contains(word)).count() as u64;
    let retained = words.iter().filter(|&&word| word != NONE).count() as u64;
    for &(failure, ctor_failure) in failures {
        unsafe {
            fixed_reset(failure, ctor_failure);
            fixed_expect(words[0], words[1], words[2]);
            let result = construct();
            let success = failure == 0 && ctor_failure == 0;
            let case = (name, failure, ctor_failure);
            assert_eq!(result, if success { 0x200 } else { NONE }, "{case:?}");
            assert_eq!(
                fixed_attempts(),
                if failure == 0 { boxes } else { failure },
                "boxing stops at the first failure: {case:?}"
            );
            assert_eq!(
                fixed_calls(),
                u64::from(failure == 0),
                "no constructor runs after a failed box: {case:?}"
            );
            assert_eq!(
                fixed_live(),
                if success { retained + 1 } else { 0 },
                "each minted owner is released exactly once: {case:?}"
            );
            assert_eq!(
                molt_exception_pending(),
                u64::from(!success),
                "the first exception stays pending: {case:?}"
            );
            if success {
                molt_dec_ref_obj(result);
            }
            assert_eq!(fixed_live(), 0, "{case:?}");
        }
    }
}

fn main() {
    for (name, construct) in [
        ("tuple", fixed_tuple as unsafe extern "C" fn() -> u64),
        ("list", fixed_list),
        ("dataclass_values", fixed_dataclass_values),
        ("class", fixed_class),
        ("slice", fixed_slice),
        ("preserved_slice", fixed_preserved_slice),
        ("direct_slice", fixed_direct_slice),
        ("dataclass_tuple", fixed_dataclass_tuple),
    ] {
        run(name, construct, REPEATED, &EVERY_FAILURE);
    }
    run("slice_start", fixed_slice_start, [0x101, NONE, NONE], &[(0, 0), (1, 0), (0, 1)]);
    run("slice_empty", fixed_slice_empty, [NONE; 3], &[(0, 0), (0, 1)]);
    for (name, discard) in [
        ("tuple", fixed_tuple_discard as unsafe extern "C" fn()),
        ("slice", fixed_slice_discard),
        ("preserved_slice", fixed_preserved_slice_discard),
        ("direct_slice", fixed_direct_slice_discard),
    ] {
        for failure in [0, 1, 2] {
            unsafe {
                fixed_reset(failure, 0);
                fixed_expect(REPEATED[0], REPEATED[1], REPEATED[2]);
                discard();
                let case = (name, failure);
                assert_eq!(fixed_live(), 0, "a discarded owner is released: {case:?}");
                assert_eq!(
                    fixed_attempts(),
                    if failure == 0 { 2 } else { failure },
                    "{case:?}"
                );
                assert_eq!(fixed_calls(), u64::from(failure == 0), "{case:?}");
                assert_eq!(
                    molt_exception_pending(),
                    u64::from(failure != 0),
                    "{case:?}"
                );
            }
        }
    }
}
"#;

#[test]
fn constructor_operands_execute_identity_failures_and_discarded_results() {
    let Some(rustc) = native_object_execution::real_rustc() else {
        return;
    };
    let ctx = Context::create();
    let backend = make_constructor_backend(&ctx);
    for (name, constructor, bound) in [
        ("fixed_tuple", OperandConstructor::Tuple, true),
        ("fixed_list", OperandConstructor::List, true),
        (
            "fixed_dataclass_values",
            OperandConstructor::DataclassValues,
            true,
        ),
        ("fixed_class", OperandConstructor::Class, true),
        ("fixed_slice", OperandConstructor::Slice, true),
        (
            "fixed_preserved_slice",
            OperandConstructor::PreservedSlice,
            true,
        ),
        ("fixed_direct_slice", OperandConstructor::DirectSlice, true),
        (
            "fixed_dataclass_tuple",
            OperandConstructor::DataclassTuple,
            true,
        ),
        ("fixed_tuple_discard", OperandConstructor::Tuple, false),
        ("fixed_slice_discard", OperandConstructor::Slice, false),
        (
            "fixed_preserved_slice_discard",
            OperandConstructor::PreservedSlice,
            false,
        ),
        (
            "fixed_direct_slice_discard",
            OperandConstructor::DirectSlice,
            false,
        ),
    ] {
        lower_tir_to_llvm(&constructor_function(name, constructor, bound), &backend);
    }
    lower_tir_to_llvm(
        &function_with("fixed_slice_start", true, |first, _, _, results| {
            slice_operation(vec![first], results)
        }),
        &backend,
    );
    lower_tir_to_llvm(
        &function_with("fixed_slice_empty", true, |_, _, _, results| {
            slice_operation(Vec::new(), results)
        }),
        &backend,
    );
    backend
        .module
        .verify()
        .expect("constructor execution module must verify");
    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("llvm-constructor-object")
        .expect("create the LLVM object within Cargo image custody");
    let object = artifacts.path().join("llvm_constructor_operands.o");
    backend
        .emit_object(&object, crate::llvm_backend::MoltOptLevel::None)
        .expect("emit the LLVM object");
    let object_bytes = std::fs::read(&object).expect("read the LLVM object");
    let none = (nanbox::QNAN | nanbox::TAG_NONE).to_string();
    let provider = PROVIDER
        .replace("@ABI@", molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL)
        .replace("@NONE@", &none);
    let harness = HARNESS.replace("@NONE@", &none);
    native_object_execution::link_and_run_native_object(
        &rustc,
        "llvm-constructor-execution",
        object_bytes,
        &provider,
        &harness,
        "LLVM constructor operand transactions",
    );
}

/// Runtime oracle for borrowed and adopted operands beyond fixed constructors.
/// Heap words are counted handles: minted integers 0x101 (`FIRST_RAW`) and
/// 0x102 (`SECOND_RAW`), a call result 0x200, a CallArgs builder 0x300, a dict
/// 0x400, and a task whose handle boxes the address of `TASK_BUF`. Each entry
/// point borrows, retains, consumes or returns exactly as the runtime entry it
/// stands in for.
const FAMILY_PROVIDER: &str = r#"#![no_std]
use core::sync::atomic::{AtomicU64, Ordering::SeqCst};

#[export_name = "@ABI@"]
pub static ABI: u8 = 0;
const NONE: u64 = @NONE@;
const TRUE: u64 = @TRUE@;
const BOXED_FIVE: u64 = @FIVE@;
const PTR_TAG: u64 = @PTR@;
const POINTER_MASK: u64 = (1 << 48) - 1;
const FIRST_RAW: i64 = 1 << 62;
const SECOND_RAW: i64 = (1 << 62) + (1 << 31);
const SLOT: usize = @SLOT@;
const BUILDER: u64 = 0x300;
const DICT: u64 = 0x400;
const EXISTING: u64 = 0x501;
const OTHER: u64 = 0x502;
static mut PENDING: u8 = 0;
static PENDING_ERROR: AtomicU64 = AtomicU64::new(0);
static mut TASK_BUF: [u64; 16] = [0; 16];
static ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static FAIL: AtomicU64 = AtomicU64::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);
static NEWS: AtomicU64 = AtomicU64::new(0);
static GROWS: AtomicU64 = AtomicU64::new(0);
static INSERTS: AtomicU64 = AtomicU64::new(0);
static BUILDER_FAIL: AtomicU64 = AtomicU64::new(0);
static DICT_FAIL: AtomicU64 = AtomicU64::new(0);
static INSERT_FAIL: AtomicU64 = AtomicU64::new(0);
static FIRST: AtomicU64 = AtomicU64::new(0);
static SECOND: AtomicU64 = AtomicU64::new(0);
static RESULT: AtomicU64 = AtomicU64::new(0);
static BUILDER_REFS: AtomicU64 = AtomicU64::new(0);
static DICT_REFS: AtomicU64 = AtomicU64::new(0);
static TASK_REFS: AtomicU64 = AtomicU64::new(0);
static EXISTING_REFS: AtomicU64 = AtomicU64::new(0);
static OTHER_REFS: AtomicU64 = AtomicU64::new(0);
static OWNED_CASE: AtomicU64 = AtomicU64::new(0);
static ABORTS: AtomicU64 = AtomicU64::new(0);
static EXTRACTS: AtomicU64 = AtomicU64::new(0);
static POSTCALLS: AtomicU64 = AtomicU64::new(0);
static BINDS: AtomicU64 = AtomicU64::new(0);
static CAPACITY: AtomicU64 = AtomicU64::new(0);
static ARGS: [AtomicU64; 4] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];
static ARGS_LEN: AtomicU64 = AtomicU64::new(0);
static EXPECTED: [AtomicU64; 4] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];
static EXPECTED_LEN: AtomicU64 = AtomicU64::new(0);
static ENTRIES: [AtomicU64; 4] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];
static ENTRIES_LEN: AtomicU64 = AtomicU64::new(0);
const HOME_UNBOUND: u64 = @HOME_UNBOUND@;
const HOME_PLAIN: u64 = @HOME_PLAIN@;
const HOME_RAW: u64 = @HOME_RAW@;
const HOME_CELL: u64 = @HOME_CELL@;
const HOME_PRIVATE_CELL: u64 = @HOME_PRIVATE_CELL@;
const HOME_MISSING: u64 = 0x600;
static mut HOME: [u64; 2] = [0; 2];
static HOME_HELD: AtomicU64 = AtomicU64::new(0);
static HOME_REENTER: AtomicU64 = AtomicU64::new(0);
static HOME_REENTRIES: AtomicU64 = AtomicU64::new(0);
static HOME_SUCCESSES: AtomicU64 = AtomicU64::new(0);
static HOME_FAILURE_HANDLERS: AtomicU64 = AtomicU64::new(0);

fn home_pair() -> (u64, u64) {
    unsafe { (core::ptr::addr_of!(HOME[0]).read(), core::ptr::addr_of!(HOME[1]).read()) }
}
#[no_mangle]
pub extern "C" fn molt_frame_homes(slots: u64) -> u64 {
    assert_eq!(slots, 1);
    core::ptr::addr_of_mut!(HOME) as u64
}
#[no_mangle]
pub extern "C" fn molt_frame_home_load(home: u64) -> u64 {
    assert_eq!(home, core::ptr::addr_of_mut!(HOME) as u64);
    let (kind, bits) = home_pair();
    match kind {
        HOME_PLAIN => bits,
        HOME_UNBOUND => HOME_MISSING,
        HOME_RAW => {
            if pending() { return HOME_MISSING; }
            let boxed = molt_int_from_i64(bits as i64);
            if pending() {
                assert_eq!(home_pair(), (HOME_RAW, bits));
                return HOME_MISSING;
            }
            unsafe { HOME = [HOME_PLAIN, boxed]; }
            boxed
        }
        _ => panic!("a plain load cannot read a cell"),
    }
}
#[no_mangle]
pub extern "C" fn molt_frame_home_take(home: u64) -> u64 {
    assert_eq!(home, core::ptr::addr_of_mut!(HOME) as u64);
    let (kind, bits) = home_pair();
    let value = if matches!(kind, HOME_CELL | HOME_PRIVATE_CELL) {
        bits
    } else {
        molt_frame_home_load(home)
    };
    if !pending() { unsafe { HOME = [HOME_UNBOUND, 0]; } }
    value
}
#[no_mangle]
pub extern "C" fn fam_home_reset(failure: u64, reenter: u64) {
    fam_reset(failure);
    assert_eq!(HOME_HELD.load(SeqCst), 0);
    assert_eq!(home_pair(), (HOME_UNBOUND, 0));
    HOME_REENTER.store(reenter, SeqCst);
    HOME_REENTRIES.store(0, SeqCst);
    HOME_SUCCESSES.store(0, SeqCst);
    HOME_FAILURE_HANDLERS.store(0, SeqCst);
    // A transferred old binding, or a cell handle, starts with one owner.
    if reenter != 0 { EXISTING_REFS.store(1, SeqCst); }
}
#[no_mangle]
pub extern "C" fn fam_home_capture(first: u64, alias: u64) {
    assert!(!pending());
    assert_eq!(first, alias, "source-marked aliases share the published object");
    assert_eq!(home_pair(), (HOME_PLAIN, first));
    assert_eq!(HOME_HELD.swap(first, SeqCst), 0);
    molt_inc_ref_obj(first);
}
#[no_mangle]
pub extern "C" fn fam_home_check(value: u64) {
    assert!(!pending());
    assert_eq!(value, HOME_HELD.load(SeqCst), "a later call or home read preserves retained identity");
    assert_eq!(home_pair(), (HOME_PLAIN, value));
}
#[no_mangle]
pub extern "C" fn fam_home_rebound(old: u64, new: u64) {
    assert_eq!((old, new), (0x101, 0x102));
    assert_eq!(HOME_HELD.load(SeqCst), old);
    assert_eq!(home_pair(), (HOME_PLAIN, new));
    assert_eq!((FIRST.load(SeqCst), SECOND.load(SeqCst)), (2, 1), "retained alias and observer survive old-home release");
}
#[no_mangle]
pub extern "C" fn fam_home_taken(view: u64, taken: u64) {
    assert_eq!((view, taken), (0x102, 0x102));
    assert_eq!(home_pair(), (HOME_UNBOUND, 0));
    assert_eq!(SECOND.load(SeqCst), 1, "take transfers the persistent owner");
}
#[no_mangle]
pub extern "C" fn fam_home_release_held() {
    assert_eq!(home_pair(), (HOME_UNBOUND, 0));
    molt_dec_ref_obj(HOME_HELD.swap(0, SeqCst));
}
#[no_mangle]
pub extern "C" fn fam_home_raw_check(value: i64) {
    assert_eq!(value, FIRST_RAW);
    assert_eq!(home_pair(), (HOME_RAW, FIRST_RAW as u64));
    assert_eq!(ATTEMPTS.load(SeqCst), 0, "unobserved raw homes do not allocate");
}
#[no_mangle]
pub extern "C" fn fam_home_store_check(view: u64) {
    assert!(!pending(), "the store's exception edge must skip the following effect");
    assert_eq!(HOME_REENTRIES.load(SeqCst), 1);
    assert_eq!(HOME_SUCCESSES.fetch_add(1, SeqCst), 0);
    assert_eq!(view, 0x101);
    assert_eq!(home_pair(), (HOME_PLAIN, view));
    assert_eq!(FIRST.load(SeqCst), 1, "the home, not call preparation, owns the box");
}
#[no_mangle]
pub extern "C" fn fam_home_store_failed() {
    assert!(pending(), "only the authored exceptional edge reaches this handler");
    assert_eq!(HOME_FAILURE_HANDLERS.fetch_add(1, SeqCst), 0);
    assert_eq!(HOME_SUCCESSES.load(SeqCst), 0);
    assert_eq!(HOME_REENTRIES.load(SeqCst), 1);
    assert_eq!(home_pair(), (HOME_RAW, FIRST_RAW as u64));
    assert_eq!(PENDING_ERROR.load(SeqCst), 0xb001);
}
#[no_mangle]
pub extern "C" fn fam_home_route_check(failure: u64) {
    assert_eq!(HOME_SUCCESSES.load(SeqCst), u64::from(failure == 0));
    assert_eq!(HOME_FAILURE_HANDLERS.load(SeqCst), u64::from(failure != 0));
    assert_eq!(home_pair(), (HOME_UNBOUND, 0));
}
#[no_mangle]
pub extern "C" fn fam_home_cell_check(view: u64, taken: u64) {
    assert_eq!((view, taken), (EXISTING, EXISTING));
    assert_eq!(home_pair(), (HOME_UNBOUND, 0));
    assert_eq!(EXISTING_REFS.load(SeqCst), 1);
}

fn pending() -> bool {
    unsafe { PENDING != 0 }
}
fn raise() -> u64 {
    let _ = PENDING_ERROR.compare_exchange(0, 0xa000, SeqCst, SeqCst);
    unsafe { PENDING = 1; }
    NONE
}
fn task_handle() -> u64 {
    PTR_TAG | (unsafe { core::ptr::addr_of_mut!(TASK_BUF) } as u64 & POINTER_MASK)
}
fn refs(word: u64) -> Option<&'static AtomicU64> {
    match word {
        0x101 => Some(&FIRST),
        0x102 => Some(&SECOND),
        0x200 => Some(&RESULT),
        BUILDER => Some(&BUILDER_REFS),
        DICT => Some(&DICT_REFS),
        EXISTING => Some(&EXISTING_REFS),
        OTHER => Some(&OTHER_REFS),
        word if word == task_handle() => Some(&TASK_REFS),
        _ => None,
    }
}
fn release_all(words: &[AtomicU64; 4], len: &AtomicU64) {
    for slot in &words[..len.swap(0, SeqCst) as usize] {
        molt_dec_ref_obj(slot.load(SeqCst));
    }
}
fn owned_result() -> u64 {
    CALLS.fetch_add(1, SeqCst);
    assert_eq!(RESULT.swap(1, SeqCst), 0, "one owned result per call");
    0x200
}

#[no_mangle]
pub extern "C" fn molt_int_from_i64(value: i64) -> u64 {
    assert!(!pending(), "boxing must stop at the first pending error");
    let attempt = ATTEMPTS.fetch_add(1, SeqCst) + 1;
    if FAIL.load(SeqCst) == attempt {
        PENDING_ERROR.store(0xb000 + attempt, SeqCst);
        return raise();
    }
    let (word, refs) = match value {
        FIRST_RAW => (0x101, &FIRST),
        SECOND_RAW => (0x102, &SECOND),
        other => panic!("an inline integer never allocates: {other}"),
    };
    assert_eq!(refs.swap(1, SeqCst), 0, "one box per distinct operand");
    word
}
#[no_mangle]
pub extern "C" fn molt_exception_pending() -> u64 {
    u64::from(pending())
}
#[no_mangle]
pub extern "C" fn molt_inc_ref_obj(word: u64) {
    if let Some(refs) = refs(word) {
        assert!(refs.fetch_add(1, SeqCst) > 0, "retain after free");
    }
}
#[no_mangle]
pub extern "C" fn molt_dec_ref_obj(word: u64) {
    let Some(refs) = refs(word) else { return };
    let old = refs.fetch_sub(1, SeqCst);
    assert!(old > 0, "duplicate release");
    if old != 1 {
        return;
    }
    if word == BUILDER {
        release_all(&ARGS, &ARGS_LEN);
    } else if word == DICT {
        release_all(&ENTRIES, &ENTRIES_LEN);
    } else if word == task_handle() {
        for index in [SLOT, SLOT + 1] {
            molt_dec_ref_obj(unsafe { core::ptr::addr_of!(TASK_BUF[index]).read() });
        }
    } else if word == EXISTING && HOME_REENTER.load(SeqCst) == 1 {
        // A displaced old binding reenters after complete publication. On OOM
        // it observes the unchanged raw new binding and the original error.
        HOME_REENTRIES.fetch_add(1, SeqCst);
        if pending() {
            assert_eq!(home_pair(), (HOME_RAW, FIRST_RAW as u64));
            assert_eq!(PENDING_ERROR.load(SeqCst), 0xb001);
        } else {
            assert_eq!(home_pair(), (HOME_PLAIN, 0x101));
            assert_eq!(FIRST.load(SeqCst), 1);
        }
    } else if matches!(word, EXISTING | OTHER) && pending() {
        // An adopted input's finalizer can raise. The runtime's canonical
        // call-input release boundary preserves the earlier boxing exception.
        PENDING_ERROR.store(0xd00d, SeqCst);
    }
}

unsafe fn argument_words<'a>(pointer: u64, count: u64) -> &'a [u64] {
    if count == 0 { &[] } else {
        unsafe { core::slice::from_raw_parts(pointer as *const u64, count as usize) }
    }
}
fn release_words(words: &[u64]) {
    for &word in words { molt_dec_ref_obj(word); }
}
fn check_adopted_refs(first: u64, second: u64) {
    assert!(!pending(), "a failed box must skip the consumer entirely");
    assert_eq!(FIRST.load(SeqCst), first, "one owner per transferred raw position, plus any borrow");
    assert_eq!(SECOND.load(SeqCst), second);
    let existing = if OWNED_CASE.load(SeqCst) == 7 { 0 } else { 2 };
    assert_eq!(EXISTING_REFS.load(SeqCst), existing, "preexisting alias positions were already funded");
    assert_eq!(OTHER_REFS.load(SeqCst), u64::from(existing != 0));
}
#[no_mangle]
pub extern "C" fn molt_call_inputs_release(callable: u64, pointer: u64, count: u64) {
    let words = unsafe { argument_words(pointer, count) };
    if OWNED_CASE.load(SeqCst) == 8 && CALLS.load(SeqCst) != 0 {
        assert!(!pending());
        assert_eq!(EXTRACTS.load(SeqCst), 3);
        assert_eq!(POSTCALLS.fetch_add(1, SeqCst), 0, "raw extracted owners retire once after the call");
        assert_eq!(callable, 0);
        assert_eq!(words, &[EXISTING, EXISTING, OTHER]);
        assert_eq!((EXISTING_REFS.load(SeqCst), OTHER_REFS.load(SeqCst)), (2, 1));
        assert_eq!((FIRST.load(SeqCst), SECOND.load(SeqCst), RESULT.load(SeqCst)), (1, 0, 1));
        release_words(words);
        return;
    }
    assert!(pending(), "uncalled inputs retire only after boxing failure");
    assert_eq!(EXTRACTS.load(SeqCst), 0, "failed boxing skips later raw extraction");
    ABORTS.fetch_add(1, SeqCst);
    let (expected_callable, expected): (u64, &[u64]) = match OWNED_CASE.load(SeqCst) {
        0 | 2 | 3 | 4 | 8 => (0, &[EXISTING, EXISTING, OTHER]),
        1 => (EXISTING, &[EXISTING, OTHER]),
        5 | 6 => (0, &[BUILDER]),
        7 => (0, &[NONE, NONE, NONE]),
        other => panic!("unknown owned case {other}"),
    };
    assert_eq!(callable, expected_callable, "callable cleanup is separate and last");
    assert_eq!(words, expected, "snapshot every adopted position before the first box");
    let first_error = PENDING_ERROR.load(SeqCst);
    release_words(words);
    molt_dec_ref_obj(callable);
    PENDING_ERROR.store(first_error, SeqCst);
}
#[no_mangle]
pub extern "C" fn fam_owned_target(
    borrowed: u64, first: u64, again: u64, second: u64,
    existing: u64, existing_again: u64, other: u64, raw: i64, borrowed_second: u64,
) -> u64 {
    assert_eq!((borrowed, first, again, second, raw), (0x101, 0x101, 0x101, 0x102, FIRST_RAW));
    assert_eq!(borrowed_second, second);
    let expected = if OWNED_CASE.load(SeqCst) == 7 { (NONE, NONE, NONE) } else { (EXISTING, EXISTING, OTHER) };
    assert_eq!((existing, existing_again, other), expected);
    check_adopted_refs(3, 2);
    release_words(&[first, again, second, existing, existing_again, other]);
    assert_eq!(FIRST.load(SeqCst), 1, "the borrowed parameter survives consumption of its aliases");
    assert_eq!(SECOND.load(SeqCst), 1, "the later borrowed parameter also survives its adopted alias");
    owned_result()
}
#[no_mangle]
pub extern "C" fn molt_int_as_i64(word: u64) -> i64 {
    assert_eq!(OWNED_CASE.load(SeqCst), 8);
    assert!(!pending());
    assert_eq!(CALLS.load(SeqCst), 0, "extract each adopted object before calling the raw entry");
    assert_eq!((EXISTING_REFS.load(SeqCst), OTHER_REFS.load(SeqCst)), (2, 1));
    EXTRACTS.fetch_add(1, SeqCst);
    match word {
        EXISTING => FIRST_RAW,
        OTHER => SECOND_RAW,
        other => panic!("unknown preboxed integer {other}"),
    }
}
#[no_mangle]
pub extern "C" fn fam_extracted_target(
    borrowed: u64, first: u64, second: u64, raw: i64, raw_again: i64, raw_other: i64,
) -> u64 {
    assert_eq!((borrowed, first, second), (0x101, 0x101, 0x102));
    assert_eq!((raw, raw_again, raw_other), (FIRST_RAW, FIRST_RAW, SECOND_RAW));
    assert_eq!(EXTRACTS.load(SeqCst), 3);
    check_adopted_refs(2, 1);
    release_words(&[first, second]);
    assert_eq!(FIRST.load(SeqCst), 1, "borrowed alias outlives the callee's adopted reference");
    // Raw parameters cannot consume their source objects. The backend retires
    // those three already-funded owners through the canonical boundary afterward.
    assert_eq!((EXISTING_REFS.load(SeqCst), OTHER_REFS.load(SeqCst)), (2, 1));
    owned_result()
}
#[no_mangle]
pub extern "C" fn molt_call_func_owned(callable: u64, pointer: u64, count: u64, site: u64) -> u64 {
    assert_eq!(site, 0);
    let words = unsafe { argument_words(pointer, count) };
    if callable == EXISTING {
        assert_eq!(words, &[0x101, 0x101, 0x102, EXISTING, OTHER]);
        check_adopted_refs(2, 1);
    } else {
        assert_eq!(callable, 0x101);
        assert_eq!(words, &[0x101, 0x101, 0x102, EXISTING, EXISTING, OTHER]);
        check_adopted_refs(3, 1);
    }
    release_words(words);
    molt_dec_ref_obj(callable);
    assert_eq!(FIRST.load(SeqCst), 0, "no borrowed owner remains on this call form");
    owned_result()
}
fn owned_method(class: Option<u64>, receiver: u64, name: u64, len: u64, pointer: u64, count: u64) -> u64 {
    assert_eq!(receiver, EXISTING);
    assert_eq!(unsafe { core::slice::from_raw_parts(name as *const u8, len as usize) }, b"owned");
    let words = unsafe { argument_words(pointer, count) };
    assert_eq!(words, &[0x101, 0x101, 0x102, EXISTING, OTHER]);
    check_adopted_refs(if class.is_some() { 3 } else { 2 }, 1);
    if let Some(class) = class { assert_eq!(class, 0x101); }
    // Self is the first positional owner; a super class remains borrowed.
    molt_dec_ref_obj(receiver);
    release_words(words);
    assert_eq!(FIRST.load(SeqCst), u64::from(class.is_some()), "borrowed class lives through the runtime call");
    owned_result()
}
#[no_mangle]
pub extern "C" fn molt_call_method_ic_owned(_site: u64, receiver: u64, name: u64, len: u64, pointer: u64, count: u64) -> u64 {
    owned_method(None, receiver, name, len, pointer, count)
}
#[no_mangle]
pub extern "C" fn molt_call_super_method_ic_owned(_site: u64, class: u64, receiver: u64, name: u64, len: u64, pointer: u64, count: u64) -> u64 {
    owned_method(Some(class), receiver, name, len, pointer, count)
}
#[no_mangle]
pub extern "C" fn molt_isinstance(object: u64, class: u64) -> u64 {
    assert_eq!(object, BOXED_FIVE, "a raw integer reaches the object ABI boxed");
    assert_eq!(class, NONE);
    CALLS.fetch_add(1, SeqCst);
    TRUE
}
#[no_mangle]
pub extern "C" fn molt_abs_builtin(value: u64) -> u64 {
    assert_eq!(value, 0x101);
    assert_eq!(FIRST.load(SeqCst), 1, "the argument is borrowed from its box owner");
    owned_result()
}
#[no_mangle]
pub extern "C" fn fam_target(value: u64) -> u64 {
    assert_eq!(value, 0x101);
    assert_eq!(FIRST.load(SeqCst), 1, "a compiled callee borrows its argument");
    owned_result()
}
#[no_mangle]
pub extern "C" fn fam_poll(_task: u64) -> u64 {
    panic!("the task is never polled")
}
#[no_mangle]
pub extern "C" fn molt_dict_update_missing(target: u64, key: u64, value: u64) -> u64 {
    assert_eq!((target, key, value), (0x101, NONE, NONE));
    assert_eq!(FIRST.load(SeqCst), 1, "the target is borrowed");
    CALLS.fetch_add(1, SeqCst);
    target
}
#[no_mangle]
pub extern "C" fn molt_callargs_new(positional: u64, keywords: u64) -> u64 {
    assert_eq!(keywords, 0);
    if pending() {
        return 0;
    }
    NEWS.fetch_add(1, SeqCst);
    if BUILDER_FAIL.load(SeqCst) != 0 {
        raise();
        return 0;
    }
    assert_eq!(BUILDER_REFS.swap(1, SeqCst), 0);
    CAPACITY.store(positional, SeqCst);
    ARGS_LEN.store(0, SeqCst);
    BUILDER
}
#[no_mangle]
pub extern "C" fn molt_callargs_push_pos(builder: u64, value: u64) -> u64 {
    if pending() || builder == 0 {
        return NONE;
    }
    assert_eq!(builder, BUILDER);
    let len = ARGS_LEN.load(SeqCst);
    if len >= CAPACITY.load(SeqCst) {
        GROWS.fetch_add(1, SeqCst);
        CAPACITY.store(len + 1, SeqCst);
    }
    molt_inc_ref_obj(value);
    ARGS[len as usize].store(value, SeqCst);
    ARGS_LEN.store(len + 1, SeqCst);
    NONE
}
fn bind(callable: u64, builder: u64) -> u64 {
    BINDS.fetch_add(1, SeqCst);
    let result = if pending() {
        NONE
    } else {
        if OWNED_CASE.load(SeqCst) == 5 {
            assert_eq!(callable, 0x101);
            assert_eq!(FIRST.load(SeqCst), 1, "the builder consumer borrows its callable");
        } else {
            assert_eq!(callable, NONE);
        }
        let len = ARGS_LEN.load(SeqCst) as usize;
        assert_eq!(len as u64, EXPECTED_LEN.load(SeqCst));
        for (arg, expected) in ARGS[..len].iter().zip(&EXPECTED) {
            assert_eq!(arg.load(SeqCst), expected.load(SeqCst), "one identity per argument value");
        }
        owned_result()
    };
    // The consuming call frees its builder on every path.
    if builder != 0 {
        molt_dec_ref_obj(builder);
    }
    result
}
#[no_mangle]
pub extern "C" fn molt_call_bind(callable: u64, builder: u64) -> u64 {
    bind(callable, builder)
}
#[no_mangle]
pub extern "C" fn molt_call_bind_ic(_site: u64, callable: u64, builder: u64) -> u64 {
    bind(callable, builder)
}
#[no_mangle]
pub extern "C" fn molt_call_indirect_ic(_site: u64, callable: u64, builder: u64) -> u64 {
    bind(callable, builder)
}
#[no_mangle]
pub extern "C" fn molt_call_bind_ic_owned(_site: u64, callable: u64, builder: u64) -> u64 {
    BINDS.fetch_add(1, SeqCst);
    assert!(!pending(), "a failed callable box skips the owned builder consumer");
    assert_eq!((callable, builder), (0x101, BUILDER));
    assert_eq!((FIRST.load(SeqCst), BUILDER_REFS.load(SeqCst)), (1, 1));
    molt_dec_ref_obj(builder);
    molt_dec_ref_obj(callable);
    owned_result()
}
#[no_mangle]
pub extern "C" fn molt_dict_new(capacity: u64) -> u64 {
    assert_eq!(capacity, 2);
    if pending() {
        return NONE;
    }
    if DICT_FAIL.load(SeqCst) != 0 {
        return raise();
    }
    assert_eq!(DICT_REFS.swap(1, SeqCst), 0);
    ENTRIES_LEN.store(0, SeqCst);
    DICT
}
#[no_mangle]
pub extern "C" fn molt_dict_set(dict: u64, key: u64, value: u64) -> u64 {
    assert_eq!(dict, DICT);
    assert!(!pending(), "no insertion runs after a failure");
    let index = INSERTS.fetch_add(1, SeqCst);
    assert_eq!(
        ATTEMPTS.load(SeqCst),
        index + 1,
        "an entry is boxed only after the previous insertion"
    );
    let expected = [(0x101, 0x101), (0x102, 0x101)][index as usize];
    assert_eq!((key, value), expected, "a repeated value keeps one identity");
    if INSERT_FAIL.load(SeqCst) == index + 1 {
        return raise();
    }
    let len = ENTRIES_LEN.load(SeqCst) as usize;
    for (offset, word) in [key, value].into_iter().enumerate() {
        molt_inc_ref_obj(word);
        ENTRIES[len + offset].store(word, SeqCst);
    }
    ENTRIES_LEN.store(len as u64 + 2, SeqCst);
    dict
}
#[no_mangle]
pub extern "C" fn molt_task_new(_poll: u64, size: u64, kind: u64) -> u64 {
    assert_eq!((size, kind), (@SIZE@, @KIND@));
    if pending() {
        return NONE;
    }
    assert_eq!(TASK_REFS.swap(1, SeqCst), 0);
    unsafe { core::ptr::addr_of_mut!(TASK_BUF).write([0; 16]) };
    task_handle()
}
#[no_mangle]
pub extern "C" fn fam_reset(failure: u64) {
    assert_eq!(fam_live(), 0);
    for counter in [
        &ATTEMPTS,
        &CALLS,
        &NEWS,
        &GROWS,
        &INSERTS,
        &BUILDER_FAIL,
        &DICT_FAIL,
        &INSERT_FAIL,
        &ARGS_LEN,
        &ENTRIES_LEN,
        &EXPECTED_LEN,
        &PENDING_ERROR,
        &OWNED_CASE,
        &ABORTS,
        &EXTRACTS,
        &POSTCALLS,
        &BINDS,
    ] {
        counter.store(0, SeqCst);
    }
    FAIL.store(failure, SeqCst);
    unsafe { PENDING = 0; }
}
#[no_mangle]
pub extern "C" fn fam_inject(builder: u64, dict: u64, insert: u64) {
    BUILDER_FAIL.store(builder, SeqCst);
    DICT_FAIL.store(dict, SeqCst);
    INSERT_FAIL.store(insert, SeqCst);
}
#[no_mangle]
pub extern "C" fn fam_expect(len: u64, first: u64, second: u64, third: u64) {
    for (slot, word) in EXPECTED.iter().zip([first, second, third]) {
        slot.store(word, SeqCst);
    }
    EXPECTED_LEN.store(len, SeqCst);
}
#[no_mangle]
pub extern "C" fn fam_live() -> u64 {
    [&FIRST, &SECOND, &RESULT, &BUILDER_REFS, &DICT_REFS, &TASK_REFS, &EXISTING_REFS, &OTHER_REFS]
        .iter()
        .map(|refs| refs.load(SeqCst))
        .sum()
}
#[no_mangle]
pub extern "C" fn fam_first_refs() -> u64 {
    FIRST.load(SeqCst)
}
#[no_mangle]
pub extern "C" fn fam_attempts() -> u64 {
    ATTEMPTS.load(SeqCst)
}
#[no_mangle]
pub extern "C" fn fam_calls() -> u64 {
    CALLS.load(SeqCst)
}
#[no_mangle]
pub extern "C" fn fam_news() -> u64 {
    NEWS.load(SeqCst)
}
#[no_mangle]
pub extern "C" fn fam_grows() -> u64 {
    GROWS.load(SeqCst)
}
#[no_mangle]
pub extern "C" fn fam_inserts() -> u64 {
    INSERTS.load(SeqCst)
}
#[no_mangle]
pub extern "C" fn fam_task_handle() -> u64 {
    task_handle()
}
#[no_mangle]
pub extern "C" fn fam_task_slot(index: u64) -> u64 {
    unsafe { core::ptr::addr_of!(TASK_BUF[index as usize]).read() }
}
#[no_mangle]
pub extern "C" fn fam_seed_owned(case: u64) {
    assert_eq!(fam_live(), 0);
    OWNED_CASE.store(case, SeqCst);
    if case <= 4 || case == 8 {
        // Model the references already funded by ownership planning: two
        // transferred positions alias one existing object, plus another input.
        EXISTING_REFS.store(2, SeqCst);
        OTHER_REFS.store(1, SeqCst);
    } else if case <= 6 {
        assert_eq!(molt_callargs_new(0, 0), BUILDER);
    }
}
#[no_mangle]
pub extern "C" fn fam_pending_error() -> u64 { PENDING_ERROR.load(SeqCst) }
#[no_mangle]
pub extern "C" fn fam_aborts() -> u64 { ABORTS.load(SeqCst) }
#[no_mangle]
pub extern "C" fn fam_postcalls() -> u64 { POSTCALLS.load(SeqCst) }
#[no_mangle]
pub extern "C" fn fam_binds() -> u64 { BINDS.load(SeqCst) }
"#;

const FAMILY_HARNESS: &str = r#"
const NONE: u64 = @NONE@;
const TRUE: u64 = @TRUE@;
const SLOT: u64 = @SLOT@;

extern "C" {
    fn fam_isinstance() -> u64;
    fn fam_abs() -> u64;
    fn fam_abs_discard();
    fn fam_dynamic_call() -> u64;
    fn fam_call_bind_pending() -> u64;
    fn fam_dict() -> u64;
    fn fam_alias() -> u64;
    fn fam_alias_discard();
    fn fam_task() -> u64;
    fn fam_direct_call() -> u64;
    fn fam_adopt_direct(a: u64, b: u64) -> u64;
    fn fam_adopt_direct_discard(a: u64, b: u64);
    fn fam_adopt_extracted(a: u64, b: u64) -> u64;
    fn fam_adopt_extracted_discard(a: u64, b: u64);
    fn fam_adopt_func(a: u64, b: u64) -> u64;
    fn fam_adopt_func_discard(a: u64, b: u64);
    fn fam_adopt_guarded(a: u64, b: u64) -> u64;
    fn fam_adopt_guarded_discard(a: u64, b: u64);
    fn fam_adopt_method(a: u64, b: u64) -> u64;
    fn fam_adopt_method_discard(a: u64, b: u64);
    fn fam_adopt_method_ic(a: u64, b: u64) -> u64;
    fn fam_adopt_method_ic_discard(a: u64, b: u64);
    fn fam_adopt_super_ic(a: u64, b: u64) -> u64;
    fn fam_adopt_super_ic_discard(a: u64, b: u64);
    fn fam_adopt_callable_alias(a: u64, b: u64) -> u64;
    fn fam_adopt_callable_alias_discard(a: u64, b: u64);
    fn fam_borrow_builder_bind(a: u64, b: u64) -> u64;
    fn fam_borrow_builder_indirect(a: u64, b: u64) -> u64;
    fn fam_adopt_builder_bind(a: u64, b: u64) -> u64;
    fn fam_adopt_builder_indirect(a: u64, b: u64) -> u64;
    fn fam_adopt_loop();
    fn fam_home_identity();
    fn fam_home_deferred();
    fn fam_home_raw_view();
    fn fam_home_inline();
    fn fam_home_reentry(old: u64);
    fn fam_home_cell(cell: u64);
    fn fam_home_private_cell(cell: u64);
    fn fam_home_reset(failure: u64, reenter: u64);
    fn fam_home_route_check(failure: u64);
    fn fam_seed_owned(case: u64);
    fn fam_pending_error() -> u64;
    fn fam_aborts() -> u64;
    fn fam_postcalls() -> u64;
    fn fam_binds() -> u64;
    fn fam_reset(failure: u64);
    fn fam_inject(builder: u64, dict: u64, insert: u64);
    fn fam_expect(len: u64, first: u64, second: u64, third: u64);
    fn fam_live() -> u64;
    fn fam_first_refs() -> u64;
    fn fam_attempts() -> u64;
    fn fam_calls() -> u64;
    fn fam_news() -> u64;
    fn fam_grows() -> u64;
    fn fam_inserts() -> u64;
    fn fam_task_handle() -> u64;
    fn fam_task_slot(index: u64) -> u64;
    fn molt_exception_pending() -> u64;
    fn molt_dec_ref_obj(value: u64);
}

/// The first exception is pending exactly on failure, and once the caller
/// releases the operation's result no owner the operation created survives.
fn settle(case: &str, result: u64, success: bool) {
    unsafe {
        assert_eq!(molt_exception_pending(), u64::from(!success), "{case}: pending exception");
        molt_dec_ref_obj(result);
        assert_eq!(fam_live(), 0, "{case}: every owner is released exactly once");
    }
}

fn main() {
    unsafe {
        fam_reset(0);
        assert_eq!(fam_isinstance(), TRUE, "isinstance");
        assert_eq!(
            (fam_attempts(), fam_calls()),
            (0, 1),
            "an inline integer is boxed without allocating"
        );
        settle("isinstance", TRUE, true);

        for failure in [0, 1] {
            let success = failure == 0;
            let case = format!("abs {failure}");
            fam_reset(failure);
            let result = fam_abs();
            assert_eq!(result, if success { 0x200 } else { NONE }, "{case}");
            assert_eq!(fam_calls(), u64::from(success), "{case}: a failed box skips the call");
            assert_eq!(fam_live(), u64::from(success), "{case}: the box dies after the call");
            settle(&case, result, success);

            fam_reset(failure);
            fam_abs_discard();
            assert_eq!(fam_calls(), u64::from(success), "discarded {case}");
            settle(&format!("discarded {case}"), NONE, success);
        }

        for (failure, builder_failure) in [(0, 0), (1, 0), (2, 0), (0, 1)] {
            let success = failure == 0 && builder_failure == 0;
            let case = format!("dynamic call {failure}/{builder_failure}");
            fam_reset(failure);
            fam_inject(builder_failure, 0, 0);
            fam_expect(3, 0x101, 0x101, 0x102);
            let result = fam_dynamic_call();
            assert_eq!(result, if success { 0x200 } else { NONE }, "{case}");
            assert_eq!(
                fam_attempts(),
                if failure == 0 { 2 } else { failure },
                "{case}: boxing stops at the first failure"
            );
            assert_eq!(fam_news(), u64::from(failure == 0), "{case}: no builder after a failed box");
            assert_eq!(fam_grows(), 0, "{case}: every positional slot is reserved");
            assert_eq!(fam_calls(), u64::from(success), "{case}: no callee runs after a failure");
            settle(&case, result, success);
        }

        for failure in [0, 1] {
            let success = failure == 0;
            let case = format!("call_bind {failure}");
            fam_reset(failure);
            fam_expect(1, 0x101, 0, 0);
            let result = fam_call_bind_pending();
            assert_eq!(result, if success { 0x200 } else { NONE }, "{case}");
            assert_eq!(fam_news(), 1, "{case}");
            assert_eq!(fam_calls(), u64::from(success), "{case}: an incomplete builder never binds");
            settle(&case, result, success);
        }

        for (failure, dict_failure, insert_failure) in
            [(0, 0, 0), (1, 0, 0), (2, 0, 0), (0, 1, 0), (0, 0, 1), (0, 0, 2)]
        {
            let success = failure == 0 && dict_failure == 0 && insert_failure == 0;
            let case = format!("dict {failure}/{dict_failure}/{insert_failure}");
            fam_reset(failure);
            fam_inject(0, dict_failure, insert_failure);
            let result = fam_dict();
            assert_eq!(result, if success { 0x400 } else { NONE }, "{case}");
            let attempts = if dict_failure != 0 {
                0
            } else if failure != 0 {
                failure
            } else if insert_failure == 1 {
                1
            } else {
                2
            };
            assert_eq!(
                fam_attempts(),
                attempts,
                "{case}: entries are boxed lazily and stop at the first failure"
            );
            let inserts = if dict_failure != 0 || failure == 1 {
                0
            } else if failure == 2 {
                1
            } else if insert_failure != 0 {
                insert_failure
            } else {
                2
            };
            assert_eq!(fam_inserts(), inserts, "{case}");
            if success {
                assert_eq!(
                    fam_first_refs(),
                    3,
                    "{case}: only the dict still references the repeated value"
                );
            }
            settle(&case, result, success);
        }

        for failure in [0, 1] {
            let success = failure == 0;
            let case = format!("borrowed alias {failure}");
            fam_reset(failure);
            let result = fam_alias();
            assert_eq!(result, if success { 0x101 } else { NONE }, "{case}");
            assert_eq!(
                fam_first_refs(),
                u64::from(success),
                "{case}: the escaping alias owns the only reference"
            );
            settle(&case, result, success);

            fam_reset(failure);
            fam_alias_discard();
            assert_eq!(fam_calls(), u64::from(success), "discarded {case}");
            settle(&format!("discarded {case}"), NONE, success);
        }

        for failure in [0, 1] {
            let success = failure == 0;
            let case = format!("task payload {failure}");
            fam_reset(failure);
            let task = fam_task();
            assert_eq!(task, fam_task_handle(), "{case}");
            let word = if success { 0x101 } else { NONE };
            assert_eq!(
                (fam_task_slot(SLOT), fam_task_slot(SLOT + 1)),
                (word, word),
                "{case}: one identity per payload value"
            );
            assert_eq!(
                fam_first_refs(),
                if success { 2 } else { 0 },
                "{case}: each payload slot owns one reference"
            );
            assert_eq!(fam_attempts(), 1, "{case}");
            settle(&case, task, success);
        }

        for failure in [0, 1] {
            let success = failure == 0;
            let case = format!("direct call {failure}");
            fam_reset(failure);
            let result = fam_direct_call();
            assert_eq!(result, if success { 0x200 } else { NONE }, "{case}");
            assert_eq!(fam_calls(), u64::from(success), "{case}: a failed argument box skips the callee");
            settle(&case, result, success);
        }

        type OwnedCase = (u64, &'static str, unsafe extern "C" fn(u64, u64) -> u64, unsafe extern "C" fn(u64, u64));
        let adopted: &[OwnedCase] = &[
            (0, "direct mixed", fam_adopt_direct, fam_adopt_direct_discard),
            (8, "direct extracted owners", fam_adopt_extracted, fam_adopt_extracted_discard),
            (1, "call_func", fam_adopt_func, fam_adopt_func_discard),
            (1, "call_guarded", fam_adopt_guarded, fam_adopt_guarded_discard),
            (1, "call_method", fam_adopt_method, fam_adopt_method_discard),
            (2, "method IC", fam_adopt_method_ic, fam_adopt_method_ic_discard),
            (3, "super IC", fam_adopt_super_ic, fam_adopt_super_ic_discard),
            (4, "callable/argument alias", fam_adopt_callable_alias, fam_adopt_callable_alias_discard),
        ];
        for &(mode, label, invoke, discard) in adopted {
            for failure in [0, 1, 2] {
                for discarded in [false, true] {
                    let success = failure == 0;
                    let case = format!("adopted {label} failure={failure} discard={discarded}");
                    fam_reset(failure);
                    fam_seed_owned(mode);
                    let result = if discarded { discard(0x501, 0x502); NONE } else { invoke(0x501, 0x502) };
                    assert_eq!(result, if success && !discarded { 0x200 } else { NONE }, "{case}");
                    assert_eq!(fam_attempts(), if success { 2 } else { failure }, "{case}: stop before later boxing");
                    assert_eq!(fam_calls(), u64::from(success), "{case}: no consumer after boxing failure");
                    assert_eq!(fam_aborts(), u64::from(!success), "{case}: one cleanup of preexisting adopted inputs");
                    assert_eq!(fam_postcalls(), u64::from(success && mode == 8), "{case}: raw-parameter owners retire after the call");
                    assert_eq!(fam_pending_error(), if success { 0 } else { 0xb000 + failure }, "{case}: preserve the first error through finalizers");
                    assert_eq!(fam_live(), u64::from(success && !discarded), "{case}: only a bound result may escape");
                    settle(&case, result, success);
                }
            }
        }

        let builders: &[(u64, &str, unsafe extern "C" fn(u64, u64) -> u64)] = &[
            (5, "borrowed call_bind", fam_borrow_builder_bind),
            (5, "borrowed call_indirect", fam_borrow_builder_indirect),
            (6, "adopted call_bind", fam_adopt_builder_bind),
            (6, "adopted call_indirect", fam_adopt_builder_indirect),
        ];
        for &(mode, label, invoke) in builders {
            for failure in [0, 1] {
                let success = failure == 0;
                let case = format!("{label} failure={failure}");
                fam_reset(failure);
                fam_seed_owned(mode);
                let result = invoke(0x300, NONE);
                assert_eq!(result, if success { 0x200 } else { NONE }, "{case}");
                assert_eq!(fam_attempts(), 1, "{case}");
                assert_eq!(fam_binds(), u64::from(success), "{case}: a failed callable box skips even the consuming entry");
                assert_eq!(fam_aborts(), u64::from(!success), "{case}: an unconsumed builder is released once");
                assert_eq!(fam_pending_error(), if success { 0 } else { 0xb001 }, "{case}");
                settle(&case, result, success);
            }
        }

        // Reenter the same generated operation in one activation. Its first
        // iteration succeeds; failure 3/4 is its first/second box on iteration 2.
        for failure in [0, 3, 4] {
            let case = format!("adopted loop failure={failure}");
            fam_reset(failure);
            fam_seed_owned(7);
            fam_adopt_loop();
            assert_eq!(fam_attempts(), if failure == 0 { 4 } else { failure }, "{case}");
            assert_eq!(fam_calls(), if failure == 0 { 2 } else { 1 }, "{case}");
            assert_eq!(fam_pending_error(), if failure == 0 { 0 } else { 0xb000 + failure }, "{case}");
            assert_eq!(fam_aborts(), u64::from(failure != 0), "{case}");
            settle(&case, NONE, failure == 0);
        }

        for (case, invoke, attempts) in [
            ("persistent boxed home", fam_home_identity as unsafe extern "C" fn(), 2),
            ("deferred raw home", fam_home_deferred as unsafe extern "C" fn(), 1),
            ("shared raw result proof", fam_home_raw_view as unsafe extern "C" fn(), 1),
            ("inline boxed home", fam_home_inline as unsafe extern "C" fn(), 0),
        ] {
            fam_home_reset(0, 0);
            invoke();
            assert_eq!(fam_attempts(), attempts, "{case}");
            settle(case, NONE, true);
        }
        for failure in [0, 1] {
            fam_home_reset(failure, 1);
            fam_home_reentry(0x501);
            fam_home_route_check(failure);
            assert_eq!(fam_attempts(), 1, "store observation allocates at most once");
            assert_eq!(fam_pending_error(), if failure == 0 { 0 } else { 0xb001 });
            settle("home publication before old finalizer", NONE, failure == 0);
        }
        for invoke in [fam_home_cell as unsafe extern "C" fn(u64), fam_home_private_cell] {
            fam_home_reset(0, 2);
            invoke(0x501);
            assert_eq!(fam_attempts(), 0);
            settle("cell home move and clear", NONE, true);
        }
    }
}
"#;

/// A zero-argument function over the constants `first` (`FIRST_RAW`), `second`
/// (`SECOND_RAW`) and None whose body is `ops`; it returns its result when
/// bound and nothing otherwise.
fn family_function(
    name: &str,
    bound: bool,
    ops: impl FnOnce(&mut TirFunction, [ValueId; 3], Vec<ValueId>) -> Vec<TirOp>,
) -> TirFunction {
    family_function_with_params(name, bound, vec![], |func, values, _, results| {
        ops(func, values, results)
    })
}

fn family_function_with_params(
    name: &str,
    bound: bool,
    param_types: Vec<TirType>,
    ops: impl FnOnce(&mut TirFunction, [ValueId; 3], &[ValueId], Vec<ValueId>) -> Vec<TirOp>,
) -> TirFunction {
    let (return_type, return_abi) = if bound {
        (TirType::DynBox, molt_ir::FunctionReturnAbi::Value)
    } else {
        (TirType::None, molt_ir::FunctionReturnAbi::Void)
    };
    let mut func = TirFunction::new(name.into(), param_types, return_type, return_abi);
    let params: Vec<ValueId> = func.blocks[&func.entry_block]
        .args
        .iter()
        .map(|arg| arg.id)
        .collect();
    let first = func.fresh_value();
    let second = func.fresh_value();
    let none = func.fresh_value();
    let results = if bound {
        vec![func.fresh_value()]
    } else {
        Vec::new()
    };
    let mut body = vec![
        const_int_def(first, FIRST_RAW),
        const_int_def(second, SECOND_RAW),
        const_none_def(none),
    ];
    body.extend(ops(
        &mut func,
        [first, second, none],
        &params,
        results.clone(),
    ));
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.extend(body);
    entry.terminator = Terminator::Return { values: results };
    func
}

fn family_op(
    opcode: OpCode,
    operands: Vec<ValueId>,
    results: Vec<ValueId>,
    attrs: &[(&str, AttrValue)],
) -> TirOp {
    TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands,
        results,
        attrs: attrs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect(),
        source_span: None,
    }
}

fn kind(value: &str) -> (&'static str, AttrValue) {
    ("_original_kind", AttrValue::Str(value.into()))
}

fn home_op(kind_name: &str, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
    family_op(
        OpCode::Copy,
        operands,
        results,
        &[kind(kind_name), ("value", AttrValue::Int(0))],
    )
}

fn home_call(name: &str, operands: Vec<ValueId>) -> TirOp {
    family_op(
        OpCode::Call,
        operands,
        vec![],
        &[kind("call"), ("s_value", AttrValue::Str(name.into()))],
    )
}

/// Store views borrow one persistent home owner. These use the same real
/// object/link/guest lane as the call-family cases; the provider separately
/// retains the first observation to expose rematerialization on later calls.
fn binding_home_functions(backend: &mut LlvmBackend<'_>) -> Vec<TirFunction> {
    for (name, arity) in [
        ("fam_home_capture", 2),
        ("fam_home_check", 1),
        ("fam_home_rebound", 2),
        ("fam_home_taken", 2),
        ("fam_home_release_held", 0),
        ("fam_home_store_check", 1),
        ("fam_home_store_failed", 0),
        ("fam_home_cell_check", 2),
    ] {
        let mut abi = test_native_linkage_abi(vec![TirType::DynBox; arity], None);
        abi.parameter_custody = vec![crate::ir::ParameterCustody::Borrowed; arity];
        backend.function_linkage_abis.insert(name.into(), abi);
    }
    let mut raw_abi = test_native_linkage_abi(vec![TirType::I64], None);
    raw_abi.parameter_custody = vec![crate::ir::ParameterCustody::Borrowed];
    backend
        .function_linkage_abis
        .insert("fam_home_raw_check".into(), raw_abi);

    let mut functions = vec![family_function(
        "fam_home_identity",
        false,
        |func, [first, second, _], _| {
            let view = func.fresh_value();
            let alias = func.fresh_value();
            let loaded = func.fresh_value();
            let rebound = func.fresh_value();
            let taken = func.fresh_value();
            vec![
                home_op("frame_home_store", vec![first], vec![view]),
                {
                    let mut copy = family_op(OpCode::Copy, vec![view], vec![alias], &[]);
                    copy.set_source_op_index(7);
                    copy
                },
                home_call("fam_home_capture", vec![view, alias]),
                home_call("fam_home_check", vec![alias]),
                home_op("frame_home_load", vec![], vec![loaded]),
                home_call("fam_home_check", vec![loaded]),
                // An independently owned alias remains valid when the home changes.
                family_op(OpCode::IncRef, vec![alias], vec![], &[]),
                home_op("frame_home_store", vec![second], vec![rebound]),
                home_call("fam_home_rebound", vec![alias, rebound]),
                family_op(OpCode::DecRef, vec![alias], vec![], &[]),
                home_op("frame_home_take", vec![], vec![taken]),
                home_call("fam_home_taken", vec![rebound, taken]),
                family_op(OpCode::DecRef, vec![taken], vec![], &[]),
                home_op("frame_home_clear", vec![], vec![]),
                home_call("fam_home_release_held", vec![]),
            ]
        },
    )];
    for (name, raw_view) in [("fam_home_deferred", false), ("fam_home_raw_view", true)] {
        let mut view_id = None;
        let func = family_function(name, false, |func, [first, _, _], _| {
            let view = func.fresh_value();
            view_id = Some(view);
            let loaded = func.fresh_value();
            vec![
                home_op(
                    "frame_home_store",
                    vec![first],
                    if raw_view { vec![view] } else { vec![] },
                ),
                home_call(
                    "fam_home_raw_check",
                    vec![if raw_view { view } else { first }],
                ),
                home_op("frame_home_load", vec![], vec![loaded]),
                home_call("fam_home_capture", vec![loaded, loaded]),
                home_call("fam_home_check", vec![loaded]),
                home_op("frame_home_clear", vec![], vec![]),
                home_call("fam_home_release_held", vec![]),
            ]
        });
        if raw_view {
            let mut facts = crate::representation_plan::LlvmReprFacts::default();
            facts
                .repr_by_value
                .insert(view_id.unwrap(), crate::Repr::RawI64FullDeopt);
            backend.function_repr_facts.insert(func.name.clone(), facts);
        }
        functions.push(func);
    }
    functions.push(family_function("fam_home_inline", false, |func, _, _| {
        let five = func.fresh_value();
        let view = func.fresh_value();
        vec![
            const_int_def(five, 5),
            home_op("frame_home_store", vec![five], vec![view]),
            home_call("fam_home_capture", vec![view, view]),
            home_call("fam_home_check", vec![view]),
            home_op("frame_home_clear", vec![], vec![]),
            home_call("fam_home_release_held", vec![]),
        ]
    }));
    functions.push(family_function_with_params(
        "fam_home_reentry",
        false,
        vec![TirType::DynBox],
        |func, [first, _, _], params, _| {
            let view = func.fresh_value();
            let handler = func.fresh_block();
            func.has_exception_handling = true;
            func.label_id_map.insert(handler.0, 51);
            func.blocks.insert(
                handler,
                TirBlock {
                    id: handler,
                    args: vec![],
                    ops: vec![
                        family_op(
                            OpCode::TryEnd,
                            vec![],
                            vec![],
                            &[("value", AttrValue::Int(51))],
                        ),
                        home_call("fam_home_store_failed", vec![]),
                        home_op("frame_home_clear", vec![], vec![]),
                    ],
                    terminator: Terminator::Return { values: vec![] },
                },
            );
            vec![
                family_op(
                    OpCode::TryStart,
                    vec![],
                    vec![],
                    &[("value", AttrValue::Int(51))],
                ),
                home_op("frame_home_store", vec![params[0]], vec![]),
                home_op("frame_home_store", vec![first], vec![view]),
                family_op(
                    OpCode::CheckException,
                    vec![],
                    vec![],
                    &[("value", AttrValue::Int(51))],
                ),
                family_op(
                    OpCode::TryEnd,
                    vec![],
                    vec![],
                    &[("value", AttrValue::Int(51))],
                ),
                home_call("fam_home_store_check", vec![view]),
                home_op("frame_home_clear", vec![], vec![]),
            ]
        },
    ));
    for (name, kind_name) in [
        ("fam_home_cell", "frame_home_cell"),
        ("fam_home_private_cell", "frame_home_private_cell"),
    ] {
        functions.push(family_function_with_params(
            name,
            false,
            vec![TirType::DynBox],
            |func, _, params, _| {
                let view = func.fresh_value();
                let taken = func.fresh_value();
                vec![
                    home_op(kind_name, vec![params[0]], vec![view]),
                    home_op("frame_home_take", vec![], vec![taken]),
                    home_call("fam_home_cell_check", vec![view, taken]),
                    home_op(kind_name, vec![taken], vec![]),
                    home_op("frame_home_clear", vec![], vec![]),
                ]
            },
        ));
    }
    functions
}

fn adopted_call_op(
    shape: &str,
    [first, second, _]: [ValueId; 3],
    existing: ValueId,
    other: ValueId,
    results: Vec<ValueId>,
) -> TirOp {
    use crate::ir::ParameterCustody::{Borrowed, Transferred};
    let args = vec![existing, first, first, second, existing, other];
    let mut op = match shape {
        "direct" => family_op(
            OpCode::Call,
            vec![
                first, first, first, second, existing, existing, other, first, second,
            ],
            results,
            &[
                kind("call"),
                ("s_value", AttrValue::Str("fam_owned_target".into())),
            ],
        ),
        "direct_extracted" => family_op(
            OpCode::Call,
            vec![first, first, second, existing, existing, other],
            results,
            &[
                kind("call"),
                ("s_value", AttrValue::Str("fam_extracted_target".into())),
            ],
        ),
        "call_func" | "call_guarded" | "call_method" => family_op(
            if shape == "call_method" {
                OpCode::CallMethod
            } else {
                OpCode::Call
            },
            args,
            results,
            &[kind(shape)],
        ),
        "call_method_ic" | "call_super_method_ic" => {
            let super_form = shape == "call_super_method_ic";
            family_op(
                if super_form {
                    OpCode::CallSuperMethodIc
                } else {
                    OpCode::CallMethodIc
                },
                if super_form {
                    [vec![first], args].concat()
                } else {
                    args
                },
                results,
                &[kind(shape), ("method", AttrValue::Str("owned".into()))],
            )
        }
        "callable_alias" => family_op(
            OpCode::Call,
            vec![first, first, first, second, existing, existing, other],
            results,
            &[kind("call_func")],
        ),
        "call_bind" | "call_indirect" | "owned_call_bind" | "owned_call_indirect" => {
            let spelling = shape.strip_prefix("owned_").unwrap_or(shape);
            family_op(
                OpCode::Call,
                vec![first, existing],
                results,
                &[kind(spelling)],
            )
        }
        _ => panic!("unknown executable call shape {shape}"),
    };
    let mut custody = vec![Transferred; op.operands.len()];
    match shape {
        "direct" => {
            custody = vec![
                Borrowed,
                Transferred,
                Transferred,
                Transferred,
                Transferred,
                Transferred,
                Transferred,
                Borrowed,
                Borrowed,
            ];
        }
        "direct_extracted" | "call_super_method_ic" | "call_bind" | "call_indirect" => {
            custody[0] = Borrowed;
        }
        _ => {}
    }
    op.set_argument_custody(&custody);
    op
}

fn adopted_call_loop() -> TirFunction {
    let mut func = family_function("fam_adopt_loop", false, |_, values, results| {
        vec![adopted_call_op(
            "direct", values, values[2], values[2], results,
        )]
    });
    let repeat = func.fresh_value();
    let stop = func.fresh_value();
    let again = func.fresh_value();
    let body = func.fresh_block();
    let done = func.fresh_block();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    let calls = entry.ops.split_off(3);
    entry.ops.extend([
        family_op(
            OpCode::ConstBool,
            vec![],
            vec![repeat],
            &[("value", AttrValue::Bool(true))],
        ),
        family_op(
            OpCode::ConstBool,
            vec![],
            vec![stop],
            &[("value", AttrValue::Bool(false))],
        ),
    ]);
    entry.terminator = Terminator::Branch {
        target: body,
        args: vec![repeat],
    };
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![TirValue {
                id: again,
                ty: TirType::Bool,
            }],
            ops: calls,
            terminator: Terminator::CondBranch {
                cond: again,
                then_block: body,
                then_args: vec![stop],
                else_block: done,
                else_args: vec![],
            },
        },
    );
    func.blocks.insert(
        done,
        TirBlock {
            id: done,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func
}

#[test]
fn borrowed_operand_consumers_execute_custody_identity_and_failures() {
    let Some(rustc) = native_object_execution::real_rustc() else {
        return;
    };
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    for symbol in [
        "molt_isinstance",
        "molt_callargs_push_pos",
        "molt_dict_update_missing",
    ] {
        backend.runtime_callable_symbols.insert(symbol.into());
    }
    for name in ["fam_target", "fam_poll"] {
        backend.function_linkage_abis.insert(
            name.into(),
            test_native_linkage_abi(vec![TirType::DynBox], Some(TirType::DynBox)),
        );
    }
    let mut owned_abi = test_native_linkage_abi(
        [
            vec![TirType::DynBox; 7],
            vec![TirType::I64, TirType::DynBox],
        ]
        .concat(),
        Some(TirType::DynBox),
    );
    use crate::ir::ParameterCustody::{Borrowed, Transferred};
    owned_abi.parameter_custody = vec![
        Borrowed,
        Transferred,
        Transferred,
        Transferred,
        Transferred,
        Transferred,
        Transferred,
        Borrowed,
        Borrowed,
    ];
    backend
        .function_linkage_abis
        .insert("fam_owned_target".into(), owned_abi);
    let mut extracted_abi = test_native_linkage_abi(
        [vec![TirType::DynBox; 3], vec![TirType::I64; 3]].concat(),
        Some(TirType::DynBox),
    );
    extracted_abi.parameter_custody = vec![
        Borrowed,
        Transferred,
        Transferred,
        Transferred,
        Transferred,
        Transferred,
    ];
    backend
        .function_linkage_abis
        .insert("fam_extracted_target".into(), extracted_abi);
    let layout = molt_tir::trampolines::TaskConstructorLayout::for_alloc_kind(Some("generator"));
    let payload_base = layout.payload_base_offset(crate::GENERATOR_CONTROL_BYTES);
    let task_size = i64::from(payload_base) + 16;
    let mut functions = vec![
        family_function("fam_isinstance", true, |func, [_, _, none], results| {
            let five = func.fresh_value();
            vec![
                const_int_def(five, 5),
                family_op(
                    OpCode::Copy,
                    vec![five, none],
                    results,
                    &[kind("isinstance")],
                ),
            ]
        }),
        family_function("fam_abs", true, |_, [first, _, _], results| {
            vec![family_op(
                OpCode::Copy,
                vec![first],
                results,
                &[kind("abs")],
            )]
        }),
        family_function("fam_abs_discard", false, |_, [first, _, _], results| {
            vec![family_op(
                OpCode::Copy,
                vec![first],
                results,
                &[kind("abs")],
            )]
        }),
        family_function(
            "fam_dynamic_call",
            true,
            |_, [first, second, none], results| {
                vec![family_op(
                    OpCode::Call,
                    vec![none, first, first, second],
                    results,
                    &[kind("call")],
                )]
            },
        ),
        family_function(
            "fam_call_bind_pending",
            true,
            |func, [first, _, none], results| {
                let builder = func.fresh_value();
                let pushed = func.fresh_value();
                vec![
                    family_op(OpCode::Copy, vec![], vec![builder], &[kind("callargs_new")]),
                    family_op(
                        OpCode::Copy,
                        vec![builder, first],
                        vec![pushed],
                        &[kind("callargs_push_pos")],
                    ),
                    family_op(
                        OpCode::Call,
                        vec![none, builder],
                        results,
                        &[kind("call_bind")],
                    ),
                ]
            },
        ),
        family_function("fam_dict", true, |_, [first, second, _], results| {
            vec![family_op(
                OpCode::BuildDict,
                vec![first, first, second, first],
                results,
                &[],
            )]
        }),
        family_function("fam_alias", true, |_, [first, _, none], results| {
            vec![family_op(
                OpCode::Copy,
                vec![first, none, none],
                results,
                &[kind("dict_update_missing")],
            )]
        }),
        family_function(
            "fam_alias_discard",
            false,
            |_, [first, _, none], results| {
                vec![family_op(
                    OpCode::Copy,
                    vec![first, none, none],
                    results,
                    &[kind("dict_update_missing")],
                )]
            },
        ),
        family_function("fam_task", true, |_, [first, _, _], results| {
            vec![family_op(
                OpCode::AllocTask,
                vec![first, first],
                results,
                &[
                    ("s_value", AttrValue::Str("fam_poll".into())),
                    ("value", AttrValue::Int(task_size)),
                    ("task_kind", AttrValue::Str("generator".into())),
                ],
            )]
        }),
        family_function("fam_direct_call", true, |_, [first, _, _], results| {
            vec![family_op(
                OpCode::Call,
                vec![first],
                results,
                &[
                    kind("call"),
                    ("s_value", AttrValue::Str("fam_target".into())),
                ],
            )]
        }),
    ];
    for (name, shape) in [
        ("fam_adopt_direct", "direct"),
        ("fam_adopt_extracted", "direct_extracted"),
        ("fam_adopt_func", "call_func"),
        ("fam_adopt_guarded", "call_guarded"),
        ("fam_adopt_method", "call_method"),
        ("fam_adopt_method_ic", "call_method_ic"),
        ("fam_adopt_super_ic", "call_super_method_ic"),
        ("fam_adopt_callable_alias", "callable_alias"),
        ("fam_borrow_builder_bind", "call_bind"),
        ("fam_borrow_builder_indirect", "call_indirect"),
        ("fam_adopt_builder_bind", "owned_call_bind"),
        ("fam_adopt_builder_indirect", "owned_call_indirect"),
    ] {
        for bound in [true, false] {
            // Builder result disposal already shares the owned result sink;
            // the adopted source-call families above exercise both forms.
            if !bound && (shape.contains("bind") || shape.contains("indirect")) {
                continue;
            }
            let name = if bound {
                name.to_owned()
            } else {
                format!("{name}_discard")
            };
            functions.push(family_function_with_params(
                &name,
                bound,
                vec![TirType::DynBox; 2],
                |_, values, params, results| {
                    vec![adopted_call_op(
                        shape, values, params[0], params[1], results,
                    )]
                },
            ));
        }
    }
    functions.push(adopted_call_loop());
    functions.extend(binding_home_functions(&mut backend));
    for func in &functions {
        let lowered = lower_tir_to_llvm(func, &backend);
        assert_entry_only_allocas(lowered, &lowered.print_to_string().to_string());
    }
    backend
        .module
        .verify()
        .expect("borrowed-operand family module must verify");
    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("llvm-borrowed-family-object")
        .expect("create the LLVM object within Cargo image custody");
    let object = artifacts.path().join("llvm_borrowed_family.o");
    backend
        .emit_object(&object, crate::llvm_backend::MoltOptLevel::None)
        .expect("emit the LLVM object");
    let object_bytes = std::fs::read(&object).expect("read the LLVM object");
    let none = (nanbox::QNAN | nanbox::TAG_NONE).to_string();
    let truth = (nanbox::QNAN | nanbox::TAG_BOOL | 1).to_string();
    let slot = (payload_base / 8).to_string();
    let provider = FAMILY_PROVIDER
        .replace("@ABI@", molt_codegen_abi::GENERATED_OBJECT_ABI_SYMBOL)
        .replace("@NONE@", &none)
        .replace("@TRUE@", &truth)
        .replace("@FIVE@", &(nanbox::QNAN | nanbox::TAG_INT | 5).to_string())
        .replace("@PTR@", &(nanbox::QNAN | nanbox::TAG_PTR).to_string())
        .replace("@SLOT@", &slot)
        .replace(
            "@HOME_UNBOUND@",
            &molt_codegen_abi::FRAME_HOME_UNBOUND.to_string(),
        )
        .replace(
            "@HOME_PLAIN@",
            &molt_codegen_abi::FRAME_HOME_PLAIN.to_string(),
        )
        .replace(
            "@HOME_RAW@",
            &molt_codegen_abi::FRAME_HOME_RAW_INT.to_string(),
        )
        .replace(
            "@HOME_CELL@",
            &molt_codegen_abi::FRAME_HOME_CELL.to_string(),
        )
        .replace(
            "@HOME_PRIVATE_CELL@",
            &molt_codegen_abi::FRAME_HOME_PRIVATE_CELL.to_string(),
        )
        .replace("@SIZE@", &task_size.to_string())
        .replace(
            "@KIND@",
            &crate::native_task_runtime_kind_bits(layout.runtime_kind()).to_string(),
        );
    let harness = FAMILY_HARNESS
        .replace("@NONE@", &none)
        .replace("@TRUE@", &truth)
        .replace("@SLOT@", &slot);
    native_object_execution::link_and_run_native_object(
        &rustc,
        "llvm-borrowed-family-execution",
        object_bytes,
        &provider,
        &harness,
        "LLVM borrowed-operand family",
    );
}
