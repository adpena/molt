use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    /// A synthesized name is one temporary owner, borrowed by the dependent
    /// runtime operation (`codegen_ssa::object::begin_owned_name`).
    pub(super) fn with_owned_name(
        &mut self,
        name: &str,
        consume: impl FnOnce(&mut Self, inkwell::values::IntValue<'ctx>) -> BasicValueEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let owned = crate::codegen_ssa::object::begin_owned_name(&mut self.codegen(), name);
        let word = owned.word;
        let consumed = consume(self, word);
        assert_eq!(
            consumed.get_type(),
            self.backend.context.i64_type().into(),
            "name consumer must return boxed i64 bits"
        );
        crate::codegen_ssa::object::finish_owned_name(
            &mut self.codegen(),
            owned,
            consumed.into_int_value(),
        )
        .into()
    }

    /// `s`'s bytes as a private constant: its address and its length
    /// (`codegen_ssa::object::raw_name_pointer`).
    pub(super) fn raw_string_const_ptr_and_len(
        &mut self,
        s: &str,
    ) -> (
        inkwell::values::PointerValue<'ctx>,
        inkwell::values::IntValue<'ctx>,
    ) {
        let (pointer, len) = crate::codegen_ssa::object::raw_name_pointer(&mut self.codegen(), s);
        (pointer.into_pointer_value(), len)
    }
}
