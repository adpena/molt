//! Core TIR -> LLVM IR lowering.
//!
//! This module converts a `TirFunction` into an LLVM `FunctionValue` using
//! type-specialized emission: when operand types are statically known (e.g.
//! I64+I64), we emit native LLVM instructions; when types are dynamic
//! (DynBox), we emit calls to the Molt runtime.

#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionModuleExt;
#[cfg(all(test, feature = "llvm"))]
use molt_ir::OpKind;
use std::cell::RefCell;

use crate::tir::ops::{AttrValue, TirOp};

#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::prepend_execution_context;
#[cfg(feature = "llvm")]
use inkwell::basic_block::BasicBlock;
#[cfg(feature = "llvm")]
use inkwell::types::BasicType;
#[cfg(feature = "llvm")]
use inkwell::values::{BasicValueEnum, FastMathFlags, FunctionValue};
#[cfg(feature = "llvm")]
use molt_codegen_abi as nanbox;
#[cfg(feature = "llvm")]
use molt_ir::runtime_fn_generated::RuntimeFn;

#[cfg(feature = "llvm")]
use crate::codegen_ssa::CodegenBuilder;
#[cfg(feature = "llvm")]
use crate::llvm_backend::LlvmBackend;
#[cfg(feature = "llvm")]
use crate::llvm_backend::types::lower_type;
#[cfg(feature = "llvm")]
use inkwell::attributes::AttributeLoc;

#[cfg(feature = "llvm")]
use crate::tir::function::TirFunction;
#[cfg(feature = "llvm")]
#[cfg(feature = "llvm")]
fn require_function_linkage_abi<'a>(
    func: &TirFunction,
    backend: &'a LlvmBackend<'_>,
) -> std::borrow::Cow<'a, molt_ir::linkage_abi::NativeFunctionLinkageAbi> {
    if let Some(abi) = backend.function_linkage_abis.get(&func.name) {
        return std::borrow::Cow::Borrowed(abi);
    }
    #[cfg(test)]
    {
        // Direct lowering unit tests intentionally bypass the production
        // compile driver. Keep their local-only fixture authority out of
        // shipped code while production fails closed on every absent row.
        let returns_value = func.return_abi.returns_value();
        let facts = backend
            .function_repr_facts
            .get(&func.name)
            .cloned()
            .unwrap_or_else(|| crate::representation_plan::ProducerCarrierPlan::build(func));
        std::borrow::Cow::Owned(facts.linkage_abi(
            func,
            crate::ir::ExternFunctionSignature {
                arity: func.param_types.len(),
                has_closure: false,
                returns_value,
                execution_context: func.execution_context,
            },
            Vec::new(),
        ))
    }
    #[cfg(not(test))]
    panic!(
        "LLVM function `{}` has no exact native linkage ABI",
        func.name
    );
}

#[cfg(feature = "llvm")]
mod codegen_builder;
#[cfg(feature = "llvm")]
pub(in crate::llvm_backend) use codegen_builder::LlvmCodegen;
#[cfg(feature = "llvm")]
mod field_slot;
#[cfg(feature = "llvm")]
mod frame_record;
mod global_slot;
mod list_item;
#[cfg(feature = "llvm")]
mod method_site;
#[cfg(feature = "llvm")]
mod numeric_ops;
#[cfg(feature = "llvm")]
mod object_lifecycle;
#[cfg(feature = "llvm")]
mod refcount;
#[cfg(feature = "llvm")]
mod task_site;

#[cfg(feature = "llvm")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlvmLoweringError {
    diagnostics: Vec<String>,
}

#[cfg(feature = "llvm")]
impl LlvmLoweringError {
    fn new(diagnostics: Vec<String>) -> Self {
        Self { diagnostics }
    }

    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }
}

#[cfg(feature = "llvm")]
impl std::fmt::Display for LlvmLoweringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "LLVM TIR lowering failed with {} diagnostic(s):",
            self.diagnostics.len()
        )?;
        for diagnostic in &self.diagnostics {
            writeln!(f, "- {diagnostic}")?;
        }
        Ok(())
    }
}

