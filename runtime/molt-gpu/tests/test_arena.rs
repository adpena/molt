//! Tests for the arena allocator.

use molt_gpu::device::arena::{Arena, ArenaConfig};

#[test]
fn test_arena_basic_alloc() {
    let arena = Arena::new(ArenaConfig {
        pool_size: 4096,
        alignment: 16,
    });

    let a1 = arena.alloc(64).expect("alloc 64 bytes");
    assert_eq!(a1.offset, 0);
    assert_eq!(a1.size, 64);

    let a2 = arena.alloc(128).expect("alloc 128 bytes");
    // Should be aligned to 16 bytes after a1
    assert!(a2.offset >= 64);
    assert_eq!(a2.offset % 16, 0);
    assert_eq!(a2.size, 128);
}

#[test]
fn test_arena_alignment() {
    let arena = Arena::new(ArenaConfig {
        pool_size: 4096,
        alignment: 256,
    });

    // First alloc: offset 0 (already aligned)
    let a1 = arena.alloc(1).expect("alloc 1 byte");
    assert_eq!(a1.offset, 0);

    // Second alloc: must be aligned to 256
    let a2 = arena.alloc(1).expect("alloc 1 byte");
    assert_eq!(a2.offset, 256);
    assert_eq!(a2.offset % 256, 0);
}

#[test]
fn test_arena_full() {
    let arena = Arena::new(ArenaConfig {
        pool_size: 256,
        alignment: 1,
    });

    let a1 = arena.alloc(200).expect("alloc 200");
    assert_eq!(a1.size, 200);

    // This should fit
    let a2 = arena.alloc(56).expect("alloc 56");
    assert_eq!(a2.size, 56);

    // This should fail — arena is full
    let a3 = arena.alloc(1);
    assert!(a3.is_none(), "arena should be full");
}

#[test]
fn test_arena_reset() {
    let arena = Arena::new(ArenaConfig {
        pool_size: 1024,
        alignment: 1,
    });

    arena.alloc(512).expect("alloc 512");
    assert_eq!(arena.bytes_used(), 512);
    assert_eq!(arena.alloc_count(), 1);
    assert_eq!(arena.generation(), 0);

    arena.reset();
    assert_eq!(arena.bytes_used(), 0);
    assert_eq!(arena.alloc_count(), 0);
    assert_eq!(arena.generation(), 1);
    assert_eq!(arena.bytes_remaining(), 1024);

    // After reset, we can allocate again from the beginning
    let a = arena.alloc(512).expect("alloc 512 after reset");
    assert_eq!(a.offset, 0);
}

#[test]
fn test_arena_write_and_read() {
    let arena = Arena::new(ArenaConfig {
        pool_size: 4096,
        alignment: 1,
    });

    let alloc = arena.alloc(8).expect("alloc 8");
    let data = [1u8, 2, 3, 4, 5, 6, 7, 8];
    arena.write(&alloc, &data);

    let readback = arena.slice(&alloc);
    assert_eq!(readback, data);
}

#[test]
fn test_arena_zero_size_alloc() {
    let arena = Arena::with_defaults();
    let a = arena.alloc(0).expect("zero-size alloc should succeed");
    assert_eq!(a.size, 0);
    // Zero-size alloc should not consume space
    assert_eq!(arena.bytes_used(), 0);
}

#[test]
fn test_arena_metrics() {
    let arena = Arena::new(ArenaConfig {
        pool_size: 4096,
        alignment: 1,
    });

    assert_eq!(arena.capacity(), 4096);
    assert_eq!(arena.bytes_used(), 0);
    assert_eq!(arena.bytes_remaining(), 4096);
    assert_eq!(arena.alloc_count(), 0);
    assert_eq!(arena.generation(), 0);

    arena.alloc(100).unwrap();
    arena.alloc(200).unwrap();

    assert_eq!(arena.bytes_used(), 300);
    assert_eq!(arena.bytes_remaining(), 3796);
    assert_eq!(arena.alloc_count(), 2);
}

#[test]
fn test_arena_multiple_resets() {
    let arena = Arena::new(ArenaConfig {
        pool_size: 1024,
        alignment: 1,
    });

    for gen in 0..10u64 {
        assert_eq!(arena.generation(), gen);
        arena.alloc(100).expect("alloc should work after reset");
        arena.reset();
    }
    assert_eq!(arena.generation(), 10);
    assert_eq!(arena.bytes_used(), 0);
}

#[test]
#[should_panic(expected = "alignment must be power of 2")]
fn test_arena_invalid_alignment() {
    Arena::new(ArenaConfig {
        pool_size: 1024,
        alignment: 3, // not power of 2
    });
}

#[test]
fn test_arena_default_config() {
    let arena = Arena::with_defaults();
    assert_eq!(arena.capacity(), 64 * 1024 * 1024); // 64 MiB
    // Should be able to allocate
    let a = arena.alloc(1024).expect("alloc 1024 from default arena");
    assert_eq!(a.offset, 0);
    assert_eq!(a.size, 1024);
}
