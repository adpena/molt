#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

mod bridge;

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
mod descriptor_admission;

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
mod kernel_storage;

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
use kernel_storage::{KernelStoragePlan, PreparedKernelOutputs};

use bridge::*;
use molt_gpu::runtime_backend::{GpuBackend, requested_gpu_backend};
#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
use molt_runtime_core::OwnedRuntimeValue;
use molt_runtime_core::prelude::{
    MoltObject, PyToken, TYPE_ID_BYTEARRAY, TYPE_ID_BYTES, TYPE_ID_LIST, TYPE_ID_TUPLE,
    TYPE_ID_TYPE, obj_from_bits,
};
#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
use serde_json::Value as JsonValue;
use std::cell::Cell;
#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
use std::collections::{BTreeMap, BTreeSet};
#[cfg(all(not(target_arch = "wasm32"), feature = "webgpu-backend"))]
use std::sync::{Arc as WgpuArc, Mutex as WgpuMutex};

#[cfg(all(target_os = "macos", feature = "metal-backend"))]
use objc2::rc::Retained;
#[cfg(all(target_os = "macos", feature = "metal-backend"))]
use objc2::runtime::ProtocolObject;
#[cfg(all(target_os = "macos", feature = "metal-backend"))]
use objc2_foundation::NSString;
#[cfg(all(target_os = "macos", feature = "metal-backend"))]
use objc2_metal::{
    MTLBuffer, MTLCommandBuffer, MTLCommandBufferStatus, MTLCommandEncoder, MTLCommandQueue,
    MTLComputeCommandEncoder, MTLComputePipelineState, MTLCreateSystemDefaultDevice, MTLDevice,
    MTLLibrary, MTLResourceOptions, MTLSize,
};
#[cfg(all(target_os = "macos", feature = "metal-backend"))]
type MetalBuffer = Retained<ProtocolObject<dyn MTLBuffer>>;
#[cfg(all(target_os = "macos", feature = "metal-backend"))]
type MetalCommandQueue = Retained<ProtocolObject<dyn MTLCommandQueue>>;
#[cfg(all(target_os = "macos", feature = "metal-backend"))]
type MetalDeviceObject = Retained<ProtocolObject<dyn MTLDevice>>;
#[cfg(all(target_os = "macos", feature = "metal-backend"))]
type MetalPipelineState = Retained<ProtocolObject<dyn MTLComputePipelineState>>;
#[cfg(all(target_os = "macos", feature = "metal-backend"))]
use std::sync::Arc;

mod tensor_runtime;

#[cfg(test)]
#[path = "../../molt-runtime-core/src/bridge_test_stubs.rs"]
mod bridge_test_stubs;

pub use tensor_runtime::{
    molt_gpu_broadcast_binary_contiguous, molt_gpu_buffer_to_list,
    molt_gpu_interop__load_safetensors, molt_gpu_linear_contiguous,
    molt_gpu_linear_split_last_dim_contiguous,
    molt_gpu_linear_squared_relu_gate_interleaved_contiguous, molt_gpu_matmul_contiguous,
    molt_gpu_permute_contiguous, molt_gpu_repeat_axis_contiguous,
    molt_gpu_rms_norm_last_axis_contiguous, molt_gpu_rope_apply_contiguous,
    molt_gpu_softmax_last_axis_contiguous, molt_gpu_squared_relu_gate_interleaved_contiguous,
    molt_gpu_tensor__tensor_concat_first_dim, molt_gpu_tensor__tensor_data_list,
    molt_gpu_tensor__tensor_linear, molt_gpu_tensor__tensor_linear_split_last_dim,
    molt_gpu_tensor__tensor_linear_squared_relu_gate_interleaved,
    molt_gpu_tensor__tensor_permute_dims, molt_gpu_tensor__tensor_reshape_view,
    molt_gpu_tensor__tensor_scaled_dot_product_attention, molt_gpu_tensor__tensor_scatter_rows,
    molt_gpu_tensor__tensor_softmax_last_axis, molt_gpu_tensor__tensor_take_rows,
    molt_gpu_tensor__zeros, molt_gpu_tensor_from_buffer, molt_gpu_tensor_from_parts,
    molt_gpu_turboquant_attention_packed,
};

#[derive(Copy, Clone, Eq, PartialEq)]
enum ScalarFormat {
    F32,
    F64,
    I64,
}

impl ScalarFormat {
    fn itemsize(self) -> usize {
        match self {
            Self::F32 => 4,
            Self::F64 | Self::I64 => 8,
        }
    }
}

fn scalar_format_from_text(value: &str) -> Option<ScalarFormat> {
    match value {
        "f" => Some(ScalarFormat::F32),
        "d" => Some(ScalarFormat::F64),
        "q" => Some(ScalarFormat::I64),
        _ => None,
    }
}

#[derive(Copy, Clone)]
struct ByteView {
    ptr: *const u8,
    len: usize,
}

/// Operation admission for configured Python kernels. Tensor backends have
/// separate implementations and do not establish descriptor execution support.
enum PythonKernelExecutor {
    Sequential,
    Metal,
    WebGpu,
}

impl PythonKernelExecutor {
    fn for_backend(backend: Option<GpuBackend>) -> Result<Self, GpuBackend> {
        match backend {
            None => Ok(Self::Sequential),
            Some(GpuBackend::Metal) => Ok(Self::Metal),
            Some(GpuBackend::WebGpu) => Ok(Self::WebGpu),
            Some(backend @ (GpuBackend::Cuda | GpuBackend::Hip)) => Err(backend),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct GpuLaunchContext {
    thread_id: i64,
    block_id: i64,
    block_dim: i64,
    grid_dim: i64,
}

impl Default for GpuLaunchContext {
    fn default() -> Self {
        Self {
            thread_id: 0,
            block_id: 0,
            block_dim: 1,
            grid_dim: 1,
        }
    }
}

thread_local! {
    static GPU_LAUNCH_CONTEXT: Cell<GpuLaunchContext> = Cell::new(GpuLaunchContext::default());
}

struct GpuLaunchContextGuard(GpuLaunchContext);

impl Drop for GpuLaunchContextGuard {
    fn drop(&mut self) {
        GPU_LAUNCH_CONTEXT.set(self.0);
    }
}

fn with_gpu_launch_context<R>(ctx: GpuLaunchContext, body: impl FnOnce() -> R) -> R {
    let _context = GpuLaunchContextGuard(GPU_LAUNCH_CONTEXT.replace(ctx));
    body()
}

fn current_gpu_launch_context() -> GpuLaunchContext {
    GPU_LAUNCH_CONTEXT.get()
}

fn trace_gpu_kernel_launch_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_GPU_KERNEL_LAUNCH").as_deref() == Ok("1"))
}

fn trace_gpu_thread_id_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_GPU_THREAD_ID").as_deref() == Ok("1"))
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
fn trace_gpu_backend_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_GPU_BACKEND").as_deref() == Ok("1"))
}

fn decode_f16_to_f32_bits(bits: u16) -> u32 {
    let sign = ((bits & 0x8000) as u32) << 16;
    let exp = (bits >> 10) & 0x1F;
    let frac = (bits & 0x03FF) as u32;
    match exp {
        0 => {
            if frac == 0 {
                sign
            } else {
                let mut mant = frac;
                let mut exp32 = 113u32;
                while (mant & 0x0400) == 0 {
                    mant <<= 1;
                    exp32 -= 1;
                }
                mant &= 0x03FF;
                sign | (exp32 << 23) | (mant << 13)
            }
        }
        0x1F => sign | 0x7F80_0000 | (frac << 13),
        _ => {
            let exp32 = (exp as u32) + 112;
            sign | (exp32 << 23) | (frac << 13)
        }
    }
}

fn decode_f16_payload_to_f32_bytes(raw: &[u8]) -> Result<Vec<u8>, &'static str> {
    if !raw.len().is_multiple_of(2) {
        return Err("F16 payload length must be even");
    }
    let mut out = Vec::with_capacity((raw.len() / 2) * 4);
    for &pair in raw.as_chunks::<2>().0 {
        let bits = u16::from_le_bytes(pair);
        out.extend_from_slice(&decode_f16_to_f32_bits(bits).to_le_bytes());
    }
    Ok(out)
}

fn decode_bf16_payload_to_f32_bytes(raw: &[u8]) -> Result<Vec<u8>, &'static str> {
    if !raw.len().is_multiple_of(2) {
        return Err("BF16 payload length must be even");
    }
    let mut out = Vec::with_capacity((raw.len() / 2) * 4);
    for &pair in raw.as_chunks::<2>().0 {
        let bits = u16::from_le_bytes(pair);
        let widened = (bits as u32) << 16;
        out.extend_from_slice(&widened.to_le_bytes());
    }
    Ok(out)
}

fn decode_half_bytes_to_f32_object(
    _py: &PyToken,
    data_bits: u64,
    decode: fn(&[u8]) -> Result<Vec<u8>, &'static str>,
) -> u64 {
    let Some(ptr) = obj_from_bits(data_bits).as_ptr() else {
        return raise_exception::<_>(_py, "TypeError", "expected bytes-like object");
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
            return raise_exception::<_>(_py, "TypeError", "expected bytes-like object");
        }
        let raw = std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr));
        let Ok(decoded) = decode(raw) else {
            return raise_exception::<_>(_py, "ValueError", "invalid half-float payload length");
        };
        let out_ptr = alloc_bytes(_py, &decoded);
        if out_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "failed to allocate decoded bytes");
        }
        MoltObject::from_ptr(out_ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_interop_decode_f16_bytes_to_f32(data_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        decode_half_bytes_to_f32_object(_py, data_bits, decode_f16_payload_to_f32_bytes)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_interop_decode_bf16_bytes_to_f32(data_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        decode_half_bytes_to_f32_object(_py, data_bits, decode_bf16_payload_to_f32_bytes)
    })
}

fn parse_i64_launch_arg(_py: &PyToken, bits: u64, role: &str) -> Result<i64, u64> {
    if obj_from_bits(bits).is_bool() {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("GPU launch {role} must be a positive integer"),
        ));
    }
    let Some(value) = to_i64(obj_from_bits(bits)) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be an integer"),
        ));
    };
    if value <= 0 {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            &format!("GPU launch {role} must be positive"),
        ));
    }
    Ok(value)
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
unsafe fn try_object_attr_bits(
    _py: &PyToken,
    obj_bits: u64,
    name: &[u8],
) -> Result<Option<u64>, u64> {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let out = molt_get_attr_name(obj_bits, name_bits);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        if clear_attribute_error_if_pending() {
            return Ok(None);
        }
        return Err(out);
    }
    if obj_from_bits(out).is_none() {
        return Ok(None);
    }
    Ok(Some(out))
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
unsafe fn gpu_kernel_descriptor_bits(
    _py: &PyToken,
    callable_bits: u64,
) -> Result<Option<u64>, u64> {
    let bits = kernel_descriptor(callable_bits);
    if exception_pending(_py) {
        return Err(bits);
    }
    Ok((!obj_from_bits(bits).is_none()).then_some(bits))
}

#[cfg(all(not(target_arch = "wasm32"), feature = "webgpu-backend"))]
type RuntimeWebGpuBufferRegistry = WgpuArc<WgpuMutex<std::collections::HashMap<u64, wgpu::Buffer>>>;

