/// Runtime helper symbol produced when SimpleIR GPU intrinsics are lifted into
/// first-class TIR `Call` ops.
pub fn gpu_runtime_symbol_for_simple_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "gpu_thread_id" => Some("molt_gpu_thread_id"),
        "gpu_block_id" => Some("molt_gpu_block_id"),
        "gpu_block_dim" => Some("molt_gpu_block_dim"),
        "gpu_grid_dim" => Some("molt_gpu_grid_dim"),
        "gpu_barrier" => Some("molt_gpu_barrier"),
        _ => None,
    }
}

/// True for fixed GPU runtime-intrinsic symbols, which are runtime-helper calls,
/// not user-defined Python call targets.
pub fn is_gpu_runtime_symbol(symbol: &str) -> bool {
    matches!(
        symbol,
        "molt_gpu_thread_id"
            | "molt_gpu_block_id"
            | "molt_gpu_block_dim"
            | "molt_gpu_grid_dim"
            | "molt_gpu_barrier"
    )
}

#[cfg(test)]
mod tests {
    use super::{gpu_runtime_symbol_for_simple_kind, is_gpu_runtime_symbol};

    #[test]
    fn gpu_simple_kinds_map_to_runtime_symbols() {
        for (kind, symbol) in [
            ("gpu_thread_id", "molt_gpu_thread_id"),
            ("gpu_block_id", "molt_gpu_block_id"),
            ("gpu_block_dim", "molt_gpu_block_dim"),
            ("gpu_grid_dim", "molt_gpu_grid_dim"),
            ("gpu_barrier", "molt_gpu_barrier"),
        ] {
            assert_eq!(gpu_runtime_symbol_for_simple_kind(kind), Some(symbol));
            assert!(is_gpu_runtime_symbol(symbol));
        }
        assert_eq!(gpu_runtime_symbol_for_simple_kind("call"), None);
        assert!(!is_gpu_runtime_symbol("user_function"));
    }
}
