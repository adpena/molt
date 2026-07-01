//! Device traits: Allocator, Compiler, Executor.
//!
//! Each backend implements all three traits. The separation provides
//! distinct ownership semantics (buffers vs programs vs execution state).

use std::alloc::{Layout, alloc_zeroed, dealloc, handle_alloc_error};
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

#[cfg(feature = "ane-backend")]
pub mod ane;
pub mod arena;
pub mod cpu;
#[cfg(target_os = "macos")]
pub mod metal;
#[cfg(feature = "opencl-backend")]
pub mod opencl;
#[cfg(feature = "wasm-backend")]
pub mod wasm_cpu;
#[cfg(feature = "webgl2-backend")]
pub mod webgl2;
#[cfg(feature = "webgpu-backend")]
pub mod webgpu;

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
    /// CPU buffer with explicit allocation-layout custody.
    Cpu(CpuBuffer),
    /// Metal buffer (raw pointer to MTLBuffer).
    #[cfg(target_os = "macos")]
    Metal(*mut std::ffi::c_void),
}

// SAFETY: Metal buffers are Send+Sync when accessed through command buffers.
unsafe impl Send for BufferHandle {}
unsafe impl Sync for BufferHandle {}

/// Owned CPU byte allocation with exact layout custody.
///
/// `Vec<u8>` may only deallocate with `u8`'s natural layout. CPU backend
/// buffers need stronger alignments (16-byte SIMD lanes and 4096-byte page
/// alignment), so the owner must retain the original [`Layout`] and release it
/// with the same alignment.
#[derive(Debug)]
pub(crate) struct CpuBuffer {
    ptr: NonNull<u8>,
    len: usize,
    layout: Option<Layout>,
}

impl CpuBuffer {
    pub(crate) fn empty() -> Self {
        Self {
            ptr: NonNull::dangling(),
            len: 0,
            layout: None,
        }
    }

    pub(crate) fn zeroed(size_bytes: usize, align: usize) -> Self {
        assert!(
            align.is_power_of_two(),
            "CPU buffer alignment must be a power of two"
        );
        if size_bytes == 0 {
            return Self::empty();
        }
        let layout = Layout::from_size_align(size_bytes, align)
            .expect("invalid CPU buffer allocation layout");
        // SAFETY: `layout` is nonzero and valid. The matching `Drop`
        // implementation deallocates with the identical layout.
        unsafe {
            let ptr = alloc_zeroed(layout);
            if ptr.is_null() {
                handle_alloc_error(layout);
            }
            Self {
                ptr: NonNull::new_unchecked(ptr),
                len: size_bytes,
                layout: Some(layout),
            }
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        // SAFETY: `ptr` owns `len` initialized bytes for nonempty buffers; for
        // empty buffers the dangling pointer is valid for zero-length slices.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    pub(crate) fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: `&mut self` proves unique access to the owned allocation.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    #[cfg(feature = "webgpu-backend")]
    #[cfg(feature = "webgpu-backend")]
    pub(crate) fn from_bytes(bytes: &[u8]) -> Self {
        let buf = Self::zeroed(bytes.len(), 16);
        buf.copy_from(bytes)
            .expect("new CPU buffer has exact source byte length");
        buf
    }

    pub(crate) fn copy_from(&self, data: &[u8]) -> Result<(), DeviceError> {
        if data.len() > self.len {
            return Err(DeviceError::InvalidArgument(format!(
                "copy_in: data ({} bytes) exceeds buffer ({} bytes)",
                data.len(),
                self.len
            )));
        }
        // SAFETY: `data.len() <= self.len` and CPU copy operations are
        // serialized by the backend device contract. This preserves the
        // existing copy_in(&DeviceBuffer) API without forging a Vec owner.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), self.ptr.as_ptr(), data.len());
        }
        Ok(())
    }
}

impl Deref for CpuBuffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl DerefMut for CpuBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl Drop for CpuBuffer {
    fn drop(&mut self) {
        if let Some(layout) = self.layout {
            // SAFETY: `ptr` was allocated with this exact layout in
            // `CpuBuffer::zeroed` and has not been deallocated elsewhere.
            unsafe {
                dealloc(self.ptr.as_ptr(), layout);
            }
        }
    }
}

// SAFETY: CpuBuffer owns its allocation. Shared mutation is only exposed
// through backend copy operations that uphold the device synchronization
// contract documented on Allocator.
unsafe impl Send for CpuBuffer {}
unsafe impl Sync for CpuBuffer {}

/// Compiled program handle for a backend-specific shader or kernel.
#[derive(Debug)]
pub struct CompiledProgram {
    /// Backend-specific compiled program handle.
    #[allow(dead_code)]
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
