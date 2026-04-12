//! Metal GPU device implementation (macOS only).
//!
//! When built with the `gpu-metal` feature on macOS, this module provides a real
//! Metal device that compiles MSL kernels at runtime, allocates GPU buffers, and
//! dispatches compute work to Apple Silicon / discrete AMD GPUs.
//!
//! Without the feature (or on non-macOS), a lightweight stub is provided so the
//! rest of the codebase can reference `MetalDevice` without conditional imports.

#[cfg(target_os = "macos")]
use super::gpu_runtime::*;

// ─── Real Metal implementation ──────────────────────────────────────────────

#[cfg(all(target_os = "macos", feature = "gpu-metal"))]
mod real {
    use super::*;
    use metal::{
        Buffer as MetalBuffer, CommandQueue, CompileOptions, ComputePipelineState, Device, Library,
        MTLResourceOptions, MTLSize, NSUInteger,
    };
    use std::sync::Arc;

    /// A compiled Metal compute pipeline, kept alive via `Arc`.
    struct MetalPipeline {
        pipeline: ComputePipelineState,
        #[allow(dead_code)]
        library: Library, // prevent library from being dropped
    }

    // Safety: Metal objects are ref-counted Objective-C objects; the runtime
    // guarantees thread-safe retain/release. We only send immutable refs across
    // threads (command buffer encoding is single-threaded per encoder).
    unsafe impl Send for MetalPipeline {}
    unsafe impl Sync for MetalPipeline {}

    /// Real Metal GPU device backed by `metal-rs`.
    pub struct MetalDevice {
        device: Device,
        command_queue: CommandQueue,
    }

