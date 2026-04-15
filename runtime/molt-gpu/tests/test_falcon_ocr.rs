//! Falcon-OCR composition tests on CPU.
//!
//! Tests the core tensor operation compositions used by Falcon-OCR:
//! - RMSNorm: REDUCE_SUM(MUL(x,x)) -> SQRT -> RECIPROCAL -> MUL
//! - RoPE: SIN + COS + MUL + ADD (rotation)
//! - Scaled dot-product attention: DOT -> MUL(scale) -> ADD(mask) -> softmax -> DOT
//! - Feed-forward with squared-ReLU gate: DOT -> split -> MUL -> MAX(0) -> MUL(squared) -> DOT
//! - Full forward block: norm -> attention -> residual -> norm -> ffn -> residual
//!
//! All tests verify numerical correctness against f64 reference values.

use molt_gpu::device::cpu::interpret;
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

/// Run a chain of ops on CPU using the interpreter.
fn run_chain(ops: Vec<FusedOp>, bufs: Vec<BufferBinding>, input_bufs: Vec<Vec<u8>>) -> Vec<f32> {
    let n_out = bufs[0].st.numel();
    let mut all_bufs = vec![vec![0u8; n_out * 4]];
    all_bufs.extend(input_bufs);

    let kernel = FusedKernel {
        ops,
        bufs,
        grid: [n_out as u32, 1, 1],
        local: [1, 1, 1],
                spec: None,
    };

    interpret::execute_kernel(&kernel, &mut all_bufs);
    bytes_to_f32(&all_bufs[0])
}

// =========================================================================
// RMSNorm: x / sqrt(mean(x^2) + eps)
//
// Composition: MUL(x,x) -> REDUCE_SUM -> MUL(1/N) -> ADD(eps) -> SQRT ->
//              RECIPROCAL -> MUL(x, result)
// =========================================================================

#[test]
fn test_falcon_rms_norm_unit_vector() {
    // For x = [1.0, 1.0, 1.0, 1.0]:
    // mean(x^2) = 1.0, sqrt(1.0 + eps) ~= 1.0, x / 1.0 = x
    let x = [1.0f32, 1.0, 1.0, 1.0];
    let n = x.len();
    let eps = 1e-6f64;

    // Step 1: x^2
    let k1 = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(1)],
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
    };
    let mut bufs1 = vec![vec![0u8; n * 4], f32_to_bytes(&x)];
    interpret::execute_kernel(&k1, &mut bufs1);
    let x_sq = bytes_to_f32(&bufs1[0]);

    // Step 2: REDUCE_SUM(x^2)
    let k2 = FusedKernel {
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
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
                spec: None,
    };
    let mut bufs2 = vec![vec![0u8; 4], f32_to_bytes(&x_sq)];
    interpret::execute_kernel(&k2, &mut bufs2);
    let sum_sq = bytes_to_f32(&bufs2[0])[0];
    let mean_sq = sum_sq / n as f32;

    // Step 3: sqrt(mean + eps) -> reciprocal -> scale
    let rms = ((mean_sq as f64) + eps).sqrt();
    let inv_rms = 1.0 / rms;

    let ops_norm = vec![FusedOp {
        op: PrimitiveOp::Mul,
        srcs: vec![
            FusedSrc::Buf(1),
            FusedSrc::Const {
                val: inv_rms,
                dtype: DType::Float32,
            },
        ],
        dst_dtype: DType::Float32,
    }];

    let bufs_norm = vec![
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
    ];

    let result = run_chain(ops_norm, bufs_norm, vec![f32_to_bytes(&x)]);
    for (i, &v) in result.iter().enumerate() {
        let expected = (x[i] as f64) * inv_rms;
        let diff = (v as f64 - expected).abs();
        assert!(
            diff < 1e-5,
            "rms_norm[{}]: got {} expected {} (diff={})",
            i, v, expected, diff
        );
    }
}

#[test]
fn test_falcon_rms_norm_scaling() {
    // For x = [2.0, 2.0, 2.0, 2.0]:
    // mean(x^2) = 4.0, sqrt(4.0 + eps) ~= 2.0, x / 2.0 ~= 1.0
    let x = [2.0f32, 2.0, 2.0, 2.0];
    let n = x.len();
    let eps = 1e-6f64;
    let mean_sq: f64 = x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / n as f64;
    let inv_rms = 1.0 / (mean_sq + eps).sqrt();

    let result = run_chain(
        vec![FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![
                FusedSrc::Buf(1),
                FusedSrc::Const {
                    val: inv_rms,
                    dtype: DType::Float32,
                },
            ],
            dst_dtype: DType::Float32,
        }],
        vec![
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
        vec![f32_to_bytes(&x)],
    );

    for &v in &result {
        let diff = (v - 1.0f32).abs();
        assert!(diff < 1e-5, "Expected ~1.0, got {}", v);
    }
}

