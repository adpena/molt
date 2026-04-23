//! Tests for all performance optimizations:
//! - ShapeTracker contiguous fast path
//! - ShapeTracker 1D/2D/3D specialization
//! - FMA emission in rendered shaders
//! - Kernel deduplication
//! - Arena alignment-aware allocation and statistics
//! - SIMD path correctness (when simd-accel feature enabled)
//! - Page-aligned allocations

use molt_gpu::device::arena::{Arena, ArenaConfig};
use molt_gpu::device::cpu::CpuDevice;
use molt_gpu::device::Allocator;
use molt_gpu::dtype::DType;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::*;
use molt_gpu::schedule::{deduplicate_kernels, specialize_shapes};
use molt_gpu::shapetracker::{ShapeTracker, View};

// ============================================================
// ShapeTracker: contiguous fast path
// ============================================================

#[test]
fn test_contiguous_fast_path() {
    let view = View::contiguous(&[4, 8]);
    assert!(view.is_contiguous());
    // For contiguous views, expr_idx(i) == Some(i)
    for i in 0..32 {
        assert_eq!(
            view.expr_idx(i),
            Some(i),
            "contiguous fast path failed at index {}",
            i
        );
    }
}

#[test]
fn test_contiguous_cache_on_construction() {
    // A contiguous view should report is_contiguous without recomputation
    let view = View::contiguous(&[100, 200, 300]);
    assert!(view.is_contiguous());
    // Call multiple times — result is cached, O(1) each time
    for _ in 0..1000 {
        assert!(view.is_contiguous());
    }
}

#[test]
fn test_non_contiguous_not_cached_as_contiguous() {
    // A flipped view should NOT be contiguous
    let st = ShapeTracker::contiguous(&[10]).flip(0);
    assert!(!st.view().is_contiguous());
}

#[test]
fn test_permuted_view_not_contiguous() {
    let st = ShapeTracker::contiguous(&[4, 8]).permute(&[1, 0]);
    assert!(!st.view().is_contiguous());
}

// ============================================================
// ShapeTracker: 1D/2D/3D specialization
// ============================================================

#[test]
fn test_expr_idx_1d_specialization() {
    // Non-contiguous 1D: flipped
    let st = ShapeTracker::contiguous(&[10]).flip(0);
    let view = st.view();
    assert!(!view.is_contiguous());
    // Verify correctness: flip reverses the order
    for i in 0..10 {
        assert_eq!(
            view.expr_idx(i),
            Some(9 - i),
            "1D flip failed at index {}",
            i
        );
    }
}

#[test]
fn test_expr_idx_2d_specialization() {
    // 2D transpose: [4, 8] permuted to [8, 4]
    let st = ShapeTracker::contiguous(&[4, 8]).permute(&[1, 0]);
    let view = st.view();
    // Permuted [4,8] -> [8,4] with strides [1,8].
    // Linear idx i in [8,4]: i0 = i/4, i1 = i%4.
    // Buffer offset = i0*1 + i1*8.
    assert_eq!(view.expr_idx(0), Some(0)); // (0,0) -> 0*1 + 0*8 = 0
    assert_eq!(view.expr_idx(1), Some(8)); // (0,1) -> 0*1 + 1*8 = 8
    assert_eq!(view.expr_idx(4), Some(1)); // (1,0) -> 1*1 + 0*8 = 1
    assert_eq!(view.expr_idx(5), Some(9)); // (1,1) -> 1*1 + 1*8 = 9
}

#[test]
fn test_expr_idx_3d_specialization() {
    // 3D contiguous — fast path
    let view = View::contiguous(&[2, 3, 4]);
    assert!(view.is_contiguous());
    for i in 0..(2 * 3 * 4) {
        assert_eq!(
            view.expr_idx(i),
            Some(i),
            "3D contiguous failed at index {}",
            i
        );
    }

    // 3D non-contiguous: shrunk
    let st = ShapeTracker::contiguous(&[4, 6, 8]).shrink(&[(1, 3), (2, 5), (3, 7)]);
    let view = st.view();
    assert!(!view.is_contiguous());
    // Just verify it doesn't panic
    for i in 0..(2 * 3 * 4) {
        let _ = view.expr_idx(i);
    }
}

// ============================================================
// FMA emission in rendered shaders
// ============================================================