#[cfg(all(not(target_arch = "wasm32"), feature = "webgpu-backend"))]
struct RuntimeWebGpuPipeline {
    pipeline: wgpu::ComputePipeline,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "webgpu-backend"))]
struct RuntimeWebGpuDevice {
    device: wgpu::Device,
    queue: wgpu::Queue,
    buffers: RuntimeWebGpuBufferRegistry,
    next_id: WgpuMutex<u64>,
    // This invocation-owned device is never shared between guest launches.
    // Callback failures remain sticky through its last readback/publication gate.
    failure: WgpuArc<WgpuMutex<Option<String>>>,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "webgpu-backend"))]
impl RuntimeWebGpuDevice {
    fn new() -> Result<Self, String> {
        pollster::block_on(async {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .map_err(|err| err.to_string())?;
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .map_err(|err| err.to_string())?;
            let failure = WgpuArc::new(WgpuMutex::new(None));
            let uncaptured = failure.clone();
            device.on_uncaptured_error(WgpuArc::new(move |error: wgpu::Error| {
                uncaptured
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .get_or_insert_with(|| format!("uncaptured WebGPU error: {error}"));
            }));
            let lost = failure.clone();
            device.set_device_lost_callback(move |reason, message| {
                lost.lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .get_or_insert_with(|| format!("WebGPU device lost ({reason:?}): {message}"));
            });
            Ok(Self {
                device,
                queue,
                failure,
                buffers: WgpuArc::new(WgpuMutex::new(std::collections::HashMap::new())),
                next_id: WgpuMutex::new(1),
            })
        })
    }

