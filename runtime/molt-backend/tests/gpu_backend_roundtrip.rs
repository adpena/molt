//! Real backend roundtrip tests for Metal and WebGPU.
//!
//! These tests exercise actual device allocation, host/device transfers,
//! kernel launch, and readback on the feature-gated GPU backends. They are
//! intentionally small and backend-only so they can run on a MacBook Pro
//! without dragging Falcon inference into the loop.

use molt_backend::tir::gpu_runtime::GpuDevice;
use molt_backend::tir::types::TirType;

#[cfg(all(target_os = "macos", feature = "gpu-metal"))]
use molt_backend::tir::gpu::{GpuBuffer, GpuBufferAccess, GpuKernel};
#[cfg(all(target_os = "macos", feature = "gpu-metal"))]
use molt_backend::tir::gpu_metal::MetalDevice;
#[cfg(all(target_os = "macos", feature = "gpu-metal"))]
use molt_backend::tir::gpu_msl::generate_msl;
#[cfg(feature = "gpu-webgpu")]
use molt_backend::tir::gpu_webgpu::WebGpuDevice;
#[cfg(feature = "gpu-webgpu")]
use molt_backend::tir::gpu_wgsl::{
    BinOp, GpuKernel as WgslKernel, GpuStatement, WgslBuffer, WgslBufferAccess, generate_wgsl,
};
#[cfg(all(target_os = "macos", feature = "gpu-metal"))]
use molt_backend::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
#[cfg(all(target_os = "macos", feature = "gpu-metal"))]
use molt_backend::tir::values::ValueId;

fn f32s_to_bytes(values: &[f32]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(values.as_ptr() as *const u8, std::mem::size_of_val(values))
    }
}

fn bytes_to_f32s(bytes: &[u8]) -> &[f32] {
    assert_eq!(
        bytes.len() % std::mem::size_of::<f32>(),
        0,
        "byte length must be a multiple of f32"
    );
    unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, bytes.len() / 4) }
}

#[cfg(all(target_os = "macos", feature = "gpu-metal"))]
fn make_metal_vector_add_kernel() -> GpuKernel {
    let ops = vec![
        TirOp {
            dialect: Dialect::Gpu,
            opcode: OpCode::Index,
            operands: vec![ValueId(0), ValueId(3)],
            results: vec![ValueId(4)],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("buffer".into(), AttrValue::Str("a".into()));
                m
            },
            source_span: None,
        },
        TirOp {
            dialect: Dialect::Gpu,
            opcode: OpCode::Index,
            operands: vec![ValueId(1), ValueId(3)],
            results: vec![ValueId(5)],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("buffer".into(), AttrValue::Str("b".into()));
                m
            },
            source_span: None,
        },
        TirOp {
            dialect: Dialect::Gpu,
            opcode: OpCode::Add,
            operands: vec![ValueId(4), ValueId(5)],
            results: vec![ValueId(6)],
            attrs: AttrDict::new(),
            source_span: None,
        },
        TirOp {
            dialect: Dialect::Gpu,
            opcode: OpCode::StoreIndex,
            operands: vec![ValueId(2), ValueId(3), ValueId(6)],
            results: vec![],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("buffer".into(), AttrValue::Str("out".into()));
                m
            },
            source_span: None,
        },
    ];

    GpuKernel {
        name: "vector_add".into(),
        buffers: vec![
            GpuBuffer {
                name: "a".into(),
                element_type: TirType::F64,
                access: GpuBufferAccess::ReadOnly,
            },
            GpuBuffer {
                name: "b".into(),
                element_type: TirType::F64,
                access: GpuBufferAccess::ReadOnly,
            },
            GpuBuffer {
                name: "out".into(),
                element_type: TirType::F64,
                access: GpuBufferAccess::WriteOnly,
            },
        ],
        scalar_params: vec![],
        body_ops: ops,
        launch_config: None,
    }
}

