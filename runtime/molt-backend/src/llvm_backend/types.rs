//! TirType -> LLVM type mapping for the LLVM backend.

#[cfg(feature = "llvm")]
use inkwell::context::Context;
#[cfg(feature = "llvm")]
use inkwell::types::BasicTypeEnum;

#[cfg(feature = "llvm")]
use crate::tir::types::TirType;

/// Map a TIR type to its LLVM representation.
///
/// Unboxed scalars map directly to machine types (i64, f64, i1).
/// Reference types (Str, List, Dict, etc.) are opaque pointers.
/// DynBox is i64 (NaN-boxed representation).
/// Tuples become LLVM struct types with element-wise lowering.
#[cfg(feature = "llvm")]
pub fn lower_type<'ctx>(ctx: &'ctx Context, ty: &TirType) -> BasicTypeEnum<'ctx> {
    match ty {
        TirType::I64 | TirType::BigInt => ctx.i64_type().into(),
        TirType::F64 => ctx.f64_type().into(),
        TirType::Bool => ctx.bool_type().into(),
        // None is a sentinel constant — we represent it as i64 so it fits
        // in the NaN-boxed universe without special-casing at every use site.
        TirType::None => ctx.i64_type().into(),
        // NaN-boxed dynamic value: 64-bit integer holding tag + payload.
        TirType::DynBox => ctx.i64_type().into(),
        // Reference types are opaque pointers to heap objects.
        TirType::Str | TirType::Bytes => {
            ctx.ptr_type(inkwell::AddressSpace::default()).into()
        }
        TirType::List(_) | TirType::Dict(_, _) | TirType::Set(_) => {
            ctx.ptr_type(inkwell::AddressSpace::default()).into()
        }
        TirType::Tuple(fields) => {
            let field_types: Vec<BasicTypeEnum<'ctx>> =
                fields.iter().map(|f| lower_type(ctx, f)).collect();
            ctx.struct_type(&field_types, false).into()
        }
        TirType::Ptr(_) | TirType::Func(_) => {
            ctx.ptr_type(inkwell::AddressSpace::default()).into()
        }
        // Box(inner) is still a NaN-boxed i64 at the machine level;
        // the inner type is only used for optimization decisions.
        TirType::Box(_) => ctx.i64_type().into(),
        // Union collapses to DynBox representation.
        TirType::Union(_) => ctx.i64_type().into(),
        // Never (bottom type) — use i64 as a placeholder; code using Never
        // is unreachable, so the choice doesn't matter.
        TirType::Never => ctx.i64_type().into(),
    }
}

#[cfg(all(test, feature = "llvm"))]
mod tests {
    use super::*;
    use crate::tir::types::{FuncSignature, TirType};
    use inkwell::context::Context;

    #[test]
    fn lower_i64_type() {
        let ctx = Context::create();
        let ty = lower_type(&ctx, &TirType::I64);
        assert!(ty.is_int_type());
        assert_eq!(ty.into_int_type().get_bit_width(), 64);
    }

    #[test]
    fn lower_f64_type() {
        let ctx = Context::create();
        let ty = lower_type(&ctx, &TirType::F64);
        assert!(ty.is_float_type());
    }

    #[test]
    fn lower_bool_type() {
        let ctx = Context::create();
        let ty = lower_type(&ctx, &TirType::Bool);
        assert!(ty.is_int_type());
        assert_eq!(ty.into_int_type().get_bit_width(), 1);
    }

    #[test]
    fn lower_dynbox_is_i64() {
        let ctx = Context::create();
        let ty = lower_type(&ctx, &TirType::DynBox);
        assert!(ty.is_int_type());
        assert_eq!(ty.into_int_type().get_bit_width(), 64);
    }

    #[test]
    fn lower_str_is_ptr() {
        let ctx = Context::create();
        let ty = lower_type(&ctx, &TirType::Str);
        assert!(ty.is_pointer_type());
    }

    #[test]
    fn lower_tuple_is_struct() {
        let ctx = Context::create();
        let ty = lower_type(&ctx, &TirType::Tuple(vec![TirType::I64, TirType::F64]));
        assert!(ty.is_struct_type());
        let st = ty.into_struct_type();
        assert_eq!(st.count_fields(), 2);
    }

    #[test]
    fn lower_func_is_ptr() {
        let ctx = Context::create();
        let ty = lower_type(
            &ctx,
            &TirType::Func(FuncSignature {
                params: vec![TirType::I64],
                return_type: Box::new(TirType::I64),
            }),
        );
        assert!(ty.is_pointer_type());
    }
}