#[cfg(feature = "llvm")]
impl std::error::Error for LlvmLoweringError {}

/// LLVM's one spelling of 64-bit word instructions. Every NaN-box primitive
/// and reference-count transition comes from the shared generators in
/// molt-codegen-abi through this builder; no encoding constant is spelled here.
#[cfg(feature = "llvm")]
#[derive(Clone, Copy)]
pub(super) struct LlvmWords<'a, 'ctx> {
    pub(super) context: &'ctx inkwell::context::Context,
    pub(super) builder: &'a inkwell::builder::Builder<'ctx>,
}

#[cfg(feature = "llvm")]
impl<'ctx> nanbox::word::WordBuilder for LlvmWords<'_, 'ctx> {
    type Word = inkwell::values::IntValue<'ctx>;
    type Predicate = inkwell::values::IntValue<'ctx>;
    fn constant(&mut self, value: u64) -> Self::Word {
        self.context.i64_type().const_int(value, false)
    }
    fn and(&mut self, value: Self::Word, mask: u64) -> Self::Word {
        let mask = self.constant(mask);
        self.builder.build_and(value, mask, "word_and").unwrap()
    }
    fn or(&mut self, value: Self::Word, bits: u64) -> Self::Word {
        let bits = self.constant(bits);
        self.builder.build_or(value, bits, "word_or").unwrap()
    }
    fn shift_left(&mut self, value: Self::Word, amount: u32) -> Self::Word {
        let amount = self.constant(u64::from(amount));
        self.builder
            .build_left_shift(value, amount, "word_shl")
            .unwrap()
    }
    fn shift_right_arithmetic(&mut self, value: Self::Word, amount: u32) -> Self::Word {
        let amount = self.constant(u64::from(amount));
        self.builder
            .build_right_shift(value, amount, true, "word_sar")
            .unwrap()
    }
    fn add_immediate(&mut self, value: Self::Word, delta: i64) -> Self::Word {
        let delta = self.constant(delta as u64);
        self.builder
            .build_int_add(value, delta, "word_add")
            .unwrap()
    }
    fn compare(
        &mut self,
        comparison: nanbox::word::WordComparison,
        left: Self::Word,
        right: Self::Word,
    ) -> Self::Predicate {
        let predicate = match comparison {
            nanbox::word::WordComparison::Equal => inkwell::IntPredicate::EQ,
            nanbox::word::WordComparison::NotEqual => inkwell::IntPredicate::NE,
            nanbox::word::WordComparison::UnsignedLessThan => inkwell::IntPredicate::ULT,
        };
        self.builder
            .build_int_compare(predicate, left, right, "word_test")
            .unwrap()
    }
    fn both(&mut self, left: Self::Predicate, right: Self::Predicate) -> Self::Predicate {
        self.builder.build_and(left, right, "word_both").unwrap()
    }
}

/// The LLVM builder a word builder spells into.
#[cfg(feature = "llvm")]
pub(super) trait LlvmBranchWords<'ctx> {
    fn llvm(&self) -> LlvmWords<'_, 'ctx>;
}

#[cfg(feature = "llvm")]
impl<'ctx> LlvmBranchWords<'ctx> for LlvmWords<'_, 'ctx> {
    fn llvm(&self) -> LlvmWords<'_, 'ctx> {
        *self
    }
}

