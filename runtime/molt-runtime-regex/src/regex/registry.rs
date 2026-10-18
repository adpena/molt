use super::*;
use std::collections::HashMap;
use std::sync::Arc;

/// Immutable compiled pattern payload stored by the runtime's green native
/// handle authority. It owns Rust data only and never a Molt object edge.
pub struct PatternHandle {
    pub(super) compiled: CompiledPattern,
}

impl PatternHandle {
    pub(super) fn new(compiled: CompiledPattern) -> Self {
        Self { compiled }
    }
}

/// Mutable scanner cursor. The handle payload contains only Rust state; the
/// Python iterator separately owns the subject and any memoryview lease.
pub struct ScannerHandle {
    pub(super) pattern: Arc<PatternHandle>,
    pub(super) pos: usize,
    pub(super) end: usize,
    pub(super) must_advance: bool,
    pub(super) text_chars: Option<Vec<char>>,
    pub(super) finished: bool,
}

impl ScannerHandle {
    pub(super) fn new(
        pattern: Arc<PatternHandle>,
        pos: usize,
        end: usize,
        text_chars: Option<Vec<char>>,
    ) -> Self {
        Self {
            pattern,
            pos,
            end,
            must_advance: false,
            text_chars,
            finished: false,
        }
    }
}
