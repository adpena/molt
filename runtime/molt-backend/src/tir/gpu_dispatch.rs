//! Unified GPU dispatch — selects the best available platform and routes
//! kernel compilation + launch through the appropriate backend.
//!
//! For full data-in / data-out execution, use [`super::gpu_pipeline::execute_gpu_kernel`].
//! This module provides the lower-level `compile_and_launch` helper that
//! dispatches without input/output buffer management.

use super::gpu_runtime::*;

/// Create the best available GPU device for this platform.
pub fn create_gpu_device() -> Result<Box<dyn GpuDevice>, GpuError> {
    let platform = detect_gpu_platform().ok_or(GpuError::DeviceNotAvailable(
        "No GPU platform available. Supported: Metal (macOS), WebGPU, CUDA, HIP".into(),
    ))?;

    match platform {
        GpuPlatform::Metal => {
            #[cfg(target_os = "macos")]
            {
                return Ok(Box::new(super::gpu_metal::MetalDevice::new()?));
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(GpuError::DeviceNotAvailable("Metal is only available on macOS".into()))
            }
        }
        GpuPlatform::WebGpu => Ok(Box::new(super::gpu_webgpu::WebGpuDevice::new()?)),
        other => Err(GpuError::DeviceNotAvailable(format!(
            "Platform {:?} not yet supported — enable the corresponding feature flag",
            other
        ))),
    }
}

/// Compile and launch a GPU kernel from TIR.
///
/// Automatically selects MSL/WGSL/CUDA/HIP based on the available platform.
/// For full buffer management (input data, output readback), use
/// [`super::gpu_pipeline::execute_gpu_kernel`] instead.
pub fn compile_and_launch(
    kernel: &super::gpu::GpuKernel,
    grid: [u32; 3],
    block: [u32; 3],
) -> Result<(), GpuError> {
    // Delegate to the pipeline for source generation + device creation.
    let platform = detect_gpu_platform()
        .ok_or(GpuError::DeviceNotAvailable("No GPU platform".into()))?;

    let source = match platform {
        GpuPlatform::Metal => super::gpu_msl::generate_msl(kernel),
        GpuPlatform::WebGpu => {
            return Err(GpuError::LaunchFailed(
                "WebGPU kernels require a WGSL-specific kernel; use compile_and_launch_wgsl".into(),
            ));
        }
        GpuPlatform::Cuda => super::gpu_cuda::generate_cuda(kernel),
        GpuPlatform::Hip => super::gpu_hip::generate_hip(kernel),
    };

    let device = create_gpu_device()?;
    let compiled = device.compile_kernel(&kernel.name, &source)?;
    device.launch_kernel(&compiled, grid, block, &[])?;
    device.synchronize()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::gpu::GpuKernel;

    #[test]
    #[cfg(all(target_os = "macos", feature = "gpu-metal"))]
    fn create_gpu_device_returns_metal_on_macos() {
        let device = create_gpu_device();
        assert!(device.is_ok(), "create_gpu_device should succeed on macOS with gpu-metal");
    }

    #[test]
    #[cfg(all(target_os = "macos", not(feature = "gpu-metal")))]
    fn create_gpu_device_returns_error_without_feature() {
        let device = create_gpu_device();
        assert!(device.is_err(), "create_gpu_device should fail without gpu-metal feature");
    }

    #[test]
    #[cfg(all(target_os = "macos", feature = "gpu-metal"))]
    fn compile_and_launch_with_simple_kernel() {
        let kernel = GpuKernel {
            name: "test_kernel".to_string(),
            buffers: vec![],
            scalar_params: vec![],
            body_ops: vec![],
            launch_config: None,
        };
        let result = compile_and_launch(&kernel, [1, 1, 1], [1, 1, 1]);
        assert!(result.is_ok(), "compile_and_launch should succeed on macOS: {:?}", result);
    }

    #[test]
    #[cfg(all(target_os = "macos", not(feature = "gpu-metal")))]
    fn compile_and_launch_fails_without_feature() {
        let kernel = GpuKernel {
            name: "test_kernel".to_string(),
            buffers: vec![],
            scalar_params: vec![],
            body_ops: vec![],
            launch_config: None,
        };
        let result = compile_and_launch(&kernel, [1, 1, 1], [1, 1, 1]);
        assert!(result.is_err(), "compile_and_launch should fail without gpu-metal");
    }
}
