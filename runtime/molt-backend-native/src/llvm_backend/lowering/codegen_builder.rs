//! LLVM's implementation of the walker's builder trait
//! (`codegen_ssa::CodegenBuilder`): instruction selection only.

use inkwell::values::{BasicValueEnum, IntValue};
use molt_codegen_abi::word::{BranchBuilder, WordBuilder};
use molt_ir::runtime_fn_generated::RuntimeFn;

use super::{LlvmBranchWords, LlvmWords, llvm_choose};
use crate::codegen_ssa::{
    CodegenBuilder, FloatBinary, FloatComparison, IntBinary, IntComparison, MachineKind,
};
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
use crate::llvm_backend::runtime_imports::runtime_function;

/// The LLVM builder at its current insertion point, with the module that
/// owns the runtime declarations a call needs.
#[derive(Clone, Copy)]
pub(in crate::llvm_backend) struct LlvmCodegen<'a, 'ctx> {
    pub(in crate::llvm_backend) words: LlvmWords<'a, 'ctx>,
    pub(in crate::llvm_backend) module: &'a inkwell::module::Module<'ctx>,
}

impl<'a, 'ctx> WordBuilder for LlvmCodegen<'a, 'ctx> {
    molt_codegen_abi::forward_word_builder!(words: LlvmWords<'a, 'ctx>);
}

impl<'ctx> LlvmBranchWords<'ctx> for LlvmCodegen<'_, 'ctx> {
    fn llvm(&self) -> LlvmWords<'_, 'ctx> {
        self.words
    }
}

impl<'ctx> BranchBuilder for LlvmCodegen<'_, 'ctx> {
    fn choose(
        &mut self,
        condition: Self::Predicate,
        yes: impl FnOnce(&mut Self) -> Self::Word,
        no: impl FnOnce(&mut Self) -> Self::Word,
    ) -> Self::Word {
        llvm_choose(self, condition, yes, no)
    }
}

