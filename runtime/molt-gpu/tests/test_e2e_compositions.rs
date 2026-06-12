//! End-to-end composition tests on CPU.
//!
//! Tests that composed operations produce correct results by comparing
//! against known mathematical identities and f64 reference computations.

use std::sync::Arc;

use molt_gpu::device::cpu::interpret;
use molt_gpu::dtype::DType;
use molt_gpu::lazy::{DeviceBufferRef, LazyOp};
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, KernelBody, ReductionDomain,
};
use molt_gpu::schedule::schedule;
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

fn u16_to_bytes(vals: &[u16]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn bytes_to_u16(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn patterned_bytes(len: usize) -> Vec<u8> {
    (0..len).map(|i| 0x40u8.wrapping_add(i as u8)).collect()
}

fn reverse_element_bytes(bytes: &[u8], elem_size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for elem in bytes.chunks_exact(elem_size).rev() {
        out.extend_from_slice(elem);
    }
    out
}

fn raw_byte_copy_dtypes() -> [DType; 15] {
    [
        DType::Bool,
        DType::Int8,
        DType::UInt8,
        DType::MxFP8,
        DType::MxFP4,
        DType::Int16,
        DType::UInt16,
        DType::Float16,
        DType::BFloat16,
        DType::Int32,
        DType::UInt32,
        DType::Float32,
        DType::Int64,
        DType::UInt64,
        DType::Float64,
    ]
}

fn run_materialize_copy_from_view(
    dtype: DType,
    buf_id: usize,
    source_elems: usize,
    view_st: ShapeTracker,
    output_shape: &[usize],
    output_fill: u8,
) -> (Vec<u8>, Vec<u8>) {
    let elem_size = dtype.size_bytes();
    let source = Arc::new(LazyOp::Buffer {
        buf: DeviceBufferRef {
            id: buf_id,
            size_bytes: source_elems * elem_size,
        },
        st: ShapeTracker::contiguous(&[source_elems]),
        dtype,
    });
    let movement = Arc::new(LazyOp::Movement {
        src: Arc::clone(&source),
        st: view_st,
    });
    let contig = Arc::new(LazyOp::Contiguous { src: movement });

    let kernels = schedule(&contig, output_shape);
    assert_eq!(kernels.len(), 1);
    let kernel = &kernels[0];
    assert_eq!(kernel.body, KernelBody::MaterializeCopy);

    let input = patterned_bytes(source_elems * elem_size);
    let mut bufs = vec![
        vec![output_fill; kernel.bufs[0].st.numel() * elem_size],
        input.clone(),
    ];
    interpret::execute_kernel(kernel, &mut bufs);

    (bufs.remove(0), input)
}

/// Run a chain of ops on CPU using the interpreter.
fn run_chain(ops: Vec<FusedOp>, bufs: Vec<BufferBinding>, input_bufs: Vec<Vec<u8>>) -> Vec<f32> {
    let n_out = bufs[0].st.numel();
    let mut all_bufs = vec![vec![0u8; n_out * 4]];
    all_bufs.extend(input_bufs);

    let kernel = FusedKernel {
        body: Default::default(),
        ops,
        bufs,
        grid: [n_out as u32, 1, 1],
        local: [1, 1, 1],
        spec: None,
        vectorize_width: 1,
    };

    interpret::execute_kernel(&kernel, &mut all_bufs);
    bytes_to_f32(&all_bufs[0])
}

#[test]
fn test_cpu_interpreter_reads_same_storage_through_distinct_movement_views() {
    let source = Arc::new(LazyOp::Buffer {
        buf: DeviceBufferRef {
            id: 42,
            size_bytes: 16,
        },
        st: ShapeTracker::contiguous(&[4]),
        dtype: DType::Float32,
    });
    let flipped_st = ShapeTracker::contiguous(&[4]).flip(0);
    let flipped = Arc::new(LazyOp::Movement {
        src: Arc::clone(&source),
        st: flipped_st.clone(),
    });
    let add = Arc::new(LazyOp::Binary {
        op: PrimitiveOp::Add,
        lhs: Arc::clone(&flipped),
        rhs: Arc::clone(&source),
    });

    let kernels = schedule(&add, &[4]);
    assert_eq!(kernels.len(), 1);
    let kernel = &kernels[0];
    assert_eq!(kernel.bufs.len(), 3);
    assert_eq!(kernel.bufs[1].buf_id, 42);
    assert_eq!(kernel.bufs[2].buf_id, 42);

    let input = f32_to_bytes(&[1.0, 2.0, 3.0, 4.0]);
    let mut bufs = vec![vec![0u8; 16], input.clone(), input];
    interpret::execute_kernel(kernel, &mut bufs);

    assert_eq!(bytes_to_f32(&bufs[0]), vec![5.0, 5.0, 5.0, 5.0]);
}

#[test]
fn test_cpu_interpreter_reads_masked_movement_padding_as_zero() {
    let source = Arc::new(LazyOp::Buffer {
        buf: DeviceBufferRef {
            id: 43,
            size_bytes: 12,
        },
        st: ShapeTracker::contiguous(&[3]),
        dtype: DType::Float32,
    });
    let padded = Arc::new(LazyOp::Movement {
        src: Arc::clone(&source),
        st: ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]),
    });
    let neg = Arc::new(LazyOp::Unary {
        op: PrimitiveOp::Neg,
        src: Arc::clone(&padded),
    });

    let kernels = schedule(&neg, &[5]);
    assert_eq!(kernels.len(), 1);
    let kernel = &kernels[0];
    assert_eq!(kernel.bufs.len(), 2);
    assert_eq!(kernel.bufs[1].buf_id, 43);

    let input = f32_to_bytes(&[1.0, 2.0, 3.0]);
    let mut bufs = vec![vec![0u8; 20], input];
    interpret::execute_kernel(kernel, &mut bufs);

    assert_eq!(bytes_to_f32(&bufs[0]), vec![-0.0, -1.0, -2.0, -3.0, -0.0]);
}

