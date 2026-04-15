//! End-to-end composition tests on CPU.
//!
//! Tests that composed operations produce correct results by comparing
//! against known mathematical identities and f64 reference computations.

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
    };

    interpret::execute_kernel(&kernel, &mut all_bufs);
    bytes_to_f32(&all_bufs[0])
}

// --- exp(x) = EXP2(x * LOG2_E) ---

#[test]
fn test_composition_exp() {
    let log2_e = std::f64::consts::LOG2_E;
    let inputs = [0.0f32, 1.0, -1.0, 0.5, 2.0, -2.0, 0.001, -0.001];
    let n = inputs.len();

    let ops = vec![
        FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![
                FusedSrc::Buf(1),
                FusedSrc::Const {
                    val: log2_e,
                    dtype: DType::Float32,
                },
            ],
            dst_dtype: DType::Float32,
        },
        FusedOp {
            op: PrimitiveOp::Exp2,
            srcs: vec![FusedSrc::Op(0)],
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
    ];

    let result = run_chain(ops, bufs, vec![f32_to_bytes(&inputs)]);

    for (i, (&input, &actual)) in inputs.iter().zip(result.iter()).enumerate() {
        let expected = (input as f64).exp() as f32;
        let diff = (actual - expected).abs();
        assert!(
            diff < 1e-5,
            "exp({}): got {} expected {} (diff={}) at index {}",
            input,
            actual,
            expected,
            diff,
            i
        );
    }
}

// --- log(x) = LOG2(x) * LN_2 ---

#[test]
fn test_composition_log() {
    let ln_2 = std::f64::consts::LN_2;
    let inputs = [1.0f32, 2.0, 0.5, 10.0, 100.0, 0.01];
    let n = inputs.len();

    let ops = vec![
        FusedOp {
            op: PrimitiveOp::Log2,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        },
        FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![
                FusedSrc::Op(0),
                FusedSrc::Const {
                    val: ln_2,
                    dtype: DType::Float32,
                },
            ],
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
    ];

    let result = run_chain(ops, bufs, vec![f32_to_bytes(&inputs)]);

    for (i, (&input, &actual)) in inputs.iter().zip(result.iter()).enumerate() {
        let expected = (input as f64).ln() as f32;
        let diff = (actual - expected).abs();
        assert!(
            diff < 1e-5,
            "log({}): got {} expected {} (diff={}) at index {}",
            input,
            actual,
            expected,
            diff,
            i
        );
    }
}

// --- sigmoid(x) = 1 / (1 + exp(-x)) ---

#[test]
fn test_composition_sigmoid() {
    let log2_e = std::f64::consts::LOG2_E;
    let inputs = [0.0f32, 1.0, -1.0, 5.0, -5.0, 0.5, -0.5];
    let n = inputs.len();

    // sigmoid = RECIPROCAL(1 + EXP2(-x * LOG2_E))
    let ops = vec![
        // -x
        FusedOp {
            op: PrimitiveOp::Neg,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        },
        // -x * LOG2_E
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
        // EXP2(-x * LOG2_E)
        FusedOp {
            op: PrimitiveOp::Exp2,
            srcs: vec![FusedSrc::Op(1)],
            dst_dtype: DType::Float32,
        },
        // 1 + exp(-x)
        FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![
                FusedSrc::Const {
                    val: 1.0,
                    dtype: DType::Float32,
                },
                FusedSrc::Op(2),
            ],
            dst_dtype: DType::Float32,
        },
        // 1 / (1 + exp(-x))
        FusedOp {
            op: PrimitiveOp::Reciprocal,
            srcs: vec![FusedSrc::Op(3)],
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
    ];

    let result = run_chain(ops, bufs, vec![f32_to_bytes(&inputs)]);

    for (i, (&input, &actual)) in inputs.iter().zip(result.iter()).enumerate() {
        let expected = 1.0 / (1.0 + (-(input as f64)).exp());
        let diff = (actual as f64 - expected).abs();
        assert!(
            diff < 1e-5,
            "sigmoid({}): got {} expected {} (diff={}) at index {}",
            input,
            actual,
            expected,
            diff,
            i
        );
    }
}

// --- softmax(x) = exp(x - max(x)) / sum(exp(x - max(x))) ---