    fn check_failure(&self) -> Result<(), String> {
        match self
            .failure
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
        {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn checked<T>(
        &self,
        stage: &str,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.check_failure()?;
        let internal = self.device.push_error_scope(wgpu::ErrorFilter::Internal);
        let memory = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let result = operation();
        // Pop in reverse order even when operation returned an error. ErrorScopeGuard
        // also pops on unwind; no retry or successful fallback follows an error.
        let mut scoped_error = None;
        for scope in [validation, memory, internal] {
            if let Some(error) = pollster::block_on(scope.pop()) {
                scoped_error.get_or_insert_with(|| format!("WebGPU {stage}: {error}"));
            }
        }
        self.check_failure()?;
        if let Some(error) = scoped_error {
            return Err(error);
        }
        result
    }

    fn compile_pipeline(
        &self,
        name: &str,
        source: &str,
    ) -> Result<WgpuArc<RuntimeWebGpuPipeline>, String> {
        self.checked("pipeline", || {
            let shader = self
                .device
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: None,
                    source: wgpu::ShaderSource::Wgsl(source.into()),
                });
            let pipeline = self
                .device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: None,
                    layout: None,
                    module: &shader,
                    entry_point: Some(name),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    cache: None,
                });
            Ok(WgpuArc::new(RuntimeWebGpuPipeline { pipeline }))
        })
    }

    fn alloc_buffer(&self, size_bytes: usize) -> Result<(u64, wgpu::Buffer), String> {
        let buffer = self.checked("allocation", || {
            if size_bytes as u64 > self.device.limits().max_buffer_size {
                return Err("WebGPU buffer exceeds device max_buffer_size".into());
            }
            Ok(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: size_bytes as u64,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }))
        })?;
        let mut next_id = self.next_id.lock().unwrap();
        let id = *next_id;
        *next_id += 1;
        self.buffers.lock().unwrap().insert(id, buffer.clone());
        Ok((id, buffer))
    }

    fn copy_to_buffer(&self, buffer: &wgpu::Buffer, data: &[u8]) -> Result<(), String> {
        self.checked("upload", || {
            self.queue.write_buffer(buffer, 0, data);
            Ok(())
        })
    }

    fn copy_from_buffer(
        &self,
        buffer: &wgpu::Buffer,
        size_bytes: usize,
    ) -> Result<Vec<u8>, String> {
        self.checked("readback", || {
            // Dispatch has already established completion even for zero outputs.
            if size_bytes == 0 {
                return Ok(Vec::new());
            }
            let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("runtime_webgpu_staging"),
                size: size_bytes as u64,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size_bytes as u64);
            self.queue.submit(Some(encoder.finish()));
            let slice = staging.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |res| {
                let _ = tx.send(res);
            });
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(|err| err.to_string())?;
            rx.recv()
                .map_err(|_| "map channel dropped".to_string())?
                .map_err(|err| err.to_string())?;
            let result = (|| {
                let mapped = slice.get_mapped_range().map_err(|err| err.to_string())?;
                if mapped.len() != size_bytes {
                    return Err("WebGPU readback extent mismatch".into());
                }
                let mut out = Vec::new();
                out.try_reserve_exact(size_bytes)
                    .map_err(|_| "WebGPU host readback allocation failed".to_string())?;
                out.extend_from_slice(&mapped);
                Ok(out)
            })();
            staging.unmap();
            result
        })
    }

    fn dispatch(
        &self,
        pipeline: &WgpuArc<RuntimeWebGpuPipeline>,
        grid: u32,
        buffers: &[&wgpu::Buffer],
    ) -> Result<(), String> {
        self.checked("dispatch", || {
            if grid > self.device.limits().max_compute_workgroups_per_dimension {
                return Err("WebGPU grid exceeds device dispatch capability".into());
            }
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: None,
                    timestamp_writes: None,
                });
                pass.set_pipeline(&pipeline.pipeline);
                if !buffers.is_empty() {
                    let layout = pipeline.pipeline.get_bind_group_layout(0);
                    let entries: Vec<wgpu::BindGroupEntry<'_>> = buffers
                        .iter()
                        .enumerate()
                        .map(|(index, buffer)| wgpu::BindGroupEntry {
                            binding: index as u32,
                            resource: buffer.as_entire_binding(),
                        })
                        .collect();
                    let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &layout,
                        entries: &entries,
                    });
                    pass.set_bind_group(0, &bind_group, &[]);
                }
                pass.dispatch_workgroups(grid, 1, 1);
            }
            self.queue.submit(Some(encoder.finish()));
            // Completion does not depend on there being any writable/readback bindings.
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(|err| err.to_string())?;
            Ok(())
        })
    }
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
struct RuntimeKernelBufferArg<'py> {
    // Attribute lookup transfers this owner. Keep the selected bytes alive even
    // if subsequent argument descriptors replace the source object's _data.
    data: OwnedRuntimeValue<'py>,
    object_ptr: *mut u8,
    original_format: String,
    size: usize,
    float_elements: bool,
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
enum RuntimeKernelArg<'py> {
    Buffer(RuntimeKernelBufferArg<'py>),
    Int(i64),
    Float(f64),
    Bool(bool),
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
#[derive(Clone)]
struct RuntimeKernelOp {
    kind: String,
    args: Vec<String>,
    out: Option<String>,
    var: Option<String>,
    value: Option<i64>,
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
#[derive(Clone)]
struct RuntimeKernelDescriptor {
    name: String,
    params: Vec<String>,
    ops: Vec<RuntimeKernelOp>,
    code_slot: u64,
    query_bindings: Vec<RuntimeKernelQuery>,
    requirements: Vec<JsonValue>,
    python_bodies: BTreeMap<String, RuntimeKernelBody>,
    numeric: JsonValue,
    query_magnitude: u64,
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
#[derive(Clone)]
struct RuntimeKernelBody {
    symbol: String,
    arity: u64,
    code_slot: u64,
    defaults: Vec<u64>,
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
#[derive(Clone)]
struct RuntimeKernelQuery {
    out: String,
    path: Vec<String>,
    conditional: bool,
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
fn parse_kernel_descriptor_json(text: &str) -> Result<RuntimeKernelDescriptor, String> {
    let root: JsonValue = serde_json::from_str(text).map_err(|err| err.to_string())?;
    let obj = root
        .as_object()
        .ok_or_else(|| "kernel descriptor must be an object".to_string())?;
    if obj.get("schema_version").and_then(JsonValue::as_u64) != Some(3) {
        return Err("unsupported kernel descriptor schema".into());
    }
    if let Some(reason) = obj.get("unsupported").and_then(JsonValue::as_str) {
        return Err(format!("hardware kernel capability unavailable: {reason}"));
    }
    let kind = obj
        .get("kind")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| "kernel descriptor missing kind".to_string())?;
    if kind != "molt_gpu_kernel" {
        return Err(format!("unsupported kernel descriptor kind: {kind}"));
    }
    let name = obj
        .get("name")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| "kernel descriptor missing name".to_string())?
        .to_string();
    let params = obj
        .get("params")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| "kernel descriptor missing params".to_string())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .ok_or_else(|| "kernel param must be a string".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let ops = obj
        .get("ops")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| "kernel descriptor missing ops".to_string())?
        .iter()
        .map(|value| {
            let op = value
                .as_object()
                .ok_or_else(|| "kernel op must be an object".to_string())?;
            let kind = op
                .get("kind")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| "kernel op missing kind".to_string())?
                .to_string();
            let args = op
                .get("args")
                .and_then(JsonValue::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|item| {
                            item.as_str()
                                .map(ToString::to_string)
                                .ok_or_else(|| "kernel op args must be strings".to_string())
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?
                .unwrap_or_default();
            Ok(RuntimeKernelOp {
                kind,
                args,
                out: op
                    .get("out")
                    .and_then(JsonValue::as_str)
                    .map(ToString::to_string),
                var: op
                    .get("var")
                    .and_then(JsonValue::as_str)
                    .map(ToString::to_string),
                value: op.get("value").and_then(JsonValue::as_i64),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let code_slot = obj
        .get("code_slot")
        .and_then(JsonValue::as_u64)
        .ok_or("kernel descriptor lacks a code slot")?;
    let query_bindings = obj
        .get("query_bindings")
        .and_then(JsonValue::as_array)
        .ok_or("kernel lacks binding obligations")?
        .iter()
        .map(|binding| {
            let out = binding
                .get("out")
                .and_then(JsonValue::as_str)
                .ok_or("query lacks output")?
                .to_string();
            let path = binding
                .get("path")
                .and_then(JsonValue::as_array)
                .ok_or("query lacks path")?
                .iter()
                .map(|name| {
                    name.as_str()
                        .map(str::to_string)
                        .ok_or("query path is not a name")
                })
                .collect::<Result<Vec<_>, _>>()?;
            let conditional = binding
                .get("conditional")
                .and_then(JsonValue::as_bool)
                .ok_or("query lacks participation fact")?;
            Ok(RuntimeKernelQuery {
                out,
                path,
                conditional,
            })
        })
        .collect::<Result<Vec<_>, &'static str>>()?;
    let python_bodies = obj
        .get("python_bodies")
        .and_then(JsonValue::as_object)
        .ok_or("kernel lacks Python body identities")?
        .iter()
        .map(|(role, body)| {
            let symbol = body
                .get("symbol")
                .and_then(JsonValue::as_str)
                .ok_or("Python body lacks executable symbol")?
                .to_owned();
            let arity = body
                .get("arity")
                .and_then(JsonValue::as_u64)
                .ok_or("Python body lacks arity")?;
            let code_slot = body
                .get("code_slot")
                .and_then(JsonValue::as_u64)
                .ok_or("Python body lacks code slot")?;
            let defaults = body
                .get("defaults")
                .and_then(JsonValue::as_array)
                .ok_or("Python body lacks defaults")?
                .iter()
                .map(|value| {
                    if value.is_null() {
                        return Ok(MoltObject::none().bits());
                    }
                    if let Some(value) = value.as_bool() {
                        return Ok(MoltObject::from_bool(value).bits());
                    }
                    let value = value.as_i64().ok_or("unsupported Python body default")?;
                    // Reference defaults are small immediate integers; no
                    // allocation or fallible guest conversion during admission.
                    let bits = MoltObject::from_int(value).bits();
                    if obj_from_bits(bits).as_int() != Some(value) {
                        return Err("Python body default is not an immediate integer");
                    }
                    Ok(bits)
                })
                .collect::<Result<Vec<_>, &'static str>>()?;
            Ok((
                role.clone(),
                RuntimeKernelBody {
                    symbol,
                    arity,
                    code_slot,
                    defaults,
                },
            ))
        })
        .collect::<Result<BTreeMap<_, _>, &'static str>>()?;
    let requirements = obj
        .get("requirements")
        .and_then(JsonValue::as_array)
        .ok_or("kernel lacks dynamic obligations")?
        .clone();
    let numeric = obj
        .get("numeric")
        .filter(|value| value.is_object())
        .ok_or("kernel lacks its compiler numeric certificate")?
        .clone();
    Ok(RuntimeKernelDescriptor {
        name,
        params,
        ops,
        code_slot,
        query_bindings,
        requirements,
        python_bodies,
        numeric,
        query_magnitude: 0,
    })
}

fn parse_format(_py: &PyToken, bits: u64, role: &str) -> Result<ScalarFormat, u64> {
    let Some(value) = string_obj_to_owned(obj_from_bits(bits)) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be a format string"),
        ));
    };
    match scalar_format_from_text(value.as_str()) {
        Some(fmt) => Ok(fmt),
        None => Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            &format!("{role} format {:?} is unsupported", value),
        )),
    }
}

fn parse_usize_arg(_py: &PyToken, bits: u64, role: &str) -> Result<usize, u64> {
    let Some(value) = to_i64(obj_from_bits(bits)) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be an integer"),
        ));
    };
    usize::try_from(value).map_err(|_| {
        raise_exception::<_>(_py, "ValueError", &format!("{role} must be non-negative"))
    })
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
fn kernel_arg_from_bits<'py>(
    _py: &'py PyToken,
    name: &str,
    bits: u64,
) -> Result<RuntimeKernelArg<'py>, u64> {
    let obj = obj_from_bits(bits);
    let unsupported = || {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            &format!("GPU scalar parameter {name:?} requires an exact builtin numeric protocol"),
        )
    };
    match exact_scalar_kind(bits) {
        1 => {
            return to_i64(obj)
                .map(RuntimeKernelArg::Int)
                .ok_or_else(unsupported);
        }
        2 => {
            return to_f64(obj)
                .map(RuntimeKernelArg::Float)
                .ok_or_else(unsupported);
        }
        3 => return Ok(RuntimeKernelArg::Bool(obj.as_bool().unwrap_or(false))),
        -1 => return Err(unsupported()),
        _ => {}
    }
    if let Some(ptr) = obj.as_ptr() {
        let type_id = unsafe { object_type_id(ptr) };
        if type_id == TYPE_ID_TYPE {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "gpu kernel runtime launch does not support class-object arguments",
            ));
        }
        let maybe_data_bits = unsafe { try_object_attr_bits(_py, bits, b"_data")? };
        if let Some(data_bits) = maybe_data_bits {
            let data = unsafe { OwnedRuntimeValue::from_owned_bits(_py, data_bits) };
            let format = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    _py,
                    object_attr_bits(_py, bits, b"_format_char", "_format_char")?,
                )
            };
            let size = unsafe {
                OwnedRuntimeValue::from_owned_bits(
                    _py,
                    object_attr_bits(_py, bits, b"_size", "_size")?,
                )
            };
            let format = string_obj_to_owned(obj_from_bits(format.bits())).ok_or_else(|| {
                raise_exception::<u64>(_py, "TypeError", "buffer format must be a string")
            })?;
            let size = parse_usize_arg(_py, size.bits(), "_size")?;
            let scalar_format = scalar_format_from_text(&format).ok_or_else(|| {
                raise_exception::<u64>(_py, "RuntimeError", "unsupported GPU buffer format")
            })?;
            let required_bytes = size.checked_mul(scalar_format.itemsize()).ok_or_else(|| {
                raise_exception::<u64>(
                    _py,
                    "OverflowError",
                    "GPU buffer size exceeds address space",
                )
            })?;
            let view = bytes_like_view(_py, data.bits(), "_data")?;
            if view.len < required_bytes {
                return Err(raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "GPU buffer payload is shorter than its declared size",
                ));
            }

            return Ok(RuntimeKernelArg::Buffer(RuntimeKernelBufferArg {
                object_ptr: ptr,
                data,
                original_format: format,
                size,
                float_elements: false,
            }));
        }
    }
    Err(raise_exception::<_>(
        _py,
        "RuntimeError",
        &format!("unsupported gpu kernel argument for parameter {:?}", name),
    ))
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
fn encode_webgpu_buffer_bytes(raw: &[u8], format: ScalarFormat) -> Result<Vec<u8>, String> {
    match format {
        ScalarFormat::F32 => Ok(raw.to_vec()),
        ScalarFormat::F64 => {
            let mut out = Vec::with_capacity(raw.len() / 2);
            for &chunk in raw.as_chunks::<8>().0 {
                let val = f64::from_le_bytes(chunk);
                out.extend_from_slice(&(val as f32).to_le_bytes());
            }
            Ok(out)
        }
        ScalarFormat::I64 => {
            let mut out = Vec::with_capacity(raw.len() / 2);
            for &chunk in raw.as_chunks::<8>().0 {
                let val = i64::from_le_bytes(chunk);
                let narrowed = i32::try_from(val)
                    .map_err(|_| "webgpu backend only supports q values that fit in i32")?;
                out.extend_from_slice(&narrowed.to_le_bytes());
            }
            Ok(out)
        }
    }
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
fn bytes_like_view_to_webgpu_bytes(
    raw_view: ByteView,
    format: ScalarFormat,
) -> Result<Vec<u8>, String> {
    let raw = unsafe { std::slice::from_raw_parts(raw_view.ptr, raw_view.len) };
    encode_webgpu_buffer_bytes(raw, format)
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
fn rebuild_host_bytes_from_gpu32_output(
    _py: &PyToken,
    format: ScalarFormat,
    elem_count: usize,
    gpu_output: &[u8],
) -> Result<Vec<u8>, u64> {
    match format {
        ScalarFormat::F32 | ScalarFormat::I64 => {
            if format == ScalarFormat::I64 {
                let mut out = Vec::with_capacity(elem_count * 8);
                for &chunk in gpu_output.as_chunks::<4>().0 {
                    let val = i64::from(i32::from_le_bytes(chunk));
                    out.extend_from_slice(&val.to_le_bytes());
                }
                Ok(out)
            } else {
                Ok(gpu_output.to_vec())
            }
        }
        ScalarFormat::F64 => {
            let mut out = Vec::with_capacity(elem_count * 8);
            for &chunk in gpu_output.as_chunks::<4>().0 {
                let val = f64::from(f32::from_le_bytes(chunk));
                out.extend_from_slice(&val.to_le_bytes());
            }
            Ok(out)
        }
    }
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
#[derive(Clone, Copy)]
enum KernelShaderDialect {
    #[cfg(all(target_os = "macos", feature = "metal-backend"))]
    Metal,
    #[cfg(any(target_arch = "wasm32", feature = "webgpu-backend"))]
    Wgsl,
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "metal-backend"),
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
fn render_kernel_source(
    desc: &RuntimeKernelDescriptor,
    plan: &KernelStoragePlan,
    grid: i64,
    threads: i64,
    dialect: KernelShaderDialect,
) -> Result<String, String> {
    let metal = match dialect {
        #[cfg(all(target_os = "macos", feature = "metal-backend"))]
        KernelShaderDialect::Metal => true,
        #[cfg(any(target_arch = "wasm32", feature = "webgpu-backend"))]
        KernelShaderDialect::Wgsl => false,
    };
    let mut source = String::new();
    let mut headers = Vec::new();
    for index in 0..plan.binding_count() {
        let name = format!("molt_binding_{index}");
        let flag = index >= plan.groups.len();
        let writable = plan.writable(index);
        if metal {
            let ty = if flag { "atomic_uint" } else { "int" };
            let qualifier = if writable { "device" } else { "device const" };
            headers.push(format!("    {qualifier} {ty}* {name} [[buffer({index})]]"));
        } else {
            let ty = if flag { "atomic<u32>" } else { "i32" };
            let access = if writable { "read_write" } else { "read" };
            source.push_str(&format!(
                "@group(0) @binding({index}) var<storage, {access}> {name}: array<{ty}>;\n"
            ));
        }
    }
    if metal {
        source.push_str("#include <metal_stdlib>\nusing namespace metal;\n\n");
        headers.push("    uint molt_raw_tid [[thread_position_in_grid]]".to_string());
        source.push_str(&format!(
            "kernel void {}(\n{}\n) {{\n    const int molt_tid = int(molt_raw_tid);\n",
            desc.name,
            headers.join(",\n")
        ));
    } else {
        source.push_str(&format!("\n@compute @workgroup_size({threads})\nfn {}(@builtin(global_invocation_id) molt_gid: vec3<u32>) {{\n    let molt_tid = i32(molt_gid.x);\n", desc.name));
    }
    for group in &plan.groups {
        if let Some(flag) = group.store_flag {
            source.push_str(&if metal {
                format!("    bool molt_dirty_{flag} = false;\n")
            } else {
                format!("    var molt_dirty_{flag}: bool = false;\n")
            });
        }
    }
    let mut exprs = BTreeMap::new();
    for (name, &index) in &plan.bindings {
        let value = if plan.groups[index].format.is_some() {
            format!("molt_binding_{index}")
        } else {
            format!("molt_binding_{index}[0]")
        };
        exprs.insert(name.clone(), value);
    }
    let mut depth = 0usize;
    for (ordinal, op) in desc.ops.iter().enumerate() {
        let lookup = |name: &str| {
            exprs
                .get(name)
                .cloned()
                .ok_or_else(|| format!("GPU operand has no admitted definition: {name}"))
        };
        let operand = |index: usize| {
            op.args
                .get(index)
                .ok_or_else(|| format!("GPU {} operand missing", op.kind))
                .and_then(|name| lookup(name))
        };
        let output = || {
            op.out
                .as_ref()
                .ok_or_else(|| format!("GPU {} output missing", op.kind))
        };
        match op.kind.as_str() {
            "store_var" => {
                let value = operand(0)?;
                exprs.insert(
                    op.var.clone().ok_or("GPU variable store lacks target")?,
                    value,
                );
            }
            "load_var" => {
                let value = lookup(op.var.as_ref().ok_or("GPU variable load lacks target")?)?;
                exprs.insert(output()?.clone(), value);
            }
            "gpu_thread_id" | "gpu_block_id" | "gpu_block_dim" | "gpu_grid_dim" => {
                let value = match op.kind.as_str() {
                    "gpu_thread_id" => "molt_tid".to_string(),
                    "gpu_block_id" => format!("(molt_tid / {threads})"),
                    "gpu_block_dim" => threads.to_string(),
                    _ => grid.to_string(),
                };
                exprs.insert(output()?.clone(), value);
            }
            "gpu_barrier" => source.push_str(if metal {
                "    threadgroup_barrier(mem_flags::mem_device);\n"
            } else {
                "    storageBarrier();\n    workgroupBarrier();\n"
            }),
            "const" => {
                exprs.insert(
                    output()?.clone(),
                    op.value.ok_or("GPU constant lacks value")?.to_string(),
                );
            }
            "lt" | "add" | "sub" | "mul" | "index" => {
                let lhs = operand(0)?;
                let rhs = operand(1)?;
                let expression = if op.kind == "index" {
                    format!("{lhs}[{rhs}]")
                } else {
                    format!(
                        "{lhs} {} {rhs}",
                        match op.kind.as_str() {
                            "lt" => "<",
                            "add" => "+",
                            "sub" => "-",
                            "mul" => "*",
                            _ => unreachable!(),
                        }
                    )
                };
                let name = format!("molt_value_{ordinal}");
                // Loads are real SSA definitions, never deferred expressions:
                // later aliased writes must not change a previously loaded value.
                source.push_str(&format!(
                    "    {} {name} = {expression};\n",
                    if metal { "const auto" } else { "let" }
                ));
                exprs.insert(output()?.clone(), name);
            }
            "if" => {
                source.push_str(&format!("    if ({}) {{\n", operand(0)?));
                depth += 1;
            }
            "end_if" => {
                depth = depth.checked_sub(1).ok_or("GPU unbalanced branch")?;
                source.push_str("    }\n");
            }
            "store_index" => {
                let buffer = op.args.first().ok_or("GPU store lacks buffer")?;
                let index = *plan
                    .bindings
                    .get(buffer)
                    .ok_or("GPU store has no physical binding")?;
                source.push_str(&format!(
                    "        {}[{}] = {};\n",
                    operand(0)?,
                    operand(1)?,
                    operand(2)?
                ));
                if let Some(flag) = plan.groups[index].store_flag {
                    source.push_str(&format!("        molt_dirty_{flag} = true;\n"));
                }
            }
            other => return Err(format!("unsupported projected GPU operation: {other}")),
        }
    }
    if depth != 0 {
        return Err("GPU branch has no normal exit".to_string());
    }
    let flag_binding = plan.groups.len();
    // One relaxed publication per participating invocation/COW group, never
    // an atomic per element and never a racy non-atomic multiwriter flag.
    for group in &plan.groups {
        if let Some(flag) = group.store_flag {
            source.push_str(&if metal {
                format!("    if (molt_dirty_{flag}) {{ atomic_store_explicit(&molt_binding_{flag_binding}[{flag}], 1u, memory_order_relaxed); }}\n")
            } else { format!("    if (molt_dirty_{flag}) {{ atomicStore(&molt_binding_{flag_binding}[{flag}], 1u); }}\n") });
        }
    }
    source.push_str("}\n");
    Ok(source)
}

#[cfg(target_arch = "wasm32")]
fn browser_webgpu_error_message(rc: i32, detail: &str) -> String {
    if !detail.is_empty() {
        return detail.to_string();
    }
    match rc.unsigned_abs() {
        12 => "browser webgpu dispatch ran out of memory".to_string(),
        22 => "browser webgpu dispatch rejected the launch record".to_string(),
        38 => {
            "browser webgpu dispatch is unavailable; run the wasm host in a worker-backed WebGPU environment"
                .to_string()
        }
        110 => "browser webgpu dispatch timed out".to_string(),
        other => format!("browser webgpu dispatch failed with errno {other}"),
    }
}

#[cfg(target_arch = "wasm32")]
fn dispatch_browser_webgpu_bindings(
    _py: &PyToken,
    source: &str,
    entry: &str,
    launch_bindings: Vec<serde_json::Value>,
    grid: u32,
    workgroup_size: u32,
) -> Result<(), u64> {
    let launch_record_bytes = serde_json::to_vec(&serde_json::json!({
        "bindings": launch_bindings,
    }))
    .map_err(|err| {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            &format!("failed to encode webgpu launch record: {err}"),
        )
    })?;
    let mut err_bytes = vec![0u8; 4096];
    let mut out_err_len = 0u32;
    let rc = unsafe {
        molt_gpu_webgpu_dispatch_host(
            source.as_ptr() as usize as u32,
            source.len() as u32,
            entry.as_ptr() as usize as u32,
            entry.len() as u32,
            launch_record_bytes.as_ptr() as usize as u32,
            launch_record_bytes.len() as u32,
            grid,
            workgroup_size,
            err_bytes.as_mut_ptr() as usize as u32,
            err_bytes.len() as u32,
            &mut out_err_len as *mut u32,
        )
    };
    if rc != 0 {
        let detail = if out_err_len == 0 {
            String::new()
        } else {
            let len = usize::min(out_err_len as usize, err_bytes.len());
            String::from_utf8_lossy(&err_bytes[..len]).into_owned()
        };
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            &browser_webgpu_error_message(rc, detail.as_str()),
        ));
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn webgpu_linear_element_type(
    x_format: ScalarFormat,
    weight_format: ScalarFormat,
    out_format: ScalarFormat,
) -> Result<&'static str, String> {
    if x_format == ScalarFormat::I64
        && weight_format == ScalarFormat::I64
        && out_format == ScalarFormat::I64
    {
        return Ok("i32");
    }
    if x_format != ScalarFormat::I64
        && weight_format != ScalarFormat::I64
        && out_format != ScalarFormat::I64
    {
        return Ok("f32");
    }
    Err("browser webgpu linear fast path supports either all-int or all-float formats".to_string())
}

#[cfg(target_arch = "wasm32")]
fn render_webgpu_linear_source(entry: &str, element_ty: &str, workgroup_size: u32) -> String {
    let zero = if element_ty == "f32" { "0.0" } else { "0" };
    format!(
        "@group(0) @binding(0) var<storage, read> x: array<{element_ty}>;\n\
@group(0) @binding(1) var<storage, read> weight: array<{element_ty}>;\n\
@group(0) @binding(2) var<storage, read_write> out: array<{element_ty}>;\n\
@group(0) @binding(3) var<storage, read> outer: array<i32>;\n\
@group(0) @binding(4) var<storage, read> in_features: array<i32>;\n\
@group(0) @binding(5) var<storage, read> out_features: array<i32>;\n\
\n\
@compute @workgroup_size({workgroup_size})\n\
fn {entry}(@builtin(global_invocation_id) gid: vec3<u32>) {{\n\
    let idx = i32(gid.x);\n\
    let outer_val = outer[0];\n\
    let in_features_val = in_features[0];\n\
    let out_features_val = out_features[0];\n\
    if (idx >= outer_val * out_features_val) {{\n\
        return;\n\
    }}\n\
    let row = idx / out_features_val;\n\
    let col = idx % out_features_val;\n\
    var acc: {element_ty} = {zero};\n\
    for (var k: i32 = 0; k < in_features_val; k = k + 1) {{\n\
        acc = acc + x[row * in_features_val + k] * weight[col * in_features_val + k];\n\
    }}\n\
    out[idx] = acc;\n\
}}\n"
    )
}

#[cfg(target_arch = "wasm32")]
fn render_webgpu_linear_squared_relu_gate_source(entry: &str, workgroup_size: u32) -> String {
    format!(
        "@group(0) @binding(0) var<storage, read> x: array<f32>;\n\
@group(0) @binding(1) var<storage, read> weight: array<f32>;\n\
@group(0) @binding(2) var<storage, read_write> out: array<f32>;\n\
@group(0) @binding(3) var<storage, read> outer: array<i32>;\n\
@group(0) @binding(4) var<storage, read> in_features: array<i32>;\n\
@group(0) @binding(5) var<storage, read> hidden: array<i32>;\n\
\n\
@compute @workgroup_size({workgroup_size})\n\
fn {entry}(@builtin(global_invocation_id) gid: vec3<u32>) {{\n\
    let idx = i32(gid.x);\n\
    let outer_val = outer[0];\n\
    let in_features_val = in_features[0];\n\
    let hidden_val = hidden[0];\n\
    if (idx >= outer_val * hidden_val) {{\n\
        return;\n\
    }}\n\
    let row = idx / hidden_val;\n\
    let hidden_idx = idx % hidden_val;\n\
    var gate: f32 = 0.0;\n\
    var up: f32 = 0.0;\n\
    let gate_row = 2 * hidden_idx;\n\
    let up_row = gate_row + 1;\n\
    for (var k: i32 = 0; k < in_features_val; k = k + 1) {{\n\
        gate = gate + x[row * in_features_val + k] * weight[gate_row * in_features_val + k];\n\
        up = up + x[row * in_features_val + k] * weight[up_row * in_features_val + k];\n\
    }}\n\
    let relu = max(gate, 0.0);\n\
    out[idx] = relu * relu * up;\n\
}}\n"
    )
}

#[cfg(target_arch = "wasm32")]
fn render_webgpu_attention_source(entry: &str, workgroup_size: u32) -> String {
    format!(
        "@group(0) @binding(0) var<storage, read> q: array<f32>;\n\
@group(0) @binding(1) var<storage, read> k: array<f32>;\n\
@group(0) @binding(2) var<storage, read> v: array<f32>;\n\
@group(0) @binding(3) var<storage, read_write> out: array<f32>;\n\
@group(0) @binding(4) var<storage, read> mask: array<f32>;\n\
@group(0) @binding(5) var<storage, read> batch: array<i32>;\n\
@group(0) @binding(6) var<storage, read> heads: array<i32>;\n\
@group(0) @binding(7) var<storage, read> seq_q: array<i32>;\n\
@group(0) @binding(8) var<storage, read> seq_k: array<i32>;\n\
@group(0) @binding(9) var<storage, read> dim: array<i32>;\n\
@group(0) @binding(10) var<storage, read> value_dim: array<i32>;\n\
@group(0) @binding(11) var<storage, read> scale: array<f32>;\n\
@group(0) @binding(12) var<storage, read> has_mask: array<i32>;\n\
\n\
@compute @workgroup_size({workgroup_size})\n\
fn {entry}(@builtin(global_invocation_id) gid: vec3<u32>) {{\n\
    let idx = i32(gid.x);\n\
    let batch_val = batch[0];\n\
    let heads_val = heads[0];\n\
    let seq_q_val = seq_q[0];\n\
    let seq_k_val = seq_k[0];\n\
    let dim_val = dim[0];\n\
    let value_dim_val = value_dim[0];\n\
    let has_mask_val = has_mask[0] != 0;\n\
    let total = batch_val * heads_val * seq_q_val * value_dim_val;\n\
    if (idx >= total) {{\n\
        return;\n\
    }}\n\
    let d = idx % value_dim_val;\n\
    let q_idx = (idx / value_dim_val) % seq_q_val;\n\
    let h = (idx / (value_dim_val * seq_q_val)) % heads_val;\n\
    let b = idx / (value_dim_val * seq_q_val * heads_val);\n\
    let q_base = ((b * heads_val + h) * seq_q_val + q_idx) * dim_val;\n\
    var max_score: f32 = -1.0e30;\n\
    for (var k_idx: i32 = 0; k_idx < seq_k_val; k_idx = k_idx + 1) {{\n\
        let k_base = ((b * heads_val + h) * seq_k_val + k_idx) * dim_val;\n\
        var score: f32 = 0.0;\n\
        for (var i: i32 = 0; i < dim_val; i = i + 1) {{\n\
            score = score + q[q_base + i] * k[k_base + i];\n\
        }}\n\
        score = score * scale[0];\n\
        if (has_mask_val) {{\n\
            score = score + mask[((b * heads_val + h) * seq_q_val + q_idx) * seq_k_val + k_idx];\n\
        }}\n\
        if (score > max_score) {{\n\
            max_score = score;\n\
        }}\n\
    }}\n\
    var sum: f32 = 0.0;\n\
    var acc: f32 = 0.0;\n\
    for (var k_idx: i32 = 0; k_idx < seq_k_val; k_idx = k_idx + 1) {{\n\
        let k_base = ((b * heads_val + h) * seq_k_val + k_idx) * dim_val;\n\
        var score: f32 = 0.0;\n\
        for (var i: i32 = 0; i < dim_val; i = i + 1) {{\n\
            score = score + q[q_base + i] * k[k_base + i];\n\
        }}\n\
        score = score * scale[0];\n\
        if (has_mask_val) {{\n\
            score = score + mask[((b * heads_val + h) * seq_q_val + q_idx) * seq_k_val + k_idx];\n\
        }}\n\
        let weight = exp(score - max_score);\n\
        sum = sum + weight;\n\
        let v_base = ((b * heads_val + h) * seq_k_val + k_idx) * value_dim_val;\n\
        acc = acc + weight * v[v_base + d];\n\
    }}\n\
    out[idx] = select(0.0, acc / sum, sum != 0.0);\n\
}}\n"
    )
}

#[cfg(all(target_os = "macos", feature = "metal-backend"))]
fn render_metal_turboquant_attention_source(entry: &str) -> String {
    format!(
        "#include <metal_stdlib>\n\
using namespace metal;\n\
\n\
kernel void {entry}(\n\
    device const float* rotated_q [[buffer(0)]],\n\
    device const float* query_sketch [[buffer(1)]],\n\
    device const float* key_mse [[buffer(2)]],\n\
    device const float* key_sign [[buffer(3)]],\n\
    device const float* key_scale [[buffer(4)]],\n\
    device const float* value_rows [[buffer(5)]],\n\
    device float* out [[buffer(6)]],\n\
    device const float* mask [[buffer(7)]],\n\
    constant int& batch [[buffer(8)]],\n\
    constant int& query_heads [[buffer(9)]],\n\
    constant int& kv_heads [[buffer(10)]],\n\
    constant int& seq_q [[buffer(11)]],\n\
    constant int& seq_k [[buffer(12)]],\n\
    constant int& dim [[buffer(13)]],\n\
    constant float& scale [[buffer(14)]],\n\
    constant int& has_mask [[buffer(15)]],\n\
    uint tid [[thread_position_in_grid]]\n\
) {{\n\
    int idx = int(tid);\n\
    int total = batch * query_heads * seq_q * dim;\n\
    if (idx >= total) {{\n\
        return;\n\
    }}\n\
    int d = idx % dim;\n\
    int q_idx = (idx / dim) % seq_q;\n\
    int q_head = (idx / (dim * seq_q)) % query_heads;\n\
    int b = idx / (dim * seq_q * query_heads);\n\
    int kv_head = (query_heads == kv_heads) ? q_head : (q_head / (query_heads / kv_heads));\n\
    int q_base = ((b * query_heads + q_head) * seq_q + q_idx) * dim;\n\
    float max_score = -1.0e30f;\n\
    for (int k_idx = 0; k_idx < seq_k; k_idx += 1) {{\n\
        int key_base = ((b * kv_heads + kv_head) * seq_k + k_idx) * dim;\n\
        float score = 0.0f;\n\
        float residual = 0.0f;\n\
        for (int i = 0; i < dim; i += 1) {{\n\
            score += rotated_q[q_base + i] * key_mse[key_base + i];\n\
            residual += query_sketch[q_base + i] * key_sign[key_base + i];\n\
        }}\n\
        score = (score + residual * key_scale[(b * kv_heads + kv_head) * seq_k + k_idx]) * scale;\n\
        if (has_mask != 0) {{\n\
            score += mask[((b * query_heads + q_head) * seq_q + q_idx) * seq_k + k_idx];\n\
        }}\n\
        if (score > max_score) {{\n\
            max_score = score;\n\
        }}\n\
    }}\n\
    float sum = 0.0f;\n\
    float acc = 0.0f;\n\
    for (int k_idx = 0; k_idx < seq_k; k_idx += 1) {{\n\
        int key_base = ((b * kv_heads + kv_head) * seq_k + k_idx) * dim;\n\
        float score = 0.0f;\n\
        float residual = 0.0f;\n\
        for (int i = 0; i < dim; i += 1) {{\n\
            score += rotated_q[q_base + i] * key_mse[key_base + i];\n\
            residual += query_sketch[q_base + i] * key_sign[key_base + i];\n\
        }}\n\
        score = (score + residual * key_scale[(b * kv_heads + kv_head) * seq_k + k_idx]) * scale;\n\
        if (has_mask != 0) {{\n\
            score += mask[((b * query_heads + q_head) * seq_q + q_idx) * seq_k + k_idx];\n\
        }}\n\
        float weight = exp(score - max_score);\n\
        sum += weight;\n\
        int v_base = ((b * kv_heads + kv_head) * seq_k + k_idx) * dim;\n\
        acc += weight * value_rows[v_base + d];\n\
    }}\n\
    out[idx] = (sum != 0.0f) ? (acc / sum) : 0.0f;\n\
}}\n"
    )
}

#[cfg(any(
    target_arch = "wasm32",
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
))]
fn render_webgpu_turboquant_attention_source(entry: &str, workgroup_size: u32) -> String {
    format!(
        "@group(0) @binding(0) var<storage, read> query_pair: array<f32>;\n\
@group(0) @binding(1) var<storage, read> key_mse: array<f32>;\n\
@group(0) @binding(2) var<storage, read> key_sign: array<f32>;\n\
@group(0) @binding(3) var<storage, read> key_scale: array<f32>;\n\
@group(0) @binding(4) var<storage, read> value_rows: array<f32>;\n\
@group(0) @binding(5) var<storage, read_write> out: array<f32>;\n\
@group(0) @binding(6) var<storage, read> mask: array<f32>;\n\
@group(0) @binding(7) var<storage, read> params: array<u32>;\n\
\n\
@compute @workgroup_size({workgroup_size})\n\
fn {entry}(@builtin(global_invocation_id) gid: vec3<u32>) {{\n\
    let idx = i32(gid.x);\n\
    let batch_val = i32(params[0]);\n\
    let query_heads_val = i32(params[1]);\n\
    let kv_heads_val = i32(params[2]);\n\
    let seq_q_val = i32(params[3]);\n\
    let seq_k_val = i32(params[4]);\n\
    let dim_val = i32(params[5]);\n\
    let scale_val = bitcast<f32>(params[6]);\n\
    let has_mask_val = params[7] != 0u;\n\
    let total = batch_val * query_heads_val * seq_q_val * dim_val;\n\
    if (idx >= total) {{\n\
        return;\n\
    }}\n\
    let d = idx % dim_val;\n\
    let q_idx = (idx / dim_val) % seq_q_val;\n\
    let q_head = (idx / (dim_val * seq_q_val)) % query_heads_val;\n\
    let b = idx / (dim_val * seq_q_val * query_heads_val);\n\
    let kv_head = select(q_head / (query_heads_val / kv_heads_val), q_head, query_heads_val == kv_heads_val);\n\
    let q_base = ((b * query_heads_val + q_head) * seq_q_val + q_idx) * dim_val;\n\
    let query_total = batch_val * query_heads_val * seq_q_val * dim_val;\n\
    var max_score: f32 = -1.0e30;\n\
    for (var k_idx: i32 = 0; k_idx < seq_k_val; k_idx = k_idx + 1) {{\n\
        let key_base = ((b * kv_heads_val + kv_head) * seq_k_val + k_idx) * dim_val;\n\
        var score: f32 = 0.0;\n\
        var residual: f32 = 0.0;\n\
        for (var i: i32 = 0; i < dim_val; i = i + 1) {{\n\
            score = score + query_pair[q_base + i] * key_mse[key_base + i];\n\
            residual = residual + query_pair[query_total + q_base + i] * key_sign[key_base + i];\n\
        }}\n\
        score = (score + residual * key_scale[(b * kv_heads_val + kv_head) * seq_k_val + k_idx]) * scale_val;\n\
        if (has_mask_val) {{\n\
            score = score + mask[((b * query_heads_val + q_head) * seq_q_val + q_idx) * seq_k_val + k_idx];\n\
        }}\n\
        if (score > max_score) {{\n\
            max_score = score;\n\
        }}\n\
    }}\n\
    var sum: f32 = 0.0;\n\
    var acc: f32 = 0.0;\n\
    for (var k_idx: i32 = 0; k_idx < seq_k_val; k_idx = k_idx + 1) {{\n\
        let key_base = ((b * kv_heads_val + kv_head) * seq_k_val + k_idx) * dim_val;\n\
        var score: f32 = 0.0;\n\
        var residual: f32 = 0.0;\n\
        for (var i: i32 = 0; i < dim_val; i = i + 1) {{\n\
            score = score + query_pair[q_base + i] * key_mse[key_base + i];\n\
            residual = residual + query_pair[query_total + q_base + i] * key_sign[key_base + i];\n\
        }}\n\
        score = (score + residual * key_scale[(b * kv_heads_val + kv_head) * seq_k_val + k_idx]) * scale_val;\n\
        if (has_mask_val) {{\n\
            score = score + mask[((b * query_heads_val + q_head) * seq_q_val + q_idx) * seq_k_val + k_idx];\n\
        }}\n\
        let weight = exp(score - max_score);\n\
        sum = sum + weight;\n\
        let v_base = ((b * kv_heads_val + kv_head) * seq_k_val + k_idx) * dim_val;\n\
        acc = acc + weight * value_rows[v_base + d];\n\
    }}\n\
    out[idx] = select(0.0, acc / sum, sum != 0.0);\n\
}}\n"
    )
}

