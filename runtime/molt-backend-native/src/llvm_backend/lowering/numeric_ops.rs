//! LLVM's numeric operators lower in the walker (`codegen_ssa::operators`);
//! this module keeps their LLVM IR tests.
#[cfg(test)]
mod numeric_family_tests {
    use super::super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    #[test]
    fn float_divrem_preserves_zero_dispatch_and_python_remainder_sign() {
        for (opcode, runtime) in [(OpCode::Div, RuntimeFn::Div), (OpCode::Mod, RuntimeFn::Mod)] {
            let context = inkwell::context::Context::create();
            let mut backend = LlvmBackend::new(&context, "float_divrem_semantics");
            let mut func = TirFunction::new(
                "float_divrem".into(),
                vec![TirType::F64, TirType::F64],
                TirType::F64,
                molt_ir::FunctionReturnAbi::Value,
            );
            let result = func.fresh_value();
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(TirOp {
                origin: None,
                runtime: None,
                callee: None,
                dialect: Dialect::Molt,
                opcode,
                operands: vec![ValueId(0), ValueId(1)],
                results: vec![result],
                attrs: AttrDict::from_iter([("fast_math".into(), AttrValue::Bool(true))]),
                source_span: None,
            });
            entry.terminator = Terminator::Return {
                values: vec![result],
            };
            // Lowerer-only raw-ABI control. Parameter annotations alone are
            // deliberately boxed in production; this test exercises the
            // machine float operation's zero, NaN and remainder-sign rules.
            let mut facts = crate::representation_plan::ProducerCarrierPlan::default();
            for value in [ValueId(0), ValueId(1), result] {
                facts
                    .repr_by_value
                    .insert(value, molt_ir::repr::Repr::FloatUnboxed);
            }
            backend.function_repr_facts.insert(func.name.clone(), facts);
            try_lower_tir_to_llvm(&func, &backend).unwrap();
            backend.module.verify().unwrap();
            let ir = backend.module.print_to_string().to_string();
            // The walker's model tests own the values; LLVM must keep the
            // zero-divisor test, its raising runtime entry and IEEE rules.
            assert!(ir.contains("fcmp une"));
            assert!(ir.contains(runtime.linker_symbol()));
            assert!(
                !ir.contains("fdiv fast") && !ir.contains("frem fast"),
                "fast-math must not erase zero/NaN/sign semantics"
            );
        }
    }
}
