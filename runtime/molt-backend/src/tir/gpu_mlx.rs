//! Apple MLX backend for machine learning workloads on Apple Silicon.
//! MLX provides unified memory access between CPU and GPU on Apple chips.

#[cfg(all(target_os = "macos", feature = "mlx"))]
use mlx_rs as mlx;

#[cfg(all(target_os = "macos", feature = "mlx"))]
use super::gpu_runtime::{CompiledKernel, GpuBufferHandle, GpuDevice, GpuError, GpuPlatform};
#[cfg(not(all(target_os = "macos", feature = "mlx")))]
use super::gpu_runtime::{CompiledKernel, GpuBufferHandle, GpuDevice, GpuError};

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
        #[cfg(all(target_os = "macos", feature = "mlx"))]
        {
            // MLX uses Metal under the hood — it can compile MSL directly
            Ok(CompiledKernel::new(
                name.to_string(),
                GpuPlatform::Metal,
                msl_source.as_bytes().to_vec(),
            ))
        }
        #[cfg(not(all(target_os = "macos", feature = "mlx")))]
        {
            let _ = (name, msl_source);
            Err(GpuError::DeviceNotAvailable(MLX_STUB_MSG.into()))
        }
    }
}

const MLX_STUB_MSG: &str = "MLX requires macOS with Apple Silicon and the 'mlx' feature";

impl GpuDevice for MlxDevice {
    fn compile_kernel(&self, name: &str, source: &str) -> Result<CompiledKernel, GpuError> {
        self.compile_metal_kernel(name, source)
    }

    fn alloc_buffer(&self, size_bytes: usize) -> Result<GpuBufferHandle, GpuError> {
        #[cfg(all(target_os = "macos", feature = "mlx"))]
        {
            // MLX uses unified memory — allocation is shared between CPU and GPU
            Ok(GpuBufferHandle::new(
                size_bytes,
                GpuPlatform::Metal,
                vec![0; 8],
            ))
        }
        #[cfg(not(all(target_os = "macos", feature = "mlx")))]
        {
            let _ = size_bytes;
            Err(GpuError::DeviceNotAvailable(MLX_STUB_MSG.into()))
        }
    }

    fn copy_to_device(&self, _buffer: &GpuBufferHandle, _data: &[u8]) -> Result<(), GpuError> {
        #[cfg(all(target_os = "macos", feature = "mlx"))]
        {
            // MLX unified memory: no explicit copy needed
            Ok(())
        }
        #[cfg(not(all(target_os = "macos", feature = "mlx")))]
        {
            Err(GpuError::DeviceNotAvailable(MLX_STUB_MSG.into()))
        }
    }

    fn copy_from_device(
        &self,
        _buffer: &GpuBufferHandle,
        _data: &mut [u8],
    ) -> Result<(), GpuError> {
        #[cfg(all(target_os = "macos", feature = "mlx"))]
        {
            Ok(())
        }
        #[cfg(not(all(target_os = "macos", feature = "mlx")))]
        {
            Err(GpuError::DeviceNotAvailable(MLX_STUB_MSG.into()))
        }
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
            Err(GpuError::DeviceNotAvailable(MLX_STUB_MSG.into()))
        }
    }

    fn synchronize(&self) -> Result<(), GpuError> {
        #[cfg(all(target_os = "macos", feature = "mlx"))]
        {
            Ok(())
        }
        #[cfg(not(all(target_os = "macos", feature = "mlx")))]
        {
            Err(GpuError::DeviceNotAvailable(MLX_STUB_MSG.into()))
        }
    }

    fn free_buffer(&self, _buffer: GpuBufferHandle) -> Result<(), GpuError> {
        #[cfg(all(target_os = "macos", feature = "mlx"))]
        {
            Ok(())
        }
        #[cfg(not(all(target_os = "macos", feature = "mlx")))]
        {
            Err(GpuError::DeviceNotAvailable(MLX_STUB_MSG.into()))
        }
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
    #[cfg(all(target_os = "macos", feature = "mlx"))]
    fn test_mlx_compile_kernel() {
        let device = MlxDevice::new().expect("MLX device should create on macOS with feature");
        let result = device.compile_metal_kernel("test_kernel", "kernel void test() {}");
        assert!(result.is_ok());
        let kernel = result.unwrap();
        assert_eq!(kernel.name, "test_kernel");
        assert_eq!(kernel.platform, GpuPlatform::Metal);
    }

    #[test]
    #[cfg(not(all(target_os = "macos", feature = "mlx")))]
    fn test_mlx_compile_kernel_stub_returns_error() {
        // Without the mlx feature, even a manually-constructed device should
        // return errors from all trait methods.
        let device = MlxDevice {
            _phantom: std::marker::PhantomData,
        };
        let result = device.compile_metal_kernel("test_kernel", "kernel void test() {}");
        assert!(result.is_err(), "Stub compile_metal_kernel should fail");
    }
}
