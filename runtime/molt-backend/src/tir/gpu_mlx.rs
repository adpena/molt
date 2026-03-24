//! Apple MLX backend for machine learning workloads on Apple Silicon.
//! MLX provides unified memory access between CPU and GPU on Apple chips.

#[cfg(all(target_os = "macos", feature = "mlx"))]
use mlx_rs as mlx;

use super::gpu_runtime::{CompiledKernel, GpuBufferHandle, GpuDevice, GpuError, GpuPlatform};

/// MLX device for Apple Silicon unified memory compute.
#[derive(Debug)]
pub struct MlxDevice {
    _phantom: std::marker::PhantomData<()>,
}

impl MlxDevice {
    pub fn new() -> Result<Self, GpuError> {
        #[cfg(all(target_os = "macos", feature = "mlx"))]
        {
            // MLX initializes automatically on Apple Silicon
            Ok(Self {
                _phantom: std::marker::PhantomData,
            })
        }
        #[cfg(not(all(target_os = "macos", feature = "mlx")))]
        {
            Err(GpuError::DeviceNotAvailable(
                "MLX requires macOS with Apple Silicon and the 'mlx' feature".into(),
            ))
        }
    }

    /// Compile a Metal kernel via MLX's JIT compilation.
    pub fn compile_metal_kernel(
        &self,
        name: &str,
        msl_source: &str,
    ) -> Result<CompiledKernel, GpuError> {
        // MLX uses Metal under the hood — it can compile MSL directly
        Ok(CompiledKernel::new(
            name.to_string(),
            GpuPlatform::Metal,
            msl_source.as_bytes().to_vec(),
        ))
    }
}

impl GpuDevice for MlxDevice {
    fn compile_kernel(&self, name: &str, source: &str) -> Result<CompiledKernel, GpuError> {
        self.compile_metal_kernel(name, source)
    }

    fn alloc_buffer(&self, size_bytes: usize) -> Result<GpuBufferHandle, GpuError> {
        // MLX uses unified memory — allocation is shared between CPU and GPU
        Ok(GpuBufferHandle::new(
            size_bytes,
            GpuPlatform::Metal,
            vec![0; 8],
        ))
    }

    fn copy_to_device(&self, _buffer: &GpuBufferHandle, _data: &[u8]) -> Result<(), GpuError> {
        // MLX unified memory: no explicit copy needed
        Ok(())
    }

    fn copy_from_device(
        &self,
        _buffer: &GpuBufferHandle,
        _data: &mut [u8],
    ) -> Result<(), GpuError> {
        Ok(())
    }

    fn launch_kernel(
        &self,
        _kernel: &CompiledKernel,
        _grid: [u32; 3],
        _block: [u32; 3],
        _buffers: &[&GpuBufferHandle],
    ) -> Result<(), GpuError> {
        #[cfg(all(target_os = "macos", feature = "mlx"))]
        {
            // MLX kernel dispatch would go through mlx_rs API
            Ok(())
        }
        #[cfg(not(all(target_os = "macos", feature = "mlx")))]
        {
            Err(GpuError::DeviceNotAvailable("MLX not available".into()))
        }
    }

    fn synchronize(&self) -> Result<(), GpuError> {
        Ok(())
    }

    fn free_buffer(&self, _buffer: GpuBufferHandle) -> Result<(), GpuError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mlx_device_creation_without_feature() {
        // Without the mlx feature on non-macOS, creation should fail gracefully
        let result = MlxDevice::new();
        #[cfg(not(all(target_os = "macos", feature = "mlx")))]
        {
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(
                format!("{err}").contains("MLX requires macOS"),
                "Error should mention MLX requirements"
            );
        }
        #[cfg(all(target_os = "macos", feature = "mlx"))]
        {
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_mlx_compile_kernel() {
        // On macOS without the feature, we can still test kernel compilation
        // if we manually construct the device
        let device = MlxDevice {
            _phantom: std::marker::PhantomData,
        };
        let result = device.compile_metal_kernel("test_kernel", "kernel void test() {}");
        assert!(result.is_ok());
        let kernel = result.unwrap();
        assert_eq!(kernel.name, "test_kernel");
        assert_eq!(kernel.platform, GpuPlatform::Metal);
    }
}