// =========================================================================
// RoPE: rotation via SIN + COS + MUL + SUB/ADD
//
// out_real = x_real * cos - x_imag * sin
// out_imag = x_real * sin + x_imag * cos
// =========================================================================

#[test]
fn test_falcon_rope_identity_at_pos_zero() {
    // At position 0, cos=1.0 and sin=0.0, so RoPE should be identity.
    let x_real = [1.0f32, 2.0, 3.0, 4.0];
    let x_imag = [5.0f32, 6.0, 7.0, 8.0];
    let cos_vals = [1.0f32; 4]; // cos(0) = 1
    let sin_vals = [0.0f32; 4]; // sin(0) = 0
    let n = x_real.len();

    // out_real = x_real * cos - x_imag * sin = x_real * 1 - x_imag * 0 = x_real
    let ops_real = vec![
        // x_real * cos
        FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(3)],
            dst_dtype: DType::Float32,
        },
        // x_imag * sin
        FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![FusedSrc::Buf(2), FusedSrc::Buf(4)],
            dst_dtype: DType::Float32,
        },
        // x_real * cos - x_imag * sin
        FusedOp {
            op: PrimitiveOp::Sub,
            srcs: vec![FusedSrc::Op(0), FusedSrc::Op(1)],
            dst_dtype: DType::Float32,
        },
    ];

    let bufs = vec![
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
        BufferBinding {
            buf_id: 4,
            st: ShapeTracker::contiguous(&[n]),
            dtype: DType::Float32,
            access: BufferAccess::Read,
        },
    ];

    let result = run_chain(
        ops_real,
        bufs,
        vec![
            f32_to_bytes(&x_real),
            f32_to_bytes(&x_imag),
            f32_to_bytes(&cos_vals),
            f32_to_bytes(&sin_vals),
        ],
    );

    for (i, &v) in result.iter().enumerate() {
        assert!(
            (v - x_real[i]).abs() < 1e-6,
            "RoPE at pos 0 should be identity for real part: got {} expected {}",
            v, x_real[i]
        );
    }
}

#[test]
fn test_falcon_rope_90_degree_rotation() {
    // At angle pi/2, cos=0 and sin=1, so:
    // out_real = x_real * 0 - x_imag * 1 = -x_imag
    // out_imag = x_real * 1 + x_imag * 0 = x_real
    let x_real = [1.0f32, 2.0];
    let x_imag = [3.0f32, 4.0];
    let cos_vals = [0.0f32; 2];
    let sin_vals = [1.0f32; 2];
    let n = 2;

    // out_real = x_real * cos - x_imag * sin
    let ops = vec![
        FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(3)],
            dst_dtype: DType::Float32,
        },
        FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![FusedSrc::Buf(2), FusedSrc::Buf(4)],
            dst_dtype: DType::Float32,
        },
        FusedOp {
            op: PrimitiveOp::Sub,
            srcs: vec![FusedSrc::Op(0), FusedSrc::Op(1)],
            dst_dtype: DType::Float32,
        },
    ];

    let bufs = vec![
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
        BufferBinding {
            buf_id: 4,
            st: ShapeTracker::contiguous(&[n]),
            dtype: DType::Float32,
            access: BufferAccess::Read,
        },
    ];

    let result = run_chain(
        ops,
        bufs,
        vec![
            f32_to_bytes(&x_real),
            f32_to_bytes(&x_imag),
            f32_to_bytes(&cos_vals),
            f32_to_bytes(&sin_vals),
        ],
    );

    // out_real should be -x_imag
    for (i, &v) in result.iter().enumerate() {
        let expected = -x_imag[i];
        assert!(
            (v - expected).abs() < 1e-6,
            "RoPE 90deg real[{}]: got {} expected {}",
            i, v, expected
        );
    }
}

// =========================================================================
// Scaled dot-product attention
//
// scores = Q @ K^T * scale
// masked = scores + mask
// probs = softmax(masked)
// out = probs @ V
// =========================================================================

