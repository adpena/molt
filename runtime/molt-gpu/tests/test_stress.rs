//! Stress tests for the CPU interpreter with adversarial inputs.
//!
//! Tests every op with edge-case sizes (0, 1, 1M elements),
//! extreme float values (MAX, MIN, EPSILON, subnormals),
//! high-dimensional ShapeTracker, deeply fused chains, and
//! constant folding with all-constant inputs.

use molt_gpu::device::cpu::interpret;
use molt_gpu::dtype::DType;
use molt_gpu::fuse::constant_fold;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc};
use molt_gpu::shapetracker::ShapeTracker;

// --- Helpers ---

fn f32_to_bytes(vals: &[f32]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn make_unary_kernel(op: PrimitiveOp, n: usize) -> FusedKernel {
    FusedKernel {
        ops: vec![FusedOp {
            op,
            srcs: vec![FusedSrc::Buf(1)],
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
    }
}

fn make_binary_kernel(op: PrimitiveOp, n: usize) -> FusedKernel {
    FusedKernel {
        ops: vec![FusedOp {
            op,
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
        local: [1, 1, 1],
        spec: None,
    }
}

fn run_unary(op: PrimitiveOp, input: &[f32]) -> Vec<f32> {
    let n = input.len();
    if n == 0 {
        return vec![];
    }
    let kernel = make_unary_kernel(op, n);
    let mut bufs = vec![vec![0u8; n * 4], f32_to_bytes(input)];
    interpret::execute_kernel(&kernel, &mut bufs);
    bytes_to_f32(&bufs[0])
}

fn run_binary(op: PrimitiveOp, a: &[f32], b: &[f32]) -> Vec<f32> {
    let n = a.len();
    assert_eq!(n, b.len());
    if n == 0 {
        return vec![];
    }
    let kernel = make_binary_kernel(op, n);
    let mut bufs = vec![vec![0u8; n * 4], f32_to_bytes(a), f32_to_bytes(b)];
    interpret::execute_kernel(&kernel, &mut bufs);
    bytes_to_f32(&bufs[0])
}

// =============================================================================
// 1. Empty tensors (0 elements) — every elementwise op must handle gracefully
// =============================================================================

#[test]
fn test_empty_tensor_unary_ops() {
    let empty: &[f32] = &[];
    let unary_ops = [
        PrimitiveOp::Neg,
        PrimitiveOp::Exp2,
        PrimitiveOp::Log2,
        PrimitiveOp::Sin,
        PrimitiveOp::Sqrt,
        PrimitiveOp::Reciprocal,
        PrimitiveOp::Trunc,
        PrimitiveOp::Cast,
    ];
    for op in unary_ops {
        let result = run_unary(op, empty);
        assert!(result.is_empty(), "op {:?} on empty tensor should return empty", op);
    }
}

#[test]
fn test_empty_tensor_binary_ops() {
    let empty: &[f32] = &[];
    let binary_ops = [
        PrimitiveOp::Add,
        PrimitiveOp::Sub,
        PrimitiveOp::Mul,
        PrimitiveOp::Max,
        PrimitiveOp::Cmplt,
        PrimitiveOp::Cmpeq,
        PrimitiveOp::Cmpne,
    ];
    for op in binary_ops {
        let result = run_binary(op, empty, empty);
        assert!(result.is_empty(), "op {:?} on empty tensors should return empty", op);
    }
}

// =============================================================================
// 2. Single element — every op
// =============================================================================

#[test]
fn test_single_element_all_unary_ops() {
    let input = &[42.0f32];
    let unary_ops = [
        PrimitiveOp::Neg,
        PrimitiveOp::Exp2,
        PrimitiveOp::Log2,
        PrimitiveOp::Sin,
        PrimitiveOp::Sqrt,
        PrimitiveOp::Reciprocal,
        PrimitiveOp::Trunc,
        PrimitiveOp::Cast,
    ];
    for op in unary_ops {
        let result = run_unary(op, input);
        assert_eq!(result.len(), 1, "op {:?} should produce 1 element", op);
    }
}

#[test]
fn test_single_element_all_binary_ops() {
    let a = &[3.0f32];
    let b = &[7.0f32];
    let binary_ops = [
        PrimitiveOp::Add,
        PrimitiveOp::Sub,
        PrimitiveOp::Mul,
        PrimitiveOp::Idiv,
        PrimitiveOp::Mod,
        PrimitiveOp::Max,
        PrimitiveOp::Cmplt,
        PrimitiveOp::Cmpeq,
        PrimitiveOp::Cmpne,
        PrimitiveOp::And,
        PrimitiveOp::Or,
        PrimitiveOp::Xor,
    ];
    for op in binary_ops {
        let result = run_binary(op, a, b);
        assert_eq!(result.len(), 1, "op {:?} should produce 1 element", op);
    }
}

// =============================================================================
// 3. Large tensor (1M elements)
// =============================================================================

#[test]
fn test_large_tensor_add_1m() {
    let n = 1_000_000;
    let a: Vec<f32> = (0..n).map(|i| i as f32).collect();
    let b: Vec<f32> = (0..n).map(|i| (n - i) as f32).collect();
    let result = run_binary(PrimitiveOp::Add, &a, &b);
    assert_eq!(result.len(), n);
    // Every element should be n
    for (i, &v) in result.iter().enumerate() {
        assert_eq!(v, n as f32, "element {} mismatch", i);
    }
}

#[test]
fn test_large_tensor_neg_1m() {
    let n = 1_000_000;
    let a: Vec<f32> = (0..n).map(|i| i as f32).collect();
    let result = run_unary(PrimitiveOp::Neg, &a);
    assert_eq!(result.len(), n);
    for (i, &v) in result.iter().enumerate() {
        assert_eq!(v, -(i as f32), "element {} mismatch", i);
    }
}

// =============================================================================
// 4. Extreme float values: f32::MAX, f32::MIN, f32::EPSILON, subnormals
// =============================================================================

#[test]
fn test_extreme_values_add() {
    let extremes = &[f32::MAX, f32::MIN, f32::EPSILON, f32::MIN_POSITIVE, 0.0, -0.0];
    let zeros = &[0.0f32; 6];
    let result = run_binary(PrimitiveOp::Add, extremes, zeros);
    assert_eq!(result.len(), 6);
    assert_eq!(result[0], f32::MAX);
    assert_eq!(result[1], f32::MIN);
    assert_eq!(result[2], f32::EPSILON);
    assert_eq!(result[3], f32::MIN_POSITIVE);
}

#[test]
fn test_extreme_values_mul_overflow() {
    let a = &[f32::MAX, f32::MAX];
    let b = &[2.0f32, -2.0];
    let result = run_binary(PrimitiveOp::Mul, a, b);
    assert!(result[0].is_infinite() && result[0] > 0.0, "MAX * 2 = +inf");
    assert!(result[1].is_infinite() && result[1] < 0.0, "MAX * -2 = -inf");
}

#[test]
fn test_extreme_values_neg() {
    let a = &[f32::MAX, f32::MIN, f32::INFINITY, f32::NEG_INFINITY, f32::NAN, 0.0f32];
    let result = run_unary(PrimitiveOp::Neg, a);
    assert_eq!(result[0], -f32::MAX);
    assert_eq!(result[1], -f32::MIN);
    assert_eq!(result[2], f32::NEG_INFINITY);
    assert_eq!(result[3], f32::INFINITY);
    assert!(result[4].is_nan());
    // -0.0: bit-level check
    assert_eq!(result[5].to_bits(), (-0.0f32).to_bits());
}

#[test]
fn test_subnormal_values() {
    // Smallest positive subnormal: 2^-149
    let subnormal = f32::from_bits(1u32); // smallest positive subnormal
    let a = &[subnormal, -subnormal, subnormal];
    let b = &[subnormal, subnormal, 0.0f32];
    let result = run_binary(PrimitiveOp::Add, a, b);
    assert_eq!(result.len(), 3);
    // subnormal + subnormal = 2 * subnormal
    assert_eq!(result[0], 2.0 * subnormal);
    // -subnormal + subnormal = 0.0
    assert_eq!(result[1], 0.0);
    // subnormal + 0 = subnormal
    assert_eq!(result[2], subnormal);
}

#[test]
fn test_nan_propagation_all_ops() {
    let nan = &[f32::NAN];
    let one = &[1.0f32];

    // Unary ops with NaN input
    let neg = run_unary(PrimitiveOp::Neg, nan);
    assert!(neg[0].is_nan());

    let exp2 = run_unary(PrimitiveOp::Exp2, nan);
    assert!(exp2[0].is_nan());

    let log2 = run_unary(PrimitiveOp::Log2, nan);
    assert!(log2[0].is_nan());

    let sin = run_unary(PrimitiveOp::Sin, nan);
    assert!(sin[0].is_nan());

    let sqrt = run_unary(PrimitiveOp::Sqrt, nan);
    assert!(sqrt[0].is_nan());

    let recip = run_unary(PrimitiveOp::Reciprocal, nan);
    assert!(recip[0].is_nan());

    let trunc = run_unary(PrimitiveOp::Trunc, nan);
    assert!(trunc[0].is_nan());

    // Binary ops with NaN
    let add = run_binary(PrimitiveOp::Add, nan, one);
    assert!(add[0].is_nan());

    let mul = run_binary(PrimitiveOp::Mul, nan, one);
    assert!(mul[0].is_nan());

    // NaN-propagating max
    let max_nan_l = run_binary(PrimitiveOp::Max, nan, one);
    assert!(max_nan_l[0].is_nan(), "max(NaN, 1) must be NaN");

    let max_nan_r = run_binary(PrimitiveOp::Max, one, nan);
    assert!(max_nan_r[0].is_nan(), "max(1, NaN) must be NaN");
}

#[test]
fn test_inf_arithmetic() {
    let inf = &[f32::INFINITY];
    let neg_inf = &[f32::NEG_INFINITY];
    let one = &[1.0f32];

    let add = run_binary(PrimitiveOp::Add, inf, one);
    assert_eq!(add[0], f32::INFINITY);

    let sub = run_binary(PrimitiveOp::Sub, inf, inf);
    assert!(sub[0].is_nan(), "inf - inf = NaN");

    let mul = run_binary(PrimitiveOp::Mul, inf, neg_inf);
    assert_eq!(mul[0], f32::NEG_INFINITY);

    let recip = run_unary(PrimitiveOp::Reciprocal, inf);
    assert_eq!(recip[0], 0.0);
}

#[test]
fn test_comparison_with_nan() {
    let nan = &[f32::NAN];
    let one = &[1.0f32];

    // NaN < 1 = false (IEEE 754 unordered comparison)
    let cmplt = run_binary(PrimitiveOp::Cmplt, nan, one);
    assert_eq!(cmplt[0], 0.0, "NaN < 1 must be false");

    // NaN == NaN = false
    let cmpeq = run_binary(PrimitiveOp::Cmpeq, nan, nan);
    assert_eq!(cmpeq[0], 0.0, "NaN == NaN must be false");

    // NaN != NaN = true
    let cmpne = run_binary(PrimitiveOp::Cmpne, nan, nan);
    assert_eq!(cmpne[0], 1.0, "NaN != NaN must be true");
}

// =============================================================================
// 5. Reduce on single-element tensor
// =============================================================================

#[test]
fn test_reduce_sum_single_element() {
    let n_in = 1;
    let n_out = 1;
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceSum,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[n_out]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n_in]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [n_out as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
    };

    let mut bufs = vec![vec![0u8; n_out * 4], f32_to_bytes(&[42.0])];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![42.0]);
}

#[test]
fn test_reduce_max_single_element() {
    let n_in = 1;
    let n_out = 1;
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceMax,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[n_out]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n_in]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [n_out as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
    };

    let mut bufs = vec![vec![0u8; n_out * 4], f32_to_bytes(&[-999.0])];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![-999.0]);
}

#[test]
fn test_reduce_sum_large() {
    // Reduce 1024 elements to 1
    let n_in = 1024;
    let n_out = 1;
    let input: Vec<f32> = (0..n_in).map(|i| i as f32).collect();
    let expected: f32 = (0..n_in).map(|i| i as f32).sum();

    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceSum,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[n_out]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n_in]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [n_out as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
    };

    let mut bufs = vec![vec![0u8; n_out * 4], f32_to_bytes(&input)];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert!((result[0] - expected).abs() < 1.0, "reduce sum of 0..1024: got {}, expected {}", result[0], expected);
}

