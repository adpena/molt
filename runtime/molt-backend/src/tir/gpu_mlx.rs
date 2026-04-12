//! Apple MLX backend stub for Apple Silicon.
//!
//! This module is intentionally honest: Molt currently does not route any
//! runtime GPU execution through MLX, so every API here reports a clear
//! `DeviceNotAvailable` error. The module exists so the feature gate and
//! future backend integration points are explicit and testable.

use super::gpu_runtime::{CompiledKernel, GpuBufferHandle, GpuDevice, GpuError};

/// MLX backend is not implemented yet in Molt.
pub const MLX_STUB_MSG: &str =
    "MLX backend is stubbed in Molt; no runtime MLX execution is wired yet";

/// MLX device placeholder.
#[derive(Debug)]
pub struct MlxDevice {
    _phantom: std::marker::PhantomData<()>,
}

impl MlxDevice {
    pub fn new() -> Result<Self, GpuError> {
        Err(GpuError::DeviceNotAvailable(MLX_STUB_MSG.into()))
    }

    /// Returns `false` until real MLX lowering and execution are implemented.
    pub const fn is_real_backend() -> bool {
        false
    }

    /// Compile a kernel via MLX.
    ///
    /// This is a stub today: real MLX compilation/execution is not wired.
    pub fn compile_metal_kernel(
        &self,
        name: &str,
        msl_source: &str,
    ) -> Result<CompiledKernel, GpuError> {
        let _ = (name, msl_source);
        Err(GpuError::DeviceNotAvailable(MLX_STUB_MSG.into()))
    }
}

impl GpuDevice for MlxDevice {
    fn compile_kernel(&self, name: &str, source: &str) -> Result<CompiledKernel, GpuError> {
        self.compile_metal_kernel(name, source)
    }

    fn alloc_buffer(&self, size_bytes: usize) -> Result<GpuBufferHandle, GpuError> {
        let _ = size_bytes;
        Err(GpuError::DeviceNotAvailable(MLX_STUB_MSG.into()))
    }

    fn copy_to_device(&self, _buffer: &GpuBufferHandle, _data: &[u8]) -> Result<(), GpuError> {
        Err(GpuError::DeviceNotAvailable(MLX_STUB_MSG.into()))
    }

    fn copy_from_device(
        &self,
        _buffer: &GpuBufferHandle,
        _data: &mut [u8],
    ) -> Result<(), GpuError> {
        Err(GpuError::DeviceNotAvailable(MLX_STUB_MSG.into()))
    }

    fn launch_kernel(
        &self,
        _kernel: &CompiledKernel,
        _grid: [u32; 3],
        _block: [u32; 3],
        _buffers: &[&GpuBufferHandle],
    ) -> Result<(), GpuError> {
        Err(GpuError::DeviceNotAvailable(MLX_STUB_MSG.into()))
    }

    fn synchronize(&self) -> Result<(), GpuError> {
        Err(GpuError::DeviceNotAvailable(MLX_STUB_MSG.into()))
    }

    fn free_buffer(&self, _buffer: GpuBufferHandle) -> Result<(), GpuError> {
        Err(GpuError::DeviceNotAvailable(MLX_STUB_MSG.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mlx_device_is_stub_only() {
        let result = MlxDevice::new();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            format!("{err}").contains("MLX backend is stubbed"),
            "Error should make the stub-only status explicit"
        );
    }

    #[test]
    fn test_mlx_trait_methods_return_stub_error() {
        let device = MlxDevice {
            _phantom: std::marker::PhantomData,
        };
        let err = format!(
            "{:?}",
            device
                .compile_metal_kernel("test_kernel", "kernel void test() {}")
                .unwrap_err()
        );
        assert!(err.contains("MLX backend is stubbed"));

        let buf =
            GpuBufferHandle::new(8, super::super::gpu_runtime::GpuPlatform::Metal, vec![0; 8]);
        let mut bytes = [0u8; 8];
        let methods = [
            device.alloc_buffer(8).map(|_| ()),
            device.copy_to_device(&buf, &bytes),
            device.copy_from_device(&buf, &mut bytes),
            device.launch_kernel(
                &CompiledKernel::new(
                    "k".into(),
                    super::super::gpu_runtime::GpuPlatform::Metal,
                    vec![],
                ),
                [1, 1, 1],
                [1, 1, 1],
                &[&buf],
            ),
            device.synchronize(),
            device.free_buffer(buf),
        ];
        for result in methods {
            let err = result.unwrap_err();
            assert!(
                format!("{err}").contains("MLX backend is stubbed"),
                "all MLX trait methods should report the same stub-only error"
            );
        }
    }

    #[test]
    fn test_mlx_backend_flag_is_false() {
        assert!(!MlxDevice::is_real_backend());
    }
}