#[cfg(all(target_os = "macos", feature = "metal-backend"))]
struct RuntimeMetalPipeline {
    pipeline: MetalPipelineState,
}

#[cfg(all(target_os = "macos", feature = "metal-backend"))]
unsafe impl Send for RuntimeMetalPipeline {}
#[cfg(all(target_os = "macos", feature = "metal-backend"))]
unsafe impl Sync for RuntimeMetalPipeline {}

#[cfg(all(target_os = "macos", feature = "metal-backend"))]
struct RuntimeMetalDevice {
    device: MetalDeviceObject,
    command_queue: MetalCommandQueue,
}

#[cfg(all(target_os = "macos", feature = "metal-backend"))]
fn require_metal_completed(
    status: MTLCommandBufferStatus,
    error: impl FnOnce() -> Option<String>,
) -> Result<(), String> {
    if status == MTLCommandBufferStatus::Completed {
        return Ok(());
    }
    let detail = error().unwrap_or_else(|| format!("terminal status {status:?}"));
    Err(format!("Metal command execution failed: {detail}"))
}

#[cfg(all(target_os = "macos", feature = "metal-backend"))]
impl RuntimeMetalDevice {
    fn new() -> Result<Self, String> {
        let device =
            MTLCreateSystemDefaultDevice().ok_or_else(|| "No Metal device found".to_string())?;
        let command_queue = device
            .newCommandQueue()
            .ok_or_else(|| "Metal command queue creation failed".to_string())?;
        Ok(Self {
            command_queue,
            device,
        })
    }