#[cfg(all(target_os = "macos", feature = "gpu-metal"))]
fn make_metal_bool_eq_kernel() -> GpuKernel {
    let ops = vec![
        TirOp {
            dialect: Dialect::Gpu,
            opcode: OpCode::Index,
            operands: vec![ValueId(0), ValueId(3)],
            results: vec![ValueId(4)],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("buffer".into(), AttrValue::Str("a".into()));
                m
            },
            source_span: None,
        },
        TirOp {
            dialect: Dialect::Gpu,
            opcode: OpCode::Index,
            operands: vec![ValueId(1), ValueId(3)],
            results: vec![ValueId(5)],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("buffer".into(), AttrValue::Str("b".into()));
                m
            },
            source_span: None,
        },
        TirOp {
            dialect: Dialect::Gpu,
            opcode: OpCode::Eq,
            operands: vec![ValueId(4), ValueId(5)],
            results: vec![ValueId(6)],
            attrs: AttrDict::new(),
            source_span: None,
        },
        TirOp {
            dialect: Dialect::Gpu,
            opcode: OpCode::StoreIndex,
            operands: vec![ValueId(2), ValueId(3), ValueId(6)],
            results: vec![],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("buffer".into(), AttrValue::Str("out".into()));
                m
            },
            source_span: None,
        },
    ];

    GpuKernel {
        name: "vector_eq_mask".into(),
        buffers: vec![
            GpuBuffer {
                name: "a".into(),
                element_type: TirType::I64,
                access: GpuBufferAccess::ReadOnly,
            },
            GpuBuffer {
                name: "b".into(),
                element_type: TirType::I64,
                access: GpuBufferAccess::ReadOnly,
            },
            GpuBuffer {
                name: "out".into(),
                element_type: TirType::Bool,
                access: GpuBufferAccess::WriteOnly,
            },
        ],
        scalar_params: vec![],
        body_ops: ops,
        launch_config: None,
    }
}

#[cfg(feature = "gpu-webgpu")]
fn make_wgsl_vector_add_kernel() -> WgslKernel {
    WgslKernel {
        name: "vector_add".into(),
        workgroup_size: 4,
        buffers: vec![
            WgslBuffer {
                name: "a".into(),
                element_type: TirType::F64,
                access: WgslBufferAccess::ReadOnly,
            },
            WgslBuffer {
                name: "b".into(),
                element_type: TirType::F64,
                access: WgslBufferAccess::ReadOnly,
            },
            WgslBuffer {
                name: "out".into(),
                element_type: TirType::F64,
                access: WgslBufferAccess::ReadWrite,
            },
        ],
        params: vec![],
        body: vec![
            GpuStatement::LoadIndex {
                dst: "va".into(),
                src: "a".into(),
                idx: "tid".into(),
            },
            GpuStatement::LoadIndex {
                dst: "vb".into(),
                src: "b".into(),
                idx: "tid".into(),
            },
            GpuStatement::BinOp {
                dst: "sum".into(),
                lhs: "va".into(),
                op: BinOp::Add,
                rhs: "vb".into(),
            },
            GpuStatement::StoreIndex {
                dst: "out".into(),
                idx: "tid".into(),
                src: "sum".into(),
            },
        ],
    }
}