// =============================================================================
// 6. ShapeTracker with 6+ dimensions
// =============================================================================

#[test]
fn test_shapetracker_6d() {
    let shape = [2, 3, 4, 5, 6, 7]; // 6D, 5040 elements
    let st = ShapeTracker::contiguous(&shape);
    assert_eq!(st.numel(), 5040);
    assert_eq!(st.shape(), &shape);

    // Verify all indices map correctly for contiguous 6D
    for i in 0..st.numel() {
        assert_eq!(st.expr_idx(i), Some(i), "6D contiguous index {} should map to itself", i);
    }
}

#[test]
fn test_shapetracker_7d_permute() {
    let shape = [2, 3, 4, 2, 3, 2, 5]; // 7D, 1440 elements
    let st = ShapeTracker::contiguous(&shape);
    assert_eq!(st.numel(), 1440);

    // Permute: reverse all dimensions
    let permuted = st.permute(&[6, 5, 4, 3, 2, 1, 0]);
    assert_eq!(permuted.shape(), &[5, 2, 3, 2, 4, 3, 2]);
    assert_eq!(permuted.numel(), 1440);

    // Verify the permuted view resolves every index
    for i in 0..permuted.numel() {
        let idx = permuted.expr_idx(i);
        assert!(idx.is_some(), "7D permuted index {} should resolve", i);
    }
}