    fn compile_pipeline(
        &self,
        name: &str,
        source: &str,
    ) -> Result<Arc<RuntimeMetalPipeline>, String> {
        let library = self
            .device
            .newLibraryWithSource_options_error(&NSString::from_str(source), None)
            .map_err(|err| format!("MSL compile error: {}", err.localizedDescription()))?;
        let function = library
            .newFunctionWithName(&NSString::from_str(name))
            .ok_or_else(|| format!("MSL function lookup failed: {name} is not in the library"))?;
        let pipeline = self
            .device
            .newComputePipelineStateWithFunction_error(&function)
            .map_err(|err| {
                format!(
                    "Metal pipeline creation failed: {}",
                    err.localizedDescription()
                )
            })?;
        Ok(Arc::new(RuntimeMetalPipeline { pipeline }))
    }

    fn alloc_buffer(&self, size_bytes: usize) -> Result<MetalBuffer, String> {
        // Metal rejects a zero-length buffer; an empty argument still needs a
        // bound slot, so it is backed by one byte.
        self.device
            .newBufferWithLength_options(size_bytes.max(1), MTLResourceOptions::StorageModeShared)
            .ok_or_else(|| format!("Metal buffer allocation of {size_bytes} bytes failed"))
    }

    fn copy_to_buffer(&self, buffer: &MetalBuffer, data: &[u8]) -> Result<(), String> {
        if data.len() > buffer.length() {
            return Err("Metal upload exceeds buffer allocation".into());
        }
        let contents = buffer.contents().as_ptr().cast::<u8>();
        // SAFETY: the shared-mode buffer is CPU-visible for its whole lifetime,
        // and every caller allocated it for at least `data.len()` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), contents, data.len());
        }
        Ok(())
    }

    fn copy_from_buffer(&self, buffer: &MetalBuffer, size_bytes: usize) -> Result<Vec<u8>, String> {
        if size_bytes > buffer.length() {
            return Err("Metal readback exceeds buffer allocation".into());
        }
        let mut out = Vec::new();
        out.try_reserve_exact(size_bytes)
            .map_err(|_| "Metal host readback allocation failed".to_string())?;
        out.resize(size_bytes, 0);
        let contents = buffer.contents().as_ptr().cast::<u8>().cast_const();
        // SAFETY: the shared-mode buffer is CPU-visible for its whole lifetime;
        // `dispatch` waited for the command buffer, so every GPU write landed,
        // and callers read back at most the size they allocated.
        unsafe {
            std::ptr::copy_nonoverlapping(contents, out.as_mut_ptr(), size_bytes);
        }
        Ok(out)
    }

    fn dispatch(
        &self,
        pipeline: &Arc<RuntimeMetalPipeline>,
        grid_threads: usize,
        group_threads: usize,
        buffers: &[&MetalBuffer],
    ) -> Result<(), String> {
        if group_threads == 0 || group_threads > pipeline.pipeline.maxTotalThreadsPerThreadgroup() {
            return Err("Metal launch workgroup exceeds pipeline capability".into());
        }
        let command_buffer = self
            .command_queue
            .commandBuffer()
            .ok_or_else(|| "Metal command buffer creation failed".to_string())?;
        let encoder = command_buffer
            .computeCommandEncoder()
            .ok_or_else(|| "Metal compute command encoder creation failed".to_string())?;
        encoder.setComputePipelineState(&pipeline.pipeline);
        for (index, buffer) in buffers.iter().enumerate() {
            // SAFETY: each buffer is a live object of this device bound at
            // offset 0 inside its own length; the encoder retains it.
            unsafe { encoder.setBuffer_offset_atIndex(Some(&***buffer), 0, index) };
        }
        encoder.dispatchThreads_threadsPerThreadgroup(
            MTLSize {
                width: grid_threads,
                height: 1,
                depth: 1,
            },
            MTLSize {
                width: group_threads,
                height: 1,
                depth: 1,
            },
        );
        encoder.endEncoding();
        command_buffer.commit();
        command_buffer.waitUntilCompleted();
        require_metal_completed(command_buffer.status(), || {
            command_buffer
                .error()
                .map(|error| error.localizedDescription().to_string())
        })
    }
}