#[test]
fn test_cpu_interpreter_materializes_contiguous_from_flipped_view() {
    let source = Arc::new(LazyOp::Buffer {
        buf: DeviceBufferRef {
            id: 44,
            size_bytes: 16,
        },
        st: ShapeTracker::contiguous(&[4]),
        dtype: DType::Float32,
    });
    let flipped_st = ShapeTracker::contiguous(&[4]).flip(0);
    let flipped = Arc::new(LazyOp::Movement {
        src: Arc::clone(&source),
        st: flipped_st.clone(),
    });
    let contig = Arc::new(LazyOp::Contiguous { src: flipped });

    let kernels = schedule(&contig, &[4]);
    assert_eq!(kernels.len(), 1);
    let kernel = &kernels[0];
    assert_eq!(kernel.body, KernelBody::MaterializeCopy);
    assert!(kernel.bufs[0].st.view().is_contiguous());
    assert_eq!(kernel.bufs[1].st, flipped_st);

    let input = f32_to_bytes(&[1.0, 2.0, 3.0, 4.0]);
    let mut bufs = vec![vec![0u8; 16], input];
    interpret::execute_kernel(kernel, &mut bufs);

    assert_eq!(bytes_to_f32(&bufs[0]), vec![4.0, 3.0, 2.0, 1.0]);
}

#[test]
fn test_cpu_interpreter_materializes_contiguous_from_padded_view() {
    let source = Arc::new(LazyOp::Buffer {
        buf: DeviceBufferRef {
            id: 45,
            size_bytes: 12,
        },
        st: ShapeTracker::contiguous(&[3]),
        dtype: DType::Float32,
    });
    let padded = Arc::new(LazyOp::Movement {
        src: Arc::clone(&source),
        st: ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]),
    });
    let contig = Arc::new(LazyOp::Contiguous { src: padded });

    let kernels = schedule(&contig, &[5]);
    assert_eq!(kernels.len(), 1);
    let kernel = &kernels[0];
    assert_eq!(kernel.body, KernelBody::MaterializeCopy);

    let input = f32_to_bytes(&[1.0, 2.0, 3.0]);
    let mut bufs = vec![vec![0u8; 20], input];
    interpret::execute_kernel(kernel, &mut bufs);

    assert_eq!(bytes_to_f32(&bufs[0]), vec![0.0, 1.0, 2.0, 3.0, 0.0]);
}

