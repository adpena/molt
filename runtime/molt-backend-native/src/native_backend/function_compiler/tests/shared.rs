use super::*;

#[test]
fn cold_module_chunk_codegen_classification_only_matches_module_chunks() {
    assert!(is_cold_module_chunk_function(
        "molt_gpu_tensor__molt_module_chunk_2"
    ));
    assert!(is_cold_module_chunk_function(
        "builtins__molt_module_chunk_4"
    ));
    assert!(!is_cold_module_chunk_function(
        "main_molt__Attention___call__"
    ));
    assert!(!is_cold_module_chunk_function(
        "molt_gpu_tensor__Tensor__broadcast_op"
    ));
    assert!(!is_cold_module_chunk_function("molt_main"));
}
