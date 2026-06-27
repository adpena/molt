#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) static EMPTY_VEC_STRING: Vec<String> = Vec::new();

#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn is_cold_module_chunk_function(
    name: &str,
) -> bool {
    name.contains("__molt_module_chunk_")
}