#[test]
fn test_cpu_interpreter_materializes_contiguous_from_shrunk_view() {
    let source = Arc::new(LazyOp::Buffer {
        buf: DeviceBufferRef {
            id: 46,
            size_bytes: 16,
        },
        st: ShapeTracker::contiguous(&[4]),
        dtype: DType::Float32,
    });
    let shrunk_st = ShapeTracker::contiguous(&[4]).shrink(&[(1, 4)]);
    let shrunk = Arc::new(LazyOp::Movement {
        src: Arc::clone(&source),
        st: shrunk_st.clone(),
    });
    let contig = Arc::new(LazyOp::Contiguous { src: shrunk });

    let kernels = schedule(&contig, &[3]);
    assert_eq!(kernels.len(), 1);
    let kernel = &kernels[0];
    assert_eq!(kernel.body, KernelBody::MaterializeCopy);
    assert!(kernel.bufs[0].st.view().is_contiguous());
    assert_eq!(kernel.bufs[1].st, shrunk_st);

    let input = f32_to_bytes(&[10.0, 20.0, 30.0, 40.0]);
    let mut bufs = vec![vec![0u8; 12], input];
    interpret::execute_kernel(kernel, &mut bufs);

    assert_eq!(bytes_to_f32(&bufs[0]), vec![20.0, 30.0, 40.0]);
}

#[test]
fn test_cpu_interpreter_materialize_copy_preserves_u16_raw_bytes() {
    let source = Arc::new(LazyOp::Buffer {
        buf: DeviceBufferRef {
            id: 47,
            size_bytes: 8,
        },
        st: ShapeTracker::contiguous(&[4]),
        dtype: DType::UInt16,
    });
    let flipped_st = ShapeTracker::contiguous(&[4]).flip(0);
    let flipped = Arc::new(LazyOp::Movement {
        src: Arc::clone(&source),
        st: flipped_st,
    });
    let contig = Arc::new(LazyOp::Contiguous { src: flipped });

    let kernels = schedule(&contig, &[4]);
    assert_eq!(kernels.len(), 1);
    let kernel = &kernels[0];
    assert_eq!(kernel.body, KernelBody::MaterializeCopy);

    let input = u16_to_bytes(&[0x0102, 0x0304, 0x0506, 0x0708]);
    let mut bufs = vec![vec![0u8; 8], input];
    interpret::execute_kernel(kernel, &mut bufs);

    assert_eq!(bytes_to_u16(&bufs[0]), vec![0x0708, 0x0506, 0x0304, 0x0102]);
}

#[test]
fn test_cpu_interpreter_materialize_copy_preserves_raw_element_bytes_by_dtype_width() {
    for (idx, dtype) in raw_byte_copy_dtypes().into_iter().enumerate() {
        let elem_size = dtype.size_bytes();
        let source = Arc::new(LazyOp::Buffer {
            buf: DeviceBufferRef {
                id: 10_000 + idx,
                size_bytes: 4 * elem_size,
            },
            st: ShapeTracker::contiguous(&[4]),
            dtype,
        });
        let flipped = Arc::new(LazyOp::Movement {
            src: Arc::clone(&source),
            st: ShapeTracker::contiguous(&[4]).flip(0),
        });
        let contig = Arc::new(LazyOp::Contiguous { src: flipped });

        let kernels = schedule(&contig, &[4]);
        assert_eq!(kernels.len(), 1);
        let kernel = &kernels[0];
        assert_eq!(kernel.body, KernelBody::MaterializeCopy);

        let input = patterned_bytes(4 * elem_size);
        let mut bufs = vec![vec![0u8; 4 * elem_size], input.clone()];
        interpret::execute_kernel(kernel, &mut bufs);

        assert_eq!(
            bufs[0],
            reverse_element_bytes(&input, elem_size),
            "MaterializeCopy must preserve raw element bytes for {:?}",
            dtype,
        );
    }
}

#[test]
fn test_cpu_interpreter_materialize_copy_preserves_shrunk_raw_element_bytes_by_dtype_width() {
    for (idx, dtype) in raw_byte_copy_dtypes().into_iter().enumerate() {
        let elem_size = dtype.size_bytes();
        let source = Arc::new(LazyOp::Buffer {
            buf: DeviceBufferRef {
                id: 20_000 + idx,
                size_bytes: 6 * elem_size,
            },
            st: ShapeTracker::contiguous(&[6]),
            dtype,
        });
        let shrunk = Arc::new(LazyOp::Movement {
            src: Arc::clone(&source),
            st: ShapeTracker::contiguous(&[6]).shrink(&[(1, 5)]),
        });
        let contig = Arc::new(LazyOp::Contiguous { src: shrunk });

        let kernels = schedule(&contig, &[4]);
        assert_eq!(kernels.len(), 1);
        let kernel = &kernels[0];
        assert_eq!(kernel.body, KernelBody::MaterializeCopy);

        let input = patterned_bytes(6 * elem_size);
        let mut bufs = vec![vec![0u8; 4 * elem_size], input.clone()];
        interpret::execute_kernel(kernel, &mut bufs);

        assert_eq!(
            bufs[0],
            input[elem_size..5 * elem_size],
            "MaterializeCopy shrink must preserve raw element bytes for {:?}",
            dtype,
        );
    }
}