    impl MetalDevice {
        fn metal_buffer_from_handle<'a>(
            &self,
            buffer: &'a GpuBufferHandle,
        ) -> Result<&'a MetalBuffer, GpuError> {
            if buffer.platform != GpuPlatform::Metal {
                return Err(GpuError::TransferFailed(format!(
                    "Expected Metal buffer handle, got {:?}",
                    buffer.platform
                )));
            }
            let ptr_val =
                usize::from_ne_bytes(buffer._handle_bytes().try_into().map_err(|_| {
                    GpuError::TransferFailed(format!(
                        "Invalid buffer handle width: expected {} bytes, got {}",
                        std::mem::size_of::<usize>(),
                        buffer._handle_bytes().len()
                    ))
                })?);
            if ptr_val == 0 {
                return Err(GpuError::TransferFailed("Null Metal buffer handle".into()));
            }
            Ok(unsafe { &*(ptr_val as *const MetalBuffer) })
        }

        /// Create a Metal device using the system default GPU.
        pub fn new() -> Result<Self, GpuError> {
            let device = Device::system_default()
                .ok_or(GpuError::DeviceNotAvailable("No Metal device found".into()))?;
            let command_queue = device.new_command_queue();
            Ok(Self {
                device,
                command_queue,
            })
        }

        /// Return the underlying Metal device name (useful for diagnostics).
        pub fn device_name(&self) -> String {
            self.device.name().to_string()
        }

        /// Compile MSL source code into a Metal compute pipeline.
        ///
        /// `name` is the kernel function entry point inside the MSL source.
        pub fn compile_msl(
            &self,
            name: &str,
            msl_source: &str,
        ) -> Result<CompiledKernel, GpuError> {
            let options = CompileOptions::new();
            let library = self
                .device
                .new_library_with_source(msl_source, &options)
                .map_err(|e| GpuError::CompilationFailed(format!("MSL compile error: {e}")))?;

            let function = library.get_function(name, None).map_err(|e| {
                GpuError::CompilationFailed(format!("Function '{name}' not found: {e}"))
            })?;

            let pipeline = self
                .device
                .new_compute_pipeline_state_with_function(&function)
                .map_err(|e| {
                    GpuError::CompilationFailed(format!("Pipeline creation failed: {e}"))
                })?;

            // Store the pipeline in an Arc so it stays alive.
            let handle = Arc::new(MetalPipeline { pipeline, library });
            // Leak the Arc into raw bytes so we can stash it in CompiledKernel._handle.
            let raw = Arc::into_raw(handle);
            let bytes = (raw as usize).to_ne_bytes().to_vec();

            Ok(CompiledKernel::new(
                name.to_string(),
                GpuPlatform::Metal,
                bytes,
            ))
        }

        /// Allocate a GPU-visible buffer of `size_bytes`.
        pub fn alloc_buffer(&self, size_bytes: usize) -> Result<MetalBuffer, GpuError> {
            let buf = self
                .device
                .new_buffer(size_bytes as u64, MTLResourceOptions::StorageModeShared);
            Ok(buf)
        }

        /// Launch a compiled kernel.
        ///
        /// `kernel_handle` must have been produced by `compile_msl` on this device.
        /// `buffers` are Metal buffers bound at consecutive indices starting from 0.
        pub fn dispatch(
            &self,
            kernel: &CompiledKernel,
            grid: [u32; 3],
            block: [u32; 3],
            buffers: &[&MetalBuffer],
        ) -> Result<(), GpuError> {
            if grid.contains(&0) {
                return Err(GpuError::LaunchFailed(format!(
                    "Invalid Metal grid dimensions: {:?}",
                    grid
                )));
            }
            if block.contains(&0) {
                return Err(GpuError::LaunchFailed(format!(
                    "Invalid Metal threadgroup dimensions: {:?}",
                    block
                )));
            }
            if grid[1] != 1 || grid[2] != 1 || block[1] != 1 || block[2] != 1 {
                return Err(GpuError::LaunchFailed(format!(
                    "Metal backend currently supports 1D launches only; got grid={:?} block={:?}",
                    grid, block
                )));
            }
            // Recover the Arc<MetalPipeline> from the opaque handle bytes.
            if kernel.platform != GpuPlatform::Metal {
                return Err(GpuError::LaunchFailed(
                    "Kernel is not a Metal kernel".into(),
                ));
            }
            let ptr_val = usize::from_ne_bytes(
                kernel
                    ._handle_bytes()
                    .try_into()
                    .map_err(|_| GpuError::LaunchFailed("Invalid kernel handle".into()))?,
            );
            // Safety: we stored this pointer via Arc::into_raw in compile_msl.
            let arc: Arc<MetalPipeline> = unsafe { Arc::from_raw(ptr_val as *const MetalPipeline) };
            // Clone the arc so we don't drop the pipeline when we're done.
            let pipeline_arc = Arc::clone(&arc);
            // Leak back to keep the original alive.
            std::mem::forget(arc);

            let command_buffer = self.command_queue.new_command_buffer();
            let encoder = command_buffer.new_compute_command_encoder();

            encoder.set_compute_pipeline_state(&pipeline_arc.pipeline);

            for (i, buf) in buffers.iter().enumerate() {
                encoder.set_buffer(i as NSUInteger, Some(*buf), 0);
            }

            let grid_size = MTLSize {
                width: grid[0] as NSUInteger,
                height: grid[1] as NSUInteger,
                depth: grid[2] as NSUInteger,
            };
            let block_size = MTLSize {
                width: block[0] as NSUInteger,
                height: block[1] as NSUInteger,
                depth: block[2] as NSUInteger,
            };

            encoder.dispatch_threads(grid_size, block_size);
            encoder.end_encoding();
            command_buffer.commit();
            command_buffer.wait_until_completed();

            Ok(())
        }
    }

    impl GpuDevice for MetalDevice {
        fn compile_kernel(&self, name: &str, source: &str) -> Result<CompiledKernel, GpuError> {
            self.compile_msl(name, source)
        }
        fn alloc_buffer(&self, size_bytes: usize) -> Result<GpuBufferHandle, GpuError> {
            // Allocate a real Metal buffer and store its pointer in the handle.
            let buf = MetalDevice::alloc_buffer(self, size_bytes)?;
            let raw = Box::into_raw(Box::new(buf));
            let bytes = (raw as usize).to_ne_bytes().to_vec();
            Ok(GpuBufferHandle::new(size_bytes, GpuPlatform::Metal, bytes))
        }
        fn copy_to_device(&self, buffer: &GpuBufferHandle, data: &[u8]) -> Result<(), GpuError> {
            if data.len() > buffer.size_bytes {
                return Err(GpuError::TransferFailed(format!(
                    "Host write overflow: {} bytes into {}-byte Metal buffer",
                    data.len(),
                    buffer.size_bytes
                )));
            }
            let metal_buf = self.metal_buffer_from_handle(buffer)?;
            let contents = metal_buf.contents() as *mut u8;
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr(), contents, data.len());
            }
            Ok(())
        }
        fn copy_from_device(
            &self,
            buffer: &GpuBufferHandle,
            data: &mut [u8],
        ) -> Result<(), GpuError> {
            if data.len() > buffer.size_bytes {
                return Err(GpuError::TransferFailed(format!(
                    "Host read overflow: {} bytes from {}-byte Metal buffer",
                    data.len(),
                    buffer.size_bytes
                )));
            }
            let metal_buf = self.metal_buffer_from_handle(buffer)?;
            let contents = metal_buf.contents() as *const u8;
            unsafe {
                std::ptr::copy_nonoverlapping(contents, data.as_mut_ptr(), data.len());
            }
            Ok(())
        }
        fn launch_kernel(
            &self,
            kernel: &CompiledKernel,
            grid: [u32; 3],
            block: [u32; 3],
            buffers: &[&GpuBufferHandle],
        ) -> Result<(), GpuError> {
            let metal_buffers: Result<Vec<&MetalBuffer>, GpuError> = buffers
                .iter()
                .map(|handle| self.metal_buffer_from_handle(handle))
                .collect();
            self.dispatch(kernel, grid, block, &metal_buffers?)
        }
        fn synchronize(&self) -> Result<(), GpuError> {
            // Metal command buffers are synchronous after wait_until_completed.
            Ok(())
        }
        fn free_buffer(&self, buffer: GpuBufferHandle) -> Result<(), GpuError> {
            if buffer.platform != GpuPlatform::Metal {
                return Err(GpuError::AllocationFailed(format!(
                    "Cannot free non-Metal buffer on Metal device: {:?}",
                    buffer.platform
                )));
            }
            let ptr_val =
                usize::from_ne_bytes(buffer._handle_bytes().try_into().map_err(|_| {
                    GpuError::AllocationFailed(format!(
                        "Invalid buffer handle width: expected {} bytes, got {}",
                        std::mem::size_of::<usize>(),
                        buffer._handle_bytes().len()
                    ))
                })?);
            if ptr_val != 0 {
                // Safety: we stored this pointer via Box::into_raw in alloc_buffer.
                let _: Box<MetalBuffer> = unsafe { Box::from_raw(ptr_val as *mut MetalBuffer) };
            }
            Ok(())
        }
    }
}

