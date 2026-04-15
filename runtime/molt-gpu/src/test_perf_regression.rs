//! Performance regression guard tests.
//!
//! These are NOT benchmarks (no precise timing). They verify that key
//! operations complete within a generous budget (2x the established
//! baseline) to catch catastrophic regressions in CI.
//!
//! Operations tested:
//!   - fused_matmul 64x64
//!   - fused_softmax 1024 elements
//!   - fused_rms_norm 1024 elements

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use crate::device::cpu::interpret;

    /// Helper: create a random-ish f32 buffer of `n` elements.
    /// Uses a simple LCG for deterministic values without external deps.
    fn make_f32_buf(n: usize, seed: u64) -> Vec<u8> {
        let mut buf = vec![0u8; n * 4];
        let mut state = seed;
        for i in 0..n {
            // LCG: state = state * 6364136223846793005 + 1
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            // Map to f32 in [-1, 1]
            let val = ((state >> 33) as f32) / (u32::MAX as f32 / 2.0) - 1.0;
            let offset = i * 4;
            buf[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
        }
        buf
    }

    /// Helper: read f32 from buffer at index.
    fn read_f32(buf: &[u8], idx: usize) -> f32 {
        let offset = idx * 4;
        f32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap())
    }

    // -----------------------------------------------------------------------
    // Matmul 64x64 regression guard
    // -----------------------------------------------------------------------

    #[test]
    fn perf_guard_matmul_64x64() {
        let m = 64;
        let k = 64;
        let n = 64;

        let a = make_f32_buf(m * k, 42);
        let b = make_f32_buf(k * n, 137);
        let mut out = vec![0u8; m * n * 4];

        // Warm up
        interpret::fused_matmul(&a, &b, &mut out, m, k, n);

        // Timed run (10 iterations)
        let start = Instant::now();
        let iters = 10;
        for _ in 0..iters {
            out.fill(0);
            interpret::fused_matmul(&a, &b, &mut out, m, k, n);
        }
        let elapsed = start.elapsed();
        let per_iter_us = elapsed.as_micros() as f64 / iters as f64;

        // Budget: 64x64 matmul should complete in < 20ms per iteration.
        // Debug builds are ~10x slower than release; this budget accommodates
        // both debug and release profiles to catch only catastrophic regressions.
        assert!(
            per_iter_us < 20_000.0,
            "matmul 64x64 took {:.1}us/iter, budget is 20000us",
            per_iter_us,
        );

        // Verify correctness: C[0,0] should be the dot product of A[0,:] and B[:,0]
        let c00 = read_f32(&out, 0);
        let mut expected = 0.0f32;
        for kk in 0..k {
            expected += read_f32(&a, kk) * read_f32(&b, kk * n);
        }
        assert!(
            (c00 - expected).abs() < 1e-2,
            "matmul C[0,0] = {}, expected {}",
            c00, expected,
        );
    }

    // -----------------------------------------------------------------------
    // Fused softmax 1024 regression guard
    // -----------------------------------------------------------------------

    #[test]
    fn perf_guard_fused_softmax_1024() {
        let n = 16; // 16 rows
        let reduce_size = 64; // 64 elements per row
        let total = n * reduce_size; // 1024 total

        let input = make_f32_buf(total, 99);
        let mut output = vec![0u8; total * 4];

        // Warm up
        interpret::fused_softmax(&input, &mut output, n, reduce_size);

        // Timed run (100 iterations)
        let start = Instant::now();
        let iters = 100;
        for _ in 0..iters {
            output.fill(0);
            interpret::fused_softmax(&input, &mut output, n, reduce_size);
        }
        let elapsed = start.elapsed();
        let per_iter_us = elapsed.as_micros() as f64 / iters as f64;

        // Budget: 1024-element softmax should complete in < 500us.
        assert!(
            per_iter_us < 500.0,
            "fused_softmax 1024 took {:.1}us/iter, budget is 500us",
            per_iter_us,
        );

        // Verify correctness: each row should sum to ~1.0
        for row in 0..n {
            let mut row_sum = 0.0f64;
            for j in 0..reduce_size {
                let val = read_f32(&output, row * reduce_size + j) as f64;
                assert!(val >= 0.0, "softmax produced negative value: {}", val);
                row_sum += val;
            }
            assert!(
                (row_sum - 1.0).abs() < 1e-5,
                "softmax row {} sums to {}, expected 1.0",
                row, row_sum,
            );
        }

        // Verify monotonicity: larger inputs should get larger softmax values
        // within each row (not guaranteed for random input, but we verify
        // the output is non-negative and sums to 1)
    }

    // -----------------------------------------------------------------------
    // Fused RMSNorm 1024 regression guard
    // -----------------------------------------------------------------------

    #[test]
    fn perf_guard_fused_rms_norm_1024() {
        let n = 4; // 4 rows
        let dim = 256; // 256 elements per row
        let total = n * dim; // 1024 total
        let eps = 1e-5f64;

        let input = make_f32_buf(total, 73);
        let mut output = vec![0u8; total * 4];

        // Warm up
        interpret::fused_rms_norm(&input, &mut output, n, dim, eps);

        // Timed run (100 iterations)
        let start = Instant::now();
        let iters = 100;
        for _ in 0..iters {
            output.fill(0);
            interpret::fused_rms_norm(&input, &mut output, n, dim, eps);
        }
        let elapsed = start.elapsed();
        let per_iter_us = elapsed.as_micros() as f64 / iters as f64;

        // Budget: 1024-element RMSNorm should complete in < 500us.
        assert!(
            per_iter_us < 500.0,
            "fused_rms_norm 1024 took {:.1}us/iter, budget is 500us",
            per_iter_us,
        );

        // Verify correctness: RMSNorm output should have unit RMS per row
        for row in 0..n {
            let mut sum_sq = 0.0f64;
            for j in 0..dim {
                let val = read_f32(&output, row * dim + j) as f64;
                assert!(
                    val.is_finite(),
                    "rms_norm produced non-finite value at row={} j={}",
                    row, j,
                );
                sum_sq += val * val;
            }
            let rms = (sum_sq / dim as f64).sqrt();
            // After RMSNorm, the RMS of the output should be ~1.0
            assert!(
                (rms - 1.0).abs() < 0.1,
                "rms_norm row {} has RMS {:.4}, expected ~1.0",
                row, rms,
            );
        }
    }

    // -----------------------------------------------------------------------
    // Fused softmax correctness: verify against naive implementation
    // -----------------------------------------------------------------------

    #[test]
    fn fused_softmax_matches_naive() {
        let n = 4;
        let reduce_size = 8;
        let total = n * reduce_size;

        // Use known values
        let mut input = vec![0u8; total * 4];
        for i in 0..total {
            let val = (i as f32) * 0.1 - 1.5;
            let offset = i * 4;
            input[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
        }

        let mut output = vec![0u8; total * 4];
        interpret::fused_softmax(&input, &mut output, n, reduce_size);

        // Compute naive softmax for comparison
        for row in 0..n {
            let start = row * reduce_size;

            // Find max
            let mut max_val = f64::NEG_INFINITY;
            for j in 0..reduce_size {
                let val = read_f32(&input, start + j) as f64;
                if val > max_val {
                    max_val = val;
                }
            }

            // Compute exp2 and sum
            let mut sum = 0.0f64;
            let mut exp_vals = vec![0.0f64; reduce_size];
            for j in 0..reduce_size {
                let val = read_f32(&input, start + j) as f64;
                let e = (val - max_val).exp2();
                exp_vals[j] = e;
                sum += e;
            }

            // Compare
            for j in 0..reduce_size {
                let expected = (exp_vals[j] / sum) as f32;
                let actual = read_f32(&output, start + j);
                assert!(
                    (actual - expected).abs() < 1e-5,
                    "softmax mismatch at row={} j={}: got {} expected {}",
                    row, j, actual, expected,
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Fused RMSNorm correctness: verify against naive implementation
    // -----------------------------------------------------------------------

    #[test]
    fn fused_rms_norm_matches_naive() {
        let n = 3;
        let dim = 16;
        let total = n * dim;
        let eps = 1e-6f64;

        // Use known values
        let mut input = vec![0u8; total * 4];
        for i in 0..total {
            let val = (i as f32) * 0.3 - 2.0;
            let offset = i * 4;
            input[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
        }

        let mut output = vec![0u8; total * 4];
        interpret::fused_rms_norm(&input, &mut output, n, dim, eps);

        // Compute naive RMSNorm for comparison
        for row in 0..n {
            let start = row * dim;

            let mut sum_sq = 0.0f64;
            for j in 0..dim {
                let val = read_f32(&input, start + j) as f64;
                sum_sq += val * val;
            }
            let mean_sq = sum_sq / dim as f64;
            let inv_rms = 1.0 / (mean_sq + eps).sqrt();

            for j in 0..dim {
                let val = read_f32(&input, start + j) as f64;
                let expected = (val * inv_rms) as f32;
                let actual = read_f32(&output, start + j);
                assert!(
                    (actual - expected).abs() < 1e-4,
                    "rms_norm mismatch at row={} j={}: got {} expected {}",
                    row, j, actual, expected,
                );
            }
        }
    }
}
