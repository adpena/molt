#[derive(Clone, Copy)]
pub(crate) struct FrameEntry {
    pub(crate) code_bits: u64,
    pub(crate) line: i64,
    /// 0-based column offset for traceback caret annotations.
    /// -1 means "not available" (fall back to inference).
    pub(crate) col_offset: i64,
    /// 0-based end column offset for traceback caret annotations.
    /// -1 means "not available".
    pub(crate) end_col_offset: i64,
    /// Optional dict snapshot for `locals()` / `frame.f_locals`.
    ///
    /// This is set by compiler-emitted ops (`frame_locals_set`) and is owned by
    /// the frame stack entry (we INCREF on set and DECREF on pop/replacement).
    pub(crate) locals_bits: u64,
}