/// `BranchBuilder::choose` in LLVM: a conditional branch to two blocks of the
/// current function that join in one i64 phi. Every LLVM word builder uses it.
#[cfg(feature = "llvm")]
pub(super) fn llvm_choose<'ctx, S: LlvmBranchWords<'ctx>>(
    s: &mut S,
    condition: inkwell::values::IntValue<'ctx>,
    yes: impl FnOnce(&mut S) -> inkwell::values::IntValue<'ctx>,
    no: impl FnOnce(&mut S) -> inkwell::values::IntValue<'ctx>,
) -> inkwell::values::IntValue<'ctx> {
    let (context, function) = {
        let words = s.llvm();
        let block = words
            .builder
            .get_insert_block()
            .expect("choose inside a block");
        (
            words.context,
            block.get_parent().expect("a block has a function"),
        )
    };
    let yes_block = context.append_basic_block(function, "choose_yes");
    let no_block = context.append_basic_block(function, "choose_no");
    let join = context.append_basic_block(function, "choose_join");
    s.llvm()
        .builder
        .build_conditional_branch(condition, yes_block, no_block)
        .unwrap();
    s.llvm().builder.position_at_end(yes_block);
    let taken = yes(s);
    let taken_end = s.llvm().builder.get_insert_block().unwrap();
    s.llvm().builder.build_unconditional_branch(join).unwrap();
    s.llvm().builder.position_at_end(no_block);
    let fallback = no(s);
    let fallback_end = s.llvm().builder.get_insert_block().unwrap();
    s.llvm().builder.build_unconditional_branch(join).unwrap();
    s.llvm().builder.position_at_end(join);
    let phi = s
        .llvm()
        .builder
        .build_phi(context.i64_type(), "choose_value")
        .unwrap();
    phi.add_incoming(&[(&taken, taken_end), (&fallback, fallback_end)]);
    phi.as_basic_value().into_int_value()
}

#[cfg(feature = "llvm")]
impl<'ctx> nanbox::word::BranchBuilder for LlvmWords<'_, 'ctx> {
    fn choose(
        &mut self,
        condition: Self::Predicate,
        yes: impl FnOnce(&mut Self) -> Self::Word,
        no: impl FnOnce(&mut Self) -> Self::Word,
    ) -> Self::Word {
        llvm_choose(self, condition, yes, no)
    }
}

// ── LLVM fast-math flag constants (from llvm-sys LLVMFastMath* definitions) ──
//
// AllowReassoc | NoNaNs | NoInfs | NoSignedZeros | AllowReciprocal
//             | AllowContract | ApproxFunc  (= "fast" in IR text)
#[cfg(feature = "llvm")]
fn llvm_fast_math_all() -> FastMathFlags {
    FastMathFlags::AllowReassoc
        | FastMathFlags::NoNaNs
        | FastMathFlags::NoInfs
        | FastMathFlags::NoSignedZeros
        | FastMathFlags::AllowReciprocal
        | FastMathFlags::AllowContract
        | FastMathFlags::ApproxFunc
}

/// Return `true` when `op.attrs[key]` is `AttrValue::Bool(true)`.
#[cfg(feature = "llvm")]
fn has_attr(op: &TirOp, key: &str) -> bool {
    matches!(op.attrs.get(key), Some(AttrValue::Bool(true)))
}

/// Holds state during lowering of a single TIR function.
/// Lower a TIR function to LLVM IR.
///
/// Returns the LLVM function value. The function is added to `backend.module`.
#[cfg(feature = "llvm")]
pub fn lower_tir_to_llvm<'ctx>(
    func: &TirFunction,
    backend: &LlvmBackend<'ctx>,
) -> FunctionValue<'ctx> {
    try_lower_tir_to_llvm(func, backend).unwrap_or_else(|err| panic!("{err}"))
}

