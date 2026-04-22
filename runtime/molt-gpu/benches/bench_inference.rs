//! Micro-transformer inference benchmark on CpuDevice.
//!
//! Builds a minimal 2-layer transformer (dim=64, heads=4) and measures
//! full forward pass time. Establishes the CPU baseline that GPU backends
//! must beat.
//!
//! Architecture:
//!   - 2 transformer layers, each:
//!     - Multi-head attention (4 heads, dim=64, head_dim=16)
//!     - RMSNorm
//!     - Feed-forward (dim=64 -> 4*dim=256 -> dim=64 with SiLU gate)
//!     - RMSNorm
//!   - Final RMSNorm + linear projection
//!
//! Reports: single pass time, 10-pass amortized time, equivalent tokens/sec.

use std::time::Instant;

use molt_gpu::device::cpu::interpret;

const DIM: usize = 64;
const HEADS: usize = 4;
const HEAD_DIM: usize = DIM / HEADS;
const FF_DIM: usize = DIM * 4;
const SEQ_LEN: usize = 16;
const LAYERS: usize = 2;
const WARMUP: usize = 3;
const MEASURE_SINGLE: usize = 50;
const MEASURE_BATCH: usize = 10;

fn f32_to_bytes(vals: &[f32]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

/// Random-ish deterministic initialization (not cryptographic, just needs variation).
fn pseudo_random_f32(seed: usize, count: usize) -> Vec<f32> {
    let mut vals = Vec::with_capacity(count);
    let mut state = seed as u64 ^ 0xDEADBEEF;
    for _ in 0..count {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let bits = ((state >> 32) as u32) & 0x7FFFFF;
        // Generate values in [-0.1, 0.1] for stable forward passes
        let val = (bits as f32 / 0x7FFFFF as f32) * 0.2 - 0.1;
        vals.push(val);
    }
    vals
}

/// Execute RMSNorm: x * rsqrt(mean(x^2) + eps)
fn rmsnorm(x: &[u8], n: usize) -> Vec<u8> {
    let x_f32 = bytes_to_f32(x);

    // Compute mean(x^2)
    let sq_sum: f32 = x_f32.iter().map(|v| v * v).sum();
    let mean = sq_sum / n as f32;
    let rsqrt = 1.0 / (mean + 1e-6_f32).sqrt();

    // Scale
    let result: Vec<f32> = x_f32.iter().map(|v| v * rsqrt).collect();
    f32_to_bytes(&result)
}

/// Execute matmul using fused_matmul: (seq_len, in_dim) @ (in_dim, out_dim) -> (seq_len, out_dim)
fn matmul(x: &[u8], w: &[u8], seq: usize, in_dim: usize, out_dim: usize) -> Vec<u8> {
    let mut out = vec![0u8; seq * out_dim * 4];
    interpret::fused_matmul(x, w, &mut out, seq, in_dim, out_dim);
    out
}

/// SiLU activation: x * sigmoid(x) = x / (1 + exp(-x))
fn silu(x: &[u8]) -> Vec<u8> {
    let vals = bytes_to_f32(x);
    let result: Vec<f32> = vals.iter().map(|&v| v / (1.0 + (-v).exp())).collect();
    f32_to_bytes(&result)
}

/// Elementwise add
fn add(a: &[u8], b: &[u8]) -> Vec<u8> {
    let a_f32 = bytes_to_f32(a);
    let b_f32 = bytes_to_f32(b);
    let result: Vec<f32> = a_f32.iter().zip(b_f32.iter()).map(|(a, b)| a + b).collect();
    f32_to_bytes(&result)
}

/// Elementwise multiply
fn mul(a: &[u8], b: &[u8]) -> Vec<u8> {
    let a_f32 = bytes_to_f32(a);
    let b_f32 = bytes_to_f32(b);
    let result: Vec<f32> = a_f32.iter().zip(b_f32.iter()).map(|(a, b)| a * b).collect();
    f32_to_bytes(&result)
}

/// Softmax over last dimension (applied per-row for seq_len rows of width head_dim).
fn softmax_rows(x: &[u8], rows: usize, cols: usize) -> Vec<u8> {
    let vals = bytes_to_f32(x);
    let mut result = vec![0.0f32; rows * cols];
    for r in 0..rows {
        let row = &vals[r * cols..(r + 1) * cols];
        let max_val = row.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let exp_vals: Vec<f32> = row.iter().map(|v| (v - max_val).exp()).collect();
        let sum: f32 = exp_vals.iter().sum();
        for (c, ev) in exp_vals.iter().enumerate() {
            result[r * cols + c] = ev / sum;
        }
    }
    f32_to_bytes(&result)
}

/// Transformer weights for one layer.
struct LayerWeights {
    wq: Vec<u8>, // (DIM, DIM)
    wk: Vec<u8>, // (DIM, DIM)
    wv: Vec<u8>, // (DIM, DIM)
    wo: Vec<u8>, // (DIM, DIM)
    w1: Vec<u8>, // (DIM, FF_DIM)  — gate projection
    w2: Vec<u8>, // (FF_DIM, DIM)  — down projection
    w3: Vec<u8>, // (DIM, FF_DIM)  — up projection
}

/// Full model weights.
struct ModelWeights {
    layers: Vec<LayerWeights>,
    output_proj: Vec<u8>, // (DIM, DIM) — final projection
}

fn init_weights() -> ModelWeights {
    let mut layers = Vec::new();
    for l in 0..LAYERS {
        layers.push(LayerWeights {
            wq: f32_to_bytes(&pseudo_random_f32(l * 7, DIM * DIM)),
            wk: f32_to_bytes(&pseudo_random_f32(l * 7 + 1, DIM * DIM)),
            wv: f32_to_bytes(&pseudo_random_f32(l * 7 + 2, DIM * DIM)),
            wo: f32_to_bytes(&pseudo_random_f32(l * 7 + 3, DIM * DIM)),
            w1: f32_to_bytes(&pseudo_random_f32(l * 7 + 4, DIM * FF_DIM)),
            w2: f32_to_bytes(&pseudo_random_f32(l * 7 + 5, FF_DIM * DIM)),
            w3: f32_to_bytes(&pseudo_random_f32(l * 7 + 6, DIM * FF_DIM)),
        });
    }
    ModelWeights {
        layers,
        output_proj: f32_to_bytes(&pseudo_random_f32(100, DIM * DIM)),
    }
}

/// Run one forward pass through the micro-transformer.
/// Input: (SEQ_LEN, DIM) f32 tensor.
/// Output: (SEQ_LEN, DIM) f32 tensor.
fn forward(x: &[u8], weights: &ModelWeights) -> Vec<u8> {
    let mut hidden = x.to_vec();

    for layer in &weights.layers {
        // --- Multi-head self-attention ---
        let normed = rmsnorm(&hidden, SEQ_LEN * DIM);

        // Q, K, V projections: (SEQ_LEN, DIM) @ (DIM, DIM) -> (SEQ_LEN, DIM)
        let q = matmul(&normed, &layer.wq, SEQ_LEN, DIM, DIM);
        let k = matmul(&normed, &layer.wk, SEQ_LEN, DIM, DIM);
        let v = matmul(&normed, &layer.wv, SEQ_LEN, DIM, DIM);

        // Multi-head attention: process each head independently.
        // Q, K, V are (SEQ_LEN, HEADS, HEAD_DIM) but stored as (SEQ_LEN, DIM).
        // For simplicity, we compute attention per head by slicing.
        let q_f32 = bytes_to_f32(&q);
        let k_f32 = bytes_to_f32(&k);
        let v_f32 = bytes_to_f32(&v);

        let mut attn_output = vec![0.0f32; SEQ_LEN * DIM];

        for h in 0..HEADS {
            // Extract head slices: (SEQ_LEN, HEAD_DIM) for Q, K, V
            let mut q_head = vec![0.0f32; SEQ_LEN * HEAD_DIM];
            let mut k_head = vec![0.0f32; SEQ_LEN * HEAD_DIM];
            let mut v_head = vec![0.0f32; SEQ_LEN * HEAD_DIM];

            for s in 0..SEQ_LEN {
                for d in 0..HEAD_DIM {
                    q_head[s * HEAD_DIM + d] = q_f32[s * DIM + h * HEAD_DIM + d];
                    k_head[s * HEAD_DIM + d] = k_f32[s * DIM + h * HEAD_DIM + d];
                    v_head[s * HEAD_DIM + d] = v_f32[s * DIM + h * HEAD_DIM + d];
                }
            }

            // Attention scores: Q @ K^T -> (SEQ_LEN, SEQ_LEN)
            // K^T: (HEAD_DIM, SEQ_LEN), so we transpose K
            let mut k_t = vec![0.0f32; HEAD_DIM * SEQ_LEN];
            for s in 0..SEQ_LEN {
                for d in 0..HEAD_DIM {
                    k_t[d * SEQ_LEN + s] = k_head[s * HEAD_DIM + d];
                }
            }

            let q_bytes = f32_to_bytes(&q_head);
            let kt_bytes = f32_to_bytes(&k_t);
            let scores_bytes = matmul(&q_bytes, &kt_bytes, SEQ_LEN, HEAD_DIM, SEQ_LEN);

            // Scale by 1/sqrt(HEAD_DIM)
            let scale = 1.0 / (HEAD_DIM as f32).sqrt();
            let scores_f32 = bytes_to_f32(&scores_bytes);
            let scaled: Vec<f32> = scores_f32.iter().map(|v| v * scale).collect();
            let scaled_bytes = f32_to_bytes(&scaled);

            // Softmax over last dimension
            let probs = softmax_rows(&scaled_bytes, SEQ_LEN, SEQ_LEN);

            // Attention output: probs @ V -> (SEQ_LEN, HEAD_DIM)
            let v_bytes = f32_to_bytes(&v_head);
            let head_out = matmul(&probs, &v_bytes, SEQ_LEN, SEQ_LEN, HEAD_DIM);
            let head_out_f32 = bytes_to_f32(&head_out);

            // Write head output back into attn_output
            for s in 0..SEQ_LEN {
                for d in 0..HEAD_DIM {
                    attn_output[s * DIM + h * HEAD_DIM + d] = head_out_f32[s * HEAD_DIM + d];
                }
            }
        }

        // Output projection: (SEQ_LEN, DIM) @ (DIM, DIM) -> (SEQ_LEN, DIM)
        let attn_bytes = f32_to_bytes(&attn_output);
        let projected = matmul(&attn_bytes, &layer.wo, SEQ_LEN, DIM, DIM);

        // Residual connection
        hidden = add(&hidden, &projected);

        // --- Feed-forward network ---
        let normed2 = rmsnorm(&hidden, SEQ_LEN * DIM);

        // SwiGLU: SiLU(x @ W1) * (x @ W3), then project down with W2
        let gate = matmul(&normed2, &layer.w1, SEQ_LEN, DIM, FF_DIM);
        let up = matmul(&normed2, &layer.w3, SEQ_LEN, DIM, FF_DIM);
        let gate_activated = silu(&gate);
        let ff_hidden = mul(&gate_activated, &up);
        let ff_out = matmul(&ff_hidden, &layer.w2, SEQ_LEN, FF_DIM, DIM);

        // Residual connection
        hidden = add(&hidden, &ff_out);
    }

    // Final norm + output projection
    let final_normed = rmsnorm(&hidden, SEQ_LEN * DIM);
    matmul(&final_normed, &weights.output_proj, SEQ_LEN, DIM, DIM)
}

fn main() {
    println!("# Micro-Transformer Inference Benchmark (CPU)\n");
    println!(
        "Architecture: {} layers, dim={}, heads={}, head_dim={}, ff_dim={}, seq_len={}",
        LAYERS, DIM, HEADS, HEAD_DIM, FF_DIM, SEQ_LEN
    );

    // Parameter count
    let params_per_layer = 4 * DIM * DIM + 3 * DIM * FF_DIM;
    let total_params = params_per_layer * LAYERS + DIM * DIM;
    println!(
        "Parameters: {} ({:.2} KB)\n",
        total_params,
        total_params as f64 * 4.0 / 1024.0
    );

    // Initialize
    let weights = init_weights();
    let input = f32_to_bytes(&pseudo_random_f32(42, SEQ_LEN * DIM));

    // Warmup
    for _ in 0..WARMUP {
        let _ = forward(&input, &weights);
    }

    // Single forward pass
    let mut total_single = std::time::Duration::ZERO;
    for _ in 0..MEASURE_SINGLE {
        let start = Instant::now();
        let _ = forward(&input, &weights);
        total_single += start.elapsed();
    }
    let avg_single_us = total_single.as_secs_f64() * 1e6 / MEASURE_SINGLE as f64;

    // 10 consecutive forward passes (amortized)
    let mut total_batch = std::time::Duration::ZERO;
    for _ in 0..MEASURE_BATCH {
        let start = Instant::now();
        for _ in 0..10 {
            let _ = forward(&input, &weights);
        }
        total_batch += start.elapsed();
    }
    let avg_batch_us = total_batch.as_secs_f64() * 1e6 / (MEASURE_BATCH * 10) as f64;

    // Token throughput: one forward pass processes SEQ_LEN tokens
    let tokens_per_sec_single = SEQ_LEN as f64 / (avg_single_us / 1e6);
    let tokens_per_sec_batch = SEQ_LEN as f64 / (avg_batch_us / 1e6);

    // FLOP count per forward pass (approximate)
    // Per layer: 4 matmul (DIM,DIM) + 3 matmul (DIM,FF_DIM or FF_DIM,DIM) + attn scores + attn output
    // Each matmul (M,K)@(K,N) = 2*M*K*N FLOPs
    let flops_per_pass: f64 = {
        let qkv_proj = 3.0 * 2.0 * SEQ_LEN as f64 * DIM as f64 * DIM as f64;
        let out_proj = 2.0 * SEQ_LEN as f64 * DIM as f64 * DIM as f64;
        let attn_scores = HEADS as f64 * 2.0 * SEQ_LEN as f64 * HEAD_DIM as f64 * SEQ_LEN as f64;
        let attn_output = HEADS as f64 * 2.0 * SEQ_LEN as f64 * SEQ_LEN as f64 * HEAD_DIM as f64;
        let ff = 3.0 * 2.0 * SEQ_LEN as f64 * DIM as f64 * FF_DIM as f64;
        let final_proj = 2.0 * SEQ_LEN as f64 * DIM as f64 * DIM as f64;
        LAYERS as f64 * (qkv_proj + out_proj + attn_scores + attn_output + ff) + final_proj
    };
    let gflops_single = flops_per_pass / (avg_single_us / 1e6) / 1e9;

    println!("## Results\n");
    println!("| Metric                   | Value          |");
    println!("|--------------------------|----------------|");
    println!("| Single pass (avg)        | {:>10.2} us  |", avg_single_us);
    println!("| 10-pass amortized (avg)  | {:>10.2} us  |", avg_batch_us);
    println!(
        "| Tokens/sec (single)      | {:>10.0}     |",
        tokens_per_sec_single
    );
    println!(
        "| Tokens/sec (amortized)   | {:>10.0}     |",
        tokens_per_sec_batch
    );
    println!("| GFLOPS (single pass)     | {:>10.3}     |", gflops_single);
    println!(
        "| FLOPs/pass               | {:>10.0}     |",
        flops_per_pass
    );

    // Verify output is not NaN/Inf (sanity check)
    let output = forward(&input, &weights);
    let output_f32 = bytes_to_f32(&output);
    let has_nan = output_f32.iter().any(|v| v.is_nan());
    let has_inf = output_f32.iter().any(|v| v.is_infinite());
    let max_abs = output_f32.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
    println!("\n## Sanity Check\n");
    println!("| Check        | Result |");
    println!("|--------------|--------|");
    println!(
        "| NaN-free     | {}  |",
        if !has_nan { "PASS" } else { "FAIL" }
    );
    println!(
        "| Inf-free     | {}  |",
        if !has_inf { "PASS" } else { "FAIL" }
    );
    println!("| Max |output| | {:.6} |", max_abs);
}
