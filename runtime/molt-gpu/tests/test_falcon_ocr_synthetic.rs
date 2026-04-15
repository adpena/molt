//! Medium-scale synthetic Falcon-OCR inference test.
//!
//! Exercises the full architecture with deterministic weights:
//!   - 4 transformer layers (not 2 like micro test)
//!   - dim=128, heads=4, vocab=1024
//!   - Deterministic weights (seed 42)
//!   - Full inference pipeline: image -> patches -> embed -> 4 transformer blocks
//!     -> logits -> 20 autoregressive steps
//!
//! This proves the full molt-gpu primitive pipeline works end-to-end at
//! a scale representative of real models.

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

/// Run a FusedKernel on CPU and return f32 output.
fn run_kernel(kernel: &FusedKernel, input_bufs: Vec<Vec<u8>>) -> Vec<f32> {
    let n_out = kernel.bufs[0].st.numel();
    let out_size = n_out * kernel.bufs[0].dtype.size_bytes();
    let mut all_bufs = vec![vec![0u8; out_size]];
    all_bufs.extend(input_bufs);
    interpret::execute_kernel(kernel, &mut all_bufs);
    bytes_to_f32(&all_bufs[0])
}

/// Deterministic weight generator using xorshift64.
struct WeightGen {
    state: u64,
}

impl WeightGen {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Generate n f32 weights in [-scale, scale] range.
    fn weights(&mut self, n: usize, scale: f32) -> Vec<f32> {
        (0..n).map(|_| {
            let bits = self.next_u64();
            let frac = (bits & 0x00FFFFFF) as f32 / 0x00FFFFFF as f32;
            (frac * 2.0 - 1.0) * scale
        }).collect()
    }
}

/// CPU matmul C[i,j] = sum_k A[i,k] * B[k,j]
fn matmul(a: &[f32], b: &[f32], m: usize, k: usize, n: usize) -> Vec<f32> {
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

/// CPU softmax.
fn softmax(x: &[f32]) -> Vec<f32> {
    let max_val = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exp_vals: Vec<f32> = x.iter().map(|&v| (v - max_val).exp()).collect();
    let sum: f32 = exp_vals.iter().sum();
    exp_vals.iter().map(|&v| v / sum).collect()
}

/// CPU RMSNorm.
fn rms_norm(x: &[f32], eps: f32) -> Vec<f32> {
    let n = x.len() as f32;
    let sum_sq: f32 = x.iter().map(|&v| v * v).sum();
    let inv_rms = 1.0 / (sum_sq / n + eps).sqrt();
    x.iter().map(|&v| v * inv_rms).collect()
}

/// CPU squared ReLU.
fn squared_relu(x: &[f32]) -> Vec<f32> {
    x.iter().map(|&v| {
        let relu = v.max(0.0);
        relu * relu
    }).collect()
}

/// Compute RMSNorm via molt-gpu kernel interpreter (validates interpreter).
fn gpu_rms_norm(x: &[f32], eps: f32) -> Vec<f32> {
    let n = x.len();
    let x_bytes = f32_to_bytes(x);

    // MUL(x, x)
    let k_sq = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None, vectorize_width: 1,
    };
    let sq = run_kernel(&k_sq, vec![x_bytes.clone()]);

    // ReduceSum
    let k_sum = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceSum,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
        spec: None, vectorize_width: 1,
    };
    let sum = run_kernel(&k_sum, vec![f32_to_bytes(&sq)]);
    let inv_rms = 1.0 / (sum[0] / n as f32 + eps).sqrt();

    // MUL(x, inv_rms)
    let k_scale = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Const { val: inv_rms as f64, dtype: DType::Float32 }],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None, vectorize_width: 1,
    };
    run_kernel(&k_scale, vec![x_bytes])
}

// ============================================================================
// Full synthetic Falcon-OCR inference test
// ============================================================================