/// Checked lowering entry point used by production compile paths.
#[cfg(feature = "llvm")]
pub fn try_lower_tir_to_llvm<'ctx>(
    func: &TirFunction,
    backend: &LlvmBackend<'ctx>,
) -> Result<FunctionValue<'ctx>, LlvmLoweringError> {
    use crate::codegen_ssa::walk::{FunctionInput, PreparedFunction, lower_function};
    use crate::representation_plan::ProducerCarrierPlan;

    let body = PreparedFunction::new(func).map_err(LlvmLoweringError::new)?;
    let abi = require_function_linkage_abi(func, backend);
    let llvm_fn = declare_tir_function_with_abi(func, backend, &abi);
    // A split activation publishes its frame size; task constructors read it.
    if let Some(AttrValue::Int(frame_size)) = func
        .attrs
        .get(crate::tir::lower_from_simple::STATEFUL_FRAME_SIZE_ATTR)
    {
        backend.define_stateful_frame_size(&func.name, *frame_size);
    }
    let plan = backend.function_repr_facts.get(&func.name);
    let default_plan;
    let plan = match plan {
        Some(plan) => plan,
        None => {
            default_plan = ProducerCarrierPlan::default();
            &default_plan
        }
    };
    let physical_entry = backend.context.append_basic_block(llvm_fn, "entry");
    backend.builder.position_at_end(physical_entry);
    let parameters: Vec<_> = llvm_fn.get_param_iter().skip(1).collect();
    let params = RefCell::new(Default::default());
    let state = codegen_builder::LlvmFunctionState::new(
        molt_ir::ir::CodeTemperature::of_function(&func.name).inline_fast_paths(),
    );
    let mut codegen = LlvmCodegen {
        words: LlvmWords {
            context: backend.context,
            builder: &backend.builder,
        },
        backend,
        params: &params,
        state: &state,
    };
    lower_function(
        &mut codegen,
        FunctionInput {
            body,
            own_abi: &abi,
            plan,
            physical_entry,
            entry: &parameters,
            linkage: &backend.function_linkage_abis,
            functions: &backend.function_table,
            runtime: &backend.runtime_callable_symbols,
        },
    )
    .map_err(LlvmLoweringError::new)?;

    // Cold code is optimized for size (`CodeTemperature`).
    if molt_ir::ir::CodeTemperature::of_function(&func.name).optimize_for_size() {
        for attribute in ["optsize", "minsize"] {
            let kind = inkwell::attributes::Attribute::get_named_enum_kind_id(attribute);
            assert_ne!(kind, 0, "LLVM must support the {attribute} attribute");
            llvm_fn.add_attribute(
                AttributeLoc::Function,
                backend.context.create_enum_attribute(kind, 0),
            );
        }
    }

    // TIR eligibility must also survive LLVM's independent module optimizer.
    if func.is_codegen_partition() {
        let kind = inkwell::attributes::Attribute::get_named_enum_kind_id("noinline");
        assert_ne!(
            kind, 0,
            "LLVM must support the required noinline partition boundary"
        );
        llvm_fn.add_attribute(
            AttributeLoc::Function,
            backend.context.create_enum_attribute(kind, 0),
        );
    }

    // 7. If any op in this function carries `fast_math = true`, annotate the
    //    function with `"unsafe-fp-math"="true"`.  This is the function-level
    //    fallback for LLVM passes that inspect function attributes rather than
    //    per-instruction fast-math flags.
    let has_any_fast_math = func
        .blocks
        .values()
        .any(|blk| blk.ops.iter().any(|op| has_attr(op, "fast_math")));
    if has_any_fast_math {
        let attr = backend
            .context
            .create_string_attribute("unsafe-fp-math", "true");
        llvm_fn.add_attribute(AttributeLoc::Function, attr);
    }

    Ok(llvm_fn)
}

#[cfg(feature = "llvm")]
fn require_llvm_function_type<'ctx>(
    symbol: &str,
    existing: FunctionValue<'ctx>,
    expected: inkwell::types::FunctionType<'ctx>,
) -> FunctionValue<'ctx> {
    let actual = existing.get_type();
    let same_shape = actual.get_return_type() == expected.get_return_type()
        && actual.get_param_types() == expected.get_param_types()
        && actual.is_var_arg() == expected.is_var_arg();
    if !same_shape {
        panic!(
            "LLVM function type mismatch for `{symbol}`: expected {}, actual {}",
            expected.print_to_string(),
            actual.print_to_string()
        );
    }
    existing
}