#[test]
#[cfg(all(target_os = "macos", feature = "gpu-metal"))]
fn metal_vector_add_roundtrip_produces_expected_output() {
    let device = MetalDevice::new().expect("MetalDevice::new should succeed");
    let kernel = make_metal_vector_add_kernel();
    let msl = generate_msl(&kernel);
    let compiled = device
        .compile_msl("vector_add", &msl)
        .expect("compile_msl should succeed");

    let a = [1.0f32, 2.0, 3.0, 4.0];
    let b = [10.0f32, 20.0, 30.0, 40.0];
    let a_buf = GpuDevice::alloc_buffer(&device, std::mem::size_of_val(&a)).expect("a buffer");
    let b_buf = GpuDevice::alloc_buffer(&device, std::mem::size_of_val(&b)).expect("b buffer");
    let out_buf = GpuDevice::alloc_buffer(&device, std::mem::size_of_val(&a)).expect("out buffer");

    device
        .copy_to_device(&a_buf, f32s_to_bytes(&a))
        .expect("copy a");
    device
        .copy_to_device(&b_buf, f32s_to_bytes(&b))
        .expect("copy b");

    let buf_refs: Vec<_> = [&a_buf, &b_buf, &out_buf].into_iter().collect();
    device
        .launch_kernel(&compiled, [4, 1, 1], [1, 1, 1], &buf_refs)
        .expect("launch_kernel should succeed");
    device.synchronize().expect("synchronize should succeed");

    let mut output = vec![0u8; std::mem::size_of_val(&a)];
    device
        .copy_from_device(&out_buf, &mut output)
        .expect("copy out");
    let out = bytes_to_f32s(&output);
    assert_eq!(out, &[11.0, 22.0, 33.0, 44.0]);

    let a2 = [5.0f32, 6.0, 7.0, 8.0];
    let b2 = [50.0f32, 60.0, 70.0, 80.0];
    device
        .copy_to_device(&a_buf, f32s_to_bytes(&a2))
        .expect("copy a2");
    device
        .copy_to_device(&b_buf, f32s_to_bytes(&b2))
        .expect("copy b2");
    device
        .launch_kernel(&compiled, [4, 1, 1], [1, 1, 1], &buf_refs)
        .expect("second launch_kernel should succeed");
    device
        .synchronize()
        .expect("second synchronize should succeed");
    device
        .copy_from_device(&out_buf, &mut output)
        .expect("copy out second");
    let out = bytes_to_f32s(&output);
    assert_eq!(out, &[55.0, 66.0, 77.0, 88.0]);

    device.free_buffer(a_buf).expect("free a");
    device.free_buffer(b_buf).expect("free b");
    device.free_buffer(out_buf).expect("free out");
}

#[test]
#[cfg(all(target_os = "macos", feature = "gpu-metal"))]
fn metal_vector_add_roundtrip_rejects_two_dimensional_dispatch() {
    let device = MetalDevice::new().expect("MetalDevice::new should succeed");
    let kernel = make_metal_vector_add_kernel();
    let msl = generate_msl(&kernel);
    let compiled = device
        .compile_msl("vector_add", &msl)
        .expect("compile_msl should succeed");

    let a = [1.0f32, 2.0, 3.0, 4.0];
    let b = [10.0f32, 20.0, 30.0, 40.0];
    let a_buf = GpuDevice::alloc_buffer(&device, std::mem::size_of_val(&a)).expect("a buffer");
    let b_buf = GpuDevice::alloc_buffer(&device, std::mem::size_of_val(&b)).expect("b buffer");
    let out_buf = GpuDevice::alloc_buffer(&device, std::mem::size_of_val(&a)).expect("out buffer");

    device
        .copy_to_device(&a_buf, f32s_to_bytes(&a))
        .expect("copy a");
    device
        .copy_to_device(&b_buf, f32s_to_bytes(&b))
        .expect("copy b");

    let buf_refs: Vec<_> = [&a_buf, &b_buf, &out_buf].into_iter().collect();
    let err = device
        .launch_kernel(&compiled, [2, 2, 1], [1, 1, 1], &buf_refs)
        .expect_err("2D dispatch must be rejected until linearization is explicit");
    assert!(
        err.to_string().contains("1D launches only"),
        "unexpected error: {err}"
    );

    device.free_buffer(a_buf).expect("free a");
    device.free_buffer(b_buf).expect("free b");
    device.free_buffer(out_buf).expect("free out");
}

