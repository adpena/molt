//! Dynamic census of the runtime entries compiled code calls (HF-198).
//!
//! Every generated attached-export wrapper calls [`hit`] with its dense census
//! index (`tools/wasm_abi_gen/attached_entries.py` assigns them, in one order
//! across every runtime crate). Without the `call-census` feature `hit` has an
//! empty body and inlines to nothing, so a shipped runtime pays nothing. With
//! it, each call bumps one relaxed atomic counter, and at process exit the
//! runtime writes every nonzero counter by entry name to the file named by
//! `MOLT_RUNTIME_CALL_CENSUS_FILE` ([`write_report`]).
//!
//! The count is exact for calls through the attached wrappers, which is every
//! runtime entry compiled code reaches except the eleven boundary entries
//! that run once at startup.

#[cfg(feature = "call-census")]
use crate::call_census_names_generated::{CALL_CENSUS_NAMES, CALL_CENSUS_SLOTS};
#[cfg(feature = "call-census")]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(feature = "call-census")]
static COUNTS: [AtomicU64; CALL_CENSUS_SLOTS] =
    [const { AtomicU64::new(0) }; CALL_CENSUS_SLOTS];

/// Count one call of the attached entry with census index `index`.
#[inline(always)]
pub fn hit(index: usize) {
    #[cfg(feature = "call-census")]
    COUNTS[index].fetch_add(1, Ordering::Relaxed);
    #[cfg(not(feature = "call-census"))]
    let _ = index;
}

/// Every entry called at least once, with its count, most-called first.
#[cfg(feature = "call-census")]
pub fn counts() -> Vec<(&'static str, u64)> {
    let mut rows: Vec<(&'static str, u64)> = COUNTS
        .iter()
        .zip(CALL_CENSUS_NAMES.iter())
        .filter_map(|(count, name)| {
            let calls = count.load(Ordering::Relaxed);
            (calls != 0).then_some((*name, calls))
        })
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    rows
}

/// Write `<calls> <entry>` lines to `MOLT_RUNTIME_CALL_CENSUS_FILE` when it is
/// set. A census build without the variable writes nothing.
#[cfg(feature = "call-census")]
pub fn write_report() {
    use std::io::Write;
    let Some(path) = std::env::var_os("MOLT_RUNTIME_CALL_CENSUS_FILE") else {
        return;
    };
    let Ok(mut file) = std::fs::File::create(&path) else {
        return;
    };
    for (name, calls) in counts() {
        let _ = writeln!(file, "{calls} {name}");
    }
}

#[cfg(all(test, feature = "call-census"))]
mod tests {
    use super::*;

    #[test]
    fn a_hit_is_counted_under_its_entry_name() {
        let index = CALL_CENSUS_NAMES.len() - 1;
        let before = COUNTS[index].load(Ordering::Relaxed);
        hit(index);
        hit(index);
        assert_eq!(COUNTS[index].load(Ordering::Relaxed), before + 2);
        let name = CALL_CENSUS_NAMES[index];
        assert!(counts().iter().any(|&(n, c)| n == name && c >= 2));
    }
}
