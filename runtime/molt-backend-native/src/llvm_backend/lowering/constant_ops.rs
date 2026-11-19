use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
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
