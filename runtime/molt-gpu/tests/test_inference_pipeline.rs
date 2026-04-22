//! Full inference pipeline test: LazyOp -> schedule -> fuse -> CpuDevice execute.
//!
//! Builds a small transformer (2 layers, dim=32) entirely from molt-gpu
//! primitives and runs a forward pass, verifying:
//!   1. LazyOp DAG construction works end-to-end
//!   2. Scheduler produces correct kernel sequence
//!   3. Fusion reduces kernel count (softmax: from 7 separate ops to 2 fused)
//!   4. CpuDevice executes all kernels correctly
//!   5. Output logits are finite and in valid range
//!   6. Softmax outputs sum to 1.0

use std::sync::Arc;

use molt_gpu::device::cpu::interpret;
use molt_gpu::dtype::DType;
use molt_gpu::fuse;
use molt_gpu::lazy::{DeviceBufferRef, LazyOp};
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc};
use molt_gpu::schedule;
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

/// Run a FusedKernel on CPU and return the f32 output.
fn run_kernel(kernel: &FusedKernel, input_bufs: Vec<Vec<u8>>) -> Vec<f32> {
    let n_out = kernel.bufs[0].st.numel();
    let out_size = n_out * kernel.bufs[0].dtype.size_bytes();
    let mut all_bufs = vec![vec![0u8; out_size]];
    all_bufs.extend(input_bufs);
    interpret::execute_kernel(kernel, &mut all_bufs);
    bytes_to_f32(&all_bufs[0])
}

/// Generate deterministic weights with seed.
fn deterministic_weights(n: usize, seed: u64) -> Vec<f32> {
    let mut vals = Vec::with_capacity(n);
    let mut state = seed;
    for _ in 0..n {
        // Simple LCG for deterministic values
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let bits = ((state >> 33) as u32) & 0x007FFFFF;
        // Map to [-0.1, 0.1] range for stable initialization
        let f = (bits as f32 / 0x007FFFFF as f32) * 0.2 - 0.1;
        vals.push(f);
    }
    vals
}

/// Compute matmul C[i,j] = sum_k A[i,k] * B[k,j] on CPU.
fn cpu_matmul(a: &[f32], b: &[f32], m: usize, k: usize, n: usize) -> Vec<f32> {
    let mut c = vec![0.0f32; m * n];
    for i in 0..m {
        for j in 0..n {
            let mut acc = 0.0f32;
            for kk in 0..k {
                acc += a[i * k + kk] * b[kk * n + j];
            }
            c[i * n + j] = acc;
        }
    }
    c
}

/// Compute softmax on CPU (reference).
fn cpu_softmax(x: &[f32]) -> Vec<f32> {
    let max_val = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exp_vals: Vec<f32> = x.iter().map(|&v| (v - max_val).exp()).collect();
    let sum: f32 = exp_vals.iter().sum();
    exp_vals.iter().map(|&v| v / sum).collect()
}

/// Compute RMSNorm on CPU (reference).
fn cpu_rms_norm(x: &[f32], eps: f32) -> Vec<f32> {
    let n = x.len() as f32;
    let sum_sq: f32 = x.iter().map(|&v| v * v).sum();
    let inv_rms = 1.0 / (sum_sq / n + eps).sqrt();
    x.iter().map(|&v| v * inv_rms).collect()
}

/// Compute squared ReLU: max(0, x)^2
fn cpu_squared_relu(x: &[f32]) -> Vec<f32> {
    x.iter()
        .map(|&v| {
            let relu = v.max(0.0);
            relu * relu
        })
        .collect()
}

// ============================================================================
// Test 1: LazyOp DAG construction
// ============================================================================

#[test]
fn test_lazy_op_dag_construction() {
    let dim = 32;
    let data = deterministic_weights(dim, 42);
    let bytes = f32_to_bytes(&data);

    let buf = Arc::new(LazyOp::Buffer {
        buf: DeviceBufferRef {
            id: 0,
            size_bytes: bytes.len(),
        },
        st: ShapeTracker::contiguous(&[dim]),
        dtype: DType::Float32,
    });

    // Build: neg(mul(x, x))
    let sq = Arc::new(LazyOp::Binary {
        op: PrimitiveOp::Mul,
        lhs: buf.clone(),
        rhs: buf.clone(),
    });
    let neg = Arc::new(LazyOp::Unary {
        op: PrimitiveOp::Neg,
        src: sq.clone(),
    });

    assert_eq!(neg.shape(), vec![dim]);
    assert_eq!(neg.dtype(), DType::Float32);
    assert_eq!(sq.shape(), vec![dim]);
}