#[cfg(feature = "llvm")]
fn add_molt_nounwind_attribute<'ctx>(backend: &LlvmBackend<'ctx>, function: FunctionValue<'ctx>) {
    // Molt reports Python failures through its pending-error ABI rather than
    // native unwinding. Do not infer termination: Python bodies and external
    // providers may loop forever, so LLVM `willreturn` would be unsound.
    let nounwind_kind = inkwell::attributes::Attribute::get_named_enum_kind_id("nounwind");
    function.add_attribute(
        AttributeLoc::Function,
        backend.context.create_enum_attribute(nounwind_kind, 0),
    );
}

#[cfg(feature = "llvm")]
pub fn declare_tir_function<'ctx>(
    func: &TirFunction,
    backend: &LlvmBackend<'ctx>,
) -> FunctionValue<'ctx> {
    declare_tir_function_with_abi(func, backend, &require_function_linkage_abi(func, backend))
}

#[cfg(feature = "llvm")]
fn declare_tir_function_with_abi<'ctx>(
    func: &TirFunction,
    backend: &LlvmBackend<'ctx>,
    linkage_abi: &molt_ir::linkage_abi::NativeFunctionLinkageAbi,
) -> FunctionValue<'ctx> {
    assert_eq!(
        linkage_abi.param_types.len(),
        func.param_types.len(),
        "LLVM definition `{}` parameter count disagrees with its exact linkage ABI",
        func.name
    );
    let param_llvm_types: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> = linkage_abi
        .param_types
        .iter()
        .map(|ty| lower_type(backend.context, ty).into())
        .collect();

    let logical_fn_ty = match linkage_abi.return_type.as_ref() {
        Some(return_type) => {
            lower_type(backend.context, return_type).fn_type(&param_llvm_types, false)
        }
        None => backend
            .context
            .void_type()
            .fn_type(&param_llvm_types, false),
    };
    let fn_ty = prepend_execution_context(logical_fn_ty);
    // Reuse an existing forward-declaration if present (e.g., from a prior
    // Call op that referenced this function before it was defined).
    // If not, create a new function.  This avoids LLVM appending `.1` to
    // the name when a declaration already exists.

    let llvm_fn = if let Some(existing) = backend.module.get_compiled_function(&func.name) {
        let existing = require_llvm_function_type(&func.name, existing, fn_ty);
        // Verify it's just a declaration (no basic blocks yet).
        if existing.count_basic_blocks() == 0 {
            existing
        } else {
            panic!(
                "LLVM function `{}` already has a body before TIR definition",
                func.name
            );
        }
    } else {
        backend
            .module
            .add_compiled_function(&func.name, logical_fn_ty, None)
    };
    if linkage_abi.checked_raw_return() {
        llvm_fn.set_linkage(inkwell::module::Linkage::Internal);
    }
    add_molt_nounwind_attribute(backend, llvm_fn);
    llvm_fn
}

/// Find or declare a compiled function by its exact linkage ABI row. Program
/// metadata (the module registry's initializer column, the app callable
/// resolver, the wasm32 isolate-import switch) takes the address of bodies that
/// another object may define. A wasm32 link checks every function signature,
/// so no placeholder type may stand in for the row.
#[cfg(feature = "llvm")]
pub(crate) fn linked_compiled_function<'ctx>(
    backend: &LlvmBackend<'ctx>,
    name: &str,
) -> FunctionValue<'ctx> {
    let linkage_abi = backend
        .function_linkage_abis
        .get(name)
        .unwrap_or_else(|| panic!("LLVM compiled function `{name}` has no exact linkage ABI"));
    let params: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> = linkage_abi
        .param_types
        .iter()
        .map(|ty| lower_type(backend.context, ty).into())
        .collect();
    let logical_fn_ty = match linkage_abi.return_type.as_ref() {
        Some(return_type) => lower_type(backend.context, return_type).fn_type(&params, false),
        None => backend.context.void_type().fn_type(&params, false),
    };
    if let Some(existing) = backend.module.get_compiled_function(name) {
        return require_llvm_function_type(
            name,
            existing,
            crate::llvm_backend::execution_abi::prepend_execution_context(logical_fn_ty),
        );
    }
    backend.module.add_compiled_function(
        name,
        logical_fn_ty,
        Some(inkwell::module::Linkage::External),
    )
}