#[test]
fn test_cpu_interpreter_materialize_copy_preserves_shrunk_flipped_raw_element_bytes_by_dtype_width()
{
    for (idx, dtype) in raw_byte_copy_dtypes().into_iter().enumerate() {
        let elem_size = dtype.size_bytes();
        let (output, input) = run_materialize_copy_from_view(
            dtype,
            50_000 + idx,
            7,
            ShapeTracker::contiguous(&[7]).shrink(&[(1, 6)]).flip(0),
            &[5],
            0xff,
        );
        let expected = reverse_element_bytes(&input[elem_size..6 * elem_size], elem_size);

        assert_eq!(
            output, expected,
            "MaterializeCopy shrink+flip must preserve raw element bytes for {:?}",
            dtype,
        );
    }
}

#[test]
fn test_cpu_interpreter_materialize_copy_zero_fills_padded_raw_elements() {
    let dtype = DType::UInt32;
    let elem_size = dtype.size_bytes();
    let source = Arc::new(LazyOp::Buffer {
        buf: DeviceBufferRef {
            id: 10_100,
            size_bytes: 3 * elem_size,
        },
        st: ShapeTracker::contiguous(&[3]),
        dtype,
    });
    let padded = Arc::new(LazyOp::Movement {
        src: Arc::clone(&source),
        st: ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]),
    });
    let contig = Arc::new(LazyOp::Contiguous { src: padded });

    let kernels = schedule(&contig, &[5]);
    assert_eq!(kernels.len(), 1);
    let kernel = &kernels[0];
    assert_eq!(kernel.body, KernelBody::MaterializeCopy);

    let input = patterned_bytes(3 * elem_size);
    let mut bufs = vec![vec![0xff; 5 * elem_size], input.clone()];
    interpret::execute_kernel(kernel, &mut bufs);

    let mut expected = Vec::with_capacity(5 * elem_size);
    expected.extend(std::iter::repeat(0u8).take(elem_size));
    expected.extend_from_slice(&input);
    expected.extend(std::iter::repeat(0u8).take(elem_size));
    assert_eq!(bufs[0], expected);
}

#[test]
fn test_cpu_interpreter_materialize_copy_zero_fills_padded_raw_elements_by_dtype_width() {
    for (idx, dtype) in raw_byte_copy_dtypes().into_iter().enumerate() {
        let elem_size = dtype.size_bytes();
        let source = Arc::new(LazyOp::Buffer {
            buf: DeviceBufferRef {
                id: 30_000 + idx,
                size_bytes: 3 * elem_size,
            },
            st: ShapeTracker::contiguous(&[3]),
            dtype,
        });
        let padded = Arc::new(LazyOp::Movement {
            src: Arc::clone(&source),
            st: ShapeTracker::contiguous(&[3]).pad(&[(1, 1)]),
        });
        let contig = Arc::new(LazyOp::Contiguous { src: padded });

        let kernels = schedule(&contig, &[5]);
        assert_eq!(kernels.len(), 1);
        let kernel = &kernels[0];
        assert_eq!(kernel.body, KernelBody::MaterializeCopy);

        let input = patterned_bytes(3 * elem_size);
        let mut bufs = vec![vec![0xff; 5 * elem_size], input.clone()];
        interpret::execute_kernel(kernel, &mut bufs);

        let mut expected = Vec::with_capacity(5 * elem_size);
        expected.extend(std::iter::repeat(0u8).take(elem_size));
        expected.extend_from_slice(&input);
        expected.extend(std::iter::repeat(0u8).take(elem_size));
        assert_eq!(
            bufs[0], expected,
            "MaterializeCopy pad must zero-fill only invalid raw elements for {:?}",
            dtype,
        );
    }
}

#[test]
fn test_cpu_interpreter_materialize_copy_zero_fills_padded_flipped_raw_elements_by_dtype_width() {
    for (idx, dtype) in raw_byte_copy_dtypes().into_iter().enumerate() {
        let elem_size = dtype.size_bytes();
        let (output, input) = run_materialize_copy_from_view(
            dtype,
            60_000 + idx,
            5,
            ShapeTracker::contiguous(&[5]).pad(&[(1, 2)]).flip(0),
            &[8],
            0xff,
        );

        let mut expected = Vec::with_capacity(8 * elem_size);
        expected.extend(std::iter::repeat(0u8).take(2 * elem_size));
        expected.extend(reverse_element_bytes(&input, elem_size));
        expected.extend(std::iter::repeat(0u8).take(elem_size));
        assert_eq!(
            output, expected,
            "MaterializeCopy pad+flip must zero-fill only invalid raw elements for {:?}",
            dtype,
        );
    }
}