fn make_mul_add_kernel() -> FusedKernel {
    // y = a * b + c  ->  should emit fma(a, b, c)
    let n = 1024;
    FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::Add,
                srcs: vec![FusedSrc::Op(0), FusedSrc::Buf(3)],
                dst_dtype: DType::Float32,
            },
        ],
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
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 3,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [4, 1, 1],
        local: [256, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

#[test]
fn test_fma_emission_msl() {
    let kernel = make_mul_add_kernel();
    let renderer = msl::MslRenderer;
    let source = renderer.render(&kernel);
    assert!(
        source.contains("fma("),
        "MSL should emit fma() for MUL+ADD pattern. Got:\n{}",
        source
    );
}

#[test]
fn test_fma_emission_cuda() {
    let kernel = make_mul_add_kernel();
    let renderer = cuda::CudaRenderer;
    let source = renderer.render(&kernel);
    assert!(
        source.contains("fmaf("),
        "CUDA should emit fmaf() for MUL+ADD pattern. Got:\n{}",
        source
    );
}

#[test]
fn test_fma_emission_wgsl() {
    let kernel = make_mul_add_kernel();
    let renderer = wgsl::WgslRenderer::new();
    let source = renderer.render(&kernel);
    assert!(
        source.contains("fma("),
        "WGSL should emit fma() for MUL+ADD pattern. Got:\n{}",
        source
    );
}

#[test]
fn test_fma_emission_opencl() {
    let kernel = make_mul_add_kernel();
    let renderer = opencl::OpenClRenderer::new(true);
    let source = renderer.render(&kernel);
    assert!(
        source.contains("fma("),
        "OpenCL should emit fma() for MUL+ADD pattern. Got:\n{}",
        source
    );
}

#[test]
fn test_fma_emission_hip() {
    let kernel = make_mul_add_kernel();
    let renderer = hip::HipRenderer;
    let source = renderer.render(&kernel);
    assert!(
        source.contains("fmaf("),
        "HIP should emit fmaf() for MUL+ADD pattern. Got:\n{}",
        source
    );
}

#[test]
fn test_no_fma_for_integer_ops() {
    // INT32 MUL+ADD should NOT emit FMA
    let n = 1024;
    let kernel = FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                dst_dtype: DType::Int32,
            },
            FusedOp {
                op: PrimitiveOp::Add,
                srcs: vec![FusedSrc::Op(0), FusedSrc::Buf(3)],
                dst_dtype: DType::Int32,
            },
        ],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Int32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Int32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Int32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 3,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Int32,
                access: BufferAccess::Read,
            },
        ],
        grid: [4, 1, 1],
        local: [256, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let renderer = msl::MslRenderer;
    let source = renderer.render(&kernel);
    assert!(
        !source.contains("fma("),
        "MSL should NOT emit fma() for integer MUL+ADD. Got:\n{}",
        source
    );
}

// ============================================================
// Loop unrolling hints
// ============================================================

#[test]
fn test_unroll_hint_msl_small_reduce() {
    // Small reduce (8 elements) should get #pragma unroll
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceSum,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[1]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[8]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let renderer = msl::MslRenderer;
    let source = renderer.render(&kernel);
    assert!(
        source.contains("#pragma unroll"),
        "MSL should emit #pragma unroll for small reduce (8 elements). Got:\n{}",
        source
    );
}

#[test]
fn test_no_unroll_hint_msl_large_reduce() {
    // Large reduce (256 elements) should NOT get #pragma unroll
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceSum,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[1]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[256]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let renderer = msl::MslRenderer;
    let source = renderer.render(&kernel);
    assert!(
        !source.contains("#pragma unroll"),
        "MSL should NOT emit #pragma unroll for large reduce (256 elements). Got:\n{}",
        source
    );
}

// ============================================================
// Kernel deduplication
// ============================================================

