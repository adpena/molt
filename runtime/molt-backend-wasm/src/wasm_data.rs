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
        let byte_len: u32 = bytes
            .len()
            .try_into()
            .expect("data segment too large for WASM (>4 GiB)");
        let align_mask = data_segment_align_mask(byte_len);
        let offset = align_data_offset(self.offset, align_mask)
            .expect("WASM data segment offset overflow (>4 GiB total data)");
        let index = self.segments.len() as u32;
        let const_expr = if reloc_enabled {
            const_expr_i32_const_padded(offset as i32)
        } else {
            ConstExpr::i32_const(offset as i32)
        };
        self.section.active(0, &const_expr, bytes.iter().copied());
        // Checked arithmetic detects overflow instead of silently wrapping and
        // corrupting shared linear-memory layout.
        self.offset = offset
            .checked_add(byte_len)
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

fn data_segment_align_mask(byte_len: u32) -> u32 {
    if byte_len <= 4 { 3 } else { 7 }
}

fn align_data_offset(offset: u32, align_mask: u32) -> Option<u32> {
    offset
        .checked_add(align_mask)
        .map(|aligned| aligned & !align_mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns_current_segment_before_placement() {
        let mut segments = WasmDataSegments::new(0);

        let first = segments.add_segment(false, &[1, 2, 3, 4]);
        let second = segments.add_segment(false, &[5, 6, 7, 8, 9, 10, 11, 12]);

        assert_eq!(first.offset, 0);
        assert_eq!(
            second.offset, 8,
            "the >4-byte segment must align its own start, not inherit the previous 4-byte segment alignment"
        );
        assert_eq!(segments.offset(), 16);
    }

    #[test]
    fn mutable_segments_share_current_segment_alignment() {
        let mut segments = WasmDataSegments::new(2);

        let data_ref = segments.add_mutable_segment(false, &[0; 8]);

        assert_eq!(
            data_ref.offset, 8,
            "mutable data segments must use the same start-alignment authority as readonly segments"
        );
        assert_eq!(segments.offset(), 16);
    }
}
