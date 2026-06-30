use super::WasmBackend;
use crate::wasm_data::DataSegmentRef;
use wasm_encoder::Function;

impl WasmBackend {
    pub(super) fn add_data_segment(&mut self, reloc_enabled: bool, bytes: &[u8]) -> DataSegmentRef {
        self.data_segments.add_segment(reloc_enabled, bytes)
    }

    /// Like [`add_data_segment`] but skips the dedup cache.  Use this for
    /// segments that are **written to at runtime** (e.g. the call-func spill
    /// buffer) — caching them would allow a read-only segment with identical
    /// content to alias the mutable region, corrupting data when the spill
    /// buffer is written.
    pub(super) fn add_data_segment_mutable(
        &mut self,
        reloc_enabled: bool,
        bytes: &[u8],
    ) -> DataSegmentRef {
        self.data_segments.add_mutable_segment(reloc_enabled, bytes)
    }

    pub(super) fn emit_data_ptr(
        &mut self,
        reloc_enabled: bool,
        func_index: u32,
        func: &mut Function,
        data: DataSegmentRef,
    ) {
        let defined_func_index = func_index.checked_sub(self.func_import_count).expect(
            "data pointer relocation can only be recorded for defined WASM function bodies",
        );
        self.data_segments
            .emit_ptr(reloc_enabled, defined_func_index, func, data);
    }

    /// Like [`emit_data_ptr`] but pushes an **i32** value (no i64 extension).
    /// Useful when the address is consumed by an instruction that expects i32,
    /// e.g. `string_from_bytes`'s `out` parameter or `I64Load`'s address.
    pub(super) fn emit_data_ptr_i32(
        &mut self,
        reloc_enabled: bool,
        func_index: u32,
        func: &mut Function,
        data: DataSegmentRef,
    ) {
        let defined_func_index = func_index.checked_sub(self.func_import_count).expect(
            "data pointer relocation can only be recorded for defined WASM function bodies",
        );
        self.data_segments
            .emit_ptr_i32(reloc_enabled, defined_func_index, func, data);
    }
}