#[test]
fn test_cpu_interpreter_materialize_copy_composed_view_uses_generic_semantics() {
    let source = Arc::new(LazyOp::Buffer {
        buf: DeviceBufferRef {
            id: 40_000,
            size_bytes: 8,
        },
        st: ShapeTracker::contiguous(&[4]),
        dtype: DType::UInt16,
    });
    let composed = Arc::new(LazyOp::Movement {
        src: Arc::clone(&source),
        st: ShapeTracker::contiguous(&[4]).flip(0).reshape(&[2, 2]),
    });
    let contig = Arc::new(LazyOp::Contiguous { src: composed });

    let kernels = schedule(&contig, &[2, 2]);
    assert_eq!(kernels.len(), 1);
    let kernel = &kernels[0];
    assert_eq!(kernel.body, KernelBody::MaterializeCopy);

    let input = u16_to_bytes(&[0x0102, 0x0304, 0x0506, 0x0708]);
    let mut bufs = vec![vec![0u8; 8], input];
    interpret::execute_kernel(kernel, &mut bufs);

    assert_eq!(bytes_to_u16(&bufs[0]), vec![0x0708, 0x0506, 0x0304, 0x0102]);
}

// --- exp(x) = EXP2(x * LOG2_E) ---

#[test]
fn test_composition_exp() {
    let log2_e = std::f64::consts::LOG2_E;
    let inputs = [0.0f32, 1.0, -1.0, 0.5, 2.0, -2.0, 0.001, -0.001];
    let n = inputs.len();

    let ops = vec![
        FusedOp::elementwise(
            PrimitiveOp::Mul,
            vec![
                FusedSrc::Buf(1),
                FusedSrc::Const {
                    val: log2_e,
                    dtype: DType::Float32,
                },
            ],
            DType::Float32,
        ),
        FusedOp::elementwise(PrimitiveOp::Exp2, vec![FusedSrc::Op(0)], DType::Float32),
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
        FusedOp::elementwise(PrimitiveOp::Log2, vec![FusedSrc::Buf(1)], DType::Float32),
        FusedOp::elementwise(
            PrimitiveOp::Mul,
            vec![
                FusedSrc::Op(0),
                FusedSrc::Const {
                    val: ln_2,
                    dtype: DType::Float32,
                },
            ],
            DType::Float32,
        ),
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
        FusedOp::elementwise(PrimitiveOp::Neg, vec![FusedSrc::Buf(1)], DType::Float32),
        // -x * LOG2_E
        FusedOp::elementwise(
            PrimitiveOp::Mul,
            vec![
                FusedSrc::Op(0),
                FusedSrc::Const {
                    val: log2_e,
                    dtype: DType::Float32,
                },
            ],
            DType::Float32,
        ),
        // EXP2(-x * LOG2_E)
        FusedOp::elementwise(PrimitiveOp::Exp2, vec![FusedSrc::Op(1)], DType::Float32),
        // 1 + exp(-x)
        FusedOp::elementwise(
            PrimitiveOp::Add,
            vec![
                FusedSrc::Const {
                    val: 1.0,
                    dtype: DType::Float32,
                },
                FusedSrc::Op(2),
            ],
            DType::Float32,
        ),
        // 1 / (1 + exp(-x))
        FusedOp::elementwise(
            PrimitiveOp::Reciprocal,
            vec![FusedSrc::Op(3)],
            DType::Float32,
        ),
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
        body: Default::default(),
        ops: vec![FusedOp::reduction(
            PrimitiveOp::ReduceMax,
            vec![FusedSrc::Buf(1)],
            DType::Float32,
            ReductionDomain::from_axis(&[n], 0),
        )],
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
    let mut bufs1 = vec![vec![0u8; 4], f32_to_bytes(&inputs)];
    interpret::execute_kernel(&k1, &mut bufs1);
    let max_val = bytes_to_f32(&bufs1[0])[0];
    assert_eq!(max_val, 4.0);

    // Step 2: Sub + Exp (manual composition)
    let log2_e = std::f64::consts::LOG2_E;
    let k2 = FusedKernel {
        body: Default::default(),
        ops: vec![
            FusedOp::elementwise(
                PrimitiveOp::Sub,
                vec![
                    FusedSrc::Buf(1),
                    FusedSrc::Const {
                        val: max_val as f64,
                        dtype: DType::Float32,
                    },
                ],
                DType::Float32,
            ),
            FusedOp::elementwise(
                PrimitiveOp::Mul,
                vec![
                    FusedSrc::Op(0),
                    FusedSrc::Const {
                        val: log2_e,
                        dtype: DType::Float32,
                    },
                ],
                DType::Float32,
            ),
            FusedOp::elementwise(PrimitiveOp::Exp2, vec![FusedSrc::Op(1)], DType::Float32),
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
    let mut bufs2 = vec![vec![0u8; n * 4], f32_to_bytes(&inputs)];
    interpret::execute_kernel(&k2, &mut bufs2);
    let exp_vals = bytes_to_f32(&bufs2[0]);

    // Step 3: ReduceSum
    let k3 = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::reduction(
            PrimitiveOp::ReduceSum,
            vec![FusedSrc::Buf(1)],
            DType::Float32,
            ReductionDomain::from_axis(&[n], 0),
        )],
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
            (0.0..=1.0).contains(&s),
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

    let ops = vec![FusedOp::elementwise(
        PrimitiveOp::Max,
        vec![
            FusedSrc::Buf(1),
            FusedSrc::Const {
                val: 0.0,
                dtype: DType::Float32,
            },
        ],
        DType::Float32,
    )];

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

    let ops = vec![FusedOp::elementwise(
        PrimitiveOp::Trunc,
        vec![FusedSrc::Buf(1)],
        DType::Float32,
    )];

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
            assert_eq!(actual, input, "trunc(inf) = inf at index {}", i);
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
        FusedOp::elementwise(
            PrimitiveOp::Mul,
            vec![
                FusedSrc::Buf(1),
                FusedSrc::Const {
                    val: log2_e,
                    dtype: DType::Float32,
                },
            ],
            DType::Float32,
        ),
        FusedOp::elementwise(PrimitiveOp::Exp2, vec![FusedSrc::Op(0)], DType::Float32),
        // log part
        FusedOp::elementwise(PrimitiveOp::Log2, vec![FusedSrc::Op(1)], DType::Float32),
        FusedOp::elementwise(
            PrimitiveOp::Mul,
            vec![
                FusedSrc::Op(2),
                FusedSrc::Const {
                    val: ln_2,
                    dtype: DType::Float32,
                },
            ],
            DType::Float32,
        ),
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
        body: Default::default(),
        ops: vec![FusedOp::reduction(
            PrimitiveOp::ReduceSum,
            vec![FusedSrc::Buf(1)],
            DType::Float32,
            ReductionDomain::from_axis(&[1], 0),
        )],
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
        spec: None,
        vectorize_width: 1,
    };

    let mut bufs = vec![vec![0u8; 4], f32_to_bytes(&[42.0])];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    assert_eq!(result, vec![42.0]);
}

#[test]
fn test_composition_reduce_max_with_neg_infinity() {
    let kernel = FusedKernel {
        body: Default::default(),
        ops: vec![FusedOp::reduction(
            PrimitiveOp::ReduceMax,
            vec![FusedSrc::Buf(1)],
            DType::Float32,
            ReductionDomain::from_axis(&[4], 0),
        )],
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
        spec: None,
        vectorize_width: 1,
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
        body: Default::default(),
        ops: vec![
            FusedOp::elementwise(
                PrimitiveOp::Mul,
                vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
                DType::Float32,
            ),
            FusedOp::reduction(
                PrimitiveOp::ReduceSum,
                vec![FusedSrc::Op(0)],
                DType::Float32,
                ReductionDomain::from_axis(&[n_in], 0),
            ),
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
        spec: None,
        vectorize_width: 1,
    };

    let a = [1.0f32, 2.0, 3.0, 4.0];
    let b = [5.0f32, 6.0, 7.0, 8.0];
    let mut bufs = vec![vec![0u8; 4], f32_to_bytes(&a), f32_to_bytes(&b)];
    interpret::execute_kernel(&kernel, &mut bufs);
    let result = bytes_to_f32(&bufs[0]);
    // dot(a, b) = 1*5 + 2*6 + 3*7 + 4*8 = 5 + 12 + 21 + 32 = 70
    assert_eq!(result, vec![70.0]);
}