#[cfg(all(target_os = "macos", feature = "metal-backend"))]
fn dispatch_metal_kernel(
    _py: &PyToken,
    callable_bits: u64,
    grid: i64,
    threads: i64,
    builder_bits: u64,
) -> Result<u64, u64> {
    if trace_gpu_backend_enabled() {
        eprintln!("[molt gpu backend] metal");
    }
    let total_threads = grid
        .checked_mul(threads)
        .and_then(|total| usize::try_from(total).ok())
        .ok_or_else(|| {
            raise_exception::<u64>(
                _py,
                "OverflowError",
                "Metal launch geometry is not representable",
            )
        })?;
    let descriptor_bits = match unsafe { gpu_kernel_descriptor_bits(_py, callable_bits) } {
        Ok(Some(bits)) => bits,
        Ok(None) => {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "metal gpu backend requires compiler-published code descriptor metadata",
            ));
        }
        Err(err) => return Err(err),
    };
    let descriptor_owner = unsafe { OwnedRuntimeValue::from_owned_bits(_py, descriptor_bits) };
    let descriptor_json =
        string_obj_to_owned(obj_from_bits(descriptor_owner.bits())).ok_or_else(|| {
            raise_exception::<u64>(_py, "TypeError", "gpu kernel descriptor must be a string")
        })?;
    let mut descriptor = parse_kernel_descriptor_json(&descriptor_json).map_err(|msg| {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            &format!("invalid gpu kernel descriptor: {msg}"),
        )
    })?;
    let arg_bits = unsafe { bound_kernel_arguments(_py, callable_bits, builder_bits) }?;
    if arg_bits.len() != descriptor.params.len() {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "gpu kernel descriptor parameter count does not match launch args",
        ));
    }
    let mut args_map = BTreeMap::new();
    for (name, bits) in descriptor.params.iter().zip(arg_bits.iter().copied()) {
        if descriptor.ops.iter().any(|op| op.args.contains(name)) {
            args_map.insert(name.clone(), kernel_arg_from_bits(_py, name, bits)?);
        }
    }
    let prepared_outputs = PreparedKernelOutputs::new(_py, &descriptor, &args_map)?;
    let _admitted_bindings = descriptor_admission::admit(
        _py,
        callable_bits,
        descriptor_bits,
        &mut descriptor,
        &mut args_map,
        grid,
        threads,
    )?;
    let mut plan = KernelStoragePlan::capture(_py, &descriptor, &args_map, &prepared_outputs)?;
    let source = render_kernel_source(
        &descriptor,
        &plan,
        grid,
        threads,
        KernelShaderDialect::Metal,
    )
    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
    let device = RuntimeMetalDevice::new()
        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
    let pipeline = device
        .compile_pipeline(&descriptor.name, &source)
        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
    let mut owned_buffers = Vec::new();
    for index in 0..plan.binding_count() {
        let bytes = plan.bytes(index);
        let buffer = device
            .alloc_buffer(bytes.len())
            .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
        device
            .copy_to_buffer(&buffer, bytes)
            .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
        owned_buffers.push(buffer);
    }
    let refs: Vec<&MetalBuffer> = owned_buffers.iter().collect();
    device
        .dispatch(&pipeline, total_threads, threads as usize, &refs)
        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
    let mut outputs = BTreeMap::new();
    for index in 0..plan.binding_count() {
        if plan.writable(index) {
            outputs.insert(
                index,
                device
                    .copy_from_buffer(&owned_buffers[index], plan.bytes(index).len())
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?,
            );
        }
    }
    plan.publish(_py, &outputs, &args_map)?;
    Ok(MoltObject::none().bits())
}

#[cfg(not(all(target_os = "macos", feature = "metal-backend")))]
fn dispatch_metal_kernel(
    _py: &PyToken,
    _callable_bits: u64,
    _grid: i64,
    _threads: i64,
    _builder_bits: u64,
) -> Result<u64, u64> {
    Err(raise_exception::<_>(
        _py,
        "RuntimeError",
        "metal gpu backend requested but molt-gpu was built without metal-backend",
    ))
}

#[cfg(all(not(target_arch = "wasm32"), feature = "webgpu-backend"))]
fn dispatch_webgpu_kernel(
    _py: &PyToken,
    callable_bits: u64,
    grid: i64,
    threads: i64,
    builder_bits: u64,
) -> Result<u64, u64> {
    if trace_gpu_backend_enabled() {
        eprintln!("[molt gpu backend] webgpu");
    }
    let grid = u32::try_from(grid).map_err(|_| {
        raise_exception::<u64>(
            _py,
            "OverflowError",
            "WebGPU grid exceeds unsigned 32-bit geometry",
        )
    })?;
    let threads = u32::try_from(threads).map_err(|_| {
        raise_exception::<u64>(
            _py,
            "OverflowError",
            "WebGPU threads exceeds unsigned 32-bit geometry",
        )
    })?;
    let descriptor_bits = match unsafe { gpu_kernel_descriptor_bits(_py, callable_bits) } {
        Ok(Some(bits)) => bits,
        Ok(None) => {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "webgpu backend requires compiler-published code descriptor metadata",
            ));
        }
        Err(err) => return Err(err),
    };
    let descriptor_owner = unsafe { OwnedRuntimeValue::from_owned_bits(_py, descriptor_bits) };
    let descriptor_json =
        string_obj_to_owned(obj_from_bits(descriptor_owner.bits())).ok_or_else(|| {
            raise_exception::<u64>(_py, "TypeError", "gpu kernel descriptor must be a string")
        })?;
    let mut descriptor = parse_kernel_descriptor_json(&descriptor_json).map_err(|msg| {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            &format!("invalid gpu kernel descriptor: {msg}"),
        )
    })?;
    let arg_bits = unsafe { bound_kernel_arguments(_py, callable_bits, builder_bits) }?;
    if arg_bits.len() != descriptor.params.len() {
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "gpu kernel descriptor parameter count does not match launch args",
        ));
    }
    let mut args_map = BTreeMap::new();
    for (name, bits) in descriptor.params.iter().zip(arg_bits.iter().copied()) {
        if descriptor.ops.iter().any(|op| op.args.contains(name)) {
            args_map.insert(name.clone(), kernel_arg_from_bits(_py, name, bits)?);
        }
    }
    let prepared_outputs = PreparedKernelOutputs::new(_py, &descriptor, &args_map)?;
    let _admitted_bindings = descriptor_admission::admit(
        _py,
        callable_bits,
        descriptor_bits,
        &mut descriptor,
        &mut args_map,
        grid as i64,
        threads as i64,
    )?;
    let mut plan = KernelStoragePlan::capture(_py, &descriptor, &args_map, &prepared_outputs)?;
    let source = render_kernel_source(
        &descriptor,
        &plan,
        i64::from(grid),
        i64::from(threads),
        KernelShaderDialect::Wgsl,
    )
    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
    let device = RuntimeWebGpuDevice::new()
        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
    let pipeline = device
        .compile_pipeline(&descriptor.name, &source)
        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
    let mut owned_buffers = Vec::new();
    for index in 0..plan.binding_count() {
        let bytes = plan.bytes(index);
        let (_, buffer) = device
            .alloc_buffer(bytes.len())
            .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
        device
            .copy_to_buffer(&buffer, bytes)
            .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
        owned_buffers.push(buffer);
    }
    let refs: Vec<&wgpu::Buffer> = owned_buffers.iter().collect();
    device
        .dispatch(&pipeline, grid, &refs)
        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
    let mut outputs = BTreeMap::new();
    for index in 0..plan.binding_count() {
        if plan.writable(index) {
            let bytes = device
                .copy_from_buffer(&owned_buffers[index], plan.bytes(index).len())
                .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
            outputs.insert(index, bytes);
        }
    }
    device
        .check_failure()
        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
    plan.publish(_py, &outputs, &args_map)?;
    Ok(MoltObject::none().bits())
}

