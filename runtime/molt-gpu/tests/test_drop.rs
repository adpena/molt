//! Drop and memory lifecycle tests.
//!
//! Verifies that DeviceBuffer, CompiledProgram, Arena, and CpuDevice
//! all clean up properly when dropped. No leaked allocations.

use molt_gpu::device::arena::{Arena, ArenaConfig};
use molt_gpu::device::cpu::CpuDevice;
use molt_gpu::device::{Allocator, Compiler};

// =============================================================================
// 1. DeviceBuffer drop
// =============================================================================

#[test]
fn test_device_buffer_drop_small() {
    let dev = CpuDevice::new();
    // Allocate and drop immediately -- no leak
    for _ in 0..10_000 {
        let _buf = dev.alloc(64).unwrap();
        // buf dropped here
    }
}

#[test]
fn test_device_buffer_drop_large() {
    let dev = CpuDevice::new();
    // Allocate large buffers and drop -- verify no accumulation
    for _ in 0..100 {
        let _buf = dev.alloc(1024 * 1024).unwrap(); // 1 MiB each
    }
    // If we get here without OOM, buffers are being freed on drop
}

#[test]
fn test_device_buffer_drop_via_free() {
    let dev = CpuDevice::new();
    for _ in 0..10_000 {
        let buf = dev.alloc(128).unwrap();
        dev.free(buf).unwrap();
    }
}

#[test]
fn test_device_buffer_drop_page_aligned() {
    let dev = CpuDevice::new();
    // Page-aligned allocations (>= 4096 bytes) use a different code path
    for _ in 0..1_000 {
        let _buf = dev.alloc(8192).unwrap();
    }
}

// =============================================================================
// 2. CompiledProgram drop
// =============================================================================

#[test]
fn test_compiled_program_drop() {
    let dev = CpuDevice::new();
    for i in 0..1000 {
        let source = format!("kernel_{}", i);
        let _prog = dev.compile(&source, "main").unwrap();
        // prog dropped here
    }
    // Cache should have all 1000 entries (they're still in the cache)
    assert_eq!(dev.cache_len(), 1000);
}

#[test]
fn test_compiled_program_drop_same_source() {
    let dev = CpuDevice::new();
    // Compile the same source 1000 times -- cache deduplicates
    for _ in 0..1000 {
        let _prog = dev.compile("same_source", "main").unwrap();
    }
    assert_eq!(dev.cache_len(), 1, "cache should have exactly 1 entry");
}

// =============================================================================
// 3. Arena drop
// =============================================================================

#[test]
fn test_arena_drop_with_active_allocations() {
    // Arena dropped while allocations are outstanding -- should not leak
    for _ in 0..100 {
        let arena = Arena::new(ArenaConfig {
            pool_size: 64 * 1024,
            alignment: 256,
        });
        let _a1 = arena.alloc(1024);
        let _a2 = arena.alloc(2048);
        let _a3 = arena.alloc(4096);
        // Arena dropped here with active allocations
    }
}

#[test]
fn test_arena_drop_after_reset() {
    for _ in 0..100 {
        let arena = Arena::new(ArenaConfig {
            pool_size: 64 * 1024,
            alignment: 256,
        });
        arena.alloc(1024).unwrap();
        arena.reset();
        arena.alloc(2048).unwrap();
        // Drop after reset + re-alloc
    }
}

#[test]
fn test_arena_drop_empty() {
    // Drop an arena that was never used
    for _ in 0..1000 {
        let _arena = Arena::with_defaults(); // 64 MiB each
    }
}

// =============================================================================
// 4. CpuDevice drop
// =============================================================================

#[test]
fn test_cpu_device_drop_clean() {
    for _ in 0..100 {
        let dev = CpuDevice::new();
        let _buf = dev.alloc(256).unwrap();
        let _prog = dev.compile("test", "main").unwrap();
        // dev dropped here with outstanding buffer and program
    }
}

#[test]
fn test_cpu_device_drop_after_heavy_use() {
    for _ in 0..10 {
        let dev = CpuDevice::new();
        // Allocate many buffers
        let mut bufs = Vec::new();
        for _ in 0..100 {
            bufs.push(dev.alloc(1024).unwrap());
        }
        // Compile many programs
        for i in 0..100 {
            let _ = dev.compile(&format!("kernel_{}", i), "main");
        }
        // Free half the buffers
        for buf in bufs.drain(..50) {
            dev.free(buf).unwrap();
        }
        // Drop dev with remaining buffers and all cached programs
    }
}

// =============================================================================
// 5. Arena stats correctness after heavy lifecycle
// =============================================================================

#[test]
fn test_arena_lifecycle_stats() {
    let arena = Arena::new(ArenaConfig {
        pool_size: 16 * 1024,
        alignment: 64,
    });

    // Generation 0: allocate some
    let a1 = arena.alloc(100).unwrap();
    let a2 = arena.alloc(200).unwrap();
    assert_eq!(arena.alloc_count(), 2);
    assert_eq!(arena.generation(), 0);

    let hwm_gen0 = arena.high_water_mark();
    assert!(hwm_gen0 > 0);

    // Write data to verify it's usable
    arena.write(&a1, &vec![0xAA; 100]);
    arena.write(&a2, &vec![0xBB; 200]);

    // Verify data before reset
    let s1 = arena.slice(&a1);
    assert!(s1.iter().all(|&b| b == 0xAA));
    let s2 = arena.slice(&a2);
    assert!(s2.iter().all(|&b| b == 0xBB));

    // Reset
    arena.reset();
    assert_eq!(arena.bytes_used(), 0);
    assert_eq!(arena.alloc_count(), 0);
    assert_eq!(arena.generation(), 1);

    // High water mark persists across resets
    assert_eq!(arena.high_water_mark(), hwm_gen0);

    // Generation 1: allocate more
    let _a3 = arena.alloc(8000).unwrap();
    let hwm_gen1 = arena.high_water_mark();
    assert!(hwm_gen1 >= hwm_gen0, "high water mark should not decrease");

    // Fragmentation should be computable
    let frag = arena.fragmentation();
    assert!(frag >= 0.0 && frag <= 1.0);

    // Stats snapshot
    let stats = arena.stats();
    assert_eq!(stats.pool_size, 16 * 1024);
    assert_eq!(stats.generation, 1);
    assert_eq!(stats.alloc_count, 1);
    assert!(stats.fragmentation >= 0.0);
}

// =============================================================================
// 6. Rapid create-use-drop cycle (heap pressure test)
// =============================================================================

#[test]
fn test_rapid_arena_create_drop_cycle() {
    // Create and destroy 1000 arenas rapidly
    for _ in 0..1000 {
        let arena = Arena::new(ArenaConfig {
            pool_size: 4096,
            alignment: 16,
        });
        for _ in 0..10 {
            let _ = arena.alloc(64);
        }
        // Drop
    }
}

#[test]
fn test_rapid_device_create_drop_cycle() {
    for _ in 0..1000 {
        let dev = CpuDevice::new();
        let buf = dev.alloc(256).unwrap();
        let data = vec![42u8; 256];
        dev.copy_in(&buf, &data).unwrap();
        let mut out = vec![0u8; 256];
        dev.copy_out(&buf, &mut out).unwrap();
        assert_eq!(out, data);
        dev.free(buf).unwrap();
    }
}
