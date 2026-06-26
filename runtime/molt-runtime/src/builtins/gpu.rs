#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

use super::gpu_backend::{GpuBackend, requested_gpu_backend};
use crate::{
    MoltObject, PyToken, TYPE_ID_BYTEARRAY, TYPE_ID_BYTES, TYPE_ID_LIST, TYPE_ID_TUPLE,
    TYPE_ID_TYPE, alloc_bytearray, alloc_bytes, alloc_tuple, attr_name_bits_from_bytes, bytes_data,
    bytes_len, dec_ref_bits, molt_call_bind, molt_exception_clear, molt_exception_kind,
    molt_exception_last, obj_from_bits, object_type_id, raise_exception, seq_vec_ref,
    string_obj_to_owned, to_f64, to_i64,
};
#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "molt_gpu_metal"),
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
))]
use serde_json::Value as JsonValue;
use std::cell::RefCell;
#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "molt_gpu_metal"),
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
))]
use std::collections::{BTreeMap, BTreeSet};
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu"))]
use std::sync::{Arc as WgpuArc, Mutex as WgpuMutex};

#[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
use metal::{
    Buffer as MetalBuffer, CommandQueue, CompileOptions, ComputePipelineState, Device, Library,
    MTLResourceOptions, MTLSize, NSUInteger,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu"))]
use pollster;
#[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
use std::sync::Arc;
#[cfg(all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu"))]
use wgpu;

mod tensor_runtime;

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

#[derive(Copy, Clone)]
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
    static GPU_LAUNCH_CONTEXT_STACK: RefCell<Vec<GpuLaunchContext>> = const { RefCell::new(Vec::new()) };
}

fn with_gpu_launch_context<R>(ctx: GpuLaunchContext, body: impl FnOnce() -> R) -> R {
    GPU_LAUNCH_CONTEXT_STACK.with(|stack| {
        stack.borrow_mut().push(ctx);
        let out = body();
        let _ = stack.borrow_mut().pop();
        out
    })
}

fn current_gpu_launch_context() -> GpuLaunchContext {
    GPU_LAUNCH_CONTEXT_STACK
        .with(|stack| stack.borrow().last().copied())
        .unwrap_or_default()
}

fn trace_gpu_kernel_launch_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_GPU_KERNEL_LAUNCH").as_deref() == Ok("1"))
}

fn trace_gpu_thread_id_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_GPU_THREAD_ID").as_deref() == Ok("1"))
}

#[allow(dead_code)]
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
    for chunk in raw.chunks_exact(2) {
        let bits = u16::from_le_bytes([chunk[0], chunk[1]]);
        out.extend_from_slice(&decode_f16_to_f32_bits(bits).to_le_bytes());
    }
    Ok(out)
}

fn decode_bf16_payload_to_f32_bytes(raw: &[u8]) -> Result<Vec<u8>, &'static str> {
    if !raw.len().is_multiple_of(2) {
        return Err("BF16 payload length must be even");
    }
    let mut out = Vec::with_capacity((raw.len() / 2) * 4);
    for chunk in raw.chunks_exact(2) {
        let bits = u16::from_le_bytes([chunk[0], chunk[1]]);
        let widened = (bits as u32) << 16;
        out.extend_from_slice(&widened.to_le_bytes());
    }
    Ok(out)
}

fn decode_half_bytes_to_f32_object(
    _py: &crate::PyToken<'_>,
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
    crate::with_gil_entry_nopanic!(_py, {
        decode_half_bytes_to_f32_object(_py, data_bits, decode_f16_payload_to_f32_bytes)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_interop_decode_bf16_bytes_to_f32(data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        decode_half_bytes_to_f32_object(_py, data_bits, decode_bf16_payload_to_f32_bytes)
    })
}

fn parse_i64_launch_arg(_py: &crate::PyToken<'_>, bits: u64, role: &str) -> Result<i64, u64> {
    let Some(value) = to_i64(obj_from_bits(bits)) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be an integer"),
        ));
    };
    Ok(value)
}

unsafe fn try_object_attr_bits(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
) -> Result<Option<u64>, u64> {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let out = crate::builtins::attributes::molt_get_attr_name(obj_bits, name_bits);
    dec_ref_bits(_py, name_bits);
    if crate::exception_pending(_py) {
        let exc_bits = molt_exception_last();
        let kind_bits = molt_exception_kind(exc_bits);
        let kind =
            string_obj_to_owned(obj_from_bits(kind_bits)).unwrap_or_else(|| "<exc>".to_string());
        dec_ref_bits(_py, kind_bits);
        if kind == "AttributeError" {
            let _ = molt_exception_clear();
            return Ok(None);
        }
        return Err(out);
    }
    if obj_from_bits(out).is_none() {
        return Ok(None);
    }
    Ok(Some(out))
}

unsafe fn gpu_kernel_callable_bits(
    _py: &crate::PyToken<'_>,
    launcher_bits: u64,
) -> Result<u64, u64> {
    if let Some(func_bits) = unsafe { try_object_attr_bits(_py, launcher_bits, b"_func")? } {
        return Ok(func_bits);
    }
    Ok(launcher_bits)
}