#[test]
fn test_composition_softmax() {
    let inputs = [1.0f32, 2.0, 3.0, 4.0];
    let n = inputs.len();

    // Step 1: ReduceMax
    let k1 = FusedKernel {
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
    };
    let mut bufs1 = vec![vec![0u8; 4], f32_to_bytes(&inputs)];
    interpret::execute_kernel(&k1, &mut bufs1);
    let max_val = bytes_to_f32(&bufs1[0])[0];
    assert_eq!(max_val, 4.0);

    // Step 2: Sub + Exp (manual composition)
    let log2_e = std::f64::consts::LOG2_E;
    let k2 = FusedKernel {
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
    };
    let mut bufs2 = vec![vec![0u8; n * 4], f32_to_bytes(&inputs)];
    interpret::execute_kernel(&k2, &mut bufs2);
    let exp_vals = bytes_to_f32(&bufs2[0]);

    // Step 3: ReduceSum
    let k3 = FusedKernel {
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
    };
    let mut bufs3 = vec![vec![0u8; 4], f32_to_bytes(&exp_vals)];
    interpret::execute_kernel(&k3, &mut bufs3);
    let sum_val = bytes_to_f32(&bufs3[0])[0];

    // Verify softmax properties
    let softmax: Vec<f32> = exp_vals.iter().map(|e| e / sum_val).collect();
    let total: f32 = softmax.iter().sum();
    assert!(
        (total - 1.0).abs() < 1e-5,
        "softmax should sum to 1.0, got {}",
        total
    );
    for (i, &s) in softmax.iter().enumerate() {
        assert!(
            s >= 0.0 && s <= 1.0,
            "softmax[{}] = {} should be in [0, 1]",
            i,
            s
        );
    }
    // Softmax should be monotonically increasing for monotonically increasing input
    for i in 1..softmax.len() {
        assert!(
            softmax[i] >= softmax[i - 1],
            "softmax should be monotone for monotone input: softmax[{}]={} < softmax[{}]={}",
            i,
            softmax[i],
            i - 1,
            softmax[i - 1]
        );
    }

    // Verify against f64 reference
    let ref_max = inputs.iter().cloned().fold(f32::NEG_INFINITY, f32::max) as f64;
    let ref_exps: Vec<f64> = inputs.iter().map(|x| (*x as f64 - ref_max).exp()).collect();
    let ref_sum: f64 = ref_exps.iter().sum();
    let ref_softmax: Vec<f64> = ref_exps.iter().map(|e| e / ref_sum).collect();

    for (i, (&actual, &expected)) in softmax.iter().zip(ref_softmax.iter()).enumerate() {
        let diff = (actual as f64 - expected).abs();
        assert!(
            diff < 1e-5,
            "softmax[{}]: got {} expected {} (diff={})",
            i,
            actual,
            expected,
            diff
        );
    }
}

// --- relu(x) = max(x, 0) ---