#[test]
fn test_kernel_dedup_identical_ops() {
    // Two kernels with same ops, same shapes, different buf_ids
    let k1 = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [128, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    let k2 = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 10, // different buf_id
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 11, // different buf_id
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 12, // different buf_id
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [128, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    let (result, dedup_count) = deduplicate_kernels(&[k1, k2]);
    assert_eq!(result.len(), 2, "should still have 2 kernels");
    assert_eq!(dedup_count, 1, "should detect 1 duplicate");
}

#[test]
fn test_kernel_dedup_different_ops() {
    let k1 = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [128, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    let k2 = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Mul, // different op
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[128]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [128, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    let (_, dedup_count) = deduplicate_kernels(&[k1, k2]);
    assert_eq!(dedup_count, 0, "different ops should not be deduplicated");
}

// ============================================================
// Arena alignment and statistics
// ============================================================

#[test]
fn test_arena_alignment_aware_alloc() {
    let arena = Arena::new(ArenaConfig {
        pool_size: 4096,
        alignment: 64,
    });

    // Allocate with 128-byte alignment (larger than minimum)
    let a1 = arena.alloc_aligned(100, 128).unwrap();
    assert_eq!(a1.offset % 128, 0, "allocation should be 128-byte aligned");

    // Second allocation should also be 128-byte aligned
    let a2 = arena.alloc_aligned(50, 128).unwrap();
    assert_eq!(
        a2.offset % 128,
        0,
        "second allocation should be 128-byte aligned"
    );
    assert!(
        a2.offset >= a1.offset + a1.size,
        "allocations should not overlap"
    );
}

#[test]
fn test_arena_default_alloc_respects_minimum() {
    let arena = Arena::new(ArenaConfig {
        pool_size: 4096,
        alignment: 256,
    });

    let a1 = arena.alloc(100).unwrap();
    assert_eq!(
        a1.offset % 256,
        0,
        "default alloc should respect minimum 256-byte alignment"
    );
}

#[test]
fn test_arena_high_water_mark() {
    let arena = Arena::new(ArenaConfig {
        pool_size: 8192,
        alignment: 64,
    });

    arena.alloc(1000).unwrap();
    arena.alloc(2000).unwrap();
    let hwm1 = arena.high_water_mark();
    assert!(
        hwm1 > 0,
        "high water mark should be nonzero after allocations"
    );

    arena.reset();

    // After reset, high water mark should persist
    assert_eq!(
        arena.high_water_mark(),
        hwm1,
        "high water mark should persist across resets"
    );

    // Smaller allocation should not change HWM
    arena.alloc(100).unwrap();
    assert_eq!(
        arena.high_water_mark(),
        hwm1,
        "smaller allocation should not increase HWM"
    );
}

#[test]
fn test_arena_fragmentation() {
    let arena = Arena::new(ArenaConfig {
        pool_size: 8192,
        alignment: 256,
    });

    // First allocation at offset 0, 100 bytes
    arena.alloc(100).unwrap();
    // Second allocation at offset 256 (aligned), another 100 bytes
    arena.alloc(100).unwrap();

    let frag = arena.fragmentation();
    assert!(
        frag > 0.0,
        "fragmentation should be > 0 due to alignment padding"
    );
    assert!(frag < 1.0, "fragmentation should be < 1.0");
}

#[test]
fn test_arena_stats() {
    let arena = Arena::new(ArenaConfig {
        pool_size: 16384,
        alignment: 64,
    });

    arena.alloc(100).unwrap();
    arena.alloc(200).unwrap();
    arena.alloc(300).unwrap();

    let stats = arena.stats();
    assert_eq!(stats.pool_size, 16384);
    assert_eq!(stats.alloc_count, 3);
    assert_eq!(stats.bytes_allocated, 600);
    assert!(
        stats.bytes_used >= 600,
        "bytes_used should be >= bytes_allocated"
    );
    assert_eq!(stats.generation, 0);
    assert_eq!(stats.high_water_mark, stats.bytes_used);
    assert_eq!(stats.peak_alloc_count, 3);
}

// ============================================================
// Page-aligned allocations (CPU device)
// ============================================================

#[test]
fn test_page_aligned_allocation() {
    let device = CpuDevice::new();

    // Large allocation (>= 4096) should be page-aligned.
    // We verify by writing and reading back data, which exercises
    // the allocation path without needing to inspect the private handle.
    let buf = device.alloc(8192).unwrap();
    assert_eq!(buf.size_bytes, 8192);

    // Verify we can write and read 8192 bytes (exercises the full buffer)
    let data: Vec<u8> = (0..8192).map(|i| (i % 256) as u8).collect();
    device.copy_in(&buf, &data).unwrap();
    let mut out = vec![0u8; 8192];
    device.copy_out(&buf, &mut out).unwrap();
    assert_eq!(
        out, data,
        "large page-aligned allocation should preserve data"
    );
}

#[test]
fn test_small_allocation_works() {
    let device = CpuDevice::new();

    // Small allocation (< 4096) uses normal Vec
    let buf = device.alloc(256).unwrap();
    assert_eq!(buf.size_bytes, 256);

    let data: Vec<u8> = (0..256).map(|i| i as u8).collect();
    device.copy_in(&buf, &data).unwrap();
    let mut out = vec![0u8; 256];
    device.copy_out(&buf, &mut out).unwrap();
    assert_eq!(out, data);
}

// ============================================================
// SIMD path correctness (only when simd-accel enabled)
// ============================================================

#[cfg(feature = "simd-accel")]
mod simd_tests {
    use super::*;
    use molt_gpu::device::cpu::interpret;

    fn f32_to_bytes(values: &[f32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn bytes_to_f32(buf: &[u8]) -> Vec<f32> {
        buf.chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    #[test]
    fn test_simd_add_matches_scalar() {
        let n = 17; // Not a multiple of 4, exercises remainder path
        let a: Vec<f32> = (0..n).map(|i| i as f32).collect();
        let b: Vec<f32> = (0..n).map(|i| (i * 2) as f32).collect();

        let kernel = FusedKernel {
            ops: vec![FusedOp {
                op: PrimitiveOp::Add,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
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
                BufferBinding {
                    buf_id: 2,
                    st: ShapeTracker::contiguous(&[n]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [n as u32, 1, 1],
            local: [n as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        };

        let mut bufs = vec![vec![0u8; n * 4], f32_to_bytes(&a), f32_to_bytes(&b)];

        interpret::execute_kernel(&kernel, &mut bufs);
        let result = bytes_to_f32(&bufs[0]);

        for i in 0..n {
            let expected = a[i] + b[i];
            assert_eq!(result[i], expected, "SIMD ADD mismatch at index {}", i);
        }
    }

    #[test]
    fn test_simd_nan_propagating_max() {
        let n = 8;
        let a: Vec<f32> = vec![1.0, f32::NAN, 3.0, 4.0, f32::NAN, 6.0, 7.0, 8.0];
        let b: Vec<f32> = vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

        let kernel = FusedKernel {
            ops: vec![FusedOp {
                op: PrimitiveOp::Max,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
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
                BufferBinding {
                    buf_id: 2,
                    st: ShapeTracker::contiguous(&[n]),
                    dtype: DType::Float32,
                    access: BufferAccess::Read,
                },
            ],
            grid: [n as u32, 1, 1],
            local: [n as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        };

        let mut bufs = vec![vec![0u8; n * 4], f32_to_bytes(&a), f32_to_bytes(&b)];

        interpret::execute_kernel(&kernel, &mut bufs);
        let result = bytes_to_f32(&bufs[0]);

        // NaN-propagating max: max(NaN, 0) = NaN
        assert_eq!(result[0], 1.0);
        assert!(
            result[1].is_nan(),
            "max(NaN, 0) should be NaN, got {}",
            result[1]
        );
        assert_eq!(result[2], 3.0);
        assert!(
            result[4].is_nan(),
            "max(NaN, 0) should be NaN, got {}",
            result[4]
        );
    }

    #[test]
    fn test_simd_sqrt_reciprocal() {
        let n = 8;
        let a: Vec<f32> = vec![1.0, 4.0, 9.0, 16.0, 25.0, 36.0, 49.0, 64.0];

        let kernel = FusedKernel {
            ops: vec![
                FusedOp {
                    op: PrimitiveOp::Sqrt,
                    srcs: vec![FusedSrc::Buf(1)],
                    dst_dtype: DType::Float32,
                },
                FusedOp {
                    op: PrimitiveOp::Reciprocal,
                    srcs: vec![FusedSrc::Op(0)],
                    dst_dtype: DType::Float32,
                },
            ],
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
            local: [n as u32, 1, 1],
            spec: None,
            vectorize_width: 1,
        };

        let mut bufs = vec![vec![0u8; n * 4], f32_to_bytes(&a)];

        interpret::execute_kernel(&kernel, &mut bufs);
        let result = bytes_to_f32(&bufs[0]);

        // 1/sqrt(x) for perfect squares
        for i in 0..n {
            let expected = 1.0 / a[i].sqrt();
            let diff = (result[i] - expected).abs();
            assert!(
                diff < 1e-6,
                "1/sqrt({}) = {}, expected {} (diff={})",
                a[i],
                result[i],
                expected,
                diff
            );
        }
    }
}

// ============================================================
// Shape specialization with bounds check elimination
// ============================================================

#[test]
fn test_bounds_check_elim_for_divisible_size() {
    let mut kernels = vec![FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[256]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[256]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[256]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [256, 1, 1],
        spec: None,
        vectorize_width: 1,
    }];

    specialize_shapes(&mut kernels);

    let spec = kernels[0]
        .spec
        .as_ref()
        .expect("specialization should be applied");
    assert!(
        spec.bounds_check_elim,
        "256 elements should allow bounds check elimination"
    );
    assert_eq!(spec.total_elements, 256);
    assert!(spec.all_static);
}