#[allow(dead_code)]
unsafe fn gpu_kernel_descriptor_bits(
    _py: &crate::PyToken<'_>,
    callable_bits: u64,
) -> Result<Option<u64>, u64> {
    unsafe { try_object_attr_bits(_py, callable_bits, b"__molt_gpu_descriptor__") }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu"))]
type RuntimeWebGpuBufferRegistry = WgpuArc<WgpuMutex<std::collections::HashMap<u64, wgpu::Buffer>>>;

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu"))]
struct RuntimeWebGpuPipeline {
    #[allow(dead_code)]
    shader: wgpu::ShaderModule,
    pipeline: wgpu::ComputePipeline,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu"))]
struct RuntimeWebGpuDevice {
    device: wgpu::Device,
    queue: wgpu::Queue,
    buffers: RuntimeWebGpuBufferRegistry,
    next_id: WgpuMutex<u64>,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu"))]
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
            Ok(Self {
                device,
                queue,
                buffers: WgpuArc::new(WgpuMutex::new(std::collections::HashMap::new())),
                next_id: WgpuMutex::new(1),
            })
        })
    }

    fn compile_pipeline(
        &self,
        name: &str,
        source: &str,
    ) -> Result<WgpuArc<RuntimeWebGpuPipeline>, String> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
        if let Some(err) = pollster::block_on(scope.pop()) {
            return Err(err.to_string());
        }
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
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
        if let Some(err) = pollster::block_on(scope.pop()) {
            return Err(err.to_string());
        }
        Ok(WgpuArc::new(RuntimeWebGpuPipeline { shader, pipeline }))
    }

    fn alloc_buffer(&self, size_bytes: usize) -> (u64, wgpu::Buffer) {
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: size_bytes as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut next_id = self.next_id.lock().unwrap();
        let id = *next_id;
        *next_id += 1;
        self.buffers.lock().unwrap().insert(id, buffer.clone());
        (id, buffer)
    }

    fn copy_to_buffer(&self, buffer: &wgpu::Buffer, data: &[u8]) {
        self.queue.write_buffer(buffer, 0, data);
    }

    fn copy_from_buffer(
        &self,
        buffer: &wgpu::Buffer,
        size_bytes: usize,
    ) -> Result<Vec<u8>, String> {
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
        let mapped = slice.get_mapped_range();
        let mut out = vec![0u8; size_bytes];
        out.copy_from_slice(&mapped[..size_bytes]);
        drop(mapped);
        staging.unmap();
        Ok(out)
    }

    fn dispatch(
        &self,
        pipeline: &WgpuArc<RuntimeWebGpuPipeline>,
        grid: u32,
        buffers: &[&wgpu::Buffer],
    ) -> Result<(), String> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
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
        if let Some(err) = pollster::block_on(scope.pop()) {
            return Err(err.to_string());
        }
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|err| err.to_string())?;
        Ok(())
    }
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "molt_gpu_metal"),
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
))]
#[derive(Clone)]
struct RuntimeKernelBufferArg {
    name: String,
    object_bits: u64,
    object_ptr: *mut u8,
    data_bits: u64,
    original_format: String,
    size: usize,
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "molt_gpu_metal"),
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
))]
#[derive(Clone)]
enum RuntimeKernelArg {
    Buffer(RuntimeKernelBufferArg),
    Int(i64),
    Float(f64),
    Bool(bool),
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "molt_gpu_metal"),
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
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
    all(target_os = "macos", feature = "molt_gpu_metal"),
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
))]
#[derive(Clone)]
struct RuntimeKernelDescriptor {
    name: String,
    params: Vec<String>,
    ops: Vec<RuntimeKernelOp>,
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "molt_gpu_metal"),
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
))]
fn parse_kernel_descriptor_json(text: &str) -> Result<RuntimeKernelDescriptor, String> {
    let root: JsonValue = serde_json::from_str(text).map_err(|err| err.to_string())?;
    let obj = root
        .as_object()
        .ok_or_else(|| "kernel descriptor must be an object".to_string())?;
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
    Ok(RuntimeKernelDescriptor { name, params, ops })
}

fn parse_format(_py: &crate::PyToken<'_>, bits: u64, role: &str) -> Result<ScalarFormat, u64> {
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

fn parse_usize_arg(_py: &crate::PyToken<'_>, bits: u64, role: &str) -> Result<usize, u64> {
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
    all(target_os = "macos", feature = "molt_gpu_metal"),
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
))]
fn kernel_arg_from_bits(
    _py: &crate::PyToken<'_>,
    name: &str,
    bits: u64,
) -> Result<RuntimeKernelArg, u64> {
    let obj = obj_from_bits(bits);
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
            let format_bits =
                unsafe { object_attr_bits(_py, bits, b"_format_char", "_format_char")? };
            let size_bits = unsafe { object_attr_bits(_py, bits, b"_size", "_size")? };
            let format = string_obj_to_owned(obj_from_bits(format_bits)).ok_or_else(|| {
                raise_exception::<u64>(_py, "TypeError", "buffer format must be a string")
            })?;
            let size = parse_usize_arg(_py, size_bits, "_size")?;
            return Ok(RuntimeKernelArg::Buffer(RuntimeKernelBufferArg {
                name: name.to_string(),
                object_bits: bits,
                object_ptr: ptr,
                data_bits,
                original_format: format,
                size,
            }));
        }
    }
    if let Some(value) = to_i64(obj) {
        return Ok(RuntimeKernelArg::Int(value));
    }
    if let Some(value) = to_f64(obj) {
        return Ok(RuntimeKernelArg::Float(value));
    }
    if obj.is_bool() {
        return Ok(RuntimeKernelArg::Bool(obj.as_bool().unwrap_or(false)));
    }
    Err(raise_exception::<_>(
        _py,
        "RuntimeError",
        &format!("unsupported gpu kernel argument for parameter {:?}", name),
    ))
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "molt_gpu_metal"),
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
))]
fn metal_scalar_type_for_buffer(format: &str) -> Result<(&'static str, usize), String> {
    match format {
        "f" | "d" => Ok(("float", 4)),
        "q" => Ok(("int64_t", 8)),
        _ => Err(format!(
            "unsupported buffer format for metal backend: {format}"
        )),
    }
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "molt_gpu_metal"),
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
))]
fn metal_scalar_type_for_arg(arg: &RuntimeKernelArg) -> Result<(&'static str, Vec<u8>), String> {
    match arg {
        RuntimeKernelArg::Int(v) => Ok(("int64_t", v.to_le_bytes().to_vec())),
        RuntimeKernelArg::Float(v) => Ok(("float", (*v as f32).to_le_bytes().to_vec())),
        RuntimeKernelArg::Bool(v) => Ok(("bool", vec![u8::from(*v)])),
        RuntimeKernelArg::Buffer(_) => Err("buffer passed as scalar param".to_string()),
    }
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "molt_gpu_metal"),
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
))]
fn buffer_host_bytes_for_gpu_compute(
    _py: &crate::PyToken<'_>,
    arg: &RuntimeKernelBufferArg,
) -> Result<Vec<u8>, String> {
    let view = bytes_like_view(_py, arg.data_bits, "_data")
        .map_err(|_| "buffer _data must be bytes-like".to_string())?;
    let raw = unsafe { std::slice::from_raw_parts(view.ptr, view.len) };
    match arg.original_format.as_str() {
        "f" | "q" => Ok(raw.to_vec()),
        "d" => {
            let mut out = Vec::with_capacity(arg.size * 4);
            for chunk in raw.chunks_exact(8) {
                let val = f64::from_le_bytes(chunk.try_into().map_err(|_| "invalid f64 bytes")?);
                out.extend_from_slice(&(val as f32).to_le_bytes());
            }
            Ok(out)
        }
        other => Err(format!(
            "unsupported buffer format for metal backend: {other}"
        )),
    }
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "molt_gpu_metal"),
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
))]
fn encode_webgpu_buffer_bytes(raw: &[u8], format: ScalarFormat) -> Result<Vec<u8>, String> {
    match format {
        ScalarFormat::F32 => Ok(raw.to_vec()),
        ScalarFormat::F64 => {
            let mut out = Vec::with_capacity(raw.len() / 2);
            for chunk in raw.chunks_exact(8) {
                let val = f64::from_le_bytes(chunk.try_into().map_err(|_| "invalid f64 bytes")?);
                out.extend_from_slice(&(val as f32).to_le_bytes());
            }
            Ok(out)
        }
        ScalarFormat::I64 => {
            let mut out = Vec::with_capacity(raw.len() / 2);
            for chunk in raw.chunks_exact(8) {
                let val = i64::from_le_bytes(chunk.try_into().map_err(|_| "invalid i64 bytes")?);
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
    all(target_os = "macos", feature = "molt_gpu_metal"),
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
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
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
))]
fn buffer_host_bytes_for_webgpu_compute(
    _py: &crate::PyToken<'_>,
    arg: &RuntimeKernelBufferArg,
) -> Result<Vec<u8>, String> {
    let view = bytes_like_view(_py, arg.data_bits, "_data")
        .map_err(|_| "buffer _data must be bytes-like".to_string())?;
    let format = scalar_format_from_text(arg.original_format.as_str()).ok_or_else(|| {
        format!(
            "unsupported buffer format for webgpu backend: {}",
            arg.original_format
        )
    })?;
    bytes_like_view_to_webgpu_bytes(view, format)
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "molt_gpu_metal"),
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
))]
fn copy_gpu32_output_back_to_buffer(
    _py: &crate::PyToken<'_>,
    arg: &RuntimeKernelBufferArg,
    gpu_output: &[u8],
) -> Result<(), u64> {
    let format = scalar_format_from_text(arg.original_format.as_str()).ok_or_else(|| {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            &format!(
                "unsupported buffer format for gpu output: {}",
                arg.original_format
            ),
        )
    })?;
    let rebuilt = rebuild_host_bytes_from_gpu32_output(_py, format, arg.size, gpu_output)?;
    let data_ptr = alloc_bytearray(_py, rebuilt.as_slice());
    if data_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    let data_bits = MoltObject::from_ptr(data_ptr).bits();
    unsafe { set_object_attr_bytes(_py, arg.object_ptr, b"_data", "_data", data_bits)? };
    dec_ref_bits(_py, data_bits);
    Ok(())
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "molt_gpu_metal"),
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
))]
fn rebuild_host_bytes_from_gpu32_output(
    _py: &crate::PyToken<'_>,
    format: ScalarFormat,
    elem_count: usize,
    gpu_output: &[u8],
) -> Result<Vec<u8>, u64> {
    match format {
        ScalarFormat::F32 | ScalarFormat::I64 => {
            if format == ScalarFormat::I64 {
                let mut out = Vec::with_capacity(elem_count * 8);
                for chunk in gpu_output.chunks_exact(4) {
                    let val = i32::from_le_bytes(chunk.try_into().map_err(|_| {
                        raise_exception::<u64>(_py, "RuntimeError", "invalid gpu i32 output bytes")
                    })?) as i64;
                    out.extend_from_slice(&val.to_le_bytes());
                }
                Ok(out)
            } else {
                Ok(gpu_output.to_vec())
            }
        }
        ScalarFormat::F64 => {
            let mut out = Vec::with_capacity(elem_count * 8);
            for chunk in gpu_output.chunks_exact(4) {
                let val = f32::from_le_bytes(chunk.try_into().map_err(|_| {
                    raise_exception::<u64>(_py, "RuntimeError", "invalid f32 output bytes")
                })?) as f64;
                out.extend_from_slice(&val.to_le_bytes());
            }
            Ok(out)
        }
    }
}

