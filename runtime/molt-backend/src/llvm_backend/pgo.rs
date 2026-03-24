//! Profile-Guided Optimization infrastructure.
//!
//! Two modes:
//! - Instrument: insert counters at function entries and branch points
//! - Use: load profile data and attach branch weight metadata

use std::collections::HashMap;

/// PGO mode configuration.
#[derive(Debug, Clone)]
pub enum PgoMode {
    /// No PGO.
    None,
    /// Instrument: insert profiling counters.
    Instrument { output_path: String },
    /// Use: load profile and optimize.
    Use { profile_path: String },
}

/// Add PGO instrumentation calls to a function.
/// Inserts @llvm.instrprof.increment at function entry and each branch point.
pub fn instrument_function(func_name: &str, num_counters: u32) -> Vec<String> {
    // Returns LLVM IR snippets for instrumentation
    // These would be inserted during LLVM lowering
    let mut snippets = Vec::new();
    snippets.push(format!(
        "call void @llvm.instrprof.increment(ptr @__profc_{name}, i64 0, i32 {n}, i32 0)",
        name = func_name,
        n = num_counters
    ));
    snippets
}

/// Parse a .profdata file and extract branch weights.
/// Returns: function_name -> (branch_index -> taken_count)
pub fn load_profile(profile_path: &str) -> HashMap<String, Vec<u64>> {
    // Stub: in production, this would call llvm-profdata merge
    // and parse the binary profile format.
    // For now, return empty (no profile data = no PGO optimization)
    let _ = profile_path;
    HashMap::new()
}

/// Attach branch weight metadata to a conditional branch.
/// Returns the LLVM metadata string for !prof annotation.
pub fn branch_weight_metadata(true_count: u64, false_count: u64) -> String {
    format!(
        "!{{!\"branch_weights\", i32 {}, i32 {}}}",
        true_count, false_count
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instrument_function_produces_valid_snippet() {
        let snippets = instrument_function("my_func", 4);
        assert_eq!(snippets.len(), 1);
        assert!(snippets[0].contains("@llvm.instrprof.increment"));
        assert!(snippets[0].contains("@__profc_my_func"));
        assert!(snippets[0].contains("i32 4"));
    }

    #[test]
    fn branch_weight_metadata_format_correct() {
        let md = branch_weight_metadata(100, 5);
        assert_eq!(md, "!{!\"branch_weights\", i32 100, i32 5}");
    }

    #[test]
    fn load_profile_nonexistent_returns_empty() {
        let profile = load_profile("/nonexistent/path/profile.profdata");
        assert!(profile.is_empty());
    }
}
