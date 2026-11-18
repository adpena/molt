use super::*;
use crate::codegen_ssa::CodegenBuilder;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    /// A synthesized name is one temporary owner, borrowed by the dependent
    /// runtime operation. On construction failure skip that operation entirely;
    /// merge a boxed None with the pending exception intact so the enclosing
    /// TIR operation's CheckException retains ownership of handler/cleanup flow.
    pub(super) fn with_owned_name(
        &mut self,
        name: &str,
        consume: impl FnOnce(&mut Self, inkwell::values::IntValue<'ctx>) -> BasicValueEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let (name_bits, status) = crate::codegen_ssa::constants::byte_literal(
            &mut self.codegen(),
            name.as_bytes(),
            &TirType::Str,
        );
        let status = status.into_int_value();
        let builder = &self.backend.builder;
        let source = builder.get_insert_block().expect("name construction block");
        let suffix = self.synthetic_block_counter;
        self.synthetic_block_counter += 1;
        let success = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, &format!("owned_name_success{suffix}"));
        let merge = self
            .backend
            .context
            .append_basic_block(self.llvm_fn, &format!("owned_name_merge{suffix}"));
        let ok = builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                status,
                self.backend.context.i32_type().const_zero(),
                "owned_name_ok",
            )
            .unwrap();
        builder
            .build_conditional_branch(ok, success, merge)
            .unwrap();
        self.backend.builder.position_at_end(success);
        let result = consume(self, name_bits);
        assert_eq!(
            result.get_type(),
            self.backend.context.i64_type().into(),
            "name consumer must return boxed i64 bits"
        );
        let drop_name = self
            .ensure_refcount_site(molt_codegen_abi::inline_refcount::RefcountOperation::Release);
        drop_name.emit(&self.backend.builder, name_bits.into());
        let consumed = self
            .backend
            .builder
            .get_insert_block()
            .expect("name consumer block");
        self.backend
            .builder
            .build_unconditional_branch(merge)
            .unwrap();
        self.backend.builder.position_at_end(merge);
        let i64_ty = self.backend.context.i64_type();
        let none = i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false);
        let phi = self
            .backend
            .builder
            .build_phi(i64_ty, "owned_name_result")
            .unwrap();
        phi.add_incoming(&[(&none, source), (&result, consumed)]);
        phi.as_basic_value()
    }

    pub(super) fn raw_string_const_ptr_and_len(
        &mut self,
        s: &str,
    ) -> (
        inkwell::values::PointerValue<'ctx>,
        inkwell::values::IntValue<'ctx>,
    ) {
        let i64_ty = self.backend.context.i64_type();
        let name_bytes = s.as_bytes();
        let ptr = self
            .codegen()
            .bytes_constant(
                name_bytes,
                "__guard_attr_str_",
                &format!("_{}", sanitize_const_name(s)),
            )
            .into_pointer_value();
        let len_bits = i64_ty.const_int(name_bytes.len() as u64, false);
        (ptr, len_bits)
    }

    pub(super) fn raw_string_const_ptr_len(
        &mut self,
        s: &str,
    ) -> (
        inkwell::values::IntValue<'ctx>,
        inkwell::values::IntValue<'ctx>,
    ) {
        let i64_ty = self.backend.context.i64_type();
        let (ptr, len_bits) = self.raw_string_const_ptr_and_len(s);
        let ptr_bits = self
            .backend
            .builder
            .build_ptr_to_int(ptr, i64_ty, "guard_attr_ptr")
            .unwrap();
        (ptr_bits, len_bits)
    }
}

fn sanitize_const_name(s: &str) -> String {
    s.replace(|c: char| !c.is_alphanumeric(), "_")
}
