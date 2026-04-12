#![cfg(target_os = "macos")]

#[cfg(feature = "gpu-metal")]
use molt_backend::tir::gpu::{GpuBuffer, GpuBufferAccess, GpuKernel, GpuLaunchConfig};
use molt_backend::tir::gpu_metal::MetalDevice;
#[cfg(feature = "gpu-metal")]
use molt_backend::tir::gpu_pipeline::execute_gpu_kernel;
use molt_backend::tir::gpu_runtime::GpuError;
#[cfg(feature = "gpu-metal")]
use molt_backend::tir::gpu_runtime::{GpuDevice, GpuPlatform};
#[cfg(feature = "gpu-metal")]
use molt_backend::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
#[cfg(feature = "gpu-metal")]
use molt_backend::tir::types::TirType;
#[cfg(feature = "gpu-metal")]
use molt_backend::tir::values::ValueId;

#[cfg(feature = "gpu-metal")]
fn as_u8_slice_f32(values: &[f32]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(values.as_ptr() as *const u8, std::mem::size_of_val(values))
    }
}

#[cfg(feature = "gpu-metal")]
fn as_f32_slice(bytes: &[u8]) -> &[f32] {
    unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, bytes.len() / 4) }
}

#[cfg(feature = "gpu-metal")]
fn make_vector_add_kernel(name: &str, n: usize) -> GpuKernel {
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
        name: name.to_string(),
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
        launch_config: Some(GpuLaunchConfig {
            grid_size: [n as u32, 1, 1],
            threadgroup_size: [n as u32, 1, 1],
        }),
    }
}

#[test]
#[cfg(feature = "gpu-metal")]
fn metal_backend_device_creation_is_runnable() {
    let device = MetalDevice::new().expect("MetalDevice::new should succeed");
    assert!(!device.device_name().is_empty());
}

#[test]
#[cfg(feature = "gpu-metal")]
fn metal_backend_compile_transfer_launch_readback() {
    let device = MetalDevice::new().expect("MetalDevice::new should succeed");
    let msl = r#"
        #include <metal_stdlib>
        using namespace metal;

        kernel void add_one(device float* a [[buffer(0)]],
                            uint id [[thread_position_in_grid]]) {
            a[id] = a[id] + 1.0;
        }
    "#;
    let kernel = device
        .compile_kernel("add_one", msl)
        .expect("compile_kernel should succeed");

    let input: Vec<f32> = vec![1.0, 3.0, 5.0, 7.0];
    let bytes = as_u8_slice_f32(&input);
    let mut output = vec![0u8; bytes.len()];
    let buf = GpuDevice::alloc_buffer(&device, bytes.len()).expect("alloc_buffer should succeed");

    device
        .copy_to_device(&buf, bytes)
        .expect("copy_to_device should succeed");
    device
        .launch_kernel(
            &kernel,
            [input.len() as u32, 1, 1],
            [input.len() as u32, 1, 1],
            &[&buf],
        )
        .expect("launch_kernel should succeed");
    device.synchronize().expect("synchronize should succeed");
    device
        .copy_from_device(&buf, &mut output)
        .expect("copy_from_device should succeed");
    device.free_buffer(buf).expect("free_buffer should succeed");

    assert_eq!(as_f32_slice(&output), &[2.0, 4.0, 6.0, 8.0]);
}

#[test]
#[cfg(feature = "gpu-metal")]
fn metal_backend_pipeline_vector_add() {
    let n = 4usize;
    let a: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0];
    let b: Vec<f32> = vec![10.0, 20.0, 30.0, 40.0];
    let kernel = make_vector_add_kernel("vector_add_lane_test", n);
    let out = execute_gpu_kernel(
        &kernel,
        &[as_u8_slice_f32(&a), as_u8_slice_f32(&b)],
        n * std::mem::size_of::<f32>(),
        [n as u32, 1, 1],
        [n as u32, 1, 1],
    )
    .expect("execute_gpu_kernel should succeed on Metal");
    assert_eq!(as_f32_slice(&out), &[11.0, 22.0, 33.0, 44.0]);
}

#[test]
#[cfg(feature = "gpu-metal")]
fn metal_backend_copy_overflow_reports_transfer_error() {
    let device = MetalDevice::new().expect("MetalDevice::new should succeed");
    let buf = GpuDevice::alloc_buffer(&device, 4).expect("alloc_buffer should succeed");
    let err = device
        .copy_to_device(&buf, &[1, 2, 3, 4, 5])
        .expect_err("oversized transfer must fail");
    device.free_buffer(buf).expect("free_buffer should succeed");

    match err {
        GpuError::TransferFailed(msg) => {
            assert!(msg.contains("overflow"), "unexpected message: {msg}")
        }
        other => panic!("expected TransferFailed, got {other:?}"),
    }
}

#[test]
#[cfg(not(feature = "gpu-metal"))]
fn metal_backend_stub_fails_without_feature() {
    let err = match MetalDevice::new() {
        Ok(_) => panic!("stub must fail without gpu-metal"),
        Err(err) => err,
    };
    match err {
        GpuError::DeviceNotAvailable(msg) => {
            assert!(msg.contains("gpu-metal"), "unexpected message: {msg}");
        }
        other => panic!("expected DeviceNotAvailable, got {other:?}"),
    }
}

#[test]
#[cfg(feature = "gpu-metal")]
fn metal_backend_rejects_non_metal_handles() {
    let device = MetalDevice::new().expect("MetalDevice::new should succeed");
    let fake = molt_backend::tir::gpu_runtime::GpuBufferHandle::new(
        8,
        GpuPlatform::WebGpu,
        vec![0; std::mem::size_of::<usize>()],
    );
    let err = device
        .free_buffer(fake)
        .expect_err("must reject non-metal handle");
    match err {
        GpuError::AllocationFailed(msg) => {
            assert!(msg.contains("non-Metal"), "unexpected message: {msg}")
        }
        other => panic!("expected AllocationFailed, got {other:?}"),
    }
}