#[cfg(feature = "llvm")]
pub fn declare_extern_tir_function<'ctx>(
    func: &TirFunction,
    backend: &LlvmBackend<'ctx>,
) -> FunctionValue<'ctx> {
    let linkage_abi = backend
        .function_linkage_abis
        .get(&func.name)
        .unwrap_or_else(|| panic!("LLVM extern `{}` lost its exact linkage ABI", func.name));
    assert_eq!(
        func.param_types.len(),
        linkage_abi.param_types.len(),
        "LLVM extern `{}` TIR parameter count disagrees with its exact linkage ABI",
        func.name
    );
    let param_llvm_types: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> = linkage_abi
        .param_types
        .iter()
        .map(|ty| lower_type(backend.context, ty).into())
        .collect();
    let logical_fn_ty = match linkage_abi.return_type.as_ref() {
        Some(return_type) => {
            lower_type(backend.context, return_type).fn_type(&param_llvm_types, false)
        }
        None => backend
            .context
            .void_type()
            .fn_type(&param_llvm_types, false),
    };
    let fn_ty = prepend_execution_context(logical_fn_ty);
    let llvm_fn = if let Some(existing) = backend.module.get_compiled_function(&func.name) {
        let existing = require_llvm_function_type(&func.name, existing, fn_ty);
        assert_eq!(
            existing.count_basic_blocks(),
            0,
            "LLVM extern `{}` must remain declaration-only",
            func.name
        );
        existing
    } else {
        backend.module.add_compiled_function(
            &func.name,
            logical_fn_ty,
            Some(inkwell::module::Linkage::External),
        )
    };
    add_molt_nounwind_attribute(backend, llvm_fn);
    llvm_fn
}

// Every staged call materializes its generic runtime entry on LLVM.
const _: () = assert!(
    molt_ir::runtime_calls::RuntimeCall::all_runtime_realized(
        molt_ir::tir::target_info::TargetKind::Llvm
    ),
    "a staged runtime call is not runtime-realized on LLVM"
);

#[cfg(all(test, feature = "llvm"))]
mod tests;

#[cfg(all(test, feature = "llvm"))]
mod operation_shape_tests {
    use super::*;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    #[test]
    fn malformed_preserved_metadata_is_rejected_before_llvm_module_mutation() {
        for (operands, owner, value) in [
            (1, Some("owner"), None),
            (3, Some("owner"), None),
            (2, None, None),
            (2, Some(""), None),
            (2, Some("owner"), Some(0)),
        ] {
            let context = inkwell::context::Context::create();
            let backend = LlvmBackend::new(&context, "shape_admission");
            let mut func = TirFunction::new(
                "bad_code_slot".into(),
                vec![],
                TirType::None,
                molt_ir::FunctionReturnAbi::Void,
            );
            let origin = Some(OpKind::CodeSlotSet);
            let mut attrs = AttrDict::from_iter([]);
            if let Some(owner) = owner {
                attrs.insert("s_value".into(), AttrValue::Str(owner.into()));
            }
            if let Some(value) = value {
                attrs.insert("value".into(), AttrValue::Int(value));
            }
            func.blocks
                .get_mut(&func.entry_block)
                .unwrap()
                .ops
                .push(TirOp {
                    origin,
                    runtime: None,
                    callee: None,
                    dialect: Dialect::Molt,
                    opcode: OpCode::Copy,
                    operands: (0..operands).map(ValueId).collect(),
                    results: vec![],
                    attrs,
                    source_span: None,
                });
            let error = try_lower_tir_to_llvm(&func, &backend).expect_err("checked shape failure");
            assert!(error.to_string().contains("code_slot_set"));
            assert_eq!(backend.module.get_functions().count(), 0);
        }
    }
}
