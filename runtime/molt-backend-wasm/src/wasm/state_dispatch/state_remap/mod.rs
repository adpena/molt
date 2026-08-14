mod br_table;
mod maps;
mod sparse;

const STATE_REMAP_TABLE_MAX_ENTRIES: usize = 4096;
const STATE_REMAP_TABLE_MAX_SPARSITY: usize = 8;

pub(super) use maps::{
    build_dense_state_remap_table, build_sparse_state_remap_entries, build_state_resume_maps,
    stateful_entry_prologue_end,
};
pub(super) use sparse::emit_sparse_state_remap_lookup;
