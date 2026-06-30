use crate::wasm_binary::{const_expr_i32_const_padded, emit_i32_const};
use std::collections::HashMap;
use wasm_encoder::{ConstExpr, DataSection, Function, Instruction};

#[derive(Clone, Copy)]
pub(crate) struct DataSegmentInfo {
    pub(crate) size: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct DataRelocSite {
    pub(crate) defined_func_index: u32,
    pub(crate) offset_in_func: u32,
    pub(crate) segment_index: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct DataSegmentRef {
    pub(crate) offset: u32,
    pub(crate) index: u32,
}

pub(crate) struct WasmDataSegments {
    section: DataSection,
    offset: u32,
    segments: Vec<DataSegmentInfo>,
    relocs: Vec<DataRelocSite>,
    // Dedup cache: maps byte content to existing segment ref.
    // HashMap is fine here: this map is only used for point lookups, never iterated.
    cache: HashMap<Vec<u8>, DataSegmentRef>,
}

impl WasmDataSegments {
    pub(crate) fn new(data_base: u32) -> Self {
        Self {
            section: DataSection::new(),
            offset: data_base,
            segments: Vec::new(),
            relocs: Vec::new(),
            cache: HashMap::new(),
        }
    }

    pub(crate) fn offset(&self) -> u32 {
        self.offset
    }

    pub(crate) fn section(&self) -> &DataSection {
        &self.section
    }

    pub(crate) fn segments(&self) -> &[DataSegmentInfo] {
        &self.segments
    }

    pub(crate) fn relocs(&self) -> &[DataRelocSite] {
        &self.relocs
    }

    pub(crate) fn segment_count(&self) -> usize {
        self.segments.len()
    }

    pub(crate) fn total_data_bytes(&self) -> u32 {
        self.segments.iter().map(|segment| segment.size).sum()
    }

    pub(crate) fn dedup_entry_count(&self) -> usize {
        self.cache.len()
    }

    pub(crate) fn add_segment(&mut self, reloc_enabled: bool, bytes: &[u8]) -> DataSegmentRef {
        self.add_segment_inner(reloc_enabled, bytes, true)
    }

    /// Like [`add_segment`] but skips the dedup cache. Use this for segments
    /// that are written to at runtime; otherwise a read-only segment with
    /// identical content could alias mutable scratch state.
    pub(crate) fn add_mutable_segment(
        &mut self,
        reloc_enabled: bool,
        bytes: &[u8],
    ) -> DataSegmentRef {
        self.add_segment_inner(reloc_enabled, bytes, false)
    }

    fn add_segment_inner(
        &mut self,
        reloc_enabled: bool,
        bytes: &[u8],
        cacheable: bool,
    ) -> DataSegmentRef {
        // Skip empty data segments entirely; they waste a segment header for zero payload.
        if bytes.is_empty() {
            return DataSegmentRef {
                offset: self.offset,
                index: self.segments.len().saturating_sub(1) as u32,
            };
        }
        if cacheable && let Some(existing) = self.cache.get(bytes) {
            return *existing;
        }
        let offset = self.offset;
        let byte_len: u32 = bytes
            .len()
            .try_into()
            .expect("data segment too large for WASM (>4 GiB)");
        let index = self.segments.len() as u32;
        let const_expr = if reloc_enabled {
            const_expr_i32_const_padded(offset as i32)
        } else {
            ConstExpr::i32_const(offset as i32)
        };
        self.section.active(0, &const_expr, bytes.iter().copied());
        // Checked arithmetic detects overflow instead of silently wrapping and
        // corrupting shared linear-memory layout.
        let align_mask: u32 = if byte_len <= 4 { 3 } else { 7 };
        self.offset = offset
            .checked_add(byte_len)
            .and_then(|value| value.checked_add(align_mask))
            .map(|value| value & !align_mask)
            .expect("WASM data segment offset overflow (>4 GiB total data)");
        self.segments.push(DataSegmentInfo { size: byte_len });
        let data_ref = DataSegmentRef { offset, index };
        if cacheable {
            self.cache.insert(bytes.to_vec(), data_ref);
        }
        data_ref
    }

    pub(crate) fn emit_ptr(
        &mut self,
        reloc_enabled: bool,
        defined_func_index: u32,
        func: &mut Function,
        data: DataSegmentRef,
    ) {
        self.record_reloc(defined_func_index, func.byte_len() as u32 + 1, data);
        emit_i32_const(func, reloc_enabled, data.offset as i32);
        func.instruction(&Instruction::I64ExtendI32U);
    }

    /// Like [`emit_ptr`] but pushes an i32 value without an i64 extension.
    pub(crate) fn emit_ptr_i32(
        &mut self,
        reloc_enabled: bool,
        defined_func_index: u32,
        func: &mut Function,
        data: DataSegmentRef,
    ) {
        self.record_reloc(defined_func_index, func.byte_len() as u32 + 1, data);
        emit_i32_const(func, reloc_enabled, data.offset as i32);
    }

    fn record_reloc(&mut self, defined_func_index: u32, offset_in_func: u32, data: DataSegmentRef) {
        self.relocs.push(DataRelocSite {
            defined_func_index,
            offset_in_func,
            segment_index: data.index,
        });
    }
}
