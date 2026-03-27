//! End-to-end GPU compute pipeline.
//!
//! Takes a [`GpuKernel`], selects the best available platform, generates shader
//! source, compiles it, allocates buffers, transfers data, launches, and returns
//! results.  This is the top-level API that Python's `@gpu.kernel` decorator
//! calls through the runtime.

use super::gpu::GpuKernel;
use super::gpu_msl::generate_msl;
use super::gpu_runtime::{GpuDevice, GpuError, GpuPlatform, detect_gpu_platform};

/// Execute a GPU kernel with the given input data and return the output.
///
/// Pipeline stages: codegen -> compile -> transfer -> dispatch -> readback.
///
/// # Arguments
/// * `kernel`      - TIR GPU kernel to execute.
/// * `inputs`      - Host-side byte slices for each input buffer, bound in order.
/// * `output_size` - Size in bytes of the output buffer.
/// * `grid`        - Threadgroup grid dimensions `[x, y, z]`.
/// * `block`       - Threads-per-threadgroup dimensions `[x, y, z]`.
///
/// # Returns
/// The output buffer contents read back from the GPU, or a [`GpuError`].
pub fn execute_gpu_kernel(
    kernel: &GpuKernel,
    inputs: &[&[u8]],
    output_size: usize,
    grid: [u32; 3],
    block: [u32; 3],
) -> Result<Vec<u8>, GpuError> {
    // 1. Detect best GPU platform
    let platform =
        detect_gpu_platform().ok_or(GpuError::DeviceNotAvailable("No GPU available".into()))?;

    // 2. Generate shader source for the platform
    let source = generate_source(kernel, platform)?;

    // 3. Create device
    let device = create_device(platform)?;

    // 4. Compile kernel — catch panics from Metal's null pointer returns on
    //    invalid MSL (foreign-types-shared asserts non-null internally).
    let compile_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        device.compile_kernel(&kernel.name, &source)
    }));
    let compiled = match compile_result {
        Ok(Ok(k)) => k,
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            return Err(GpuError::CompilationFailed(
                "Metal library compilation returned null (invalid MSL source)".into(),
            ));
        }
    };

    // 5. Allocate and fill input buffers (minimum 1 byte to avoid null pointers)
    let mut gpu_buffers = Vec::with_capacity(inputs.len() + 1);
    for input in inputs {
        let alloc_size = if input.is_empty() { 1 } else { input.len() };
        let buf = device.alloc_buffer(alloc_size)?;
        if !input.is_empty() {
            device.copy_to_device(&buf, input)?;
        }
        gpu_buffers.push(buf);
    }

    // 6. Allocate output buffer — use at least 1 byte to avoid null Metal buffers
    let alloc_output_size = if output_size == 0 { 1 } else { output_size };
    let output_buf = device.alloc_buffer(alloc_output_size)?;
    gpu_buffers.push(output_buf);

    // 7. Launch kernel
    let buf_refs: Vec<_> = gpu_buffers.iter().collect();
    device.launch_kernel(&compiled, grid, block, &buf_refs)?;

    // 8. Synchronize
    device.synchronize()?;

    // 9. Read back output
    let mut output = vec![0u8; output_size];
    if output_size > 0 {
        let output_buf_ref = gpu_buffers.last().unwrap();
        device.copy_from_device(output_buf_ref, &mut output)?;
    }

    // 10. Cleanup — free in reverse order (output first, then inputs)
    for buf in gpu_buffers {
        device.free_buffer(buf)?;
    }

    Ok(output)
}

/// Generate shader source for a kernel on the given platform.
///
/// WebGPU uses its own `gpu_wgsl::GpuKernel` with different buffer/param
/// semantics, so it cannot be generated from the standard `gpu::GpuKernel`.
/// Callers targeting WebGPU should build a `gpu_wgsl::GpuKernel` directly and
/// use the WGSL-specific path.
fn generate_source(kernel: &GpuKernel, platform: GpuPlatform) -> Result<String, GpuError> {
    match platform {
        GpuPlatform::Metal => Ok(generate_msl(kernel)),
        GpuPlatform::WebGpu => Err(GpuError::LaunchFailed(
            "WebGPU requires a WGSL-specific kernel — use the WGSL pipeline directly".into(),
        )),
        GpuPlatform::Cuda => Ok(super::gpu_cuda::generate_cuda(kernel)),
        GpuPlatform::Hip => Ok(super::gpu_hip::generate_hip(kernel)),
    }
}