#[test]
#[cfg(all(target_os = "macos", feature = "gpu-metal"))]
fn metal_bool_eq_roundtrip_produces_expected_mask_output() {
    let device = MetalDevice::new().expect("MetalDevice::new should succeed");
    let kernel = make_metal_bool_eq_kernel();
    let msl = generate_msl(&kernel);
    assert!(
        msl.contains("device const int64_t* a [[buffer(0)]]"),
        "Metal codegen must lower integer buffers to int64_t pointers"
    );
    assert!(
        msl.contains("device bool* out [[buffer(2)]]"),
        "Metal codegen must lower bool outputs to bool pointers"
    );

    let compiled = device
        .compile_msl("vector_eq_mask", &msl)
        .expect("compile_msl should succeed");

    let a: [i64; 4] = [1, 2, 3, 4];
    let b: [i64; 4] = [1, 9, 3, 0];
    let a_buf =
        GpuDevice::alloc_buffer(&device, std::mem::size_of_val(&a)).expect("a buffer");
    let b_buf =
        GpuDevice::alloc_buffer(&device, std::mem::size_of_val(&b)).expect("b buffer");
    let out_buf =
        GpuDevice::alloc_buffer(&device, 4).expect("out buffer");

    device
        .copy_to_device(&a_buf, unsafe {
            std::slice::from_raw_parts(a.as_ptr() as *const u8, std::mem::size_of_val(&a))
        })
        .expect("copy a");
    device
        .copy_to_device(&b_buf, unsafe {
            std::slice::from_raw_parts(b.as_ptr() as *const u8, std::mem::size_of_val(&b))
        })
        .expect("copy b");

    let buf_refs: Vec<_> = [&a_buf, &b_buf, &out_buf].into_iter().collect();
    device
        .launch_kernel(&compiled, [4, 1, 1], [1, 1, 1], &buf_refs)
        .expect("launch_kernel should succeed");
    device.synchronize().expect("synchronize should succeed");

    let mut output = vec![0u8; 4];
    device
        .copy_from_device(&out_buf, &mut output)
        .expect("copy out");
    assert_eq!(output, vec![1, 0, 1, 0]);

    device.free_buffer(a_buf).expect("free a");
    device.free_buffer(b_buf).expect("free b");
    device.free_buffer(out_buf).expect("free out");
}

#[test]
#[cfg(feature = "gpu-webgpu")]
fn webgpu_vector_add_roundtrip_produces_expected_output() {
    let device = WebGpuDevice::new().expect("WebGpuDevice::new should succeed");
    let kernel = make_wgsl_vector_add_kernel();
    let wgsl = generate_wgsl(&kernel);
    let compiled = device
        .compile_kernel("vector_add", &wgsl)
        .expect("compile_kernel should succeed");

    let a = [1.0f32, 2.0, 3.0, 4.0];
    let b = [10.0f32, 20.0, 30.0, 40.0];
    let a_buf = device
        .alloc_buffer(std::mem::size_of_val(&a))
        .expect("a buffer");
    let b_buf = device
        .alloc_buffer(std::mem::size_of_val(&b))
        .expect("b buffer");
    let out_buf = device
        .alloc_buffer(std::mem::size_of_val(&a))
        .expect("out buffer");

    device
        .copy_to_device(&a_buf, f32s_to_bytes(&a))
        .expect("copy a");
    device
        .copy_to_device(&b_buf, f32s_to_bytes(&b))
        .expect("copy b");

    let buf_refs: Vec<_> = [&a_buf, &b_buf, &out_buf].into_iter().collect();
    device
        .launch_kernel(&compiled, [1, 1, 1], [4, 1, 1], &buf_refs)
        .expect("launch_kernel should succeed");
    device.synchronize().expect("synchronize should succeed");

    let mut output = vec![0u8; std::mem::size_of_val(&a)];
    device
        .copy_from_device(&out_buf, &mut output)
        .expect("copy out");
    let out = bytes_to_f32s(&output);
    assert_eq!(out, &[11.0, 22.0, 33.0, 44.0]);

    let a2 = [5.0f32, 6.0, 7.0, 8.0];
    let b2 = [50.0f32, 60.0, 70.0, 80.0];
    device
        .copy_to_device(&a_buf, f32s_to_bytes(&a2))
        .expect("copy a2");
    device
        .copy_to_device(&b_buf, f32s_to_bytes(&b2))
        .expect("copy b2");
    device
        .launch_kernel(&compiled, [1, 1, 1], [4, 1, 1], &buf_refs)
        .expect("second launch_kernel should succeed");
    device
        .synchronize()
        .expect("second synchronize should succeed");
    device
        .copy_from_device(&out_buf, &mut output)
        .expect("copy out second");
    let out = bytes_to_f32s(&output);
    assert_eq!(out, &[55.0, 66.0, 77.0, 88.0]);

    device.free_buffer(a_buf).expect("free a");
    device.free_buffer(b_buf).expect("free b");
    device.free_buffer(out_buf).expect("free out");
}