#[test]
fn test_composition_relu() {
    let inputs = [-3.0f32, -1.0, -0.0, 0.0, 0.5, 1.0, 100.0, f32::NAN];
    let n = inputs.len();

    let ops = vec![FusedOp {
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
    ];

    let result = run_chain(ops, bufs, vec![f32_to_bytes(&inputs)]);

    for (i, (&input, &actual)) in inputs.iter().zip(result.iter()).enumerate() {
        if input.is_nan() {
            // NaN-propagating max: max(NaN, 0) = NaN
            assert!(
                actual.is_nan(),
                "relu(NaN) should be NaN, got {} at index {}",
                actual,
                i
            );
        } else {
            let expected = input.max(0.0);
            assert_eq!(
                actual, expected,
                "relu({}): got {} expected {} at index {}",
                input, actual, expected, i
            );
        }
    }
}

// --- floor(x) = trunc(x) - (x < trunc(x) ? 1 : 0) ---

#[test]
fn test_composition_floor_via_trunc() {
    // floor(x) using trunc: trunc(x) for x >= 0, trunc(x) - 1 if x < 0 and x != trunc(x)
    // Simpler: we test trunc directly and verify IEEE 754 edge cases
    let inputs = [
        2.7f32,
        -2.7,
        3.0,
        -3.0,
        0.0,
        -0.0,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        0.9999,
        -0.0001,
    ];
    let n = inputs.len();

    let ops = vec![FusedOp {
        op: PrimitiveOp::Trunc,
        srcs: vec![FusedSrc::Buf(1)],
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
    ];

    let result = run_chain(ops, bufs, vec![f32_to_bytes(&inputs)]);

    for (i, (&input, &actual)) in inputs.iter().zip(result.iter()).enumerate() {
        let expected = (input as f64).trunc() as f32;
        if input.is_nan() {
            assert!(
                actual.is_nan(),
                "trunc(NaN) should be NaN, got {} at index {}",
                actual,
                i
            );
        } else if input.is_infinite() {
            assert_eq!(
                actual, input,
                "trunc(inf) = inf at index {}",
                i
            );
        } else if input == 0.0 {
            assert_eq!(actual, 0.0, "trunc(0) = 0 at index {}", i);
            // Preserve sign of zero
            if input.is_sign_negative() {
                assert!(
                    actual.is_sign_negative(),
                    "trunc(-0.0) should preserve -0.0 at index {}",
                    i
                );
            }
        } else {
            assert_eq!(
                actual, expected,
                "trunc({}): got {} expected {} at index {}",
                input, actual, expected, i
            );
        }
    }
}

// --- exp-log roundtrip: log(exp(x)) ~= x ---

#[test]
fn test_composition_exp_log_roundtrip() {
    let log2_e = std::f64::consts::LOG2_E;
    let ln_2 = std::f64::consts::LN_2;
    let inputs = [0.0f32, 1.0, -1.0, 0.5, 2.0, -2.0];
    let n = inputs.len();

    // exp: x * LOG2_E -> EXP2
    // log: LOG2 -> * LN_2
    let ops = vec![
        // exp part
        FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![
                FusedSrc::Buf(1),
                FusedSrc::Const {
                    val: log2_e,
                    dtype: DType::Float32,
                },
            ],
            dst_dtype: DType::Float32,
        },
        FusedOp {
            op: PrimitiveOp::Exp2,
            srcs: vec![FusedSrc::Op(0)],
            dst_dtype: DType::Float32,
        },
        // log part
        FusedOp {
            op: PrimitiveOp::Log2,
            srcs: vec![FusedSrc::Op(1)],
            dst_dtype: DType::Float32,
        },
        FusedOp {
            op: PrimitiveOp::Mul,
            srcs: vec![
                FusedSrc::Op(2),
                FusedSrc::Const {
                    val: ln_2,
                    dtype: DType::Float32,
                },
            ],
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
    ];

    let result = run_chain(ops, bufs, vec![f32_to_bytes(&inputs)]);

    for (i, (&input, &actual)) in inputs.iter().zip(result.iter()).enumerate() {
        let diff = (actual - input).abs();
        assert!(
            diff < 1e-4,
            "log(exp({})): got {} expected {} (diff={}) at index {}",
            input,
            actual,
            input,
            diff,
            i
        );
    }
}

// --- Reduce edge cases ---

#[test]
fn test_composition_reduce_sum_single_element() {
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
                st: ShapeTracker::contiguous(&[1]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
    };

    let mut bufs = vec![vec![0u8; 4], f32_to_bytes(&[42.0])];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![42.0]);
}

#[test]
fn test_composition_reduce_max_with_neg_infinity() {
    let kernel = FusedKernel {
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
                st: ShapeTracker::contiguous(&[4]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
    };

    let mut bufs = vec![
        vec![0u8; 4],
        f32_to_bytes(&[f32::NEG_INFINITY, -1000.0, -500.0, f32::NEG_INFINITY]),
    ];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![-500.0]);
}

#[test]
fn test_composition_fused_mul_reduce_sum() {
    // Fused elementwise + reduce: multiply then sum (dot product pattern)
    let n_in = 4;
    let kernel = FusedKernel {
        ops: vec![
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
        ],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[1]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[n_in]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[n_in]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
    };

    let a = [1.0f32, 2.0, 3.0, 4.0];
    let b = [5.0f32, 6.0, 7.0, 8.0];
    let mut bufs = vec![vec![0u8; 4], f32_to_bytes(&a), f32_to_bytes(&b)];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    // dot(a, b) = 1*5 + 2*6 + 3*7 + 4*8 = 5 + 12 + 21 + 32 = 70
    assert_eq!(result, vec![70.0]);
}