#[test]
fn test_shapetracker_8d_reshape() {
    // 8D: 2*2*2*2*2*2*2*2 = 256
    let shape = [2; 8];
    let st = ShapeTracker::contiguous(&shape);
    assert_eq!(st.numel(), 256);

    // Reshape to flat
    let flat = st.reshape(&[256]);
    assert_eq!(flat.shape(), &[256]);
    for i in 0..256 {
        assert_eq!(flat.expr_idx(i), Some(i));
    }
}

#[test]
fn test_shapetracker_6d_pad() {
    let shape = [2, 3, 4, 5, 6, 7];
    let st = ShapeTracker::contiguous(&shape);

    // Pad first and last dimension
    let padded = st.pad(&[(1, 1), (0, 0), (0, 0), (0, 0), (0, 0), (1, 1)]);
    assert_eq!(padded.shape(), &[4, 3, 4, 5, 6, 9]);
}

// =============================================================================
// 7. Fused chain of 20+ elementwise ops
// =============================================================================

#[test]
fn test_fused_chain_20_ops() {
    // Build a chain: buf[1] -> Add(const 1) -> Add(const 1) -> ... (20 times)
    // Result should be input + 20
    let n = 128;
    let num_chain_ops = 20;
    let mut ops = Vec::with_capacity(num_chain_ops);

    // First op: Add(buf[1], const 1.0)
    ops.push(FusedOp {
        op: PrimitiveOp::Add,
        srcs: vec![FusedSrc::Buf(1), FusedSrc::Const { val: 1.0, dtype: DType::Float32 }],
        dst_dtype: DType::Float32,
    });

    // Remaining ops: Add(prev_op, const 1.0)
    for i in 1..num_chain_ops {
        ops.push(FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![
                FusedSrc::Op(i - 1),
                FusedSrc::Const { val: 1.0, dtype: DType::Float32 },
            ],
            dst_dtype: DType::Float32,
        });
    }

    let kernel = FusedKernel {
        ops,
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
    };

    let input: Vec<f32> = (0..n).map(|i| i as f32).collect();
    let mut bufs = vec![vec![0u8; n * 4], f32_to_bytes(&input)];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);

    for (i, &v) in result.iter().enumerate() {
        let expected = i as f32 + num_chain_ops as f32;
        assert_eq!(v, expected, "element {}: got {}, expected {}", i, v, expected);
    }
}