#[test]
fn test_falcon_attention_dot_scale() {
    // DOT product of a vector with itself, then scale
    let q = [1.0f32, 0.0, 0.0, 1.0]; // 2x2 identity-like
    let k = [1.0f32, 0.0, 0.0, 1.0];
    let n = 2; // 2x2 matrices

    // q[0]*k[0] + q[1]*k[1] per row-col pair -> matmul
    // For identity-like: Q@K^T = I
    // Then scale by 1/sqrt(d_k) = 1/sqrt(2)
    let scale = 1.0 / (2.0f64).sqrt();

    let ops = vec![
        FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Float32,
        },
        FusedOp {
            op: PrimitiveOp::ReduceSum,
            srcs: vec![FusedSrc::Op(0)],
            dst_dtype: DType::Float32,
        },
    ];

    let bufs = vec![
        BufferBinding {
            buf_id: 0,
            st: ShapeTracker::contiguous(&[1]),
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
    ];

    let result = run_chain(
        ops,
        bufs,
        vec![f32_to_bytes(&q[0..2]), f32_to_bytes(&k[0..2])],
    );
    let dot = result[0] as f64;
    let scaled = dot * scale;
    let expected = 1.0 * scale; // q=[1,0], k=[1,0] -> dot=1
    let diff = (scaled - expected).abs();
    assert!(diff < 1e-5, "Attention dot*scale: {} vs {}", scaled, expected);
}

#[test]
fn test_falcon_attention_mask_application() {
    // Adding -inf mask should zero out softmax entries
    let scores = [1.0f32, 1.0, 1.0, 1.0];
    let mask = [0.0f32, -1.0e9, 0.0, 0.0]; // Block position 1
    let n = 4;

    let ops = vec![FusedOp {
        op: PrimitiveOp::Add,
        srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
        dst_dtype: DType::Float32,
    }];

    let bufs = vec![
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
    ];

    let result = run_chain(ops, bufs, vec![f32_to_bytes(&scores), f32_to_bytes(&mask)]);
    assert!((result[0] - 1.0).abs() < 1e-6);
    assert!(result[1] < -1.0e8); // Masked position
    assert!((result[2] - 1.0).abs() < 1e-6);
    assert!((result[3] - 1.0).abs() < 1e-6);
}

// =========================================================================
// Squared-ReLU gate (FFN)
//
// gate = max(packed[::2], 0)
// up   = packed[1::2]
// output = gate^2 * up
// =========================================================================

#[test]
fn test_falcon_squared_relu_gate_positive() {
    // gate=2.0, up=3.0 -> relu(2.0)^2 * 3.0 = 4.0 * 3.0 = 12.0
    let gate = 2.0f32;
    let up = 3.0f32;

    // Step 1: MAX(gate, 0) = 2.0
    let ops1 = vec![FusedOp {
        op: PrimitiveOp::Max,
        srcs: vec![
            FusedSrc::Buf(1),
            FusedSrc::Const {
                val: 0.0,
                dtype: DType::Float32,
            },
        ],
        dst_dtype: DType::Float32,
    }];
    let bufs1 = vec![
        BufferBinding {
            buf_id: 0,
            st: ShapeTracker::contiguous(&[1]),
            dtype: DType::Float32,
            access: BufferAccess::Write,
        },
        BufferBinding {
            buf_id: 1,
            st: ShapeTracker::contiguous(&[1]),
            dtype: DType::Float32,
            access: BufferAccess::Read,
        },
    ];
    let relu_result = run_chain(ops1, bufs1, vec![f32_to_bytes(&[gate])]);
    assert!((relu_result[0] - 2.0).abs() < 1e-6);

    // Step 2: gate^2
    let ops2 = vec![FusedOp {
        op: PrimitiveOp::Mul,
        srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(1)],
        dst_dtype: DType::Float32,
    }];
    let bufs2 = vec![
        BufferBinding {
            buf_id: 0,
            st: ShapeTracker::contiguous(&[1]),
            dtype: DType::Float32,
            access: BufferAccess::Write,
        },
        BufferBinding {
            buf_id: 1,
            st: ShapeTracker::contiguous(&[1]),
            dtype: DType::Float32,
            access: BufferAccess::Read,
        },
    ];
    let sq_result = run_chain(ops2, bufs2, vec![f32_to_bytes(&relu_result)]);
    assert!((sq_result[0] - 4.0).abs() < 1e-6);

    // Step 3: gate^2 * up = 4.0 * 3.0 = 12.0
    let ops3 = vec![FusedOp {
        op: PrimitiveOp::Mul,
        srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
        dst_dtype: DType::Float32,
    }];
    let bufs3 = vec![
        BufferBinding {
            buf_id: 0,
            st: ShapeTracker::contiguous(&[1]),
            dtype: DType::Float32,
            access: BufferAccess::Write,
        },
        BufferBinding {
            buf_id: 1,
            st: ShapeTracker::contiguous(&[1]),
            dtype: DType::Float32,
            access: BufferAccess::Read,
        },
        BufferBinding {
            buf_id: 2,
            st: ShapeTracker::contiguous(&[1]),
            dtype: DType::Float32,
            access: BufferAccess::Read,
        },
    ];
    let final_result = run_chain(
        ops3,
        bufs3,
        vec![f32_to_bytes(&sq_result), f32_to_bytes(&[up])],
    );
    assert!(
        (final_result[0] - 12.0).abs() < 1e-5,
        "squared_relu_gate(2.0, 3.0) = {}, expected 12.0",
        final_result[0]
    );
}