#[cfg(any(
    target_arch = "wasm32",
    all(target_os = "macos", feature = "molt_gpu_metal"),
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
))]
fn webgpu_scalar_bytes_for_arg(arg: &RuntimeKernelArg) -> Result<Vec<u8>, String> {
    match arg {
        RuntimeKernelArg::Int(v) => Ok((*v as i32).to_le_bytes().to_vec()),
        RuntimeKernelArg::Float(v) => Ok((*v as f32).to_le_bytes().to_vec()),
        RuntimeKernelArg::Bool(v) => Ok(u32::from(*v).to_le_bytes().to_vec()),
        RuntimeKernelArg::Buffer(_) => Err("buffer passed as scalar param".to_string()),
    }
}

#[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
fn render_metal_source(
    desc: &RuntimeKernelDescriptor,
    args: &BTreeMap<String, RuntimeKernelArg>,
) -> Result<(String, Vec<String>, Vec<String>, Vec<String>), String> {
    let mut write_buffers = BTreeSet::new();
    let mut read_buffers = BTreeSet::new();
    for op in &desc.ops {
        match op.kind.as_str() {
            "index" => {
                if let Some(name) = op.args.first() {
                    read_buffers.insert(name.clone());
                }
            }
            "store_index" => {
                if let Some(name) = op.args.first() {
                    write_buffers.insert(name.clone());
                }
            }
            _ => {}
        }
    }

    let mut source = String::from("#include <metal_stdlib>\nusing namespace metal;\n\n");
    source.push_str(&format!("kernel void {}(\n", desc.name));

    let mut buffer_names = Vec::new();
    let mut scalar_names = Vec::new();
    let mut param_lines = Vec::new();
    let mut binding_index = 0usize;
    for name in &desc.params {
        match args.get(name) {
            Some(RuntimeKernelArg::Buffer(buf)) => {
                let (ty, _) = metal_scalar_type_for_buffer(buf.original_format.as_str())?;
                let qualifier = if write_buffers.contains(name) {
                    "device"
                } else {
                    "device const"
                };
                param_lines.push(format!(
                    "    {qualifier} {ty}* {name} [[buffer({binding_index})]]"
                ));
                buffer_names.push(name.clone());
                binding_index += 1;
            }
            Some(arg) => {
                let (ty, _) = metal_scalar_type_for_arg(arg)?;
                param_lines.push(format!(
                    "    constant {ty}& {name} [[buffer({binding_index})]]"
                ));
                scalar_names.push(name.clone());
                binding_index += 1;
            }
            None => return Err(format!("missing kernel arg for parameter {name}")),
        }
    }
    param_lines.push("    uint tid [[thread_position_in_grid]]".to_string());
    source.push_str(&param_lines.join(",\n"));
    source.push_str("\n) {\n");

    let mut exprs: BTreeMap<String, String> = BTreeMap::new();
    let mut if_stack: Vec<String> = Vec::new();
    for op in &desc.ops {
        match op.kind.as_str() {
            "missing" | "line" | "const_none" | "ret" => {}
            "store_var" => {
                if let (Some(var), Some(src)) = (op.var.as_ref(), op.args.first()) {
                    let src_expr = exprs.get(src).cloned().unwrap_or_else(|| src.clone());
                    exprs.insert(var.clone(), src_expr);
                }
            }
            "load_var" => {
                if let (Some(out), Some(var)) = (op.out.as_ref(), op.var.as_ref()) {
                    let src = exprs.get(var).cloned().unwrap_or_else(|| var.clone());
                    exprs.insert(out.clone(), src);
                }
            }
            "gpu_thread_id" => {
                if let Some(out) = op.out.as_ref() {
                    exprs.insert(out.clone(), "tid".to_string());
                }
            }
            "const" => {
                if let Some(out) = op.out.as_ref() {
                    let value = op
                        .value
                        .ok_or_else(|| "const op missing value".to_string())?;
                    exprs.insert(out.clone(), value.to_string());
                }
            }
            "lt" | "add" | "sub" | "mul" | "div" => {
                if let (Some(out), Some(lhs), Some(rhs)) =
                    (op.out.as_ref(), op.args.first(), op.args.get(1))
                {
                    let lhs_expr = exprs.get(lhs).cloned().unwrap_or_else(|| lhs.clone());
                    let rhs_expr = exprs.get(rhs).cloned().unwrap_or_else(|| rhs.clone());
                    let op_str = match op.kind.as_str() {
                        "lt" => "<",
                        "add" => "+",
                        "sub" => "-",
                        "mul" => "*",
                        "div" => "/",
                        _ => unreachable!(),
                    };
                    source.push_str(&format!(
                        "    auto {out} = {lhs_expr} {op_str} {rhs_expr};\n"
                    ));
                    exprs.insert(out.clone(), out.clone());
                }
            }
            "index" => {
                if let (Some(out), Some(buf), Some(idx)) =
                    (op.out.as_ref(), op.args.first(), op.args.get(1))
                {
                    let idx_expr = exprs.get(idx).cloned().unwrap_or_else(|| idx.clone());
                    source.push_str(&format!("    auto {out} = {buf}[{idx_expr}];\n"));
                    exprs.insert(out.clone(), out.clone());
                }
            }
            "if" => {
                if let Some(cond_name) = op.args.first() {
                    let cond_expr = exprs
                        .get(cond_name)
                        .cloned()
                        .unwrap_or_else(|| cond_name.clone());
                    source.push_str(&format!("    if ({cond_expr}) {{\n"));
                    if_stack.push(cond_expr);
                }
            }
            "end_if" => {
                if if_stack.pop().is_some() {
                    source.push_str("    }\n");
                }
            }
            "store_index" => {
                if let (Some(buf), Some(idx), Some(src)) =
                    (op.args.first(), op.args.get(1), op.args.get(2))
                {
                    let idx_expr = exprs.get(idx).cloned().unwrap_or_else(|| idx.clone());
                    let src_expr = exprs.get(src).cloned().unwrap_or_else(|| src.clone());
                    source.push_str(&format!("        {buf}[{idx_expr}] = {src_expr};\n"));
                }
            }
            other => return Err(format!("unsupported metal kernel op: {other}")),
        }
    }
    while if_stack.pop().is_some() {
        source.push_str("    }\n");
    }
    source.push_str("}\n");
    Ok((
        source,
        buffer_names,
        scalar_names,
        write_buffers.into_iter().collect(),
    ))
}

