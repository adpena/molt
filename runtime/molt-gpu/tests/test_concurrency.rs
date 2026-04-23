//! Concurrency safety tests for the CPU device backend.
//!
//! Verifies that CpuDevice, Arena, and compile cache are safe under
//! concurrent access from multiple threads with independent workloads.

use std::sync::Arc;
use std::thread;

use molt_gpu::device::arena::{Arena, ArenaConfig};
use molt_gpu::device::cpu::interpret;
use molt_gpu::device::cpu::CpuDevice;
use molt_gpu::device::{Allocator, Compiler};
use molt_gpu::dtype::DType;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc};
use molt_gpu::shapetracker::ShapeTracker;

fn f32_to_bytes(vals: &[f32]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

// =============================================================================
// 1. Four threads running independent compute on CpuDevice
// =============================================================================

#[test]
fn test_concurrent_cpu_device_independent_compute() {
    let device = Arc::new(CpuDevice::new());
    let num_threads = 4;
    let n = 10_000;

    let handles: Vec<_> = (0..num_threads)
        .map(|thread_id| {
            let dev = Arc::clone(&device);
            thread::spawn(move || {
                // Each thread computes: output[i] = input[i] + thread_id
                let input: Vec<f32> = (0..n).map(|i| i as f32).collect();
                let addend = thread_id as f32;

                let kernel = FusedKernel {
                    ops: vec![FusedOp {
                        op: PrimitiveOp::Add,
                        srcs: vec![
                            FusedSrc::Buf(1),
                            FusedSrc::Const {
                                val: addend as f64,
                                dtype: DType::Float32,
                            },
                        ],
                        dst_dtype: DType::Float32,
                    }],
                    bufs: vec![
                        BufferBinding {
                            buf_id: 0,
                            st: ShapeTracker::contiguous(&[n]),
                            dtype: DType::Float32,
                            access: BufferAccess::Write,
                        },
                        BufferBinding {
                            buf_id: 1,
                            st: ShapeTracker::contiguous(&[n]),
                            dtype: DType::Float32,
                            access: BufferAccess::Read,
                        },
                    ],
                    grid: [n as u32, 1, 1],
                    local: [1, 1, 1],
                    spec: None,
                    vectorize_width: 1,
                };

                let mut bufs = vec![vec![0u8; n * 4], f32_to_bytes(&input)];
                interpret::execute_kernel(&kernel, &mut bufs);
                let result = bytes_to_f32(&bufs[0]);

                // Also test Allocator trait methods under concurrency
                let buf = dev.alloc(1024).expect("alloc should succeed");
                let data = vec![0xABu8; 1024];
                dev.copy_in(&buf, &data).expect("copy_in should succeed");
                let mut out = vec![0u8; 1024];
                dev.copy_out(&buf, &mut out)
                    .expect("copy_out should succeed");
                assert_eq!(out, data, "copy_out should match copy_in");
                dev.free(buf).expect("free should succeed");

                (thread_id, result)
            })
        })
        .collect();

    for handle in handles {
        let (thread_id, result) = handle.join().expect("thread should not panic");
        let addend = thread_id as f32;
        for (i, &v) in result.iter().enumerate() {
            let expected = i as f32 + addend;
            assert_eq!(v, expected, "thread {} element {} mismatch", thread_id, i);
        }
    }
}

// =============================================================================
// 2. Arena allocator under concurrent access
// =============================================================================

#[test]
fn test_arena_concurrent_alloc() {
    let arena = Arc::new(Arena::new(ArenaConfig {
        pool_size: 4 * 1024 * 1024, // 4 MiB
        alignment: 256,
    }));
    let num_threads = 4;
    let allocs_per_thread = 100;

    let handles: Vec<_> = (0..num_threads)
        .map(|_| {
            let arena = Arc::clone(&arena);
            thread::spawn(move || {
                let mut allocations = Vec::new();
                for i in 0..allocs_per_thread {
                    let size = (i + 1) * 64; // varying sizes
                    match arena.alloc(size) {
                        Some(alloc) => {
                            assert_eq!(alloc.size, size);
                            assert_eq!(alloc.offset % 256, 0, "allocation must be aligned");
                            allocations.push(alloc);
                        }
                        None => {
                            // Arena full -- acceptable under concurrent pressure
                            break;
                        }
                    }
                }
                allocations
            })
        })
        .collect();

    let mut all_allocations = Vec::new();
    for handle in handles {
        let allocs = handle.join().expect("thread should not panic");
        all_allocations.extend(allocs);
    }

    // Verify no overlapping allocations
    let mut sorted: Vec<_> = all_allocations.iter().filter(|a| a.size > 0).collect();
    sorted.sort_by_key(|a| a.offset);
    for i in 1..sorted.len() {
        let prev_end = sorted[i - 1].offset + sorted[i - 1].size;
        // Account for alignment padding
        assert!(
            sorted[i].offset >= prev_end,
            "overlapping allocations: prev ends at {}, next starts at {}",
            prev_end,
            sorted[i].offset
        );
    }
}

#[test]
fn test_arena_concurrent_alloc_and_reset() {
    // Stress: one thread resets while others allocate.
    // After reset, all allocations from other threads become invalid (by contract),
    // but the arena must not panic or corrupt state.
    let arena = Arc::new(Arena::new(ArenaConfig {
        pool_size: 1024 * 1024,
        alignment: 256,
    }));
    let num_threads = 4;

    let handles: Vec<_> = (0..num_threads)
        .map(|tid| {
            let arena = Arc::clone(&arena);
            thread::spawn(move || {
                for _ in 0..50 {
                    if tid == 0 {
                        // Thread 0 periodically resets
                        arena.reset();
                    } else {
                        // Other threads allocate
                        let _ = arena.alloc(512);
                    }
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("thread should not panic");
    }

    // Arena should be in a consistent state
    let stats = arena.stats();
    assert!(stats.pool_size > 0);
    assert!(
        stats.generation > 0,
        "at least one reset should have occurred"
    );
}

// =============================================================================
// 3. Compile cache under concurrent access
// =============================================================================

#[test]
fn test_compile_cache_concurrent() {
    let device = Arc::new(CpuDevice::new());
    let num_threads = 4;

    let handles: Vec<_> = (0..num_threads)
        .map(|tid| {
            let dev = Arc::clone(&device);
            thread::spawn(move || {
                // Each thread compiles the same source multiple times
                // and some unique sources
                for i in 0..50 {
                    let source = if i % 2 == 0 {
                        "shared_kernel_source".to_string() // shared across threads
                    } else {
                        format!("unique_kernel_thread_{}_iter_{}", tid, i)
                    };
                    let result = dev.compile(&source, "main");
                    assert!(result.is_ok(), "compile should succeed");
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("thread should not panic");
    }

    // Cache should have entries
    assert!(device.cache_len() > 0, "compile cache should have entries");
}

// =============================================================================
// 4. Arena write/read under concurrent access
// =============================================================================

#[test]
fn test_arena_concurrent_write_read() {
    let arena = Arc::new(Arena::new(ArenaConfig {
        pool_size: 2 * 1024 * 1024,
        alignment: 256,
    }));

    // Pre-allocate slots sequentially (allocations are NOT concurrent-safe for the same slot)
    let allocs: Vec<_> = (0..8)
        .map(|_| arena.alloc(1024).expect("alloc should succeed"))
        .collect();

    // Each thread owns its own allocation slot and writes/reads independently
    let handles: Vec<_> = allocs
        .into_iter()
        .enumerate()
        .map(|(tid, alloc)| {
            let arena = Arc::clone(&arena);
            thread::spawn(move || {
                let pattern = vec![(tid as u8).wrapping_mul(17); 1024];
                arena.write(&alloc, &pattern);
                let readback = arena.slice(&alloc);
                assert_eq!(readback, pattern, "thread {} readback mismatch", tid);
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("thread should not panic");
    }
}

// =============================================================================
// 5. Multiple threads running different kernel shapes concurrently
// =============================================================================

#[test]
fn test_concurrent_different_kernel_shapes() {
    let num_threads = 4;

    let handles: Vec<_> = (0..num_threads)
        .map(|tid| {
            thread::spawn(move || {
                // Each thread uses a different tensor size
                let n = (tid + 1) * 1000;
                let input: Vec<f32> = (0..n).map(|i| (i * (tid + 1)) as f32).collect();

                let kernel = FusedKernel {
                    ops: vec![FusedOp {
                        op: PrimitiveOp::Mul,
                        srcs: vec![
                            FusedSrc::Buf(1),
                            FusedSrc::Const {
                                val: 2.0,
                                dtype: DType::Float32,
                            },
                        ],
                        dst_dtype: DType::Float32,
                    }],
                    bufs: vec![
                        BufferBinding {
                            buf_id: 0,
                            st: ShapeTracker::contiguous(&[n]),
                            dtype: DType::Float32,
                            access: BufferAccess::Write,
                        },
                        BufferBinding {
                            buf_id: 1,
                            st: ShapeTracker::contiguous(&[n]),
                            dtype: DType::Float32,
                            access: BufferAccess::Read,
                        },
                    ],
                    grid: [n as u32, 1, 1],
                    local: [1, 1, 1],
                    spec: None,
                    vectorize_width: 1,
                };

                let mut bufs = vec![vec![0u8; n * 4], f32_to_bytes(&input)];
                interpret::execute_kernel(&kernel, &mut bufs);
                let result = bytes_to_f32(&bufs[0]);

                for (i, &v) in result.iter().enumerate() {
                    let expected = (i * (tid + 1)) as f32 * 2.0;
                    assert_eq!(v, expected, "thread {} element {} mismatch", tid, i);
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("thread should not panic");
    }
}
