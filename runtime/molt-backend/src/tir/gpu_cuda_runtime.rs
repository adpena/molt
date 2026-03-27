//! CUDA runtime device implementation.
//!
//! Provides a `CudaDevice` that wraps `cudarc::driver::CudaDevice` for
//! compiling and launching CUDA kernels at runtime.
//!
//! Enabled by the `gpu-cuda` feature flag. Without the feature, a
//! non-functional stub is provided that always returns `DeviceNotAvailable`.

/// CUDA device — real implementation backed by cudarc.
#[cfg(feature = "gpu-cuda")]
pub struct CudaDevice {
    inner: std::sync::Arc<cudarc::driver::CudaDevice>,
}

#[cfg(feature = "gpu-cuda")]
impl CudaDevice {
    /// Open CUDA device 0.
    ///
    /// Returns `GpuError::DeviceNotAvailable` if no CUDA-capable device is
    /// present or the driver is not installed.
    pub fn new() -> Result<Self, super::gpu_runtime::GpuError> {
        match cudarc::driver::CudaDevice::new(0) {
            Ok(dev) => Ok(Self { inner: dev }),
            Err(e) => Err(super::gpu_runtime::GpuError::DeviceNotAvailable(format!(
                "CUDA init failed: {e}"
            ))),
        }
    }

    /// Return a reference to the underlying cudarc device.
    pub fn inner(&self) -> &std::sync::Arc<cudarc::driver::CudaDevice> {
        &self.inner
    }
}

/// Stub `CudaDevice` used when the `gpu-cuda` feature is disabled.
///
/// All methods return `GpuError::DeviceNotAvailable` immediately so that
/// code that references this type still compiles without the feature.
#[cfg(not(feature = "gpu-cuda"))]
pub struct CudaDevice {
    _private: (),
}

#[cfg(not(feature = "gpu-cuda"))]
impl CudaDevice {
    /// Always returns `DeviceNotAvailable` — CUDA support not compiled in.
    pub fn new() -> Result<Self, super::gpu_runtime::GpuError> {
        Err(super::gpu_runtime::GpuError::DeviceNotAvailable(
            "molt-backend was compiled without the `gpu-cuda` feature".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without the feature, `CudaDevice::new()` must return an error — never
    /// panic. With the feature enabled the test is skipped (it would require
    /// an actual GPU in the test environment).
    #[test]
    #[cfg(not(feature = "gpu-cuda"))]
    fn stub_returns_device_not_available() {
        let result = CudaDevice::new();
        assert!(
            result.is_err(),
            "stub CudaDevice::new() must return Err when feature is disabled"
        );
    }
}