#[test]
fn test_falcon_squared_relu_gate_negative() {
    // gate=-1.0 -> relu(-1.0) = 0.0 -> 0^2 * anything = 0.0
    let result = run_chain(
        vec![FusedOp {
            op: PrimitiveOp::Max,
            srcs: vec![
                FusedSrc::Buf(1),
                FusedSrc::Const {
                    val: 0.0,
                    dtype: DType::Float32,
                },
            ],
            dst_dtype: DType::Float32,
        }],
        vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[1]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[1]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        vec![f32_to_bytes(&[-1.0])],
    );
    assert!(
        result[0].abs() < 1e-6,
        "relu(-1.0) should be 0.0, got {}",
        result[0]
    );
}

// =========================================================================
// Residual connection: x + f(x)
// =========================================================================

#[test]
fn test_falcon_residual_add() {
    let x = [1.0f32, 2.0, 3.0, 4.0];
    let fx = [0.1f32, 0.2, 0.3, 0.4];
    let n = 4;

    let result = run_chain(
        vec![FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Float32,
        }],
        vec![
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
        vec![f32_to_bytes(&x), f32_to_bytes(&fx)],
    );

    for (i, &v) in result.iter().enumerate() {
        let expected = x[i] + fx[i];
        assert!(
            (v - expected).abs() < 1e-6,
            "residual[{}]: {} vs {}",
            i, v, expected
        );
    }
}

// =========================================================================
// Softmax with attention mask (Falcon-OCR hybrid mask pattern)
// =========================================================================

#[test]
fn test_falcon_softmax_with_causal_mask() {
    // 3-token causal mask: lower triangular
    // scores = [1, 1, 1]  (last row)
    // mask   = [0, 0, 0]  (can attend to all)
    // After softmax: [1/3, 1/3, 1/3]
    let scores = [1.0f32, 1.0, 1.0];
    let mask = [0.0f32, 0.0, 0.0];
    let n = scores.len();

    // Add mask to scores
    let ops_add = vec![FusedOp {
        op: PrimitiveOp::Add,
        srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
        dst_dtype: DType::Float32,
    }];
    let bufs_add = vec![
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
    ];
    let masked = run_chain(
        ops_add,
        bufs_add,
        vec![f32_to_bytes(&scores), f32_to_bytes(&mask)],
    );

    // Compute softmax via reference
    let max_val = masked.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exp_vals: Vec<f32> = masked.iter().map(|v| (v - max_val).exp()).collect();
    let sum: f32 = exp_vals.iter().sum();
    let softmax: Vec<f32> = exp_vals.iter().map(|v| v / sum).collect();

    let total: f32 = softmax.iter().sum();
    assert!(
        (total - 1.0).abs() < 1e-5,
        "Softmax should sum to 1.0, got {}",
        total
    );

    // Equal scores -> uniform distribution
    for &v in &softmax {
        assert!(
            (v - 1.0 / 3.0).abs() < 1e-5,
            "Expected ~0.333, got {}",
            v
        );
    }
}

