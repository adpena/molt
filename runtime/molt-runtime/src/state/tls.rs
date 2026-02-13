use std::cell::{Cell, RefCell};

use crate::PtrSlot;
use crate::arena::TempArena;
use crate::builtins::frames::FrameEntry;

pub(crate) const DEFAULT_RECURSION_LIMIT: usize = 1000;

thread_local! {
    pub(crate) static PARSE_ARENA: RefCell<TempArena> = RefCell::new(TempArena::new(8 * 1024));
    pub(crate) static CONTEXT_STACK: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    pub(crate) static FRAME_STACK: RefCell<Vec<FrameEntry>> = const { RefCell::new(Vec::new()) };
    pub(crate) static RECURSION_LIMIT: Cell<usize> = const { Cell::new(DEFAULT_RECURSION_LIMIT) };
    pub(crate) static RECURSION_DEPTH: Cell<usize> = const { Cell::new(0) };
    pub(crate) static GIL_DEPTH: Cell<usize> = const { Cell::new(0) };
    pub(crate) static REPR_STACK: RefCell<Vec<PtrSlot>> = const { RefCell::new(Vec::new()) };
    pub(crate) static REPR_DEPTH: Cell<usize> = const { Cell::new(0) };
    pub(crate) static TRACEBACK_SUPPRESS: Cell<usize> = const { Cell::new(0) };
}