#[cfg(any(
    target_arch = "wasm32",
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
))]
fn webgpu_scalar_type_for_buffer(format: &str) -> Result<&'static str, String> {
    match format {
        "f" | "d" => Ok("f32"),
        "q" => Ok("i32"),
        _ => Err(format!(
            "unsupported buffer format for webgpu backend: {format}"
        )),
    }
}

#[cfg(any(
    target_arch = "wasm32",
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
))]
fn render_webgpu_source(
    desc: &RuntimeKernelDescriptor,
    args: &BTreeMap<String, RuntimeKernelArg>,
    workgroup_size: u32,
) -> Result<(String, Vec<String>, Vec<String>, Vec<String>), String> {
    let mut write_buffers = BTreeSet::new();
    for op in &desc.ops {
        if op.kind == "store_index"
            && let Some(name) = op.args.first()
        {
            write_buffers.insert(name.clone());
        }
    }

    let mut source = String::new();
    let mut buffer_names = Vec::new();
    let mut scalar_names = Vec::new();
    let mut binding = 0usize;
    for name in &desc.params {
        match args.get(name) {
            Some(RuntimeKernelArg::Buffer(buf)) => {
                let ty = webgpu_scalar_type_for_buffer(buf.original_format.as_str())?;
                let access = if write_buffers.contains(name) {
                    "read_write"
                } else {
                    "read"
                };
                source.push_str(&format!(
                    "@group(0) @binding({binding}) var<storage, {access}> {name}: array<{ty}>;\n"
                ));
                buffer_names.push(name.clone());
                binding += 1;
            }
            Some(_) => {
                let ty = match args.get(name).expect("scalar arg missing") {
                    RuntimeKernelArg::Int(_) => "i32",
                    RuntimeKernelArg::Float(_) => "f32",
                    RuntimeKernelArg::Bool(_) => "u32",
                    RuntimeKernelArg::Buffer(_) => unreachable!(),
                };
                source.push_str(&format!(
                    "@group(0) @binding({binding}) var<storage, read> {name}: array<{ty}>;\n"
                ));
                scalar_names.push(name.clone());
                binding += 1;
            }
            None => return Err(format!("missing kernel arg for parameter {name}")),
        }
    }
    source.push_str(&format!(
        "\n@compute @workgroup_size({workgroup_size})\nfn {}(@builtin(global_invocation_id) gid: vec3<u32>) {{\n",
        desc.name
    ));
    source.push_str("    let tid = i32(gid.x);\n");

    let mut exprs = BTreeMap::new();
    for name in &scalar_names {
        exprs.insert(name.clone(), format!("{name}[0]"));
    }
    let mut if_depth = 0usize;
    for op in &desc.ops {
        match op.kind.as_str() {
            "missing" | "line" | "const_none" | "ret" => {}
            "store_var" => {
                if let (Some(var), Some(src)) = (op.var.as_ref(), op.args.first()) {
                    let src_expr = exprs.get(src).cloned().unwrap_or_else(|| src.clone());
                    exprs.insert(var.clone(), src_expr);
                }
            }
            "load_var" => {
                if let (Some(out), Some(var)) = (op.out.as_ref(), op.var.as_ref()) {
                    let src_expr = exprs.get(var).cloned().unwrap_or_else(|| var.clone());
                    exprs.insert(out.clone(), src_expr);
                }
            }
            "gpu_thread_id" => {
                if let Some(out) = op.out.as_ref() {
                    exprs.insert(out.clone(), "tid".to_string());
                }
            }
            "const" => {
                if let Some(out) = op.out.as_ref() {
                    let value = op
                        .value
                        .ok_or_else(|| "const op missing value".to_string())?;
                    exprs.insert(out.clone(), value.to_string());
                }
            }
            "lt" | "add" | "sub" | "mul" | "div" => {
                if let (Some(out), Some(lhs), Some(rhs)) =
                    (op.out.as_ref(), op.args.first(), op.args.get(1))
                {
                    let lhs_expr = exprs.get(lhs).cloned().unwrap_or_else(|| lhs.clone());
                    let rhs_expr = exprs.get(rhs).cloned().unwrap_or_else(|| rhs.clone());
                    let op_str = match op.kind.as_str() {
                        "lt" => "<",
                        "add" => "+",
                        "sub" => "-",
                        "mul" => "*",
                        "div" => "/",
                        _ => unreachable!(),
                    };
                    source.push_str(&format!(
                        "    let {out} = {lhs_expr} {op_str} {rhs_expr};\n"
                    ));
                    exprs.insert(out.clone(), out.clone());
                }
            }
            "index" => {
                if let (Some(out), Some(buf), Some(idx)) =
                    (op.out.as_ref(), op.args.first(), op.args.get(1))
                {
                    let idx_expr = exprs.get(idx).cloned().unwrap_or_else(|| idx.clone());
                    source.push_str(&format!("    let {out} = {buf}[{idx_expr}];\n"));
                    exprs.insert(out.clone(), out.clone());
                }
            }
            "if" => {
                if let Some(cond_name) = op.args.first() {
                    let cond_expr = exprs
                        .get(cond_name)
                        .cloned()
                        .unwrap_or_else(|| cond_name.clone());
                    source.push_str(&format!("    if ({cond_expr}) {{\n"));
                    if_depth += 1;
                }
            }
            "end_if" => {
                if if_depth > 0 {
                    if_depth -= 1;
                    source.push_str("    }\n");
                }
            }
            "store_index" => {
                if let (Some(buf), Some(idx), Some(src)) =
                    (op.args.first(), op.args.get(1), op.args.get(2))
                {
                    let idx_expr = exprs.get(idx).cloned().unwrap_or_else(|| idx.clone());
                    let src_expr = exprs.get(src).cloned().unwrap_or_else(|| src.clone());
                    source.push_str(&format!("        {buf}[{idx_expr}] = {src_expr};\n"));
                }
            }
            other => return Err(format!("unsupported webgpu kernel op: {other}")),
        }
    }
    while if_depth > 0 {
        if_depth -= 1;
        source.push_str("    }\n");
    }
    source.push_str("}\n");
    Ok((
        source,
        buffer_names,
        scalar_names,
        write_buffers.into_iter().collect(),
    ))
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
    _py: &crate::PyToken<'_>,
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
        crate::molt_gpu_webgpu_dispatch_host(
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

#[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
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
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
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

#[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
struct RuntimeMetalPipeline {
    pipeline: ComputePipelineState,
    #[allow(dead_code)]
    library: Library,
}

#[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
unsafe impl Send for RuntimeMetalPipeline {}
#[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
unsafe impl Sync for RuntimeMetalPipeline {}

#[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
struct RuntimeMetalDevice {
    device: Device,
    command_queue: CommandQueue,
}

#[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
impl RuntimeMetalDevice {
    fn new() -> Result<Self, String> {
        let device = Device::system_default().ok_or_else(|| "No Metal device found".to_string())?;
        Ok(Self {
            command_queue: device.new_command_queue(),
            device,
        })
    }

    fn compile_pipeline(
        &self,
        name: &str,
        source: &str,
    ) -> Result<Arc<RuntimeMetalPipeline>, String> {
        let options = CompileOptions::new();
        let library = self
            .device
            .new_library_with_source(source, &options)
            .map_err(|err| format!("MSL compile error: {err}"))?;
        let function = library
            .get_function(name, None)
            .map_err(|err| format!("MSL function lookup failed: {err}"))?;
        let pipeline = self
            .device
            .new_compute_pipeline_state_with_function(&function)
            .map_err(|err| format!("Metal pipeline creation failed: {err}"))?;
        Ok(Arc::new(RuntimeMetalPipeline { pipeline, library }))
    }

    fn alloc_buffer(&self, size_bytes: usize) -> MetalBuffer {
        self.device
            .new_buffer(size_bytes as u64, MTLResourceOptions::StorageModeShared)
    }

    fn copy_to_buffer(&self, buffer: &MetalBuffer, data: &[u8]) {
        let contents = buffer.contents() as *mut u8;
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), contents, data.len());
        }
    }

    fn copy_from_buffer(&self, buffer: &MetalBuffer, size_bytes: usize) -> Vec<u8> {
        let mut out = vec![0u8; size_bytes];
        let contents = buffer.contents() as *const u8;
        unsafe {
            std::ptr::copy_nonoverlapping(contents, out.as_mut_ptr(), size_bytes);
        }
        out
    }

    fn dispatch(
        &self,
        pipeline: &Arc<RuntimeMetalPipeline>,
        grid_threads: usize,
        buffers: &[&MetalBuffer],
    ) -> Result<(), String> {
        let command_buffer = self.command_queue.new_command_buffer();
        let encoder = command_buffer.new_compute_command_encoder();
        encoder.set_compute_pipeline_state(&pipeline.pipeline);
        for (index, buffer) in buffers.iter().enumerate() {
            encoder.set_buffer(index as NSUInteger, Some(*buffer), 0);
        }
        encoder.dispatch_threads(
            MTLSize {
                width: grid_threads as NSUInteger,
                height: 1,
                depth: 1,
            },
            MTLSize {
                width: 1,
                height: 1,
                depth: 1,
            },
        );
        encoder.end_encoding();
        command_buffer.commit();
        command_buffer.wait_until_completed();
        Ok(())
    }
}