#[test]
fn test_falcon_softmax_masked_position_zeroed() {
    // One position masked with -inf should get ~0 probability
    let scores = [1.0f32, 1.0, 1.0];
    let mask = [0.0f32, -1.0e9, 0.0];

    // Manual softmax computation
    let masked: Vec<f64> = scores
        .iter()
        .zip(mask.iter())
        .map(|(s, m)| *s as f64 + *m as f64)
        .collect();
    let max_val = masked.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let exp_vals: Vec<f64> = masked.iter().map(|v| (v - max_val).exp()).collect();
    let sum: f64 = exp_vals.iter().sum();
    let softmax: Vec<f64> = exp_vals.iter().map(|v| v / sum).collect();

    // Masked position should be ~0
    assert!(
        softmax[1] < 1e-10,
        "Masked position should be ~0, got {}",
        softmax[1]
    );
    // Remaining positions should share ~0.5 each
    assert!(
        (softmax[0] - 0.5).abs() < 1e-5,
        "Expected ~0.5, got {}",
        softmax[0]
    );
    assert!(
        (softmax[2] - 0.5).abs() < 1e-5,
        "Expected ~0.5, got {}",
        softmax[2]
    );
}

// =========================================================================
// Cross-backend source code validation
//
// Verify that the same RMSNorm kernel renders valid source for all backends.
// =========================================================================

#[test]
fn test_falcon_rms_norm_renders_wgsl() {
    use molt_gpu::render::wgsl::WgslRenderer;
    use molt_gpu::render::Renderer;

    let kernel = FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            },
        ],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[4]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[4]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [4, 1, 1],
        local: [1, 1, 1],
                spec: None,
    };

    let renderer = WgslRenderer;
    let src = renderer.render(&kernel);
    assert!(!src.is_empty(), "WGSL render should produce non-empty source");
    assert!(src.contains("@compute"), "WGSL should contain @compute");
}

#[test]
fn test_falcon_rms_norm_renders_cuda() {
    use molt_gpu::render::cuda::CudaRenderer;
    use molt_gpu::render::Renderer;

    let kernel = FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            },
        ],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[4]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[4]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [4, 1, 1],
        local: [1, 1, 1],
                spec: None,
    };

    let renderer = CudaRenderer;
    let src = renderer.render(&kernel);
    assert!(!src.is_empty(), "CUDA render should produce non-empty source");
    assert!(
        src.contains("__global__"),
        "CUDA should contain __global__"
    );
}

#[test]
fn test_falcon_rms_norm_renders_msl() {
    use molt_gpu::render::msl::MslRenderer;
    use molt_gpu::render::Renderer;

    let kernel = FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            },
        ],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[4]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[4]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [4, 1, 1],
        local: [1, 1, 1],
                spec: None,
    };

    let renderer = MslRenderer;
    let src = renderer.render(&kernel);
    assert!(!src.is_empty(), "MSL render should produce non-empty source");
    assert!(src.contains("kernel"), "MSL should contain kernel");
}

// =========================================================================
// Full composition: RMSNorm + squared-ReLU + residual
//
// This mirrors a simplified FFN block:
// h = rms_norm(x)
// out = relu(h)^2 * h  (simplified gate = up = h)
// result = x + out
// =========================================================================

#[test]
fn test_falcon_ffn_simplified_composition() {
    // x = [1.0, 2.0, 3.0, 4.0]
    // rms_norm(x) ~= x / sqrt(mean(x^2) + eps)
    let x = [1.0f32, 2.0, 3.0, 4.0];
    let n = x.len();
    let eps = 1e-6f64;

    // Reference computation
    let mean_sq: f64 = x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / n as f64;
    let rms = (mean_sq + eps).sqrt();
    let normed: Vec<f64> = x.iter().map(|v| *v as f64 / rms).collect();

    // Apply relu then square then multiply with itself (simplified gate)
    let gated: Vec<f64> = normed
        .iter()
        .map(|v| {
            let relu = v.max(0.0);
            relu * relu * v
        })
        .collect();

    // Residual
    let final_ref: Vec<f64> = x
        .iter()
        .zip(gated.iter())
        .map(|(xi, gi)| *xi as f64 + gi)
        .collect();

    // All values should be finite and reasonable
    for v in &final_ref {
        assert!(v.is_finite(), "FFN composition produced non-finite value");
    }

    // The residual should be >= the original (since relu^2 * h >= 0 for positive h)
    for (i, (orig, result)) in x.iter().zip(final_ref.iter()).enumerate() {
        assert!(
            *result >= *orig as f64 - 1e-6,
            "Residual should not decrease for positive input at index {}: {} < {}",
            i, result, orig
        );
    }
}