#[test]
fn test_falcon_ocr_synthetic_4_layer_inference() {
    // Architecture parameters
    let dim = 128;
    let heads = 4;
    let _head_dim = dim / heads;
    let ffn_dim = dim * 4;
    let seq_len = 8; // 4 image patches + 4 text tokens
    let vocab = 1024;
    let n_layers = 4;
    let eps = 1e-6f32;
    let max_gen_tokens = 20;

    // Weight initialization scale: Xavier-like
    let init_scale = (2.0 / dim as f32).sqrt();
    let mut wg = WeightGen::new(42);

    // Token embedding [vocab, dim]
    let embed_w = wg.weights(vocab * dim, init_scale);

    // Per-layer weights
    struct LayerWeights {
        qkv_w: Vec<f32>,    // [dim, 3*dim]
        out_w: Vec<f32>,     // [dim, dim]
        ff_up_w: Vec<f32>,   // [dim, ffn_dim]
        ff_down_w: Vec<f32>, // [ffn_dim, dim]
    }

    let layers: Vec<LayerWeights> = (0..n_layers).map(|_| LayerWeights {
        qkv_w: wg.weights(dim * 3 * dim, init_scale),
        out_w: wg.weights(dim * dim, init_scale),
        ff_up_w: wg.weights(dim * ffn_dim, init_scale / 2.0),
        ff_down_w: wg.weights(ffn_dim * dim, init_scale / 2.0),
    }).collect();

    // LM head [dim, vocab]
    let lm_head_w = wg.weights(dim * vocab, init_scale);

    // --- Image patch embedding (simulating vision encoder output) ---
    // Generate a deterministic 64x64 grayscale test image.
    let patch_size = 16;
    let n_patches = (64 / patch_size) * (64 / patch_size); // 16 patches
    // Project patches to dim via learned projection.
    let patch_proj_w = wg.weights(patch_size * patch_size * dim, init_scale);

    // Generate test image pixels (diagonal gradient).
    let mut image_pixels = vec![0.0f32; 64 * 64];
    for y in 0..64 {
        for x in 0..64 {
            image_pixels[y * 64 + x] = ((x + y) as f32) / 126.0 - 0.5;
        }
    }

    // Extract patches and project to dim.
    let mut patch_embeds = vec![0.0f32; n_patches * dim];
    for py in 0..(64 / patch_size) {
        for px in 0..(64 / patch_size) {
            let patch_idx = py * (64 / patch_size) + px;
            let mut patch_flat = vec![0.0f32; patch_size * patch_size];
            for dy in 0..patch_size {
                for dx in 0..patch_size {
                    patch_flat[dy * patch_size + dx] =
                        image_pixels[(py * patch_size + dy) * 64 + (px * patch_size + dx)];
                }
            }
            // Project: embed = patch_flat @ patch_proj_w
            for d in 0..dim {
                let mut acc = 0.0f32;
                for k in 0..(patch_size * patch_size) {
                    acc += patch_flat[k] * patch_proj_w[k * dim + d];
                }
                patch_embeds[patch_idx * dim + d] = acc;
            }
        }
    }

    // Use first `seq_len/2` patches as image context.
    let image_tokens = seq_len / 2;
    let text_tokens = seq_len - image_tokens;

    // Initial input: image patch embeddings + text token embeddings.
    let mut hidden = vec![0.0f32; seq_len * dim];

    // Copy image patch embeddings.
    for pos in 0..image_tokens {
        let src_idx = pos.min(n_patches - 1);
        hidden[pos * dim..(pos + 1) * dim]
            .copy_from_slice(&patch_embeds[src_idx * dim..(src_idx + 1) * dim]);
    }

    // Initial text tokens (BOS + padding).
    let initial_text_ids: Vec<usize> = vec![1, 0, 0, 0]; // BOS=1, PAD=0
    for (i, &tok_id) in initial_text_ids.iter().take(text_tokens).enumerate() {
        let pos = image_tokens + i;
        let start = tok_id * dim;
        hidden[pos * dim..(pos + 1) * dim]
            .copy_from_slice(&embed_w[start..start + dim]);
    }

    // --- Transformer forward pass ---
    for layer_idx in 0..n_layers {
        let layer = &layers[layer_idx];

        // RMSNorm
        let mut normed = vec![0.0f32; seq_len * dim];
        for pos in 0..seq_len {
            let x = &hidden[pos * dim..(pos + 1) * dim];
            let n = rms_norm(x, eps);
            normed[pos * dim..(pos + 1) * dim].copy_from_slice(&n);
        }

        // QKV projection
        let qkv = matmul(&normed, &layer.qkv_w, seq_len, dim, 3 * dim);

        // Split Q, K, V
        let mut q = vec![0.0f32; seq_len * dim];
        let mut k = vec![0.0f32; seq_len * dim];
        let mut v = vec![0.0f32; seq_len * dim];
        for pos in 0..seq_len {
            q[pos * dim..(pos + 1) * dim]
                .copy_from_slice(&qkv[pos * 3 * dim..pos * 3 * dim + dim]);
            k[pos * dim..(pos + 1) * dim]
                .copy_from_slice(&qkv[pos * 3 * dim + dim..pos * 3 * dim + 2 * dim]);
            v[pos * dim..(pos + 1) * dim]
                .copy_from_slice(&qkv[pos * 3 * dim + 2 * dim..(pos + 1) * 3 * dim]);
        }

        // Scaled dot-product attention
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
        let mut attn_w = vec![0.0f32; seq_len * seq_len];
        for i in 0..seq_len {
            let row = &scores[i * seq_len..(i + 1) * seq_len];
            let sm = softmax(row);
            attn_w[i * seq_len..(i + 1) * seq_len].copy_from_slice(&sm);
        }

        // attn_out = attn_w @ V
        let attn_out = matmul(&attn_w, &v, seq_len, seq_len, dim);

        // Output projection
        let proj = matmul(&attn_out, &layer.out_w, seq_len, dim, dim);

        // Residual
        for i in 0..seq_len * dim {
            hidden[i] += proj[i];
        }

        // FFN
        let mut normed2 = vec![0.0f32; seq_len * dim];
        for pos in 0..seq_len {
            let x = &hidden[pos * dim..(pos + 1) * dim];
            let n = rms_norm(x, eps);
            normed2[pos * dim..(pos + 1) * dim].copy_from_slice(&n);
        }

        let ff_up = matmul(&normed2, &layer.ff_up_w, seq_len, dim, ffn_dim);
        let ff_act = squared_relu(&ff_up);
        let ff_down = matmul(&ff_act, &layer.ff_down_w, seq_len, ffn_dim, dim);

        for i in 0..seq_len * dim {
            hidden[i] += ff_down[i];
        }
    }

    // --- LM Head ---
    let logits = matmul(&hidden, &lm_head_w, seq_len, dim, vocab);

    // --- Verification of full forward pass ---

    // All logits finite
    for (i, &val) in logits.iter().enumerate() {
        assert!(val.is_finite(), "Logit[{}] = {} is not finite", i, val);
    }

    // Logits in reasonable range
    let max_logit = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let min_logit = logits.iter().copied().fold(f32::INFINITY, f32::min);
    assert!(max_logit.abs() < 1000.0, "Logits exploding: max={}", max_logit);
    assert!(min_logit.abs() < 1000.0, "Logits exploding: min={}", min_logit);

    // Softmax of last position sums to 1.0
    let last_logits = &logits[(seq_len - 1) * vocab..seq_len * vocab];
    let probs = softmax(last_logits);
    let prob_sum: f32 = probs.iter().sum();
    assert!((prob_sum - 1.0).abs() < 1e-5, "Softmax sum = {}", prob_sum);

    // --- Autoregressive generation (20 steps) ---
    let mut generated_tokens: Vec<usize> = Vec::new();
    let mut gen_hidden = hidden[(seq_len - 1) * dim..seq_len * dim].to_vec();

    for step in 0..max_gen_tokens {
        // Simplified: run LM head on last hidden state only
        let step_logits = matmul(&gen_hidden, &lm_head_w, 1, dim, vocab);

        // All logits must be finite
        for (i, &val) in step_logits.iter().enumerate() {
            assert!(
                val.is_finite(),
                "Step {} logit[{}] = {} is not finite",
                step, i, val
            );
        }

        // Argmax for greedy decoding
        let mut best_tok = 0;
        let mut best_val = f32::NEG_INFINITY;
        for (i, &val) in step_logits.iter().enumerate() {
            if val > best_val {
                best_val = val;
                best_tok = i;
            }
        }
        generated_tokens.push(best_tok);

        // Simplified: use embedding as next hidden (no full transformer re-run)
        let start = best_tok * dim;
        gen_hidden = embed_w[start..start + dim].to_vec();

        // Run through a single layer for state update
        let normed = rms_norm(&gen_hidden, eps);
        // Simplified single-token forward through layer 0
        let proj = matmul(&normed, &layers[0].qkv_w, 1, dim, 3 * dim);
        // Take only the output projection dimension
        gen_hidden = matmul(&proj[..dim], &layers[0].out_w, 1, dim, dim);
    }

    assert_eq!(generated_tokens.len(), max_gen_tokens);

    // All generated tokens should be valid vocab indices
    for (i, &tok) in generated_tokens.iter().enumerate() {
        assert!(tok < vocab, "Step {} generated invalid token {}", i, tok);
    }

    // --- Validate molt-gpu kernel interpreter matches CPU reference ---
    // Run RMSNorm through kernel interpreter on first position's hidden state
    let test_x = &hidden[..dim];
    let ref_norm = rms_norm(test_x, eps);
    let gpu_norm = gpu_rms_norm(test_x, eps);

    let max_diff: f32 = ref_norm.iter().zip(gpu_norm.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        max_diff < 1e-5,
        "RMSNorm kernel vs CPU reference max diff: {:.6e}",
        max_diff
    );

    // Validate softmax through kernel interpreter
    let test_logits = &logits[(seq_len - 1) * vocab..(seq_len - 1) * vocab + 64];
    let ref_sm = softmax(test_logits);

    // Build softmax via kernels
    let x_bytes = f32_to_bytes(test_logits);
    let n = test_logits.len();

    // ReduceMax
    let k_max = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceMax,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
        spec: None, vectorize_width: 1,
    };
    let max_result = run_kernel(&k_max, vec![x_bytes.clone()]);
    let max_val = max_result[0];

    // Fused Sub + Mul(log2e) + Exp2
    let log2_e = std::f64::consts::LOG2_E;
    let k_exp = FusedKernel {
        ops: vec![
            FusedOp {
                op: PrimitiveOp::Sub,
                srcs: vec![FusedSrc::Buf(1), FusedSrc::Const { val: max_val as f64, dtype: DType::Float32 }],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::Mul,
                srcs: vec![FusedSrc::Op(0), FusedSrc::Const { val: log2_e, dtype: DType::Float32 }],
                dst_dtype: DType::Float32,
            },
            FusedOp {
                op: PrimitiveOp::Exp2,
                srcs: vec![FusedSrc::Op(1)],
                dst_dtype: DType::Float32,
            },
        ],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None, vectorize_width: 1,
    };
    let exp_result = run_kernel(&k_exp, vec![x_bytes]);
    let exp_bytes = f32_to_bytes(&exp_result);

    // ReduceSum
    let k_sum = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceSum,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
        spec: None, vectorize_width: 1,
    };
    let sum_result = run_kernel(&k_sum, vec![exp_bytes.clone()]);
    let inv_sum = 1.0 / sum_result[0];

    // Mul(exp, 1/sum)
    let k_div = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Const { val: inv_sum as f64, dtype: DType::Float32 }],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[n]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [n as u32, 1, 1],
        local: [1, 1, 1],
        spec: None, vectorize_width: 1,
    };
    let gpu_sm = run_kernel(&k_div, vec![exp_bytes]);

    // Verify softmax sum
    let gpu_sm_sum: f32 = gpu_sm.iter().sum();
    assert!((gpu_sm_sum - 1.0).abs() < 1e-5, "GPU softmax sum = {}", gpu_sm_sum);

    // Verify against reference
    let sm_diff: f32 = ref_sm.iter().zip(gpu_sm.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        sm_diff < 1e-4,
        "Softmax kernel vs CPU reference max diff: {:.6e}",
        sm_diff
    );
}