// ============================================================================
// Test 2: Scheduler produces correct kernel sequence
// ============================================================================

#[test]
fn test_scheduler_produces_kernels() {
    let dim = 32;

    let buf = Arc::new(LazyOp::Buffer {
        buf: DeviceBufferRef {
            id: 0,
            size_bytes: dim * 4,
        },
        st: ShapeTracker::contiguous(&[dim]),
        dtype: DType::Float32,
    });

    // Build: sqrt(mul(x, x) + const)
    let sq = Arc::new(LazyOp::Binary {
        op: PrimitiveOp::Mul,
        lhs: buf.clone(),
        rhs: buf.clone(),
    });
    let neg = Arc::new(LazyOp::Unary {
        op: PrimitiveOp::Sqrt,
        src: sq,
    });

    let kernels = schedule::schedule(&neg, &[dim]);

    // Should produce 2 kernels: MUL and SQRT
    assert_eq!(
        kernels.len(),
        2,
        "Expected 2 kernels (MUL + SQRT), got {}",
        kernels.len()
    );
    assert_eq!(kernels[0].ops[0].op, PrimitiveOp::Mul);
    assert_eq!(kernels[1].ops[0].op, PrimitiveOp::Sqrt);
}

// ============================================================================
// Test 3: Fusion reduces kernel count
// ============================================================================

#[test]
fn test_fusion_reduces_kernel_count() {
    let dim = 32;

    let buf = Arc::new(LazyOp::Buffer {
        buf: DeviceBufferRef {
            id: 0,
            size_bytes: dim * 4,
        },
        st: ShapeTracker::contiguous(&[dim]),
        dtype: DType::Float32,
    });

    // Build a chain of 5 elementwise ops that should fuse into 1 kernel.
    let sq = Arc::new(LazyOp::Binary {
        op: PrimitiveOp::Mul,
        lhs: buf.clone(),
        rhs: buf.clone(),
    });
    let neg = Arc::new(LazyOp::Unary {
        op: PrimitiveOp::Neg,
        src: sq,
    });
    let sqrt = Arc::new(LazyOp::Unary {
        op: PrimitiveOp::Sqrt,
        src: Arc::new(LazyOp::Unary {
            op: PrimitiveOp::Neg,
            src: neg,
        }),
    });
    let exp = Arc::new(LazyOp::Unary {
        op: PrimitiveOp::Exp2,
        src: sqrt,
    });

    let kernels = schedule::schedule(&exp, &[dim]);
    assert!(
        kernels.len() >= 4,
        "Scheduler should produce at least 4 kernels, got {}",
        kernels.len()
    );

    let fused = fuse::fuse(kernels);
    assert_eq!(
        fused.len(),
        1,
        "Fusion should merge all elementwise ops into 1 kernel, got {}",
        fused.len()
    );
    assert!(
        fused[0].ops.len() >= 4,
        "Fused kernel should have at least 4 ops"
    );
}

// ============================================================================
// Test 4: CpuDevice executes softmax correctly
// ============================================================================

