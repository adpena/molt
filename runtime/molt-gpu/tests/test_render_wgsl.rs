use molt_gpu::dtype::DType;
use molt_gpu::ops::PrimitiveOp;
use molt_gpu::render::wgsl::WgslRenderer;
use molt_gpu::render::{BufferAccess, BufferBinding, FusedKernel, FusedOp, FusedSrc, Renderer};
use molt_gpu::shapetracker::ShapeTracker;

fn make_simple_binary_kernel(op: PrimitiveOp, n: usize) -> FusedKernel {
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
        local: [256, 1, 1],
        spec: None,
        vectorize_width: 1,
    }
}

#[test]
fn test_wgsl_render_add() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 1024);
    let wgsl = WgslRenderer::new().render(&kernel);
    assert!(wgsl.contains("@compute @workgroup_size(256"));
    assert!(wgsl.contains("fn molt_kernel"));
    assert!(wgsl.contains("@builtin(global_invocation_id)"));
    assert!(wgsl.contains("buf1[gid] + buf2[gid]"));
    assert!(wgsl.contains("buf0[gid] = v0"));
}

#[test]
fn test_wgsl_render_mul() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Mul, 512);
    let wgsl = WgslRenderer::new().render(&kernel);
    assert!(wgsl.contains("buf1[gid] * buf2[gid]"));
}

#[test]
fn test_wgsl_render_select_not_ternary() {
    // WGSL must use select() instead of ternary operator
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Where,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2), FusedSrc::Buf(3)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Bool,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 3,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let wgsl = WgslRenderer::new().render(&kernel);
    assert!(
        wgsl.contains("select("),
        "WGSL must use select(), not ternary"
    );
    assert!(
        !wgsl.contains(" ? "),
        "WGSL must not contain ternary operator"
    );
}

#[test]
fn test_wgsl_render_bitcast() {
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Bitcast,
            srcs: vec![FusedSrc::Buf(1)],
            dst_dtype: DType::Float32,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float32,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Int32,
                access: BufferAccess::Read,
            },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let wgsl = WgslRenderer::new().render(&kernel);
    assert!(
        wgsl.contains("bitcast<f32>"),
        "WGSL must use bitcast<T> syntax"
    );
}

#[test]
fn test_wgsl_storage_bindings() {
    let kernel = make_simple_binary_kernel(PrimitiveOp::Add, 128);
    let wgsl = WgslRenderer::new().render(&kernel);
    assert!(wgsl.contains("@group(0) @binding(0)"));
    assert!(wgsl.contains("@group(0) @binding(1)"));
    assert!(wgsl.contains("@group(0) @binding(2)"));
    assert!(wgsl.contains("var<storage, read_write>"));
    assert!(wgsl.contains("var<storage, read>"));
}

#[test]
fn test_wgsl_dtype_narrowing() {
    // f64 should be narrowed to f32 in WGSL
    let kernel = FusedKernel {
        ops: vec![FusedOp {
            op: PrimitiveOp::Add,
            srcs: vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            dst_dtype: DType::Float64,
        }],
        bufs: vec![
            BufferBinding {
                buf_id: 0,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float64,
                access: BufferAccess::Write,
            },
            BufferBinding {
                buf_id: 1,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float64,
                access: BufferAccess::Read,
            },
            BufferBinding {
                buf_id: 2,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float64,
                access: BufferAccess::Read,
            },
        ],
        grid: [64, 1, 1],
        local: [64, 1, 1],
        spec: None,
        vectorize_width: 1,
    };
    let wgsl = WgslRenderer::new().render(&kernel);
    // Should use f32, not f64 (WGSL has no f64)
    assert!(wgsl.contains("f32"), "WGSL should narrow f64 to f32");
    assert!(!wgsl.contains("f64"), "WGSL should not contain f64");
}

#[test]
fn test_wgsl_all_26_ops_have_render_patterns() {
    let elementwise_ops = PrimitiveOp::ALL
        .iter()
        .filter(|op| op.is_elementwise())
        .collect::<Vec<_>>();

    for &&op in &elementwise_ops {
        let srcs = match op.arity() {
            1 => vec![FusedSrc::Buf(1)],
            2 => vec![FusedSrc::Buf(1), FusedSrc::Buf(2)],
            3 => vec![FusedSrc::Buf(1), FusedSrc::Buf(2), FusedSrc::Buf(3)],
            _ => unreachable!(),
        };
        let mut bufs = vec![BufferBinding {
            buf_id: 0,
            st: ShapeTracker::contiguous(&[64]),
            dtype: DType::Float32,
            access: BufferAccess::Write,
        }];
        for i in 1..=op.arity() {
            bufs.push(BufferBinding {
                buf_id: i,
                st: ShapeTracker::contiguous(&[64]),
                dtype: DType::Float32,
                access: BufferAccess::Read,
            });
        }
        let kernel = FusedKernel {
            ops: vec![FusedOp {
                op,
                srcs,
                dst_dtype: if matches!(
                    op,
                    PrimitiveOp::Cmplt | PrimitiveOp::Cmpeq | PrimitiveOp::Cmpne
                ) {
                    DType::Bool
                } else {
                    DType::Float32
                },
            }],
            bufs,
            grid: [64, 1, 1],
            local: [64, 1, 1],
            spec: None,
            vectorize_width: 1,
        };
        let wgsl = WgslRenderer::new().render(&kernel);
        assert!(
            wgsl.contains("molt_kernel"),
            "op {:?} failed to render WGSL",
            op
        );
    }
}
