use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};

use molt_ir::{FunctionIR, OpIR};

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

fn value_body(extra_ops: usize) -> FunctionIR {
    let mut ops = Vec::with_capacity(extra_ops + 1);
    ops.extend((0..extra_ops).map(|_| OpIR {
        kind: "nop".to_string(),
        ..OpIR::default()
    }));
    ops.push(OpIR {
        kind: "ret".to_string(),
        args: Some(vec!["value".to_string()]),
        ..OpIR::default()
    });
    FunctionIR {
        name: "value_body".to_string(),
        params: vec!["value".to_string()],
        ops,
        param_types: Some(vec!["i64".to_string()]),
        source_file: Some("allocation_probe.py".to_string()),
        ..FunctionIR::default()
    }
}

fn declaration_allocation_count(function: &FunctionIR) -> usize {
    ALLOCATIONS.store(0, Ordering::Relaxed);
    let declaration = black_box(
        function
            .extern_declaration()
            .expect("canonical declaration"),
    );
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    black_box(declaration);
    allocations
}

#[test]
fn extern_declaration_projection_has_body_independent_allocations_and_zero_cost_validation() {
    let small = value_body(0);
    let large = value_body(10_000);

    // Initialize the shared canonical signature template outside the measured
    // region so the measurement represents steady-state batch planning.
    black_box(small.extern_declaration().expect("warm signature template"));

    let small_allocations = declaration_allocation_count(&small);
    let large_allocations = declaration_allocation_count(&large);
    assert_eq!(large_allocations, small_allocations);

    let declaration = value_body(0)
        .extern_declaration()
        .expect("canonical declaration");
    black_box(declaration.extern_signature().expect("warm validation"));

    ALLOCATIONS.store(0, Ordering::Relaxed);
    for _ in 0..10_000 {
        black_box(declaration.extern_signature().expect("validate signature"));
    }
    assert_eq!(ALLOCATIONS.load(Ordering::Relaxed), 0);
}