#[cfg(target_arch = "wasm32")]
fn dispatch_webgpu_kernel(
    _py: &PyToken,
    callable_bits: u64,
    grid: i64,
    threads: i64,
    builder_bits: u64,
) -> Result<u64, u64> {
    if trace_gpu_backend_enabled() {
        eprintln!("[molt gpu backend] webgpu");
    }
    let grid = u32::try_from(grid).map_err(|_| {
        raise_exception::<u64>(
            _py,
            "OverflowError",
            "WebGPU grid exceeds unsigned 32-bit geometry",
        )
    })?;
    let threads = u32::try_from(threads).map_err(|_| {
        raise_exception::<u64>(
            _py,
            "OverflowError",
            "WebGPU threads exceeds unsigned 32-bit geometry",
        )
    })?;
    let descriptor_bits = match unsafe { gpu_kernel_descriptor_bits(_py, callable_bits) } {
        Ok(Some(bits)) => bits,
        Ok(None) => {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "webgpu backend requires compiler-published code descriptor metadata",
            ));
        }
        Err(err) => return Err(err),
    };
    let descriptor_owner = unsafe { OwnedRuntimeValue::from_owned_bits(_py, descriptor_bits) };
    let descriptor_json =
        string_obj_to_owned(obj_from_bits(descriptor_owner.bits())).ok_or_else(|| {
            raise_exception::<u64>(_py, "TypeError", "gpu kernel descriptor must be a string")
        })?;
    let mut descriptor = parse_kernel_descriptor_json(&descriptor_json).map_err(|msg| {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            &format!("invalid gpu kernel descriptor: {msg}"),
        )
    })?;
    let arg_bits = unsafe { bound_kernel_arguments(_py, callable_bits, builder_bits) }?;
    if arg_bits.len() != descriptor.params.len() {
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "gpu kernel descriptor parameter count does not match launch args",
        ));
    }
    let mut args_map = BTreeMap::new();
    for (name, bits) in descriptor.params.iter().zip(arg_bits.iter().copied()) {
        if descriptor.ops.iter().any(|op| op.args.contains(name)) {
            args_map.insert(name.clone(), kernel_arg_from_bits(_py, name, bits)?);
        }
    }
    let prepared_outputs = PreparedKernelOutputs::new(_py, &descriptor, &args_map)?;
    let _admitted_bindings = descriptor_admission::admit(
        _py,
        callable_bits,
        descriptor_bits,
        &mut descriptor,
        &mut args_map,
        grid as i64,
        threads as i64,
    )?;
    let mut plan = KernelStoragePlan::capture(_py, &descriptor, &args_map, &prepared_outputs)?;
    let source = render_kernel_source(
        &descriptor,
        &plan,
        i64::from(grid),
        i64::from(threads),
        KernelShaderDialect::Wgsl,
    )
    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
    let mut staging: Vec<Vec<u8>> = (0..plan.binding_count())
        .map(|index| plan.bytes(index).to_vec())
        .collect();
    let mut launch_bindings = Vec::new();
    for (index, bytes) in staging.iter_mut().enumerate() {
        launch_bindings.push(serde_json::json!({
            "binding": index,
            "name": format!("molt_binding_{index}"),
            "kind": "buffer",
            "access": if plan.writable(index) { "read_write" } else { "read" },
            "ptr": bytes.as_mut_ptr() as usize as u32,
            "len": u32::try_from(bytes.len()).map_err(|_| raise_exception::<u64>(_py, "OverflowError", "GPU browser binding exceeds WASM32 extent"))?,
        }));
    }
    dispatch_browser_webgpu_bindings(
        _py,
        &source,
        &descriptor.name,
        launch_bindings,
        grid,
        threads,
    )?;
    let outputs = staging
        .into_iter()
        .enumerate()
        .filter(|(index, _)| plan.writable(*index))
        .collect();
    plan.publish(_py, &outputs, &args_map)?;
    Ok(MoltObject::none().bits())
}

#[cfg(not(any(
    target_arch = "wasm32",
    all(not(target_arch = "wasm32"), feature = "webgpu-backend")
)))]
fn dispatch_webgpu_kernel(
    _py: &PyToken,
    _callable_bits: u64,
    _grid: i64,
    _threads: i64,
    _builder_bits: u64,
) -> Result<u64, u64> {
    Err(raise_exception::<u64>(
        _py,
        "RuntimeError",
        "webgpu backend requested but molt-gpu was built without webgpu-backend",
    ))
}

fn bytes_like_view(_py: &PyToken, bits: u64, role: &str) -> Result<ByteView, u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be bytes-like"),
        ));
    };
    let type_id = unsafe { object_type_id(ptr) };
    if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be bytes or bytearray"),
        ));
    }
    Ok(ByteView {
        ptr: unsafe { bytes_data(ptr) },
        len: unsafe { bytes_len(ptr) },
    })
}

unsafe fn require_class_ptr(_py: &PyToken, bits: u64, role: &str) -> Result<*mut u8, u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be a class object"),
        ));
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TYPE {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be a class object"),
        ));
    }
    Ok(ptr)
}

fn normalize_shape_bits(_py: &PyToken, bits: u64) -> Result<(u64, bool), u64> {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        return match unsafe { object_type_id(ptr) } {
            TYPE_ID_TUPLE => Ok((bits, false)),
            TYPE_ID_LIST => {
                let Some(shape) = (unsafe {
                    seq_access::snapshot(_py, ptr, "sequence snapshot allocation failed")
                }) else {
                    return Err(MoltObject::none().bits());
                };
                let tuple_ptr = alloc_tuple(_py, &shape);
                if tuple_ptr.is_null() {
                    Err(MoltObject::none().bits())
                } else {
                    Ok((MoltObject::from_ptr(tuple_ptr).bits(), true))
                }
            }
            _ => {
                if to_i64(obj).is_some() {
                    let tuple_ptr = alloc_tuple(_py, &[bits]);
                    if tuple_ptr.is_null() {
                        Err(MoltObject::none().bits())
                    } else {
                        Ok((MoltObject::from_ptr(tuple_ptr).bits(), true))
                    }
                } else {
                    Err(raise_exception::<_>(
                        _py,
                        "TypeError",
                        "shape must be a tuple, list, or int",
                    ))
                }
            }
        };
    }
    if to_i64(obj).is_some() {
        let tuple_ptr = alloc_tuple(_py, &[bits]);
        if tuple_ptr.is_null() {
            Err(MoltObject::none().bits())
        } else {
            Ok((MoltObject::from_ptr(tuple_ptr).bits(), true))
        }
    } else {
        Err(raise_exception::<_>(
            _py,
            "TypeError",
            "shape must be a tuple, list, or int",
        ))
    }
}

unsafe fn set_object_attr_bytes(
    _py: &PyToken,
    obj_ptr: *mut u8,
    name: &[u8],
    name_str: &str,
    val_bits: u64,
) -> Result<(), u64> {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let out = unsafe { object_setattr_raw(_py, obj_ptr, name_bits, name_str, val_bits) };
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        return Err(out);
    }
    Ok(())
}

unsafe fn object_attr_bits(
    _py: &PyToken,
    obj_bits: u64,
    name: &[u8],
    name_str: &str,
) -> Result<u64, u64> {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let out = molt_get_attr_name(obj_bits, name_bits);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        return Err(out);
    }
    if obj_from_bits(out).is_none() {
        return Err(raise_exception::<_>(
            _py,
            "AttributeError",
            &format!("object has no attribute {:?}", name_str),
        ));
    }
    Ok(out)
}

unsafe fn build_buffer_instance(
    _py: &PyToken,
    buffer_class_bits: u64,
    data_bits: u64,
    element_type_bits: u64,
    size: usize,
    format_bits: u64,
    itemsize: usize,
) -> Result<u64, u64> {
    let buffer_class_ptr = unsafe { require_class_ptr(_py, buffer_class_bits, "buffer_class")? };
    let buffer_bits = unsafe { alloc_instance_for_class(_py, buffer_class_ptr) };
    let Some(buffer_ptr) = obj_from_bits(buffer_bits).as_ptr() else {
        return Err(buffer_bits);
    };
    let size_bits = MoltObject::from_int(size as i64).bits();
    let itemsize_bits = MoltObject::from_int(itemsize as i64).bits();
    if unsafe { set_object_attr_bytes(_py, buffer_ptr, b"_data", "_data", data_bits) }.is_err()
        || unsafe {
            set_object_attr_bytes(
                _py,
                buffer_ptr,
                b"_element_type",
                "_element_type",
                element_type_bits,
            )
        }
        .is_err()
        || unsafe { set_object_attr_bytes(_py, buffer_ptr, b"_size", "_size", size_bits) }.is_err()
        || unsafe {
            set_object_attr_bytes(
                _py,
                buffer_ptr,
                b"_format_char",
                "_format_char",
                format_bits,
            )
        }
        .is_err()
        || unsafe {
            set_object_attr_bytes(_py, buffer_ptr, b"_itemsize", "_itemsize", itemsize_bits)
        }
        .is_err()
    {
        dec_ref_bits(_py, buffer_bits);
        return Err(MoltObject::none().bits());
    }
    Ok(buffer_bits)
}