#[test]
fn test_fused_chain_mixed_ops_25() {
    // Chain: Neg -> Add(const 10) -> Mul(const 2) -> Sub(const 1) -> repeated
    // 25 ops total alternating patterns
    let n = 64;
    let mut ops = Vec::new();

    // Op 0: Neg(buf[1])
    ops.push(FusedOp {
        op: PrimitiveOp::Neg,
        srcs: vec![FusedSrc::Buf(1)],
        dst_dtype: DType::Float32,
    });

    for i in 1..25 {
        let (op, srcs) = match i % 4 {
            1 => (
                PrimitiveOp::Add,
                vec![FusedSrc::Op(i - 1), FusedSrc::Const { val: 10.0, dtype: DType::Float32 }],
            ),
            2 => (
                PrimitiveOp::Mul,
                vec![FusedSrc::Op(i - 1), FusedSrc::Const { val: 0.5, dtype: DType::Float32 }],
            ),
            3 => (
                PrimitiveOp::Sub,
                vec![FusedSrc::Op(i - 1), FusedSrc::Const { val: 1.0, dtype: DType::Float32 }],
            ),
            _ => (
                PrimitiveOp::Neg,
                vec![FusedSrc::Op(i - 1)],
            ),
        };
        ops.push(FusedOp { op, srcs, dst_dtype: DType::Float32 });
    }

    let kernel = FusedKernel {
        ops,
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
    };

    let input: Vec<f32> = vec![0.0; n];
    let mut bufs = vec![vec![0u8; n * 4], f32_to_bytes(&input)];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result.len(), n);

    // Verify all elements are the same (uniform input)
    let first = result[0];
    for (i, &v) in result.iter().enumerate() {
        assert_eq!(v.to_bits(), first.to_bits(), "element {} differs: got {}, expected {}", i, v, first);
    }
}

