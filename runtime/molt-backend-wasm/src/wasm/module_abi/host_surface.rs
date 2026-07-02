use std::collections::BTreeSet;

use wasm_encoder::{EntityType, ExportKind, MemoryType};

use super::runtime_surface::effective_manifest_intrinsic_names;
use crate::wasm::WasmBackend;
use crate::wasm_data::DataSegmentRef;

pub(super) struct WasmModuleHostSurface {
    pub(super) manifest_segment: DataSegmentRef,
    pub(super) manifest_len: usize,
    pub(super) intrinsic_manifest_names: BTreeSet<String>,
    pub(super) call_func_spill_offset: u32,
    pub(super) class_def_spill_offset: u32,
    pub(super) const_str_scratch_segment: DataSegmentRef,
}

impl WasmBackend {
    pub(super) fn emit_linear_memory_surface(&mut self) {
        let page_size: u64 = 64 * 1024;
        let required_pages = (self.data_segments.offset() as u64).div_ceil(page_size);
        let floor_pages = std::env::var("MOLT_WASM_MIN_PAGES")
            .ok()
            .and_then(|val| val.parse::<u64>().ok())
            .unwrap_or(64);
        let minimum_pages = required_pages.max(floor_pages);
        let memory_ty = MemoryType {
            minimum: minimum_pages,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        };
        self.imports
            .import("env", "memory", EntityType::Memory(memory_ty));
        self.exports.export("molt_memory", ExportKind::Memory, 0);
    }

    pub(super) fn prepare_module_host_surface(
        &mut self,
        reloc_enabled: bool,
        manifest_intrinsic_names: BTreeSet<String>,
        max_call_arity: usize,
        max_class_def_words: usize,
    ) -> WasmModuleHostSurface {
        let manifest_intrinsic_names = effective_manifest_intrinsic_names(manifest_intrinsic_names);
        let manifest_bytes: Vec<u8> = {
            let mut buf = Vec::new();
            for (i, name) in manifest_intrinsic_names.iter().enumerate() {
                if i > 0 {
                    buf.push(0);
                }
                buf.extend_from_slice(name.as_bytes());
            }
            buf
        };
        let manifest_segment = self.add_data_segment(reloc_enabled, &manifest_bytes);
        let manifest_len = manifest_bytes.len();

        // The runtime copies call_func args before dispatching, so nested
        // WASM->runtime->WASM calls cannot observe stale data in this buffer.
        let spill_slots = max_call_arity.max(1);
        let spill_bytes = vec![0u8; spill_slots * 8];
        let spill_segment = self.add_data_segment_mutable(reloc_enabled, &spill_bytes);

        // The class_def helper snapshots the bases/attrs payload before nested calls.
        let class_def_words = max_class_def_words.max(2);
        let class_def_bytes = vec![0u8; class_def_words * 8];
        let class_def_segment = self.add_data_segment_mutable(reloc_enabled, &class_def_bytes);

        // A fixed scratch slot replaces per-const_str heap allocations and leaks.
        let const_str_scratch_bytes = vec![0u8; 8];
        let const_str_scratch_segment =
            self.add_data_segment_mutable(reloc_enabled, &const_str_scratch_bytes);

        WasmModuleHostSurface {
            manifest_segment,
            manifest_len,
            intrinsic_manifest_names: manifest_intrinsic_names,
            call_func_spill_offset: spill_segment.offset,
            class_def_spill_offset: class_def_segment.offset,
            const_str_scratch_segment,
        }
    }
}
