;; WASM SIMD matmul kernels for Falcon-OCR inference.
;;
;; Exports:
;;   matmul_f32(a_ptr, b_ptr, out_ptr, M, K, N)    -- F32 x F32 -> F32
;;   matmul_dequant_i8(a_ptr, b_ptr, scales_ptr, out_ptr, M, K, N) -- F32 x INT8 -> F32
;;
;; Memory layout: caller allocates a, b, out in linear memory.
;; Uses WASM SIMD v128 for 4-wide f32 accumulation.
;; IKJ loop order for cache locality (writes to out are sequential).

(module
  (memory (export "memory") 256 4096)  ;; 16 MB initial, 256 MB max

  ;; -----------------------------------------------------------------------
  ;; matmul_f32: [M, K] x [K, N] -> [M, N]
  ;; IKJ loop order: for each row m, for each k, broadcast a[m,k] and
  ;; accumulate across N columns using SIMD 4-wide f32.
  ;; -----------------------------------------------------------------------
  (func (export "matmul_f32")
    (param $a_ptr i32) (param $b_ptr i32) (param $out_ptr i32)
    (param $M i32) (param $K i32) (param $N i32)

    (local $m i32) (local $k i32) (local $n i32)
    (local $a_off i32) (local $b_off i32) (local $o_off i32)
    (local $a_val f32)
    (local $a_splat v128)
    (local $N4 i32)  ;; N rounded down to multiple of 4

    ;; Zero the output buffer: M * N * 4 bytes
    (local $total_bytes i32)
    (local.set $total_bytes (i32.mul (i32.mul (local.get $M) (local.get $N)) (i32.const 4)))
    (memory.fill (local.get $out_ptr) (i32.const 0) (local.get $total_bytes))

    ;; N4 = N & ~3 (SIMD-aligned portion)
    (local.set $N4 (i32.and (local.get $N) (i32.const -4)))

    ;; for m = 0..M
    (local.set $m (i32.const 0))
    (block $break_m (loop $loop_m
      (br_if $break_m (i32.ge_u (local.get $m) (local.get $M)))

      ;; a_off = a_ptr + m * K * 4
      (local.set $a_off
        (i32.add (local.get $a_ptr)
                 (i32.mul (i32.mul (local.get $m) (local.get $K)) (i32.const 4))))
      ;; o_off = out_ptr + m * N * 4
      (local.set $o_off
        (i32.add (local.get $out_ptr)
                 (i32.mul (i32.mul (local.get $m) (local.get $N)) (i32.const 4))))

      ;; for k = 0..K
      (local.set $k (i32.const 0))
      (block $break_k (loop $loop_k
        (br_if $break_k (i32.ge_u (local.get $k) (local.get $K)))

        ;; a_val = a[m * K + k]
        (local.set $a_val
          (f32.load (i32.add (local.get $a_off)
                             (i32.mul (local.get $k) (i32.const 4)))))
        ;; a_splat = {a_val, a_val, a_val, a_val}
        (local.set $a_splat (f32x4.splat (local.get $a_val)))

        ;; b_off = b_ptr + k * N * 4
        (local.set $b_off
          (i32.add (local.get $b_ptr)
                   (i32.mul (i32.mul (local.get $k) (local.get $N)) (i32.const 4))))

        ;; SIMD loop: n = 0..N4, step 4
        (local.set $n (i32.const 0))
        (block $break_n4 (loop $loop_n4
          (br_if $break_n4 (i32.ge_u (local.get $n) (local.get $N4)))

          ;; out[m*N + n .. +4] += a_val * b[k*N + n .. +4]
          (v128.store
            (i32.add (local.get $o_off) (i32.mul (local.get $n) (i32.const 4)))
            (f32x4.add
              (v128.load (i32.add (local.get $o_off) (i32.mul (local.get $n) (i32.const 4))))
              (f32x4.mul
                (local.get $a_splat)
                (v128.load (i32.add (local.get $b_off) (i32.mul (local.get $n) (i32.const 4)))))))

          (local.set $n (i32.add (local.get $n) (i32.const 4)))
          (br $loop_n4)
        ))

        ;; Scalar tail: n = N4..N
        (local.set $n (local.get $N4))
        (block $break_nt (loop $loop_nt
          (br_if $break_nt (i32.ge_u (local.get $n) (local.get $N)))

          (f32.store
            (i32.add (local.get $o_off) (i32.mul (local.get $n) (i32.const 4)))
            (f32.add
              (f32.load (i32.add (local.get $o_off) (i32.mul (local.get $n) (i32.const 4))))
              (f32.mul
                (local.get $a_val)
                (f32.load (i32.add (local.get $b_off) (i32.mul (local.get $n) (i32.const 4)))))))

          (local.set $n (i32.add (local.get $n) (i32.const 1)))
          (br $loop_nt)
        ))

        (local.set $k (i32.add (local.get $k) (i32.const 1)))
        (br $loop_k)
      ))

      (local.set $m (i32.add (local.get $m) (i32.const 1)))
      (br $loop_m)
    ))
  )

  ;; -----------------------------------------------------------------------
  ;; matmul_dequant_i8: F32 [M, K] x INT8 [K, N] -> F32 [M, N]
  ;; B values are signed int8 stored as bytes. Dequantized on the fly:
  ;;   b_f32 = scale * (signed_int8)
  ;; Scale is a single f32 passed via pointer (first f32 at scales_ptr).
  ;; IKJ loop with SIMD: load 4 int8 -> extend to i32x4 -> convert to f32x4.
  ;; -----------------------------------------------------------------------
  (func (export "matmul_dequant_i8")
    (param $a_ptr i32) (param $b_ptr i32) (param $scales_ptr i32)
    (param $out_ptr i32) (param $M i32) (param $K i32) (param $N i32)

    (local $m i32) (local $k i32) (local $n i32)
    (local $a_off i32) (local $b_off i32) (local $o_off i32)
    (local $a_val f32)
    (local $scale f32)
    (local $a_scaled_splat v128)
    (local $N4 i32)
    (local $total_bytes i32)
    (local $b_byte i32)
    (local $b_signed i32)

    ;; Load scale
    (local.set $scale (f32.load (local.get $scales_ptr)))

    ;; Zero output
    (local.set $total_bytes (i32.mul (i32.mul (local.get $M) (local.get $N)) (i32.const 4)))
    (memory.fill (local.get $out_ptr) (i32.const 0) (local.get $total_bytes))

    (local.set $N4 (i32.and (local.get $N) (i32.const -4)))

    ;; for m = 0..M
    (local.set $m (i32.const 0))
    (block $break_m (loop $loop_m
      (br_if $break_m (i32.ge_u (local.get $m) (local.get $M)))

      (local.set $a_off
        (i32.add (local.get $a_ptr)
                 (i32.mul (i32.mul (local.get $m) (local.get $K)) (i32.const 4))))
      (local.set $o_off
        (i32.add (local.get $out_ptr)
                 (i32.mul (i32.mul (local.get $m) (local.get $N)) (i32.const 4))))

      ;; for k = 0..K
      (local.set $k (i32.const 0))
      (block $break_k (loop $loop_k
        (br_if $break_k (i32.ge_u (local.get $k) (local.get $K)))

        ;; a_val = a[m*K + k] * scale  (pre-multiply scale into a)
        (local.set $a_val
          (f32.mul
            (f32.load (i32.add (local.get $a_off)
                               (i32.mul (local.get $k) (i32.const 4))))
            (local.get $scale)))
        (local.set $a_scaled_splat (f32x4.splat (local.get $a_val)))

        ;; b_off = b_ptr + k * N  (INT8: 1 byte per element)
        (local.set $b_off
          (i32.add (local.get $b_ptr)
                   (i32.mul (local.get $k) (local.get $N))))

        ;; SIMD loop: process 4 int8 values at a time
        ;; Load 4 bytes, sign-extend to i32x4, convert to f32x4, multiply, accumulate
        (local.set $n (i32.const 0))
        (block $break_n4 (loop $loop_n4
          (br_if $break_n4 (i32.ge_u (local.get $n) (local.get $N4)))

          ;; Load 4 bytes from b, sign-extend each to i32, pack into v128
          ;; We load a 32-bit value and use i16x8.extend / i32x4.extend chains
          ;; to sign-extend bytes to i32.
          ;; Strategy: load 4 bytes as i32, use i8x16 -> i16x8 -> i32x4 widening.
          (v128.store
            (i32.add (local.get $o_off) (i32.mul (local.get $n) (i32.const 4)))
            (f32x4.add
              (v128.load (i32.add (local.get $o_off) (i32.mul (local.get $n) (i32.const 4))))
              (f32x4.mul
                (local.get $a_scaled_splat)
                ;; Convert 4 signed int8 to f32x4:
                ;; 1. Load 4 bytes into low 32 bits of v128
                ;; 2. Widen i8x16 low -> i16x8 (signed)
                ;; 3. Widen i16x8 low -> i32x4 (signed)
                ;; 4. Convert i32x4 -> f32x4
                (f32x4.convert_i32x4_s
                  (i32x4.extend_low_i16x8_s
                    (i16x8.extend_low_i8x16_s
                      ;; Load 4 bytes into a v128 (only low 32 bits matter)
                      ;; Use v128.load32_zero to load 32 bits into lane 0
                      (v128.load32_zero
                        (i32.add (local.get $b_off) (local.get $n)))))))))

          (local.set $n (i32.add (local.get $n) (i32.const 4)))
          (br $loop_n4)
        ))

        ;; Scalar tail
        (local.set $n (local.get $N4))
        (block $break_nt (loop $loop_nt
          (br_if $break_nt (i32.ge_u (local.get $n) (local.get $N)))

          ;; Sign-extend byte to i32
          (local.set $b_byte (i32.load8_u (i32.add (local.get $b_off) (local.get $n))))
          (local.set $b_signed
            (select
              (i32.sub (local.get $b_byte) (i32.const 256))
              (local.get $b_byte)
              (i32.gt_u (local.get $b_byte) (i32.const 127))))

          (f32.store
            (i32.add (local.get $o_off) (i32.mul (local.get $n) (i32.const 4)))
            (f32.add
              (f32.load (i32.add (local.get $o_off) (i32.mul (local.get $n) (i32.const 4))))
              (f32.mul
                (local.get $a_val)
                (f32.convert_i32_s (local.get $b_signed)))))

          (local.set $n (i32.add (local.get $n) (i32.const 1)))
          (br $loop_nt)
        ))

        (local.set $k (i32.add (local.get $k) (i32.const 1)))
        (br $loop_k)
      ))

      (local.set $m (i32.add (local.get $m) (i32.const 1)))
      (br $loop_m)
    ))
  )
)