/// Create a [`GpuDevice`] for the given platform.
///
/// Returns `DeviceNotAvailable` if the corresponding feature flag is not
/// enabled at compile time.
fn create_device(platform: GpuPlatform) -> Result<Box<dyn GpuDevice>, GpuError> {
    match platform {
        GpuPlatform::Metal => {
            #[cfg(target_os = "macos")]
            {
                return Ok(Box::new(super::gpu_metal::MetalDevice::new()?));
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(GpuError::DeviceNotAvailable(
                    "Metal is only available on macOS".into(),
                ))
            }
        }
        GpuPlatform::WebGpu => Ok(Box::new(super::gpu_webgpu::WebGpuDevice::new()?)),
        GpuPlatform::Cuda | GpuPlatform::Hip => Err(GpuError::DeviceNotAvailable(format!(
            "Platform {:?} not yet supported via the pipeline — enable the corresponding feature flag",
            platform
        ))),
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::gpu::{GpuBuffer, GpuBufferAccess, GpuKernel, GpuLaunchConfig};
    use crate::tir::gpu_runtime::GpuPlatform;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    /// Helper: build a vector_add TIR kernel (out[tid] = a[tid] + b[tid]).
    fn make_vector_add_kernel() -> GpuKernel {
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
            launch_config: Some(GpuLaunchConfig {
                grid_size: [4, 1, 1],
                threadgroup_size: [4, 1, 1],
            }),
        }
    }

    #[test]
    fn platform_detection_returns_metal_on_macos() {
        let platform = detect_gpu_platform();
        #[cfg(target_os = "macos")]
        assert_eq!(platform, Some(GpuPlatform::Metal));
        #[cfg(not(target_os = "macos"))]
        let _ = platform;
    }

    #[test]
    fn generate_source_produces_msl_for_metal() {
        let kernel = make_vector_add_kernel();
        let source = generate_source(&kernel, GpuPlatform::Metal).unwrap();
        assert!(source.contains("#include <metal_stdlib>"));
        assert!(source.contains("kernel void vector_add("));
    }

    #[test]
    fn create_device_returns_error_for_unsupported_platform() {
        // CUDA/HIP are never compiled in tests
        let result = create_device(GpuPlatform::Cuda);
        assert!(result.is_err());
        let result = create_device(GpuPlatform::Hip);
        assert!(result.is_err());
    }

    #[test]
    fn pipeline_error_on_empty_kernel_compilation() {
        // An empty kernel will generate valid MSL but the pipeline exercises the
        // full path. Without the gpu-metal feature the stub will fail compilation.
        let kernel = GpuKernel {
            name: "empty".into(),
            buffers: vec![],
            scalar_params: vec![],
            body_ops: vec![],
            launch_config: None,
        };
        // Just ensure the pipeline doesn't panic — the result depends on features
        let _result = execute_gpu_kernel(&kernel, &[], 0, [1, 1, 1], [1, 1, 1]);
    }

    /// Full end-to-end test: vector_add on real Metal hardware.
    ///
    /// Only runs when `gpu-metal` feature is enabled — otherwise the stub
    /// returns `DeviceNotAvailable`.
    ///
    /// Note: TIR `F64` is narrowed to `float` (f32) on Metal because Metal
    /// does not support 64-bit floats.  Host data must be f32 to match.
    #[test]
    #[cfg(all(target_os = "macos", feature = "gpu-metal"))]
    fn execute_vector_add_on_metal() {
        let kernel = make_vector_add_kernel();
        let n = 4usize;
        // Metal narrows F64 -> float (f32), so host data must be f32.
        let a: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0];
        let b: Vec<f32> = vec![10.0, 20.0, 30.0, 40.0];

        let a_bytes = unsafe {
            std::slice::from_raw_parts(a.as_ptr() as *const u8, n * std::mem::size_of::<f32>())
        };
        let b_bytes = unsafe {
            std::slice::from_raw_parts(b.as_ptr() as *const u8, n * std::mem::size_of::<f32>())
        };
        let output_size = n * std::mem::size_of::<f32>();

        let result = execute_gpu_kernel(
            &kernel,
            &[a_bytes, b_bytes],
            output_size,
            [n as u32, 1, 1],
            [n as u32, 1, 1],
        )
        .expect("execute_gpu_kernel should succeed on Metal");

        // Interpret output bytes as f32
        assert_eq!(result.len(), output_size);
        let out: &[f32] = unsafe { std::slice::from_raw_parts(result.as_ptr() as *const f32, n) };
        assert_eq!(out, &[11.0, 22.0, 33.0, 44.0]);
    }

    /// Pipeline should return an error when the gpu-metal feature is off but
    /// we are on macOS (the stub device will report not available).
    #[test]
    #[cfg(all(target_os = "macos", not(feature = "gpu-metal")))]
    fn pipeline_returns_error_without_metal_feature() {
        let kernel = make_vector_add_kernel();
        let result = execute_gpu_kernel(&kernel, &[], 0, [1, 1, 1], [1, 1, 1]);
        assert!(
            result.is_err(),
            "Pipeline should fail without gpu-metal feature"
        );
    }
}
