//! GPU runtime dispatch stubs.
//!
//! Provides the interface for compiling and launching GPU kernels at runtime.
//! Platform-specific implementations (Metal, WebGPU, CUDA, HIP) are behind
//! feature flags and loaded dynamically.

/// GPU device abstraction.
pub trait GpuDevice {
    /// Compile a kernel from source code.
    fn compile_kernel(&self, name: &str, source: &str) -> Result<CompiledKernel, GpuError>;
    /// Allocate a buffer on the GPU.
    fn alloc_buffer(&self, size_bytes: usize) -> Result<GpuBufferHandle, GpuError>;
    /// Copy data from host to GPU buffer.
    fn copy_to_device(&self, buffer: &GpuBufferHandle, data: &[u8]) -> Result<(), GpuError>;
    /// Copy data from GPU buffer to host.
    fn copy_from_device(&self, buffer: &GpuBufferHandle, data: &mut [u8])
        -> Result<(), GpuError>;
    /// Launch a compiled kernel.
    fn launch_kernel(
        &self,
        kernel: &CompiledKernel,
        grid: [u32; 3],
        block: [u32; 3],
        buffers: &[&GpuBufferHandle],
    ) -> Result<(), GpuError>;
    /// Wait for all GPU operations to complete.
    fn synchronize(&self) -> Result<(), GpuError>;
    /// Free a GPU buffer.
    fn free_buffer(&self, buffer: GpuBufferHandle) -> Result<(), GpuError>;
}

/// Opaque handle to a compiled GPU kernel.
#[derive(Debug)]
pub struct CompiledKernel {
    pub name: String,
    pub platform: GpuPlatform,
    /// Platform-specific handle (opaque bytes).
    _handle: Vec<u8>,
}

impl CompiledKernel {
    /// Create a new compiled kernel handle.
    pub fn new(name: String, platform: GpuPlatform, handle: Vec<u8>) -> Self {
        Self {
            name,
            platform,
            _handle: handle,
        }
    }
}

/// Opaque handle to a GPU buffer.
#[derive(Debug)]
pub struct GpuBufferHandle {
    pub size_bytes: usize,
    pub platform: GpuPlatform,
    _handle: Vec<u8>,
}

impl GpuBufferHandle {
    /// Create a new GPU buffer handle.
    pub fn new(size_bytes: usize, platform: GpuPlatform, handle: Vec<u8>) -> Self {
        Self {
            size_bytes,
            platform,
            _handle: handle,
        }
    }
}

/// Supported GPU platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuPlatform {
    Metal,
    WebGpu,
    Cuda,
    Hip,
}

/// Errors returned by GPU operations.
#[derive(Debug)]
pub enum GpuError {
    CompilationFailed(String),
    AllocationFailed(String),
    TransferFailed(String),
    LaunchFailed(String),
    DeviceNotAvailable(String),
}

impl std::fmt::Display for GpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CompilationFailed(s) => write!(f, "GPU compilation failed: {s}"),
            Self::AllocationFailed(s) => write!(f, "GPU allocation failed: {s}"),
            Self::TransferFailed(s) => write!(f, "GPU transfer failed: {s}"),
            Self::LaunchFailed(s) => write!(f, "GPU launch failed: {s}"),
            Self::DeviceNotAvailable(s) => write!(f, "GPU device not available: {s}"),
        }
    }
}

impl std::error::Error for GpuError {}

/// Detect the best available GPU platform.
pub fn detect_gpu_platform() -> Option<GpuPlatform> {
    // Platform detection order (matching spec priorities):
    // 1. Metal (macOS)
    // 2. WebGPU (browser/wgpu)
    // 3. CUDA (NVIDIA)
    // 4. HIP (AMD)

    #[cfg(target_os = "macos")]
    return Some(GpuPlatform::Metal);

    #[cfg(target_arch = "wasm32")]
    return Some(GpuPlatform::WebGpu);

    #[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
    {
        // Check for CUDA/HIP availability at runtime
        // For now: return None (no GPU available)
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_gpu_platform_returns_some_on_macos() {
        // This test is platform-specific; on macOS it should return Metal.
        let platform = detect_gpu_platform();
        #[cfg(target_os = "macos")]
        assert_eq!(platform, Some(GpuPlatform::Metal));
        #[cfg(not(target_os = "macos"))]
        let _ = platform; // just ensure it doesn't panic
    }

    #[test]
    fn gpu_error_display_formatting() {
        let err = GpuError::CompilationFailed("syntax error".into());
        assert_eq!(format!("{err}"), "GPU compilation failed: syntax error");

        let err = GpuError::AllocationFailed("out of memory".into());
        assert_eq!(format!("{err}"), "GPU allocation failed: out of memory");

        let err = GpuError::TransferFailed("bus error".into());
        assert_eq!(format!("{err}"), "GPU transfer failed: bus error");

        let err = GpuError::LaunchFailed("invalid grid".into());
        assert_eq!(format!("{err}"), "GPU launch failed: invalid grid");

        let err = GpuError::DeviceNotAvailable("no GPU".into());
        assert_eq!(format!("{err}"), "GPU device not available: no GPU");
    }

    #[test]
    fn compiled_kernel_construction() {
        let kernel = CompiledKernel::new(
            "matmul".into(),
            GpuPlatform::Metal,
            vec![0xDE, 0xAD],
        );
        assert_eq!(kernel.name, "matmul");
        assert_eq!(kernel.platform, GpuPlatform::Metal);
    }

    #[test]
    fn gpu_buffer_handle_construction() {
        let buf = GpuBufferHandle::new(4096, GpuPlatform::Cuda, vec![0x01, 0x02]);
        assert_eq!(buf.size_bytes, 4096);
        assert_eq!(buf.platform, GpuPlatform::Cuda);
    }
}