#[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
fn try_dispatch_metal_kernel(
    _py: &crate::PyToken<'_>,
    callable_bits: u64,
    grid: i64,
    threads: i64,
    builder_bits: u64,
) -> Result<Option<u64>, u64> {
    if requested_gpu_backend() != Some(GpuBackend::Metal) {
        return Ok(None);
    }
    if trace_gpu_backend_enabled() {
        eprintln!("[molt gpu backend] metal");
    }
    let descriptor_bits = match unsafe { gpu_kernel_descriptor_bits(_py, callable_bits) } {
        Ok(Some(bits)) => bits,
        Ok(None) => {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "metal gpu backend requires __molt_gpu_descriptor__ metadata",
            ));
        }
        Err(err) => return Err(err),
    };
    let descriptor_json = string_obj_to_owned(obj_from_bits(descriptor_bits)).ok_or_else(|| {
        raise_exception::<u64>(_py, "TypeError", "gpu kernel descriptor must be a string")
    })?;
    let descriptor = parse_kernel_descriptor_json(&descriptor_json).map_err(|msg| {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            &format!("invalid gpu kernel descriptor: {msg}"),
        )
    })?;
    let arg_bits = unsafe { crate::call::bind::callargs_positional_snapshot(_py, builder_bits) }?;
    if arg_bits.len() != descriptor.params.len() {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "gpu kernel descriptor parameter count does not match launch args",
        ));
    }
    let mut args_map = BTreeMap::new();
    for (name, bits) in descriptor.params.iter().zip(arg_bits.iter().copied()) {
        args_map.insert(name.clone(), kernel_arg_from_bits(_py, name, bits)?);
    }
    let (source, buffer_names, scalar_names, output_buffers) =
        render_metal_source(&descriptor, &args_map).map_err(|msg| {
            raise_exception::<u64>(
                _py,
                "RuntimeError",
                &format!("metal kernel render failed: {msg}"),
            )
        })?;
    let device = RuntimeMetalDevice::new()
        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
    let pipeline = device
        .compile_pipeline(&descriptor.name, &source)
        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;

    let mut owned_buffers: Vec<MetalBuffer> = Vec::new();
    let mut buffer_index_map: BTreeMap<String, usize> = BTreeMap::new();
    for name in &buffer_names {
        let RuntimeKernelArg::Buffer(buf) = args_map.get(name).expect("buffer arg missing") else {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "expected buffer arg",
            ));
        };
        let host_bytes = buffer_host_bytes_for_gpu_compute(_py, buf)
            .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
        let metal_buf = device.alloc_buffer(host_bytes.len());
        if !host_bytes.is_empty() {
            device.copy_to_buffer(&metal_buf, &host_bytes);
        }
        buffer_index_map.insert(name.clone(), owned_buffers.len());
        owned_buffers.push(metal_buf);
    }
    for name in &scalar_names {
        let arg = args_map.get(name).expect("scalar arg missing");
        let (_, scalar_bytes) = metal_scalar_type_for_arg(arg)
            .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
        let metal_buf = device.alloc_buffer(scalar_bytes.len().max(1));
        if !scalar_bytes.is_empty() {
            device.copy_to_buffer(&metal_buf, &scalar_bytes);
        }
        owned_buffers.push(metal_buf);
    }
    let refs: Vec<&MetalBuffer> = owned_buffers.iter().collect();
    device
        .dispatch(&pipeline, (grid.saturating_mul(threads)) as usize, &refs)
        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;

    for name in &output_buffers {
        let RuntimeKernelArg::Buffer(buf) = args_map.get(name).expect("output buffer missing")
        else {
            continue;
        };
        let buffer_idx = *buffer_index_map.get(name).expect("buffer index missing");
        let output_size = match buf.original_format.as_str() {
            "f" => buf.size * 4,
            "d" => buf.size * 4,
            "q" => buf.size * 8,
            other => {
                return Err(raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    &format!("unsupported output buffer format for metal backend: {other}"),
                ));
            }
        };
        let gpu_output = device.copy_from_buffer(&owned_buffers[buffer_idx], output_size);
        copy_gpu32_output_back_to_buffer(_py, buf, &gpu_output)?;
    }
    Ok(Some(MoltObject::none().bits()))
}

