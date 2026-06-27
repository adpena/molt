use wasm_encoder::{Function, Instruction, ValType};

/// The result of lowering a single TIR/LIR function to WASM.
#[derive(Debug, Clone)]
pub struct WasmFunctionOutput {
    /// WASM parameter types.
    pub param_types: Vec<ValType>,
    /// WASM result types.
    pub result_types: Vec<ValType>,
    /// Local variable types (excludes parameters).
    pub locals: Vec<ValType>,
    /// WASM instruction sequence (function body).
    pub instructions: Vec<Instruction<'static>>,
    /// Runtime imports this body calls, in emission order. Each entry pairs
    /// positionally with one `Instruction::Call(NAMED_RUNTIME_CALL_PLACEHOLDER)`
    /// in `instructions`; the module assembler walks the stream and replaces
    /// the k-th placeholder with the import index of `runtime_calls[k]`.
    /// Positional, not index-keyed, because the peephole pass rewrites the
    /// stream and shifts instruction indexes. Distinct from the `Call(0)` bail
    /// sentinel and the `u32::MAX` skipped-import sentinel.
    pub runtime_calls: Vec<&'static str>,
}

/// Placeholder callee index for a named runtime call recorded in
/// [`WasmFunctionOutput::runtime_calls`].
///
/// The module assembler resolves it to a real import index. `u32::MAX - 1`
/// keeps it distinct from the `Call(0)` bail sentinel and the `u32::MAX`
/// skipped-import sentinel.
pub const NAMED_RUNTIME_CALL_PLACEHOLDER: u32 = u32::MAX - 1;

/// Fast-output sentinel: this function must be rejected and lowered by the
/// generic WASM path.
pub const LIR_FAST_OUTPUT_BAIL_CALL: u32 = 0;

#[must_use]
pub fn is_named_runtime_call_placeholder(instruction: &Instruction<'_>) -> bool {
    matches!(
        instruction,
        Instruction::Call(NAMED_RUNTIME_CALL_PLACEHOLDER)
    )
}

#[must_use]
pub fn is_lir_fast_output_bail_call(instruction: &Instruction<'_>) -> bool {
    matches!(instruction, Instruction::Call(LIR_FAST_OUTPUT_BAIL_CALL))
}

#[must_use]
pub fn has_lir_fast_output_bail_call(output: &WasmFunctionOutput) -> bool {
    output.instructions.iter().any(is_lir_fast_output_bail_call)
}

#[must_use]
pub fn named_runtime_call_placeholder_count(output: &WasmFunctionOutput) -> usize {
    output
        .instructions
        .iter()
        .filter(|instruction| is_named_runtime_call_placeholder(instruction))
        .count()
}

pub fn assert_named_runtime_call_pairing(func_name: &str, output: &WasmFunctionOutput) {
    assert_eq!(
        named_runtime_call_placeholder_count(output),
        output.runtime_calls.len(),
        "LIR fast output for '{func_name}' must pair named-call placeholders 1:1 with runtime_calls entries"
    );
}

pub fn emit_lir_fast_output_body(
    func_name: &str,
    output: &WasmFunctionOutput,
    mut import_index_for: impl FnMut(&str) -> u32,
    func: &mut Function,
) {
    assert_named_runtime_call_pairing(func_name, output);
    let mut named_calls = output.runtime_calls.iter();
    for instruction in &output.instructions {
        if is_named_runtime_call_placeholder(instruction) {
            let name = named_calls.next().unwrap_or_else(|| {
                panic!(
                    "LIR fast output for '{func_name}' has more named-call placeholders than runtime_calls entries"
                )
            });
            let import_index = import_index_for(name);
            assert!(
                import_index != u32::MAX,
                "LIR fast output for '{func_name}' calls runtime import '{name}' which was skipped/pruned from the import set"
            );
            func.instruction(&Instruction::Call(import_index));
            continue;
        }
        func.instruction(instruction);
    }
    assert!(
        named_calls.next().is_none(),
        "LIR fast output for '{func_name}' has unconsumed runtime_calls entries"
    );
}
