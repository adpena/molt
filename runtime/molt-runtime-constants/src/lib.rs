//! Runtime constants, counters, and wasm table-base authority.

mod constants;

pub use constants::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_gc_edges_and_raw_words_partition_the_compiler_control_region() {
        // The compiler reserves the region; the runtime's GC visits only its
        // owning edges. A missing, overlapping or out-of-region slot would
        // leak a send/throw/delegate owner or interpret raw state as a pointer.
        // Frame locations belong to the activation sidecar, not this region.
        let bytes = usize::try_from(molt_codegen_abi::GENERATOR_CONTROL_BYTES).unwrap();
        let word = std::mem::size_of::<u64>();
        assert_eq!(bytes % word, 0);
        let mut covered = vec![false; bytes / word];
        for offset in GEN_EDGE_OFFSETS
            .into_iter()
            .chain(GEN_RESERVED_OFFSETS)
            .chain([GEN_STATE_OFFSET])
        {
            assert_eq!(offset % word, 0, "unaligned control word at {offset}");
            let slot = covered.get_mut(offset / word).expect("control-region word");
            assert!(!*slot, "control word has two owners at {offset}");
            *slot = true;
        }
        assert!(
            covered.into_iter().all(|slot| slot),
            "unclassified control word"
        );
    }
}