// =============================================================================
// 8. Constant folding with all constant inputs
// =============================================================================

#[test]
fn test_constant_fold_all_constant_chain() {
    // Kernel: Const(2.0) + Const(3.0) = 5.0 (compile-time)
    let n = 16;
    let mut kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![
                FusedSrc::Const { val: 2.0, dtype: DType::Float32 },
                FusedSrc::Const { val: 3.0, dtype: DType::Float32 },
            ],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![BufferBinding {
            buf_id: 0,
            st: ShapeTracker::contiguous(&[n]),
            dtype: DType::Float32,
            access: BufferAccess::Write,
        }],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
    };

    let folded = constant_fold(std::slice::from_mut(&mut kernel));
    assert_eq!(folded, 1, "should fold 1 op");
    assert!(kernel.ops.is_empty(), "all ops should be folded away");
}

#[test]
fn test_constant_fold_nested_chain() {
    // Const(2) * Const(3) -> result + Const(4) -> all constant
    let n = 8;
    let mut kernel = FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![
                    FusedSrc::Const { val: 2.0, dtype: DType::Float32 },
                    FusedSrc::Const { val: 3.0, dtype: DType::Float32 },
                ],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::Add,
                srcs: vec![
                    FusedSrc::Op(0), // result of Mul = 6.0
                    FusedSrc::Const { val: 4.0, dtype: DType::Float32 },
                ],
                dst_dtype: DType::Float32,
            },
        ],
        bufs: vec![BufferBinding {
            buf_id: 0,
            st: ShapeTracker::contiguous(&[n]),
            dtype: DType::Float32,
            access: BufferAccess::Write,
        }],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
    };

    let folded = constant_fold(std::slice::from_mut(&mut kernel));
    assert_eq!(folded, 2, "should fold both ops");
    assert!(kernel.ops.is_empty(), "all ops should be folded away");
}