#[cfg(all(target_os = "macos", feature = "gpu-metal"))]
pub use real::MetalDevice;

// ─── Stub implementation (non-macOS or feature disabled) ────────────────────

#[cfg(all(target_os = "macos", not(feature = "gpu-metal")))]
pub struct MetalDevice {
    _phantom: std::marker::PhantomData<()>,
}

#[cfg(all(target_os = "macos", not(feature = "gpu-metal")))]
const METAL_STUB_MSG: &str = "Metal GPU support requires the `gpu-metal` feature";

#[cfg(all(target_os = "macos", not(feature = "gpu-metal")))]
impl MetalDevice {
    pub fn new() -> Result<Self, GpuError> {
        Err(GpuError::DeviceNotAvailable(METAL_STUB_MSG.into()))
    }

    pub fn compile_msl(&self, _name: &str, _msl_source: &str) -> Result<CompiledKernel, GpuError> {
        Err(GpuError::DeviceNotAvailable(METAL_STUB_MSG.into()))
    }

    pub fn dispatch(
        &self,
        _kernel: &CompiledKernel,
        _grid: [u32; 3],
        _block: [u32; 3],
        _buffers: &[&GpuBufferHandle],
    ) -> Result<(), GpuError> {
        Err(GpuError::DeviceNotAvailable(METAL_STUB_MSG.into()))
    }
}

