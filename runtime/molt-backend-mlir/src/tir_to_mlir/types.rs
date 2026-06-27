use melior::{
    Context as MlirContext,
    ir::{Type, r#type::IntegerType},
};
use molt_backend::tir::types::TirType;

pub(super) fn mlir_type_for_tir<'c>(ctx: &'c MlirContext, ty: &TirType) -> Type<'c> {
    match ty {
        TirType::I64 | TirType::BigInt | TirType::DynBox | TirType::None => {
            IntegerType::new(ctx, 64).into()
        }
        TirType::F64 => Type::float64(ctx),
        TirType::Bool => IntegerType::new(ctx, 1).into(),
        // Reference types are represented as opaque i64 pointers at this stage.
        // A future MoltPtr dialect type would replace this.
        TirType::Str | TirType::Bytes | TirType::Ptr(_) => IntegerType::new(ctx, 64).into(),
        TirType::Never => IntegerType::new(ctx, 64).into(),
        // Compound and callable types default to i64 (boxed representation).
        TirType::List(_)
        | TirType::Dict(_, _)
        | TirType::Set(_)
        | TirType::Tuple(_)
        | TirType::Box(_)
        | TirType::Func(_)
        | TirType::Union(_) => IntegerType::new(ctx, 64).into(),
    }
}