#[cfg(not(all(target_os = "macos", feature = "molt_gpu_metal")))]
fn try_dispatch_metal_kernel(
    _py: &crate::PyToken<'_>,
    _callable_bits: u64,
    _grid: i64,
    _threads: i64,
    _builder_bits: u64,
) -> Result<Option<u64>, u64> {
    if requested_gpu_backend() == Some(GpuBackend::Metal) {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "metal gpu backend requested but runtime was built without molt_gpu_metal",
        ));
    }
    Ok(None)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu"))]
fn try_dispatch_webgpu_kernel(
    _py: &crate::PyToken<'_>,
    callable_bits: u64,
    grid: i64,
    threads: i64,
    builder_bits: u64,
) -> Result<Option<u64>, u64> {
    if requested_gpu_backend() != Some(GpuBackend::WebGpu) {
        return Ok(None);
    }
    if trace_gpu_backend_enabled() {
        eprintln!("[molt gpu backend] webgpu");
    }
    let descriptor_bits = match unsafe { gpu_kernel_descriptor_bits(_py, callable_bits) } {
        Ok(Some(bits)) => bits,
        Ok(None) => {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "webgpu backend requires __molt_gpu_descriptor__ metadata",
            ));
        }
        Err(err) => return Err(err),
    };
    let descriptor_json = string_obj_to_owned(obj_from_bits(descriptor_bits)).ok_or_else(|| {
        raise_exception::<u64>(_py, "TypeError", "gpu kernel descriptor must be a string")
    })?;
    let descriptor = parse_kernel_descriptor_json(&descriptor_json).map_err(|msg| {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            &format!("invalid gpu kernel descriptor: {msg}"),
        )
    })?;
    let arg_bits = unsafe { crate::call::bind::callargs_positional_snapshot(_py, builder_bits) }?;
    if arg_bits.len() != descriptor.params.len() {
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "gpu kernel descriptor parameter count does not match launch args",
        ));
    }
    let mut args_map = BTreeMap::new();
    for (name, bits) in descriptor.params.iter().zip(arg_bits.iter().copied()) {
        args_map.insert(name.clone(), kernel_arg_from_bits(_py, name, bits)?);
    }
    let (source, buffer_names, scalar_names, output_buffers) =
        render_webgpu_source(&descriptor, &args_map, threads as u32).map_err(|msg| {
            raise_exception::<u64>(
                _py,
                "RuntimeError",
                &format!("webgpu kernel render failed: {msg}"),
            )
        })?;
    let device = RuntimeWebGpuDevice::new()
        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
    let pipeline = device
        .compile_pipeline(&descriptor.name, &source)
        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;

    let mut owned_buffers: Vec<wgpu::Buffer> = Vec::new();
    let mut buffer_index_map: BTreeMap<String, usize> = BTreeMap::new();
    for name in &buffer_names {
        let RuntimeKernelArg::Buffer(buf) = args_map.get(name).expect("buffer arg missing") else {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "expected buffer arg",
            ));
        };
        let host_bytes = buffer_host_bytes_for_gpu_compute(_py, buf)
            .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
        let (_, gpu_buf) = device.alloc_buffer(host_bytes.len().max(1));
        if !host_bytes.is_empty() {
            device.copy_to_buffer(&gpu_buf, &host_bytes);
        }
        buffer_index_map.insert(name.clone(), owned_buffers.len());
        owned_buffers.push(gpu_buf);
    }
    for name in &scalar_names {
        let arg = args_map.get(name).expect("scalar arg missing");
        let scalar_bytes = webgpu_scalar_bytes_for_arg(arg)
            .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
        let (_, gpu_buf) = device.alloc_buffer(scalar_bytes.len().max(1));
        device.copy_to_buffer(&gpu_buf, &scalar_bytes);
        owned_buffers.push(gpu_buf);
    }
    let refs: Vec<&wgpu::Buffer> = owned_buffers.iter().collect();
    device
        .dispatch(&pipeline, grid as u32, &refs)
        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;

    for name in &output_buffers {
        let RuntimeKernelArg::Buffer(buf) = args_map.get(name).expect("output buffer missing")
        else {
            continue;
        };
        let buffer_idx = *buffer_index_map.get(name).expect("buffer index missing");
        let output_size = match buf.original_format.as_str() {
            "f" => buf.size * 4,
            "d" => buf.size * 4,
            "q" => buf.size * 4,
            other => {
                return Err(raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    &format!("unsupported output buffer format for webgpu backend: {other}"),
                ));
            }
        };
        let gpu_output = device
            .copy_from_buffer(&owned_buffers[buffer_idx], output_size)
            .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
        copy_gpu32_output_back_to_buffer(_py, buf, &gpu_output)?;
    }
    Ok(Some(MoltObject::none().bits()))
}

