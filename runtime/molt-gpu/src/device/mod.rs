//! Device traits: Allocator, Compiler, Executor.
//!
//! Each backend implements all three traits. The separation provides
//! distinct ownership semantics (buffers vs programs vs execution state).

pub mod arena;
#[cfg(target_os = "macos")]
pub mod metal;
pub mod cpu;
#[cfg(feature = "wasm-backend")]
pub mod wasm_cpu;
#[cfg(feature = "webgpu-backend")]
pub mod webgpu;
#[cfg(feature = "webgl2-backend")]
pub mod webgl2;
#[cfg(feature = "opencl-backend")]
pub mod opencl;

/// Opaque GPU buffer handle backed by a device-specific implementation.
#[derive(Debug)]
pub struct DeviceBuffer {
    /// Backend-specific opaque handle (pointer, id, etc.).
    pub(crate) handle: BufferHandle,
    /// Size in bytes.
    pub size_bytes: usize,
}

/// Backend-specific buffer handle.
#[derive(Debug)]
pub(crate) enum BufferHandle {
    /// CPU buffer (owned Vec<u8>).
    Cpu(Vec<u8>),
    /// Metal buffer (raw pointer to MTLBuffer).
    #[cfg(target_os = "macos")]
    Metal(*mut std::ffi::c_void),
}

// SAFETY: Metal buffers are Send+Sync when accessed through command buffers.
unsafe impl Send for BufferHandle {}
unsafe impl Sync for BufferHandle {}

/// Compiled program handle for a backend-specific shader or kernel.
#[derive(Debug)]
pub struct CompiledProgram {
    /// Backend-specific compiled program handle.
    pub(crate) handle: ProgramHandle,
    /// Entry point function name.
    pub entry: String,
}

/// Backend-specific program handle.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) enum ProgramHandle {
    /// CPU: compiled function pointer.
    Cpu(CpuKernelFn),
    /// Metal: raw pointer to MTLComputePipelineState.
    #[cfg(target_os = "macos")]
    Metal(*mut std::ffi::c_void),
}

// SAFETY: Metal pipeline state objects are thread-safe once created.
// CPU function pointers are inherently Send+Sync.
unsafe impl Send for ProgramHandle {}
unsafe impl Sync for ProgramHandle {}

/// CPU kernel function type.
pub(crate) type CpuKernelFn = fn(bufs: &[&[u8]], out: &mut [u8], num_elements: usize);

/// Device error type.
#[derive(Debug)]
pub enum DeviceError {
    /// Buffer allocation failed.
    AllocationFailed(String),
    /// Compilation failed.
    CompilationFailed(String),
    /// Execution failed.
    ExecutionFailed(String),
    /// Invalid argument.
    InvalidArgument(String),
    /// Out of memory.
    OutOfMemory,
}

impl std::fmt::Display for DeviceError {
    #[cold]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AllocationFailed(msg) => write!(f, "allocation failed: {}", msg),
            Self::CompilationFailed(msg) => write!(f, "compilation failed: {}", msg),
            Self::ExecutionFailed(msg) => write!(f, "execution failed: {}", msg),
            Self::InvalidArgument(msg) => write!(f, "invalid argument: {}", msg),
            Self::OutOfMemory => write!(f, "out of memory"),
        }
    }
}

impl std::error::Error for DeviceError {}

/// Memory management trait. Owns buffer lifetimes.
/// SAFETY CONTRACT: free() internally synchronizes before releasing GPU memory.
pub trait Allocator: Send + Sync {
    fn alloc(&self, size_bytes: usize) -> Result<DeviceBuffer, DeviceError>;
    fn free(&self, buf: DeviceBuffer) -> Result<(), DeviceError>;
    fn copy_in(&self, buf: &DeviceBuffer, data: &[u8]) -> Result<(), DeviceError>;
    fn copy_out(&self, buf: &DeviceBuffer, data: &mut [u8]) -> Result<(), DeviceError>;
}

/// Kernel compilation trait. Owns compiled program cache internally.
pub trait Compiler: Send + Sync {
    fn compile(&self, source: &str, entry: &str) -> Result<CompiledProgram, DeviceError>;

    /// Maximum local (threadgroup/workgroup) size per dimension.
    fn max_local_size(&self) -> [u32; 3];
    /// Maximum grid size per dimension.
    fn max_grid_size(&self) -> [u32; 3];
}

/// Kernel execution trait.
pub trait Executor: Send + Sync {
    fn exec(
        &self,
        prog: &CompiledProgram,
        bufs: &[&DeviceBuffer],
        grid: [u32; 3],
        local: [u32; 3],
    ) -> Result<(), DeviceError>;
    fn synchronize(&self) -> Result<(), DeviceError>;
}
