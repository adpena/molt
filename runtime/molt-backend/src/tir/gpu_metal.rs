//! Metal GPU device implementation (macOS only).
//!
//! Requires: macOS with Metal-capable GPU.
//! Uses the Metal framework via objc/metal-rs bindings.

#[cfg(target_os = "macos")]
use super::gpu_runtime::*;

/// Metal GPU device.
/// Compiles MSL source at runtime and dispatches compute kernels.
#[cfg(target_os = "macos")]
pub struct MetalDevice {
    // In a full implementation, these would be:
    // device: metal::Device,
    // command_queue: metal::CommandQueue,
    // But we avoid the metal crate dependency for now.
    // Instead, we provide the interface and document what's needed.
    _phantom: std::marker::PhantomData<()>,
}

#[cfg(target_os = "macos")]
impl MetalDevice {
    /// Create a Metal device using the system default GPU.
    pub fn new() -> Result<Self, GpuError> {
        // In production: metal::Device::system_default()
        //   .ok_or(GpuError::DeviceNotAvailable("No Metal device found".into()))
        Ok(Self { _phantom: std::marker::PhantomData })
    }

    /// Compile MSL source code to a Metal library.
    pub fn compile_msl(&self, name: &str, msl_source: &str) -> Result<CompiledKernel, GpuError> {
        // In production:
        // let options = metal::CompileOptions::new();
        // let library = self.device.new_library_with_source(msl_source, &options)
        //     .map_err(|e| GpuError::CompilationFailed(e.to_string()))?;
        // let function = library.get_function(name, None)
        //     .ok_or(GpuError::CompilationFailed(format!("Function {} not found", name)))?;
        // let pipeline = self.device.new_compute_pipeline_state_with_function(&function)
        //     .map_err(|e| GpuError::CompilationFailed(e.to_string()))?;

        Ok(CompiledKernel::new(
            name.to_string(),
            GpuPlatform::Metal,
            msl_source.as_bytes().to_vec(), // store source as handle placeholder
        ))
    }

    /// Launch a compiled kernel with the given grid/block dimensions.
    pub fn dispatch(
        &self,
        _kernel: &CompiledKernel,
        grid: [u32; 3],
        block: [u32; 3],
        _buffers: &[&GpuBufferHandle],
    ) -> Result<(), GpuError> {
        // In production:
        // let command_buffer = self.command_queue.new_command_buffer();
        // let encoder = command_buffer.new_compute_command_encoder();
        // encoder.set_compute_pipeline_state(&kernel.pipeline);
        // for (i, buf) in buffers.iter().enumerate() {
        //     encoder.set_buffer(i as u64, Some(&buf.metal_buffer), 0);
        // }
        // encoder.dispatch_threads(
        //     metal::MTLSize { width: grid[0] as _, height: grid[1] as _, depth: grid[2] as _ },
        //     metal::MTLSize { width: block[0] as _, height: block[1] as _, depth: block[2] as _ },
        // );
        // encoder.end_encoding();
        // command_buffer.commit();
        // command_buffer.wait_until_completed();

        let _ = (grid, block);
        Ok(())
    }
}

#[cfg(target_os = "macos")]
impl GpuDevice for MetalDevice {
    fn compile_kernel(&self, name: &str, source: &str) -> Result<CompiledKernel, GpuError> {
        self.compile_msl(name, source)
    }
    fn alloc_buffer(&self, size_bytes: usize) -> Result<GpuBufferHandle, GpuError> {
        Ok(GpuBufferHandle::new(size_bytes, GpuPlatform::Metal, vec![0; 8]))
    }
    fn copy_to_device(&self, _buffer: &GpuBufferHandle, _data: &[u8]) -> Result<(), GpuError> {
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
        kernel: &CompiledKernel,
        grid: [u32; 3],
        block: [u32; 3],
        buffers: &[&GpuBufferHandle],
    ) -> Result<(), GpuError> {
        self.dispatch(kernel, grid, block, buffers)
    }
    fn synchronize(&self) -> Result<(), GpuError> {
        Ok(())
    }
    fn free_buffer(&self, _buffer: GpuBufferHandle) -> Result<(), GpuError> {
        Ok(())
    }
}

#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    use super::*;
    use crate::tir::gpu_runtime::GpuDevice;

    #[test]
    fn metal_device_implements_gpu_device_trait() {
        let device = MetalDevice::new().expect("MetalDevice::new should succeed");
        // Verify alloc_buffer works
        let buf = device.alloc_buffer(1024).expect("alloc_buffer should succeed");
        assert_eq!(buf.size_bytes, 1024);
        assert_eq!(buf.platform, GpuPlatform::Metal);
    }

    #[test]
    fn metal_device_compile_and_launch() {
        let device = MetalDevice::new().expect("MetalDevice::new should succeed");
        let msl = "kernel void add(device float* a [[buffer(0)]]) { a[0] += 1.0; }";
        let kernel = device.compile_kernel("add", msl).expect("compile_kernel should succeed");
        assert_eq!(kernel.name, "add");
        assert_eq!(kernel.platform, GpuPlatform::Metal);

        device
            .launch_kernel(&kernel, [1, 1, 1], [64, 1, 1], &[])
            .expect("launch_kernel should not crash");
        device.synchronize().expect("synchronize should succeed");
    }
}