#[cfg(target_arch = "wasm32")]
fn try_dispatch_webgpu_kernel(
    _py: &crate::PyToken<'_>,
    callable_bits: u64,
    grid: i64,
    threads: i64,
    builder_bits: u64,
) -> Result<Option<u64>, u64> {
    if requested_gpu_backend() != Some(GpuBackend::WebGpu) {
        return Ok(None);
    }
    if trace_gpu_backend_enabled() {
        eprintln!("[molt gpu backend] webgpu");
    }
    let descriptor_bits = match unsafe { gpu_kernel_descriptor_bits(_py, callable_bits) } {
        Ok(Some(bits)) => bits,
        Ok(None) => {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "webgpu backend requires __molt_gpu_descriptor__ metadata",
            ));
        }
        Err(err) => return Err(err),
    };
    let descriptor_json = string_obj_to_owned(obj_from_bits(descriptor_bits)).ok_or_else(|| {
        raise_exception::<u64>(_py, "TypeError", "gpu kernel descriptor must be a string")
    })?;
    let descriptor = parse_kernel_descriptor_json(&descriptor_json).map_err(|msg| {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            &format!("invalid gpu kernel descriptor: {msg}"),
        )
    })?;
    let arg_bits = unsafe { crate::call::bind::callargs_positional_snapshot(_py, builder_bits) }?;
    if arg_bits.len() != descriptor.params.len() {
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "gpu kernel descriptor parameter count does not match launch args",
        ));
    }
    let mut args_map = BTreeMap::new();
    for (name, bits) in descriptor.params.iter().zip(arg_bits.iter().copied()) {
        args_map.insert(name.clone(), kernel_arg_from_bits(_py, name, bits)?);
    }
    let (source, buffer_names, scalar_names, output_buffers) =
        render_webgpu_source(&descriptor, &args_map, threads as u32).map_err(|msg| {
            raise_exception::<u64>(
                _py,
                "RuntimeError",
                &format!("webgpu kernel render failed: {msg}"),
            )
        })?;
    let output_buffer_names: BTreeSet<String> = output_buffers.iter().cloned().collect();
    let mut staging_buffers: Vec<Vec<u8>> = Vec::new();
    let mut launch_bindings = Vec::new();
    let mut output_records: Vec<(RuntimeKernelBufferArg, usize)> = Vec::new();
    let mut binding_index = 0usize;

    for name in &buffer_names {
        let RuntimeKernelArg::Buffer(buf) = args_map.get(name).expect("buffer arg missing") else {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "expected buffer arg",
            ));
        };
        let bytes = buffer_host_bytes_for_webgpu_compute(_py, buf)
            .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
        let staging_index = staging_buffers.len();
        staging_buffers.push(bytes);
        let staging = &staging_buffers[staging_index];
        let ptr = if staging.is_empty() {
            0
        } else {
            staging.as_ptr() as usize as u32
        };
        let access = if output_buffer_names.contains(name) {
            output_records.push((buf.clone(), staging_index));
            "read_write"
        } else {
            "read"
        };
        launch_bindings.push(serde_json::json!({
            "binding": binding_index,
            "name": name,
            "kind": "buffer",
            "access": access,
            "ptr": ptr,
            "len": staging.len() as u32,
        }));
        binding_index += 1;
    }

    for name in &scalar_names {
        let arg = args_map.get(name).expect("scalar arg missing");
        let staging_index = staging_buffers.len();
        staging_buffers.push(
            webgpu_scalar_bytes_for_arg(arg)
                .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?,
        );
        let staging = &staging_buffers[staging_index];
        let ptr = if staging.is_empty() {
            0
        } else {
            staging.as_ptr() as usize as u32
        };
        launch_bindings.push(serde_json::json!({
            "binding": binding_index,
            "name": name,
            "kind": "scalar",
            "access": "read",
            "ptr": ptr,
            "len": staging.len() as u32,
        }));
        binding_index += 1;
    }

    dispatch_browser_webgpu_bindings(
        _py,
        source.as_str(),
        descriptor.name.as_str(),
        launch_bindings,
        grid as u32,
        threads as u32,
    )?;
    for (buf, staging_index) in output_records {
        copy_gpu32_output_back_to_buffer(_py, &buf, &staging_buffers[staging_index])?;
    }
    Ok(Some(MoltObject::none().bits()))
}

