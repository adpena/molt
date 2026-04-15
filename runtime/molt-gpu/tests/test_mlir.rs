use molt_gpu::dtype::DType;
use molt_gpu::mlir::to_mlir_text;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::{
    BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc,
};
use molt_gpu::shapetracker::ShapeTracker;

#[test]
fn test_mlir_add_f32() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[64]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
    };
    let mlir = to_mlir_text(&kernel);
    assert!(mlir.contains("arith.addf"));
    assert!(mlir.contains("f32"));
    assert!(mlir.contains("func.func @molt_kernel"));
}

#[test]
fn test_mlir_add_i32() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Int32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[64]), dtype: DType::Int32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[64]), dtype: DType::Int32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[64]), dtype: DType::Int32, access: BufferAccess::Read },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
    };
    let mlir = to_mlir_text(&kernel);
    assert!(mlir.contains("arith.addi"));
    assert!(mlir.contains("i32"));
}

#[test]
fn test_mlir_cmplt() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Cmplt,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Bool,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[32]), dtype: DType::Bool, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[32]), dtype: DType::Float32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[32]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [32, 1, 1],
        local: [32, 1, 1],
    };
    let mlir = to_mlir_text(&kernel);
    assert!(mlir.contains("arith.cmpf \"olt\""));
}

#[test]
fn test_mlir_shr_signed_vs_unsigned() {
    let kernel_signed = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Shr,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Int32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[16]), dtype: DType::Int32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[16]), dtype: DType::Int32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[16]), dtype: DType::Int32, access: BufferAccess::Read },
        ],
        grid: [16, 1, 1],
        local: [16, 1, 1],
    };
    assert!(to_mlir_text(&kernel_signed).contains("arith.shrsi"));

    let kernel_unsigned = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Shr,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::UInt32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[16]), dtype: DType::UInt32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[16]), dtype: DType::UInt32, access: BufferAccess::Read },
            BufferBinding { buf_id: 2, st: ShapeTracker::contiguous(&[16]), dtype: DType::UInt32, access: BufferAccess::Read },
        ],
        grid: [16, 1, 1],
        local: [16, 1, 1],
    };
    assert!(to_mlir_text(&kernel_unsigned).contains("arith.shrui"));
}

#[test]
fn test_mlir_reduce_sum() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::ReduceSum,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[1]), dtype: DType::Float32, access: BufferAccess::Write },
            BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[256]), dtype: DType::Float32, access: BufferAccess::Read },
        ],
        grid: [1, 1, 1],
        local: [1, 1, 1],
    };
    let mlir = to_mlir_text(&kernel);
    assert!(mlir.contains("arith.addf"));
}

#[test]
fn test_mlir_math_ops() {
    for (op, expected) in [
        (PrimitiveOp::Exp2, "math.exp2"),
        (PrimitiveOp::Log2, "math.log2"),
        (PrimitiveOp::Sin, "math.sin"),
        (PrimitiveOp::Sqrt, "math.sqrt"),
        (PrimitiveOp::Trunc, "math.trunc"),
    ] {
        let kernel = FusedKernel {
            ops: vec![FusedOp {
                op,
                srcs: vec![FusedSrc::Buf(1)],
                dst_dtype: DType::Float32,
            }],
            bufs: vec![
                BufferBinding { buf_id: 0, st: ShapeTracker::contiguous(&[32]), dtype: DType::Float32, access: BufferAccess::Write },
                BufferBinding { buf_id: 1, st: ShapeTracker::contiguous(&[32]), dtype: DType::Float32, access: BufferAccess::Read },
            ],
            grid: [32, 1, 1],
            local: [32, 1, 1],
        };
        let mlir = to_mlir_text(&kernel);
        assert!(mlir.contains(expected), "op {:?} should emit {}", op, expected);
    }
}
