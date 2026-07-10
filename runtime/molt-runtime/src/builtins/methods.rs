mod common;
mod core_types;
mod dispatch;
mod io;
mod numeric;
mod sequence;
mod singletons;
mod specialized;

pub(crate) use common::{
    builtin_func_bits, builtin_func_bits_with_bind_kind, builtin_func_bits_with_defaults_tuple,
};
pub(crate) use core_types::{
    memoryview_method_bits, object_method_bits, range_method_bits, type_method_bits,
};
pub(crate) use dispatch::builtin_class_method_bits;
pub(crate) use io::file_method_bits;
pub(crate) use numeric::{complex_method_bits, float_method_bits, int_method_bits};
pub(crate) use sequence::{
    bytearray_method_bits, bytes_method_bits, slice_method_bits, string_method_bits,
};
pub(crate) use singletons::{
    ellipsis_bits, is_missing_bits, is_not_implemented_bits, missing_bits, not_implemented_bits,
};
pub(crate) use specialized::{
    asyncgen_method_bits, coroutine_method_bits, generator_method_bits, property_method_bits,
};
