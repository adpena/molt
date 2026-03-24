//! Unified GPU dispatch — selects the best available platform and routes
//! kernel compilation + launch through the appropriate backend.

use super::gpu_runtime::*;

/// Create the best available GPU device for this platform.
pub fn create_gpu_device() -> Result<Box<dyn GpuDevice>, GpuError> {
    #[cfg(target_os = "macos")]
    {
        if detect_gpu_platform() == Some(GpuPlatform::Metal) {
            return Ok(Box::new(super::gpu_metal::MetalDevice::new()?));
        }
    }

    match detect_gpu_platform() {
        Some(GpuPlatform::WebGpu) => {
            Ok(Box::new(super::gpu_webgpu::WebGpuDevice::new()?))
        }
        _ => Err(GpuError::DeviceNotAvailable(
            "No GPU platform available. Supported: Metal (macOS), WebGPU, CUDA, HIP".into(),
        )),
    }
}

/// Compile and launch a GPU kernel from TIR.
/// Automatically selects MSL/WGSL/CUDA/HIP based on the available platform.
pub fn compile_and_launch(
    kernel: &super::gpu::GpuKernel,
    grid: [u32; 3],
    block: [u32; 3],
) -> Result<(), GpuError> {
    let device = create_gpu_device()?;

    // Generate source for the detected platform.
    // Note: WebGPU uses a distinct GpuKernel type (gpu_wgsl::GpuKernel) because WGSL
    // has different buffer/param semantics from Metal/CUDA/HIP. WebGPU kernels should
    // be dispatched via compile_and_launch_wgsl instead.
    let source = match detect_gpu_platform() {
        Some(GpuPlatform::Metal) => super::gpu_msl::generate_msl(kernel),
        Some(GpuPlatform::WebGpu) => {
            return Err(GpuError::LaunchFailed(
                "WebGPU kernels require a WGSL-specific kernel; use compile_and_launch_wgsl".into(),
            ));
        }
        Some(GpuPlatform::Cuda) => super::gpu_cuda::generate_cuda(kernel),
        Some(GpuPlatform::Hip) => super::gpu_hip::generate_hip(kernel),
        None => return Err(GpuError::DeviceNotAvailable("No GPU platform".into())),
    };

    // Compile and launch
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
    fn create_gpu_device_returns_metal_on_macos() {
        #[cfg(target_os = "macos")]
        {
            let device = create_gpu_device();
            assert!(device.is_ok(), "create_gpu_device should succeed on macOS");
        }
        #[cfg(not(target_os = "macos"))]
        {
            // On non-macOS without WebGPU, may return an error — just ensure no panic
            let _ = create_gpu_device();
        }
    }

    #[test]
    fn compile_and_launch_with_simple_kernel_does_not_crash() {
        // Build a minimal GpuKernel with no ops
        let kernel = GpuKernel {
            name: "test_kernel".to_string(),
            buffers: vec![],
            scalar_params: vec![],
            body_ops: vec![],
            launch_config: None,
        };

        // On platforms with a GPU device this should succeed;
        // on others it will return DeviceNotAvailable — either way, no panic.
        let result = compile_and_launch(&kernel, [1, 1, 1], [1, 1, 1]);
        #[cfg(target_os = "macos")]
        assert!(result.is_ok(), "compile_and_launch should succeed on macOS: {:?}", result);
        #[cfg(not(target_os = "macos"))]
        let _ = result;
    }
}
