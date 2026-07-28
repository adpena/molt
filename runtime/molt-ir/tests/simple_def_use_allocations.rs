use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use molt_ir::ir::OpIR;
use molt_ir::tir::simple_def_use::{
    visit_simple_ir_defined_names, visit_simple_ir_reads, visit_simple_ir_result_names,
};

struct CountingAllocator;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

#[test]
fn simple_ir_def_use_visitors_are_all_heap_allocation_free() {
    let op = OpIR {
        kind: "unpack_sequence".to_string(),
        args: Some(vec![
            "sequence".to_string(),
            "first".to_string(),
            "second".to_string(),
        ]),
        ..OpIR::default()
    };

    let mut checksum = 0;
    ALLOCATIONS.store(0, Ordering::Relaxed);
    for _ in 0..10_000 {
        visit_simple_ir_reads(&op, |read| checksum += read.name.len());
        visit_simple_ir_result_names(&op, |name| checksum += name.len());
        visit_simple_ir_defined_names(&op, |name| checksum += name.len());
    }
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);

    assert_eq!(checksum, 300_000);
    assert_eq!(allocations, 0, "visitor traversal allocated on the heap");
}