impl<'ctx> CodegenBuilder for LlvmCodegen<'_, 'ctx> {
    type Value = BasicValueEnum<'ctx>;

    fn kind(&self, value: Self::Value) -> MachineKind {
        match value {
            BasicValueEnum::IntValue(int) => match int.get_type().get_bit_width() {
                1 => MachineKind::Bit,
                64 => MachineKind::Word,
                width if width < 64 => MachineKind::NarrowInt,
                width => panic!("no lowered value is a {width}-bit integer"),
            },
            BasicValueEnum::FloatValue(_) => MachineKind::Float,
            BasicValueEnum::PointerValue(_) => MachineKind::Address,
            other => panic!("{other:?} is not a scalar machine value"),
        }
    }
    fn word_value(&self, word: IntValue<'ctx>) -> Self::Value {
        word.into()
    }
    fn bit_value(&self, bit: IntValue<'ctx>) -> Self::Value {
        bit.into()
    }
    fn expect_word(&self, value: Self::Value) -> IntValue<'ctx> {
        value.into_int_value()
    }
    fn expect_bit(&self, value: Self::Value) -> IntValue<'ctx> {
        value.into_int_value()
    }

    fn zero_extend(&mut self, value: Self::Value) -> IntValue<'ctx> {
        self.words
            .builder
            .build_int_z_extend(
                value.into_int_value(),
                self.words.context.i64_type(),
                "zext_i64",
            )
            .unwrap()
    }
    fn low_bit(&mut self, word: IntValue<'ctx>) -> IntValue<'ctx> {
        self.words
            .builder
            .build_int_truncate(word, self.words.context.bool_type(), "low_bit")
            .unwrap()
    }
    fn float_to_bits(&mut self, value: Self::Value) -> IntValue<'ctx> {
        self.words
            .builder
            .build_bit_cast(
                value.into_float_value(),
                self.words.context.i64_type(),
                "f2i",
            )
            .unwrap()
            .into_int_value()
    }
    fn bits_to_float(&mut self, word: IntValue<'ctx>) -> Self::Value {
        self.words
            .builder
            .build_bit_cast(word, self.words.context.f64_type(), "i2f")
            .unwrap()
    }
    fn address_to_word(&mut self, value: Self::Value) -> IntValue<'ctx> {
        self.words
            .builder
            .build_ptr_to_int(
                value.into_pointer_value(),
                self.words.context.i64_type(),
                "ptr2i",
            )
            .unwrap()
    }

    fn int_binary(
        &mut self,
        op: IntBinary,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
        no_signed_wrap: bool,
    ) -> IntValue<'ctx> {
        let builder = self.words.builder;
        match (op, no_signed_wrap) {
            (IntBinary::Add, true) => builder.build_int_nsw_add(left, right, "add"),
            (IntBinary::Add, false) => builder.build_int_add(left, right, "add"),
            (IntBinary::Sub, true) => builder.build_int_nsw_sub(left, right, "sub"),
            (IntBinary::Sub, false) => builder.build_int_sub(left, right, "sub"),
            (IntBinary::Mul, true) => builder.build_int_nsw_mul(left, right, "mul"),
            (IntBinary::Mul, false) => builder.build_int_mul(left, right, "mul"),
            (IntBinary::And, _) => builder.build_and(left, right, "band"),
            (IntBinary::Or, _) => builder.build_or(left, right, "bor"),
            (IntBinary::Xor, _) => builder.build_xor(left, right, "bxor"),
            (IntBinary::ShiftLeft, _) => builder.build_left_shift(left, right, "shl"),
            (IntBinary::ShiftRightArithmetic, _) => {
                builder.build_right_shift(left, right, true, "shr")
            }
        }
        .unwrap()
    }
    fn int_compare(
        &mut self,
        comparison: IntComparison,
        left: IntValue<'ctx>,
        right: IntValue<'ctx>,
    ) -> IntValue<'ctx> {
        let predicate = match comparison {
            IntComparison::Equal => inkwell::IntPredicate::EQ,
            IntComparison::NotEqual => inkwell::IntPredicate::NE,
            IntComparison::SignedLess => inkwell::IntPredicate::SLT,
            IntComparison::SignedLessOrEqual => inkwell::IntPredicate::SLE,
            IntComparison::SignedGreater => inkwell::IntPredicate::SGT,
            IntComparison::SignedGreaterOrEqual => inkwell::IntPredicate::SGE,
        };
        self.words
            .builder
            .build_int_compare(predicate, left, right, "int_test")
            .unwrap()
    }
    fn int_negate(&mut self, word: IntValue<'ctx>) -> IntValue<'ctx> {
        self.words.builder.build_int_neg(word, "neg").unwrap()
    }
    fn int_invert(&mut self, word: IntValue<'ctx>) -> IntValue<'ctx> {
        self.words.builder.build_not(word, "invert").unwrap()
    }
    fn bit_not(&mut self, bit: IntValue<'ctx>) -> IntValue<'ctx> {
        self.words.builder.build_not(bit, "not").unwrap()
    }

    fn float_binary(
        &mut self,
        op: FloatBinary,
        left: Self::Value,
        right: Self::Value,
        fast_math: bool,
    ) -> Self::Value {
        let builder = self.words.builder;
        let (left, right) = (left.into_float_value(), right.into_float_value());
        let value = match op {
            FloatBinary::Add => builder.build_float_add(left, right, "fadd"),
            FloatBinary::Sub => builder.build_float_sub(left, right, "fsub"),
            FloatBinary::Mul => builder.build_float_mul(left, right, "fmul"),
        }
        .unwrap();
        // A constant-folded result is not an instruction and takes no flags.
        if fast_math && let Some(instruction) = value.as_instruction() {
            instruction
                .set_fast_math_flags(super::llvm_fast_math_all())
                .unwrap();
        }
        value.into()
    }
    fn float_negate(&mut self, value: Self::Value) -> Self::Value {
        self.words
            .builder
            .build_float_neg(value.into_float_value(), "fneg")
            .unwrap()
            .into()
    }
    fn float_constant(&mut self, value: f64) -> Self::Value {
        self.words.context.f64_type().const_float(value).into()
    }
    fn float_compare(
        &mut self,
        comparison: FloatComparison,
        left: Self::Value,
        right: Self::Value,
    ) -> IntValue<'ctx> {
        let predicate = match comparison {
            FloatComparison::OrderedEqual => inkwell::FloatPredicate::OEQ,
            FloatComparison::UnorderedOrNotEqual => inkwell::FloatPredicate::UNE,
            FloatComparison::OrderedLess => inkwell::FloatPredicate::OLT,
            FloatComparison::OrderedLessOrEqual => inkwell::FloatPredicate::OLE,
            FloatComparison::OrderedGreater => inkwell::FloatPredicate::OGT,
            FloatComparison::OrderedGreaterOrEqual => inkwell::FloatPredicate::OGE,
        };
        self.words
            .builder
            .build_float_compare(
                predicate,
                left.into_float_value(),
                right.into_float_value(),
                "float_test",
            )
            .unwrap()
    }

    fn call_runtime(
        &mut self,
        function: RuntimeFn,
        args: &[IntValue<'ctx>],
    ) -> Option<IntValue<'ctx>> {
        let declared = runtime_function(self.words.context, self.module, function);
        let args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            args.iter().map(|&arg| arg.into()).collect();
        self.words
            .builder
            .build_execution_call(declared, &args, "runtime")
            .unwrap()
            .try_as_basic_value()
            .basic()
            .map(BasicValueEnum::into_int_value)
    }
}
