use super::{Allocation, Block, InterBlockSources, PReg, RegClass, SpillSlot, VRegIndex};

#[test]
fn inter_block_sources_broad_then_sparse_generations() {
    let mut sources = InterBlockSources::new(64);
    let broad = VRegIndex::new(0);
    let register = Allocation::reg(PReg::new(0, RegClass::Int));
    for index in 0..64 {
        sources.insert(broad, Block::new(index), register);
    }
    for index in 0..64 {
        assert_eq!(sources.get(broad, Block::new(index)), register);
    }

    // Earlier registers must not defeat a new generation's stack allocation.
    // Reuse the same sparse block repeatedly, then touch a previously broad one.
    for generation in 1..32 {
        let vreg = VRegIndex::new(generation);
        let stack = Allocation::stack(SpillSlot::new(generation));
        sources.insert(vreg, Block::new(7), stack);
        assert_eq!(sources.get(vreg, Block::new(7)), stack);
    }
    let last = VRegIndex::new(32);
    let stack = Allocation::stack(SpillSlot::new(32));
    sources.insert(last, Block::new(63), stack);
    assert_eq!(sources.get(last, Block::new(63)), stack);
}

#[test]
#[should_panic(expected = "missing inter-block source for current vreg")]
fn inter_block_sources_reject_stale_generation() {
    let mut sources = InterBlockSources::new(2);
    sources.insert(
        VRegIndex::new(0),
        Block::new(0),
        Allocation::reg(PReg::new(0, RegClass::Int)),
    );
    sources.insert(
        VRegIndex::new(1),
        Block::new(1),
        Allocation::stack(SpillSlot::new(0)),
    );
    // A source exists at this block, but belongs to a different value.
    sources.get(VRegIndex::new(1), Block::new(0));
}

#[test]
#[should_panic(expected = "missing inter-block source")]
fn inter_block_sources_reject_unpopulated_block() {
    InterBlockSources::new(1).get(VRegIndex::new(0), Block::new(0));
}

#[test]
fn inter_block_sources_preserve_allocation_preference_in_all_classes() {
    let mut sources = InterBlockSources::new(1);
    let block = Block::new(0);
    let stack0 = Allocation::stack(SpillSlot::new(0));
    let stack1 = Allocation::stack(SpillSlot::new(1));
    for (generation, class) in [RegClass::Int, RegClass::Float, RegClass::Vector]
        .iter()
        .copied()
        .enumerate()
    {
        let vreg = VRegIndex::new(generation);
        let first_register = Allocation::reg(PReg::new(0, class));
        let later_register = Allocation::reg(PReg::new(1, class));
        sources.insert(vreg, block, stack0);
        assert_eq!(sources.get(vreg, block), stack0);
        sources.insert(vreg, block, stack1);
        assert_eq!(sources.get(vreg, block), stack1);
        sources.insert(vreg, block, first_register);
        assert_eq!(sources.get(vreg, block), first_register);
        sources.insert(vreg, block, later_register);
        assert_eq!(sources.get(vreg, block), first_register);
        sources.insert(vreg, block, stack0);
        assert_eq!(sources.get(vreg, block), first_register);
    }
}

#[test]
fn inter_block_sources_without_sources_keep_no_backing_storage() {
    let sources = InterBlockSources::new(10_000);
    assert_eq!(sources.slots.capacity(), 0);
}
