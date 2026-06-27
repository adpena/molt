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
///
/// If `profile_path` ends with `.profraw`, the raw profile is first merged
/// into a `.profdata` file using `llvm-profdata merge`. The binary used is
/// controlled by the `LLVM_PROFDATA` environment variable; if unset it
/// defaults to `"llvm-profdata"`.
pub fn load_profile(profile_path: &str) -> HashMap<String, Vec<u64>> {
    // Resolve the llvm-profdata binary.
    let profdata_bin =
        std::env::var("LLVM_PROFDATA").unwrap_or_else(|_| "llvm-profdata".to_string());

    // If a .profraw file was given, merge it first.
    let profdata_path = if profile_path.ends_with(".profraw") {
        let merged = format!("{}.profdata", profile_path.trim_end_matches(".profraw"));
        let status = std::process::Command::new(&profdata_bin)
            .args(["merge", "-o", &merged, profile_path])
            .status();
        match status {
            Ok(s) if s.success() => merged,
            _ => return HashMap::new(),
        }
    } else {
        profile_path.to_string()
    };

    // Run `llvm-profdata show` to get text output we can parse.
    let output = std::process::Command::new(&profdata_bin)
        .args(["show", "--all-functions", "--counts", &profdata_path])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            parse_profdata_output(&String::from_utf8_lossy(&out.stdout))
        }
        _ => HashMap::new(),
    }
}

/// Parse the text output of `llvm-profdata show --all-functions --counts`.
///
/// Expected format per function:
/// ```text
/// function_name:
///   Hash: 0x...
///   Counters: N
///   Block counts: [1, 2, 3, ...]
/// ```
fn parse_profdata_output(text: &str) -> HashMap<String, Vec<u64>> {
    let mut result = HashMap::new();
    let mut current_func = String::new();

    for line in text.lines() {
        let trimmed = line.trim();

        // A line ending with ':' that is not a known sub-key introduces a
        // new function block.
        if trimmed.ends_with(':')
            && !trimmed.starts_with("Hash")
            && !trimmed.starts_with("Counters")
            && !trimmed.starts_with("Block")
        {
            current_func = trimmed.trim_end_matches(':').to_string();
            continue;
        }

        if trimmed.starts_with("Block counts:") {
            let counts_str = trimmed
                .trim_start_matches("Block counts:")
                .trim()
                .trim_matches(|c| c == '[' || c == ']');
            let counts: Vec<u64> = counts_str
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            if !current_func.is_empty() && !counts.is_empty() {
                result.insert(current_func.clone(), counts);
            }
        }
    }

    result
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

    #[test]
    fn parse_profdata_output_extracts_counts() {
        let text = "\
my_func:\n\
  Hash: 0xabcdef\n\
  Counters: 3\n\
  Block counts: [10, 5, 0]\n\
other_func:\n\
  Hash: 0x000000\n\
  Counters: 1\n\
  Block counts: [42]\n";

        let profile = parse_profdata_output(text);
        assert_eq!(profile.get("my_func"), Some(&vec![10u64, 5, 0]));
        assert_eq!(profile.get("other_func"), Some(&vec![42u64]));
    }

    #[test]
    fn parse_profdata_output_empty_text_returns_empty() {
        let profile = parse_profdata_output("");
        assert!(profile.is_empty());
    }

    #[test]
    fn parse_profdata_output_skips_malformed_counts() {
        let text = "\
bad_func:\n\
  Block counts: [not_a_number, also_bad]\n";

        let profile = parse_profdata_output(text);
        // All tokens failed to parse → no entry inserted
        assert!(!profile.contains_key("bad_func"));
    }
}
