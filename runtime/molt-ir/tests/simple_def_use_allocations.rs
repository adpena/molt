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

    // Keep all process-global allocator observations in this one test case.
    ssa_dead_name_chain_does_not_allocate_full_exit_environments();
}

fn ssa_dead_name_chain_does_not_allocate_full_exit_environments() {
    use molt_ir::tir::blocks::Terminator;
    use molt_ir::tir::cfg::CFG;
    use molt_ir::tir::ops::OpCode;
    use molt_ir::tir::ssa::convert_to_ssa;

    let measure = |count: usize| {
        let mut ops = Vec::new();
        for index in 0..count {
            ops.push(OpIR {
                kind: "label".into(),
                value: Some(index as i64),
                ..OpIR::default()
            });
            ops.push(OpIR {
                kind: "const".into(),
                value: Some(index as i64),
                out: Some(format!("dead_name_{index}")),
                ..OpIR::default()
            });
        }
        ops.push(OpIR {
            kind: "ret".into(),
            args: Some(vec![format!("dead_name_{}", count - 1)]),
            ..OpIR::default()
        });
        let cfg = CFG::build(&ops);
        ALLOCATIONS.store(0, Ordering::Relaxed);
        let output = convert_to_ssa(&cfg, &ops);
        let allocations = ALLOCATIONS.load(Ordering::Relaxed);
        let constants: Vec<_> = output
            .blocks
            .iter()
            .flat_map(|block| &block.ops)
            .filter(|op| op.opcode == OpCode::ConstInt)
            .collect();
        assert_eq!(
            constants.len(),
            count,
            "allocation control must retain the whole fixture"
        );
        let last = constants.last().unwrap().results[0];
        assert!(output.blocks.iter().any(|block| {
            matches!(&block.terminator, Terminator::Return { values } if values == &[last])
        }));
        allocations
    };
    const DEFINITIONS: usize = 512;
    const MAX_ALLOCATIONS_PER_DEFINITION: usize = 128;
    let allocations = measure(DEFINITIONS);
    // None of the earlier names is live at a later block boundary. Allow a
    // generous fixed budget for the public SSA phases, including validation,
    // liveness and output construction. Full exit snapshots alone would clone
    // 512 * 513 / 2 = 131,328 names, exceeding this 65,536-allocation budget.
    // This bounds one fixture, not elapsed time or every SSA graph's complexity.
    assert!(
        allocations <= MAX_ALLOCATIONS_PER_DEFINITION * DEFINITIONS,
        "dead-name SSA retained full environments: {allocations} allocations"
    );
}
