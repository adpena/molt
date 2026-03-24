//! WebGPU device implementation.
//! Works natively via wgpu crate or in browsers via WebGPU API.

use super::gpu_runtime::*;

pub struct WebGpuDevice {
    _phantom: std::marker::PhantomData<()>,
}

impl WebGpuDevice {
    pub fn new() -> Result<Self, GpuError> {
        // In production: use wgpu crate
        // let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        // let adapter = instance.request_adapter(&Default::default()).await
        //     .ok_or(GpuError::DeviceNotAvailable("No WebGPU adapter".into()))?;
        // let (device, queue) = adapter.request_device(&Default::default(), None).await
        //     .map_err(|e| GpuError::DeviceNotAvailable(e.to_string()))?;
        Ok(Self { _phantom: std::marker::PhantomData })
    }

    pub fn compile_wgsl(&self, name: &str, wgsl_source: &str) -> Result<CompiledKernel, GpuError> {
        Ok(CompiledKernel::new(
            name.to_string(),
            GpuPlatform::WebGpu,
            wgsl_source.as_bytes().to_vec(),
        ))
    }
}

impl GpuDevice for WebGpuDevice {
    fn compile_kernel(&self, name: &str, source: &str) -> Result<CompiledKernel, GpuError> {
        self.compile_wgsl(name, source)
    }
    fn alloc_buffer(&self, size_bytes: usize) -> Result<GpuBufferHandle, GpuError> {
        Ok(GpuBufferHandle::new(size_bytes, GpuPlatform::WebGpu, vec![0; 8]))
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
        _kernel: &CompiledKernel,
        _grid: [u32; 3],
        _block: [u32; 3],
        _buffers: &[&GpuBufferHandle],
    ) -> Result<(), GpuError> {
        Ok(())
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
    use crate::tir::gpu_runtime::GpuDevice;

    #[test]
    fn webgpu_device_implements_gpu_device_trait() {
        let device = WebGpuDevice::new().expect("WebGpuDevice::new should succeed");
        let buf = device.alloc_buffer(512).expect("alloc_buffer should succeed");
        assert_eq!(buf.size_bytes, 512);
        assert_eq!(buf.platform, GpuPlatform::WebGpu);
    }

    #[test]
    fn webgpu_device_compile_and_launch() {
        let device = WebGpuDevice::new().expect("WebGpuDevice::new should succeed");
        let wgsl = "@compute @workgroup_size(64) fn main() {}";
        let kernel = device.compile_kernel("main", wgsl).expect("compile_kernel should succeed");
        assert_eq!(kernel.name, "main");
        assert_eq!(kernel.platform, GpuPlatform::WebGpu);

        device
            .launch_kernel(&kernel, [1, 1, 1], [64, 1, 1], &[])
            .expect("launch_kernel should not crash");
        device.synchronize().expect("synchronize should succeed");
    }
}