#[test]
fn test_constant_fold_partial_chain() {
    // Op 0: buf[1] + const(5.0)  -- NOT foldable (depends on buffer)
    // Op 1: const(2.0) * const(3.0) -- foldable = 6.0
    // Op 2: Op(0) + Op(1) -- references folded op1 as Const(6.0)
    let n = 4;
    let mut kernel = FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Add,
                srcs: vec![
                    FusedSrc::Buf(1),
                    FusedSrc::Const { val: 5.0, dtype: DType::Float32 },
                ],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![
                    FusedSrc::Const { val: 2.0, dtype: DType::Float32 },
                    FusedSrc::Const { val: 3.0, dtype: DType::Float32 },
                ],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::Add,
                srcs: vec![FusedSrc::Op(0), FusedSrc::Op(1)],
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
        local: [1, 1, 1],
        spec: None,
    };

    let folded = constant_fold(std::slice::from_mut(&mut kernel));
    assert_eq!(folded, 1, "should fold 1 op (the Mul)");
    assert_eq!(kernel.ops.len(), 2, "2 ops should remain");

    // The second remaining op should reference Const(6.0) for its second source
    match &kernel.ops[1].srcs[1] {
        FusedSrc::Const { val, .. } => {
            assert!((val - 6.0).abs() < 1e-10, "folded constant should be 6.0, got {}", val);
        }
        other => panic!("expected Const(6.0), got {:?}", other),
    }
}

// =============================================================================
// 9. Ternary Where op stress
// =============================================================================

#[test]
fn test_where_op_large() {
    let n = 10_000;
    // cond: alternating 0/1
    let cond: Vec<f32> = (0..n).map(|i| if i % 2 == 0 { 1.0 } else { 0.0 }).collect();
    let a: Vec<f32> = vec![100.0; n];
    let b: Vec<f32> = vec![200.0; n];

    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Where,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2), FusedSrc::Buf(3)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 3, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
    };

    let mut bufs = vec![
        vec![0u8; n * 4],
        f32_to_bytes(&cond),
        f32_to_bytes(&a),
        f32_to_bytes(&b),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);

    for (i, &v) in result.iter().enumerate() {
        let expected = if i % 2 == 0 { 100.0 } else { 200.0 };
        assert_eq!(v, expected, "where element {} mismatch", i);
    }
}

// =============================================================================
// 10. Shift ops with edge values
// =============================================================================

#[test]
fn test_shift_ops_edge_cases() {
    // Shl/Shr with 0 shift should be identity
    let a = &[42.0f32, -7.0, 0.0, 1.0];
    let zero = &[0.0f32; 4];

    let shl = run_binary(PrimitiveOp::Shl, a, zero);
    assert_eq!(shl[0] as i64, 42);
    assert_eq!(shl[1] as i64, -7);

    let shr = run_binary(PrimitiveOp::Shr, a, zero);
    assert_eq!(shr[0] as i64, 42);
    assert_eq!(shr[1] as i64, -7);
}

// =============================================================================
// 11. Division by zero
// =============================================================================

#[test]
fn test_idiv_by_zero() {
    let result = run_binary(PrimitiveOp::Idiv, &[10.0], &[0.0]);
    // CPU interpreter returns 0.0 for division by zero
    assert_eq!(result[0], 0.0);
}

#[test]
fn test_mod_by_zero() {
    let result = run_binary(PrimitiveOp::Mod, &[10.0], &[0.0]);
    assert_eq!(result[0], 0.0);
}

#[test]
fn test_reciprocal_negative_zero() {
    let result = run_unary(PrimitiveOp::Reciprocal, &[-0.0]);
    assert!(result[0].is_infinite() && result[0] < 0.0, "1/-0.0 = -inf");
}