unsafe fn build_tensor_instance(
    _py: &PyToken,
    tensor_class_bits: u64,
    buf_bits: u64,
    shape_bits: u64,
    dtype_bits: u64,
) -> Result<u64, u64> {
    let tensor_class_ptr = unsafe { require_class_ptr(_py, tensor_class_bits, "tensor_class")? };
    let tensor_bits = unsafe { alloc_instance_for_class(_py, tensor_class_ptr) };
    let Some(tensor_ptr) = obj_from_bits(tensor_bits).as_ptr() else {
        return Err(tensor_bits);
    };
    if unsafe { set_object_attr_bytes(_py, tensor_ptr, b"_buf", "_buf", buf_bits) }.is_err()
        || unsafe { set_object_attr_bytes(_py, tensor_ptr, b"_shape", "_shape", shape_bits) }
            .is_err()
        || unsafe { set_object_attr_bytes(_py, tensor_ptr, b"_dtype", "_dtype", dtype_bits) }
            .is_err()
    {
        dec_ref_bits(_py, tensor_bits);
        return Err(MoltObject::none().bits());
    }
    Ok(tensor_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_thread_id() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let tid = current_gpu_launch_context().thread_id;
        if trace_gpu_thread_id_enabled() {
            eprintln!("[molt gpu thread_id] tid={tid}");
        }
        molt_runtime_core::rt_int(tid)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_block_id() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        molt_runtime_core::rt_int(current_gpu_launch_context().block_id)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_block_dim() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        molt_runtime_core::rt_int(current_gpu_launch_context().block_dim)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_grid_dim() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        molt_runtime_core::rt_int(current_gpu_launch_context().grid_dim)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_barrier() -> u64 {
    // This callable runs on the sequential host executor. Admitted hardware
    // descriptors lower gpu_barrier to the device's real collective operation.
    molt_runtime_core::with_core_gil!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "GPU barrier requires a parallel hardware kernel execution context",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_kernel_launch(
    callable_bits: u64,
    grid_bits: u64,
    threads_bits: u64,
    builder_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let trace_launch = trace_gpu_kernel_launch_enabled();
        let grid = match parse_i64_launch_arg(_py, grid_bits, "grid") {
            Ok(value) => value,
            Err(err) => return err,
        };
        let threads = match parse_i64_launch_arg(_py, threads_bits, "threads") {
            Ok(value) => value,
            Err(err) => return err,
        };
        let Some(total_threads) = grid.checked_mul(threads) else {
            return raise_exception::<u64>(
                _py,
                "OverflowError",
                "GPU launch geometry exceeds signed 64-bit indices",
            );
        };
        // Geometry errors retain precedence. Capture the operation's target
        // once, before any kernel argument binding or device work. Dispatch
        // cannot reread selection or turn a failed explicit request into CPU.
        let executor = match PythonKernelExecutor::for_backend(requested_gpu_backend()) {
            Ok(executor) => executor,
            Err(backend) => {
                return raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    &format!(
                        "requested {backend:?} Python-kernel descriptor execution is unavailable; tensor device support is separate"
                    ),
                );
            }
        };
        match executor {
            PythonKernelExecutor::Metal => {
                return dispatch_metal_kernel(_py, callable_bits, grid, threads, builder_bits)
                    .unwrap_or_else(std::convert::identity);
            }
            PythonKernelExecutor::WebGpu => {
                return dispatch_webgpu_kernel(_py, callable_bits, grid, threads, builder_bits)
                    .unwrap_or_else(std::convert::identity);
            }
            PythonKernelExecutor::Sequential => {}
        }
        let block_dim = threads;
        for tid in 0..total_threads {
            let block_id = tid / block_dim;
            if trace_launch {
                eprintln!(
                    "[molt gpu launch] tid={} block_id={} block_dim={} grid_dim={}",
                    tid, block_id, block_dim, grid
                );
            }
            let call_builder_bits = match unsafe { clone_callargs_builder_bits(_py, builder_bits) }
            {
                Ok(bits) => bits,
                Err(err) => return err,
            };
            let out_bits = with_gpu_launch_context(
                GpuLaunchContext {
                    thread_id: tid,
                    block_id,
                    block_dim,
                    grid_dim: grid,
                },
                // molt_call_bind consumes the builder on every path.
                || molt_call_bind(callable_bits, call_builder_bits),
            );
            if exception_pending(_py) {
                if trace_launch {
                    eprintln!("[molt gpu launch] tid={} exception_pending", tid);
                }
                return out_bits;
            }
            if trace_launch {
                eprintln!("[molt gpu launch] tid={} ok", tid);
            }
            dec_ref_bits(_py, out_bits);
        }
        MoltObject::none().bits()
    })
}

#[cfg(test)]
mod launch_context_tests {
    use super::{GpuLaunchContext, current_gpu_launch_context, with_gpu_launch_context};

    #[test]
    fn launch_context_restores_nested_and_unwinding_scopes() {
        let outer = GpuLaunchContext {
            thread_id: 5,
            block_id: 1,
            block_dim: 4,
            grid_dim: 2,
        };
        let inner = GpuLaunchContext {
            thread_id: 0,
            block_id: 0,
            block_dim: 3,
            grid_dim: 1,
        };
        assert_eq!(current_gpu_launch_context(), GpuLaunchContext::default());
        with_gpu_launch_context(outer, || {
            let failure = std::panic::catch_unwind(|| {
                with_gpu_launch_context(inner, || {
                    assert_eq!(current_gpu_launch_context(), inner);
                    panic!("kernel unwind");
                })
            });
            assert!(failure.is_err());
            assert_eq!(current_gpu_launch_context(), outer);
        });
        assert_eq!(current_gpu_launch_context(), GpuLaunchContext::default());
    }

    #[test]
    fn launch_context_is_local_to_each_overlapping_thread() {
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            for thread_id in [3, 7] {
                let barrier = &barrier;
                scope.spawn(move || {
                    let expected = GpuLaunchContext {
                        thread_id,
                        ..GpuLaunchContext::default()
                    };
                    with_gpu_launch_context(expected, || {
                        barrier.wait();
                        assert_eq!(current_gpu_launch_context(), expected);
                        barrier.wait();
                    });
                    assert_eq!(current_gpu_launch_context(), GpuLaunchContext::default());
                });
            }
        });
    }
}

#[cfg(all(test, not(target_arch = "wasm32"), feature = "webgpu-backend"))]
mod webgpu_device_tests {
    use super::*;

    #[test]
    #[ignore = "requires an admitted native WebGPU adapter; run explicitly in the hardware cell"]
    fn actual_device_errors_precede_readback_publication() {
        let device = RuntimeWebGpuDevice::new().expect("admitted WebGPU adapter");
        let source = "@group(0) @binding(0) var<storage, read_write> a: array<i32>; @group(0) @binding(1) var<storage, read_write> b: array<i32>; @compute @workgroup_size(1) fn main() { a[0] = 42; b[0] = 17; }";
        let pipeline = device.compile_pipeline("main", source).unwrap();
        let (_, a) = device.alloc_buffer(4).unwrap();
        let (_, b) = device.alloc_buffer(4).unwrap();
        device.copy_to_buffer(&a, &9i32.to_le_bytes()).unwrap();
        device.copy_to_buffer(&b, &8i32.to_le_bytes()).unwrap();
        device.dispatch(&pipeline, 1, &[&a, &b]).unwrap();
        assert_eq!(device.copy_from_buffer(&a, 4).unwrap(), 42i32.to_le_bytes());
        assert_eq!(device.copy_from_buffer(&b, 4).unwrap(), 17i32.to_le_bytes());
        assert!(device.compile_pipeline("main", "invalid WGSL").is_err());
        assert!(device.copy_to_buffer(&a, &[0; 8]).is_err());
        let limit = device.device.limits().max_compute_workgroups_per_dimension;
        assert!(
            device
                .dispatch(&pipeline, limit.checked_add(1).unwrap(), &[&a, &b])
                .is_err()
        );
        // First readback succeeds; the second fails. The transaction may not
        // publish even its first output. This is the same Result propagation
        // used before KernelStoragePlan::publish by the production caller.
        let mut published = (vec![9; 4], vec![8; 4]);
        let readbacks = (|| -> Result<_, String> {
            let first = device.copy_from_buffer(&a, 4)?;
            let second = device.copy_from_buffer(&b, 8)?;
            Ok((first, second))
        })();
        assert!(readbacks.is_err());
        if let Ok(result) = readbacks {
            published = result;
        }
        assert_eq!(published, (vec![9; 4], vec![8; 4]));
        // Empty/no-write execution still drains the queue and checks all scopes.
        let empty = device
            .compile_pipeline("main", "@compute @workgroup_size(1) fn main() {}")
            .unwrap();
        device.dispatch(&empty, 1, &[]).unwrap();
        device.device.destroy();
        let _ = device.device.poll(wgpu::PollType::wait_indefinitely());
        assert!(device.dispatch(&empty, 1, &[]).is_err());
    }
}

#[cfg(all(test, target_os = "macos", feature = "metal-backend"))]
mod metal_device_tests {
    use super::*;

    #[test]
    fn only_successful_terminal_status_allows_readback_continuation() {
        for status in [
            MTLCommandBufferStatus::NotEnqueued,
            MTLCommandBufferStatus::Enqueued,
            MTLCommandBufferStatus::Committed,
            MTLCommandBufferStatus::Scheduled,
            MTLCommandBufferStatus::Error,
        ] {
            let mut readback_ran = false;
            let result =
                require_metal_completed(status, || Some("controlled device failure".into())).map(
                    |()| {
                        readback_ran = true;
                    },
                );
            assert!(result.unwrap_err().contains("controlled device failure"));
            assert!(!readback_ran);
        }
        assert!(
            require_metal_completed(MTLCommandBufferStatus::Completed, || panic!(
                "success needs no error materialization"
            ))
            .is_ok()
        );
    }

    #[test]
    #[ignore = "requires an admitted Metal device; run explicitly in the hardware cell"]
    fn actual_device_success_and_capability_error_preserve_outputs() {
        let device = RuntimeMetalDevice::new().expect("admitted Metal device");
        let source = "#include <metal_stdlib>\nusing namespace metal; kernel void main0(device int* a [[buffer(0)]], device int* b [[buffer(1)]]) { a[0] = 42; b[0] = 17; }";
        let pipeline = device.compile_pipeline("main0", source).unwrap();
        let a = device.alloc_buffer(4).unwrap();
        let b = device.alloc_buffer(4).unwrap();
        device.copy_to_buffer(&a, &9i32.to_ne_bytes()).unwrap();
        device.copy_to_buffer(&b, &8i32.to_ne_bytes()).unwrap();
        device.dispatch(&pipeline, 1, 1, &[&a, &b]).unwrap();
        assert_eq!(device.copy_from_buffer(&a, 4).unwrap(), 42i32.to_ne_bytes());
        assert_eq!(device.copy_from_buffer(&b, 4).unwrap(), 17i32.to_ne_bytes());
        let invalid = pipeline.pipeline.maxTotalThreadsPerThreadgroup() + 1;
        assert!(
            device
                .dispatch(&pipeline, invalid, invalid, &[&a, &b])
                .is_err()
        );
        assert!(device.copy_to_buffer(&a, &[0; 8]).is_err());
        assert!(device.copy_from_buffer(&b, 8).is_err());
        assert_eq!(device.copy_from_buffer(&a, 4).unwrap(), 42i32.to_ne_bytes());
        assert_eq!(device.copy_from_buffer(&b, 4).unwrap(), 17i32.to_ne_bytes());
    }
}