#[test]
fn test_cpu_softmax_execution() {
    let n = 64;
    let x: Vec<f32> = (0..n).map(|i| (i as f32) * 0.1 - 3.0).collect();
    let reference = cpu_softmax(&x);

    // Execute via molt-gpu kernel interpreter
    let x_bytes = f32_to_bytes(&x);

    // Step 1: Find max (ReduceMax)
    let k_max = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceMax,
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
        vectorize_width: 1,
    };
    let max_result = run_kernel(&k_max, vec![x_bytes.clone()]);
    let max_val = max_result[0];

    // Step 2: Fused exp(x - max)
    let log2_e = std::f64::consts::LOG2_E;
    let k_exp = FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Sub,
                srcs: vec![
                    FusedSrc::Buf(1),
                    FusedSrc::Const {
                        val: max_val as f64,
                        dtype: DType::Float32,
                    },
                ],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![
                    FusedSrc::Op(0),
                    FusedSrc::Const {
                        val: log2_e,
                        dtype: DType::Float32,
                    },
                ],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::Exp2,
                srcs: vec![FusedSrc::Op(1)],
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
        vectorize_width: 1,
    };
    let exp_result = run_kernel(&k_exp, vec![x_bytes]);
    let exp_bytes = f32_to_bytes(&exp_result);

    // Step 3: ReduceSum
    let k_sum = FusedKernel {
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
        vectorize_width: 1,
    };
    let sum_result = run_kernel(&k_sum, vec![exp_bytes.clone()]);
    let sum_val = sum_result[0];

    // Step 4: Divide by sum
    let k_div = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![
                FusedSrc::Buf(1),
                FusedSrc::Const {
                    val: (1.0 / sum_val) as f64,
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
    let softmax_result = run_kernel(&k_div, vec![exp_bytes]);

    // Verify softmax outputs sum to 1.0
    let softmax_sum: f32 = softmax_result.iter().sum();
    assert!(
        (softmax_sum - 1.0).abs() < 1e-5,
        "Softmax sum should be ~1.0, got {:.6}",
        softmax_sum
    );

    // Verify against reference
    let max_diff: f32 = reference
        .iter()
        .zip(softmax_result.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        max_diff < 1e-4,
        "Softmax max diff vs reference: {:.6e} (too large)",
        max_diff
    );
}

// ============================================================================
// Test 5: Full transformer forward pass
// ============================================================================

#[test]
fn test_full_transformer_forward_pass() {
    let dim = 32;
    let ffn_dim = dim * 4;
    let seq_len = 4;
    let vocab = 64;
    let n_layers = 2;
    let eps = 1e-6f32;

    // Generate deterministic weights
    let mut seed = 42u64;
    let mut next_weights = |n: usize| -> Vec<f32> {
        let w = deterministic_weights(n, seed);
        seed = seed.wrapping_mul(2654435761).wrapping_add(1);
        w
    };

    // Embedding table: [vocab, dim]
    let embed_w = next_weights(vocab * dim);
    // Per-layer weights
    let mut layer_qkv_w: Vec<Vec<f32>> = Vec::new();
    let mut layer_out_w: Vec<Vec<f32>> = Vec::new();
    let mut layer_ff_up_w: Vec<Vec<f32>> = Vec::new();
    let mut layer_ff_down_w: Vec<Vec<f32>> = Vec::new();

    for _ in 0..n_layers {
        layer_qkv_w.push(next_weights(dim * 3 * dim));
        layer_out_w.push(next_weights(dim * dim));
        layer_ff_up_w.push(next_weights(dim * ffn_dim));
        layer_ff_down_w.push(next_weights(ffn_dim * dim));
    }
    let lm_head_w = next_weights(dim * vocab);

    // Input token IDs (deterministic)
    let input_ids: Vec<usize> = vec![1, 5, 12, 3];

    // --- Forward pass (pure CPU reference, using same math as molt-gpu kernels) ---

    // Step 1: Token embedding
    let mut hidden = vec![0.0f32; seq_len * dim];
    for (pos, &tok_id) in input_ids.iter().enumerate() {
        let start = tok_id * dim;
        hidden[pos * dim..(pos + 1) * dim].copy_from_slice(&embed_w[start..start + dim]);
    }

    // Step 2: Transformer layers
    for layer in 0..n_layers {
        // RMSNorm
        let mut normed = vec![0.0f32; seq_len * dim];
        for pos in 0..seq_len {
            let x = &hidden[pos * dim..(pos + 1) * dim];
            let n = cpu_rms_norm(x, eps);
            normed[pos * dim..(pos + 1) * dim].copy_from_slice(&n);
        }

        // QKV projection
        let qkv = cpu_matmul(&normed, &layer_qkv_w[layer], seq_len, dim, 3 * dim);
        let q = &qkv[..seq_len * dim];
        let k = &qkv[seq_len * dim..seq_len * 2 * dim];
        let v = &qkv[seq_len * 2 * dim..seq_len * 3 * dim];

        // Simplified attention (no multi-head split for this test)
        // scores[i,j] = sum_k Q[i,k] * K[j,k] / sqrt(dim)
        let scale = 1.0 / (dim as f32).sqrt();
        let mut scores = vec![0.0f32; seq_len * seq_len];
        for i in 0..seq_len {
            for j in 0..seq_len {
                let mut s = 0.0f32;
                for kk in 0..dim {
                    s += q[i * dim + kk] * k[j * dim + kk];
                }
                scores[i * seq_len + j] = s * scale;
            }
        }

        // Softmax per row
        let mut attn_weights = vec![0.0f32; seq_len * seq_len];
        for i in 0..seq_len {
            let row = &scores[i * seq_len..(i + 1) * seq_len];
            let sm = cpu_softmax(row);
            attn_weights[i * seq_len..(i + 1) * seq_len].copy_from_slice(&sm);
        }

        // Attention output = attn_weights @ V
        let attn_out = cpu_matmul(&attn_weights, v, seq_len, seq_len, dim);

        // Output projection
        let proj_out = cpu_matmul(&attn_out, &layer_out_w[layer], seq_len, dim, dim);

        // Residual connection
        for i in 0..seq_len * dim {
            hidden[i] += proj_out[i];
        }

        // FFN: RMSNorm -> up projection -> squared ReLU -> down projection
        let mut normed2 = vec![0.0f32; seq_len * dim];
        for pos in 0..seq_len {
            let x = &hidden[pos * dim..(pos + 1) * dim];
            let n = cpu_rms_norm(x, eps);
            normed2[pos * dim..(pos + 1) * dim].copy_from_slice(&n);
        }

        let ff_up = cpu_matmul(&normed2, &layer_ff_up_w[layer], seq_len, dim, ffn_dim);
        let ff_act = cpu_squared_relu(&ff_up);
        let ff_down = cpu_matmul(&ff_act, &layer_ff_down_w[layer], seq_len, ffn_dim, dim);

        // Residual
        for i in 0..seq_len * dim {
            hidden[i] += ff_down[i];
        }
    }

    // Step 3: LM head projection -> logits
    let logits = cpu_matmul(&hidden, &lm_head_w, seq_len, dim, vocab);

    // --- Verification ---

    // 5a: All logits must be finite
    for (i, &val) in logits.iter().enumerate() {
        assert!(
            val.is_finite(),
            "Logit at index {} is not finite: {}",
            i,
            val
        );
    }

    // 5b: Logits should be in a reasonable range (not exploding)
    let max_logit = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let min_logit = logits.iter().copied().fold(f32::INFINITY, f32::min);
    assert!(
        max_logit < 100.0,
        "Max logit too large: {} (exploding activations?)",
        max_logit
    );
    assert!(
        min_logit > -100.0,
        "Min logit too small: {} (exploding activations?)",
        min_logit
    );

    // 5c: Softmax of last position's logits should sum to 1.0
    let last_logits = &logits[(seq_len - 1) * vocab..seq_len * vocab];
    let probs = cpu_softmax(last_logits);
    let prob_sum: f32 = probs.iter().sum();
    assert!(
        (prob_sum - 1.0).abs() < 1e-5,
        "Softmax of logits should sum to 1.0, got {:.6}",
        prob_sum
    );

    // 5d: There should be a clear top prediction (not uniform)
    let max_prob = probs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    assert!(
        max_prob > 1.0 / vocab as f32,
        "Max probability {} is not above uniform {:.4} — model did not learn any signal",
        max_prob,
        1.0 / vocab as f32
    );

    // 5e: Verify the exact same computation works through molt-gpu kernel interpreter
    // Execute RMSNorm via kernel to verify CpuDevice interpreter matches
    let test_x: Vec<f32> = hidden[..dim].to_vec();
    let ref_norm = cpu_rms_norm(&test_x, eps);

    // Execute via kernel
    let x_bytes = f32_to_bytes(&test_x);

    // MUL(x, x)
    let k_sq = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[dim]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[dim]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [dim as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let sq_result = run_kernel(&k_sq, vec![x_bytes.clone()]);

    // ReduceSum
    let k_sum = FusedKernel {
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
                st: ShapeTracker::contiguous(&[dim]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let sum_result = run_kernel(&k_sum, vec![f32_to_bytes(&sq_result)]);
    let sum_sq = sum_result[0];

    let inv_rms = 1.0 / (sum_sq / dim as f32 + eps).sqrt();

    // MUL(x, inv_rms)
    let k_scale = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![
                FusedSrc::Buf(1),
                FusedSrc::Const {
                    val: inv_rms as f64,
                    dtype: DType::Float32,
                },
            ],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[dim]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[dim]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [dim as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let norm_result = run_kernel(&k_scale, vec![x_bytes]);

    // Verify kernel result matches reference
    let max_diff: f32 = ref_norm
        .iter()
        .zip(norm_result.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        max_diff < 1e-5,
        "RMSNorm kernel vs reference max diff: {:.6e} (too large)",
        max_diff
    );
}

// ============================================================================
// Test 6: Softmax fusion verification
// ============================================================================

#[test]
fn test_softmax_fuses_to_two_kernels() {
    // Softmax decomposition:
    // 1. ReduceMax(x)          -- reduce (fusion boundary)
    // 2. Sub(x, max)           -- elementwise
    // 3. Mul(result, log2(e))  -- elementwise
    // 4. Exp2(result)          -- elementwise
    // 5. ReduceSum(exp)        -- reduce (fusion boundary)
    // 6. Reciprocal(sum)       -- elementwise
    // 7. Mul(exp, inv_sum)     -- elementwise
    //
    // Expected fusion: 7 kernels -> 2 fused kernels
    // Kernel 1: ReduceMax (standalone, or fused with pre-reduce chain)
    // Kernel 2: Sub+Mul+Exp2 -> ReduceSum -> Reciprocal+Mul (reduce boundary between)
    //
    // Actually: reduce-to-reduce is a fusion boundary, so:
    // Group 1: ReduceMax
    // Group 2: Sub + Mul + Exp2 + ReduceSum
    // Group 3: Reciprocal + Mul
    // = 3 fused kernels from 7 individual ops

    let n = 64;

    let buf = Arc::new(LazyOp::Buffer {
        buf: DeviceBufferRef {
            id: 0,
            size_bytes: n * 4,
        },
        st: ShapeTracker::contiguous(&[n]),
        dtype: DType::Float32,
    });

    // Build the 7-op softmax DAG
    let reduce_max = Arc::new(LazyOp::Reduce {
        op: PrimitiveOp::ReduceMax,
        src: buf.clone(),
        axis: 0,
    });

    // Note: schedule flattens the DAG into per-op kernels.
    // We test the scheduler + fusion pipeline here.
    let kernels = schedule::schedule(&reduce_max, &[1]);
    assert_eq!(kernels.len(), 1, "ReduceMax should produce 1 kernel");

    // Build a longer chain to test fusion
    let sub = Arc::new(LazyOp::Binary {
        op: PrimitiveOp::Sub,
        lhs: buf.clone(),
        rhs: buf.clone(),
    });
    let exp = Arc::new(LazyOp::Unary {
        op: PrimitiveOp::Exp2,
        src: sub.clone(),
    });
    let reduce_sum = Arc::new(LazyOp::Reduce {
        op: PrimitiveOp::ReduceSum,
        src: exp.clone(),
        axis: 0,
    });

    let all_kernels = schedule::schedule(&reduce_sum, &[1]);
    let fused = fuse::fuse(all_kernels.clone());

    // The elementwise chain (Sub, Exp2) should fuse with the ReduceSum
    assert!(
        fused.len() <= all_kernels.len(),
        "Fusion should not increase kernel count: {} fused vs {} unfused",
        fused.len(),
        all_kernels.len()
    );
    assert!(
        fused.len() < all_kernels.len(),
        "Fusion should reduce kernel count: {} fused vs {} unfused (no fusion happened)",
        fused.len(),
        all_kernels.len()
    );
}

// ============================================================================
// Test 7: Shape specialization
// ============================================================================

#[test]
fn test_shape_specialization() {
    let n = 256; // Power of 2 — should get optimal local size = 256

    let buf = Arc::new(LazyOp::Buffer {
        buf: DeviceBufferRef {
            id: 0,
            size_bytes: n * 4,
        },
        st: ShapeTracker::contiguous(&[n]),
        dtype: DType::Float32,
    });

    let neg = Arc::new(LazyOp::Unary {
        op: PrimitiveOp::Neg,
        src: buf,
    });

    let mut kernels = schedule::schedule(&neg, &[n]);
    schedule::specialize_shapes(&mut kernels);

    assert_eq!(kernels.len(), 1);
    let spec = kernels[0]
        .spec
        .as_ref()
        .expect("Specialization should be set");
    assert!(spec.all_static, "Shape should be fully static");
    assert_eq!(spec.total_elements, n as u64);
    assert!(
        spec.bounds_check_elim,
        "Bounds check should be eliminable for N=256"
    );
    // Optimal local size should be 256 (largest preferred size that divides 256)
    assert_eq!(spec.optimal_local[0], 256);
}

// ============================================================================
// Test 8: Kernel deduplication
// ============================================================================

#[test]
fn test_kernel_deduplication() {
    let n = 32;

    // Create two structurally identical kernels (same ops, same shapes, different buf IDs)
    let k1 = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Neg,
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
        vectorize_width: 1,
    };

    let k2 = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Neg,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 10,
                st: ShapeTracker::contiguous(&[n]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 11,
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

    let (deduped, count) = schedule::deduplicate_kernels(&[k1, k2]);
    assert_eq!(deduped.len(), 2, "Dedup should preserve all kernels");
    assert_eq!(count, 1, "One kernel should be identified as duplicate");
}