#[cfg(not(any(
    target_arch = "wasm32",
    all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu")
)))]
fn try_dispatch_webgpu_kernel(
    _py: &crate::PyToken<'_>,
    _callable_bits: u64,
    _grid: i64,
    _threads: i64,
    _builder_bits: u64,
) -> Result<Option<u64>, u64> {
    if requested_gpu_backend() == Some(GpuBackend::WebGpu) {
        return Err(raise_exception::<u64>(
            _py,
            "RuntimeError",
            "webgpu backend requested but runtime was built without molt_gpu_webgpu",
        ));
    }
    Ok(None)
}

fn bytes_like_view(_py: &crate::PyToken<'_>, bits: u64, role: &str) -> Result<ByteView, u64> {
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

unsafe fn require_class_ptr(
    _py: &crate::PyToken<'_>,
    bits: u64,
    role: &str,
) -> Result<*mut u8, u64> {
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

fn normalize_shape_bits(_py: &crate::PyToken<'_>, bits: u64) -> Result<(u64, bool), u64> {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        return match unsafe { object_type_id(ptr) } {
            TYPE_ID_TUPLE => Ok((bits, false)),
            TYPE_ID_LIST => {
                let tuple_ptr = alloc_tuple(_py, unsafe { seq_vec_ref(ptr) });
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
    _py: &crate::PyToken<'_>,
    obj_ptr: *mut u8,
    name: &[u8],
    name_str: &str,
    val_bits: u64,
) -> Result<(), u64> {
    let Some(name_bits) = crate::attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let out = unsafe {
        crate::builtins::attributes::object_setattr_raw(_py, obj_ptr, name_bits, name_str, val_bits)
    } as u64;
    crate::dec_ref_bits(_py, name_bits);
    if crate::exception_pending(_py) {
        return Err(out);
    }
    Ok(())
}

unsafe fn object_attr_bits(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
    name_str: &str,
) -> Result<u64, u64> {
    let Some(name_bits) = crate::attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let out = crate::builtins::attributes::molt_get_attr_name(obj_bits, name_bits);
    crate::dec_ref_bits(_py, name_bits);
    if crate::exception_pending(_py) {
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
    _py: &crate::PyToken<'_>,
    buffer_class_bits: u64,
    data_bits: u64,
    element_type_bits: u64,
    size: usize,
    format_bits: u64,
    itemsize: usize,
) -> Result<u64, u64> {
    let buffer_class_ptr = unsafe { require_class_ptr(_py, buffer_class_bits, "buffer_class")? };
    let buffer_bits = unsafe { crate::alloc_instance_for_class(_py, buffer_class_ptr) };
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
        crate::dec_ref_bits(_py, buffer_bits);
        return Err(MoltObject::none().bits());
    }
    Ok(buffer_bits)
}

unsafe fn build_tensor_instance(
    _py: &crate::PyToken<'_>,
    tensor_class_bits: u64,
    buf_bits: u64,
    shape_bits: u64,
    dtype_bits: u64,
) -> Result<u64, u64> {
    let tensor_class_ptr = unsafe { require_class_ptr(_py, tensor_class_bits, "tensor_class")? };
    let tensor_bits = unsafe { crate::alloc_instance_for_class(_py, tensor_class_ptr) };
    let Some(tensor_ptr) = obj_from_bits(tensor_bits).as_ptr() else {
        return Err(tensor_bits);
    };
    if unsafe { set_object_attr_bytes(_py, tensor_ptr, b"_buf", "_buf", buf_bits) }.is_err()
        || unsafe { set_object_attr_bytes(_py, tensor_ptr, b"_shape", "_shape", shape_bits) }
            .is_err()
        || unsafe { set_object_attr_bytes(_py, tensor_ptr, b"_dtype", "_dtype", dtype_bits) }
            .is_err()
    {
        crate::dec_ref_bits(_py, tensor_bits);
        return Err(MoltObject::none().bits());
    }
    Ok(tensor_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_thread_id() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let tid = current_gpu_launch_context().thread_id;
        if trace_gpu_thread_id_enabled() {
            eprintln!("[molt gpu thread_id] tid={tid}");
        }
        MoltObject::from_int(tid).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_block_id() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_int(current_gpu_launch_context().block_id).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_block_dim() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_int(current_gpu_launch_context().block_dim).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_grid_dim() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        MoltObject::from_int(current_gpu_launch_context().grid_dim).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_barrier() -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_kernel_launch(
    launcher_bits: u64,
    grid_bits: u64,
    threads_bits: u64,
    builder_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let trace_launch = trace_gpu_kernel_launch_enabled();
        let grid = match parse_i64_launch_arg(_py, grid_bits, "grid") {
            Ok(value) => value,
            Err(err) => return err,
        };
        let threads = match parse_i64_launch_arg(_py, threads_bits, "threads") {
            Ok(value) => value,
            Err(err) => return err,
        };
        let callable_bits = match unsafe { gpu_kernel_callable_bits(_py, launcher_bits) } {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        match try_dispatch_metal_kernel(_py, callable_bits, grid, threads, builder_bits) {
            Ok(Some(bits)) => return bits,
            Ok(None) => {}
            Err(err) => return err,
        }
        match try_dispatch_webgpu_kernel(_py, callable_bits, grid, threads, builder_bits) {
            Ok(Some(bits)) => return bits,
            Ok(None) => {}
            Err(err) => return err,
        }
        let total_threads = grid.saturating_mul(threads);
        if total_threads <= 0 {
            return MoltObject::none().bits();
        }
        let block_dim = if threads <= 0 { 1 } else { threads };
        for tid in 0..total_threads {
            let block_id = if block_dim <= 0 { 0 } else { tid / block_dim };
            if trace_launch {
                eprintln!(
                    "[molt gpu launch] tid={} block_id={} block_dim={} grid_dim={}",
                    tid, block_id, block_dim, grid
                );
            }
            let call_builder_bits = match unsafe {
                crate::call::bind::clone_callargs_builder_bits(_py, builder_bits)
            } {
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
                || molt_call_bind(callable_bits, call_builder_bits),
            );
            if crate::exception_pending(_py) {
                let exc_bits = molt_exception_last();
                let kind_bits = molt_exception_kind(exc_bits);
                let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                    .unwrap_or_else(|| "<exc>".to_string());
                dec_ref_bits(_py, kind_bits);
                dec_ref_bits(_py, call_builder_bits);
                if trace_launch {
                    eprintln!("[molt gpu launch] tid={} exception={}", tid, kind);
                }
                if kind == "IndexError" {
                    let _ = molt_exception_clear();
                    continue;
                }
                return out_bits;
            }
            if trace_launch {
                eprintln!("[molt gpu launch] tid={} ok", tid);
            }
            dec_ref_bits(_py, call_builder_bits);
            dec_ref_bits(_py, out_bits);
        }
        MoltObject::none().bits()
    })
}