#[cfg(all(target_os = "macos", not(feature = "gpu-metal")))]
impl GpuDevice for MetalDevice {
    fn compile_kernel(&self, _name: &str, _source: &str) -> Result<CompiledKernel, GpuError> {
        Err(GpuError::DeviceNotAvailable(METAL_STUB_MSG.into()))
    }
    fn alloc_buffer(&self, _size_bytes: usize) -> Result<GpuBufferHandle, GpuError> {
        Err(GpuError::DeviceNotAvailable(METAL_STUB_MSG.into()))
    }
    fn copy_to_device(&self, _buffer: &GpuBufferHandle, _data: &[u8]) -> Result<(), GpuError> {
        Err(GpuError::DeviceNotAvailable(METAL_STUB_MSG.into()))
    }
    fn copy_from_device(
        &self,
        _buffer: &GpuBufferHandle,
        _data: &mut [u8],
    ) -> Result<(), GpuError> {
        Err(GpuError::DeviceNotAvailable(METAL_STUB_MSG.into()))
    }
    fn launch_kernel(
        &self,
        _kernel: &CompiledKernel,
        _grid: [u32; 3],
        _block: [u32; 3],
        _buffers: &[&GpuBufferHandle],
    ) -> Result<(), GpuError> {
        Err(GpuError::DeviceNotAvailable(METAL_STUB_MSG.into()))
    }
    fn synchronize(&self) -> Result<(), GpuError> {
        Err(GpuError::DeviceNotAvailable(METAL_STUB_MSG.into()))
    }
    fn free_buffer(&self, _buffer: GpuBufferHandle) -> Result<(), GpuError> {
        Err(GpuError::DeviceNotAvailable(METAL_STUB_MSG.into()))
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    use super::*;
    #[cfg(feature = "gpu-metal")]
    use crate::tir::gpu_runtime::GpuDevice;

    #[test]
    #[cfg(feature = "gpu-metal")]
    fn metal_device_creates_successfully() {
        let device = MetalDevice::new().expect("MetalDevice::new should succeed");
        let name = device.device_name();
        assert!(!name.is_empty(), "Metal device should have a name");
    }

    #[test]
    #[cfg(not(feature = "gpu-metal"))]
    fn metal_device_stub_returns_error() {
        let result = MetalDevice::new();
        assert!(result.is_err(), "Stub MetalDevice::new should fail");
    }

    #[test]
    #[cfg(feature = "gpu-metal")]
    fn metal_device_compile_msl_vector_add() {
        let device = MetalDevice::new().expect("MetalDevice::new should succeed");
        let msl = r#"
            #include <metal_stdlib>
            using namespace metal;

            kernel void vector_add(
                device const float* a [[buffer(0)]],
                device const float* b [[buffer(1)]],
                device float* result [[buffer(2)]],
                uint id [[thread_position_in_grid]]
            ) {
                result[id] = a[id] + b[id];
            }
        "#;
        let kernel = device
            .compile_msl("vector_add", msl)
            .expect("compile_msl should succeed for valid MSL");
        assert_eq!(kernel.name, "vector_add");
        assert_eq!(kernel.platform, GpuPlatform::Metal);
    }

    #[test]
    #[cfg(feature = "gpu-metal")]
    fn metal_device_compile_invalid_msl_fails() {
        let device = MetalDevice::new().expect("MetalDevice::new should succeed");
        let bad_msl = "this is not valid MSL code at all!!!";
        let result = device.compile_msl("bad_kernel", bad_msl);
        assert!(result.is_err(), "Invalid MSL should fail compilation");
    }

    #[test]
    #[cfg(feature = "gpu-metal")]
    fn metal_device_compile_wrong_function_name_fails() {
        let device = MetalDevice::new().expect("MetalDevice::new should succeed");
        let msl = r#"
            #include <metal_stdlib>
            using namespace metal;
            kernel void real_name(device float* a [[buffer(0)]],
                                  uint id [[thread_position_in_grid]]) {
                a[id] = 0.0;
            }
        "#;
        let result = device.compile_msl("wrong_name", msl);
        assert!(result.is_err(), "Wrong function name should fail");
    }

    #[test]
    #[cfg(feature = "gpu-metal")]
    fn metal_device_implements_gpu_device_trait() {
        let device = MetalDevice::new().expect("MetalDevice::new should succeed");
        let buf = GpuDevice::alloc_buffer(&device, 1024).expect("alloc_buffer should succeed");
        assert_eq!(buf.size_bytes, 1024);
        assert_eq!(buf.platform, GpuPlatform::Metal);
    }

    #[test]
    #[cfg(feature = "gpu-metal")]
    fn metal_device_compile_and_launch() {
        let device = MetalDevice::new().expect("MetalDevice::new should succeed");
        let msl = r#"
            #include <metal_stdlib>
            using namespace metal;
            kernel void add_one(device float* a [[buffer(0)]],
                                uint id [[thread_position_in_grid]]) {
                a[id] = a[id] + 1.0;
            }
        "#;
        let kernel = device
            .compile_kernel("add_one", msl)
            .expect("compile_kernel should succeed");
        assert_eq!(kernel.name, "add_one");
        assert_eq!(kernel.platform, GpuPlatform::Metal);

        let input: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0];
        let mut output = vec![0u8; input.len() * std::mem::size_of::<f32>()];
        let bytes = unsafe {
            std::slice::from_raw_parts(
                input.as_ptr() as *const u8,
                input.len() * std::mem::size_of::<f32>(),
            )
        };
        let buf =
            GpuDevice::alloc_buffer(&device, bytes.len()).expect("alloc_buffer should succeed");
        device
            .copy_to_device(&buf, bytes)
            .expect("copy_to_device should succeed");
        device
            .launch_kernel(
                &kernel,
                [input.len() as u32, 1, 1],
                [input.len() as u32, 1, 1],
                &[&buf],
            )
            .expect("launch_kernel should not crash");
        device.synchronize().expect("synchronize should succeed");
        device
            .copy_from_device(&buf, &mut output)
            .expect("copy_from_device should succeed");
        device.free_buffer(buf).expect("free_buffer should succeed");

        let out: &[f32] =
            unsafe { std::slice::from_raw_parts(output.as_ptr() as *const f32, input.len()) };
        assert_eq!(out, &[2.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    #[cfg(feature = "gpu-metal")]
    fn metal_device_roundtrip_buffer_transfer() {
        let device = MetalDevice::new().expect("MetalDevice::new should succeed");
        let source: Vec<u8> = vec![1, 5, 9, 13, 17, 21, 25, 29];
        let mut dst = vec![0u8; source.len()];
        let buf =
            GpuDevice::alloc_buffer(&device, source.len()).expect("alloc_buffer should succeed");
        device
            .copy_to_device(&buf, &source)
            .expect("copy_to_device should succeed");
        device
            .copy_from_device(&buf, &mut dst)
            .expect("copy_from_device should succeed");
        device.free_buffer(buf).expect("free_buffer should succeed");
        assert_eq!(dst, source);
    }

    #[test]
    #[cfg(feature = "gpu-metal")]
    fn metal_device_rejects_oversized_copy_to_device() {
        let device = MetalDevice::new().expect("MetalDevice::new should succeed");
        let buf = GpuDevice::alloc_buffer(&device, 4).expect("alloc_buffer should succeed");
        let result = device.copy_to_device(&buf, &[1, 2, 3, 4, 5]);
        assert!(
            result.is_err(),
            "copy_to_device must reject oversized writes"
        );
        device.free_buffer(buf).expect("free_buffer should succeed");
    }

    #[test]
    #[cfg(feature = "gpu-metal")]
    fn metal_device_rejects_oversized_copy_from_device() {
        let device = MetalDevice::new().expect("MetalDevice::new should succeed");
        let buf = GpuDevice::alloc_buffer(&device, 4).expect("alloc_buffer should succeed");
        let mut out = [0u8; 5];
        let result = device.copy_from_device(&buf, &mut out);
        assert!(
            result.is_err(),
            "copy_from_device must reject oversized reads"
        );
        device.free_buffer(buf).expect("free_buffer should succeed");
    }

    #[test]
    #[cfg(feature = "gpu-metal")]
    fn metal_device_rejects_non_metal_buffer_handle() {
        let device = MetalDevice::new().expect("MetalDevice::new should succeed");
        let fake = GpuBufferHandle::new(
            8,
            GpuPlatform::WebGpu,
            vec![0; std::mem::size_of::<usize>()],
        );
        let result = device.free_buffer(fake);
        assert!(result.is_err(), "free_buffer must reject non-metal handles");
    }

    #[test]
    #[cfg(feature = "gpu-metal")]
    fn metal_device_rejects_zero_dimension_dispatch() {
        let device = MetalDevice::new().expect("MetalDevice::new should succeed");
        let msl = r#"
            #include <metal_stdlib>
            using namespace metal;
            kernel void noop(device uint* out [[buffer(0)]],
                             uint id [[thread_position_in_grid]]) {
                out[id] = id;
            }
        "#;
        let kernel = device
            .compile_kernel("noop", msl)
            .expect("compile_kernel should succeed");
        let buf = GpuDevice::alloc_buffer(&device, 16).expect("alloc_buffer should succeed");
        let result = device.launch_kernel(&kernel, [0, 1, 1], [1, 1, 1], &[&buf]);
        assert!(result.is_err(), "launch_kernel must reject zero-sized grid");
        device.free_buffer(buf).expect("free_buffer should succeed");
    }
}
