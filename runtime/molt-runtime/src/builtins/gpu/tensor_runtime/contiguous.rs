use super::*;

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_interop__load_safetensors(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let load_bits = match unsafe {
            module_global_bits(
                _py,
                b"molt.gpu.interop",
                b"load_safetensors",
                "load_safetensors",
            )
        } {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let out_bits = unsafe { crate::call::dispatch::call_callable1(_py, load_bits, path_bits) };
        crate::dec_ref_bits(_py, load_bits);
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_tensor_from_parts(
    tensor_class_bits: u64,
    buffer_class_bits: u64,
    data_bits: u64,
    element_type_bits: u64,
    size_bits: u64,
    format_bits: u64,
    shape_bits: u64,
    dtype_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let size = match parse_usize_arg(_py, size_bits, "size") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let format = match parse_format(_py, format_bits, "format_char") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let data_view = match bytes_like_view(_py, data_bits, "data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let Some(required_len) = size.checked_mul(format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "buffer size overflow");
        };
        if data_view.len < required_len {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "Buffer payload too small for requested size and format",
            );
        }
        let (shape_bits, owns_shape_bits) = match normalize_shape_bits(_py, shape_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let buffer_bits = match unsafe {
            build_buffer_instance(
                _py,
                buffer_class_bits,
                data_bits,
                element_type_bits,
                size,
                format_bits,
                format.itemsize(),
            )
        } {
            Ok(bits) => bits,
            Err(bits) => {
                if owns_shape_bits {
                    crate::dec_ref_bits(_py, shape_bits);
                }
                return bits;
            }
        };
        let tensor_bits = match unsafe {
            build_tensor_instance(_py, tensor_class_bits, buffer_bits, shape_bits, dtype_bits)
        } {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, buffer_bits);
                if owns_shape_bits {
                    crate::dec_ref_bits(_py, shape_bits);
                }
                return bits;
            }
        };
        crate::dec_ref_bits(_py, buffer_bits);
        if owns_shape_bits {
            crate::dec_ref_bits(_py, shape_bits);
        }
        tensor_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_repeat_axis_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    shape_bits: u64,
    axis_bits: u64,
    repeats_bits: u64,
    out_format_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let x_format = match parse_format(_py, x_format_bits, "x_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out_format = match parse_format(_py, out_format_bits, "out_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if x_format != out_format {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "repeat_axis requires matching input/output formats",
            );
        }
        let shape = match parse_shape(_py, shape_bits, "shape") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if shape.is_empty() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "repeat_axis requires a tensor with at least 1 dimension",
            );
        }
        let axis = match parse_usize_arg(_py, axis_bits, "axis") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if axis >= shape.len() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!("Invalid axis {} for tensor with {} dims", axis, shape.len()),
            );
        }
        let repeats = match parse_usize_arg(_py, repeats_bits, "repeats") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let x_view = match bytes_like_view(_py, x_data_bits, "x_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let total_elems = product(&shape);
        let itemsize = x_format.itemsize();
        let Some(required) = total_elems.checked_mul(itemsize) else {
            return raise_exception::<_>(_py, "OverflowError", "repeat_axis shape overflow");
        };
        if x_view.len < required {
            return raise_exception::<_>(_py, "ValueError", "x_data buffer is too small");
        }

        let outer = if axis > 0 { product(&shape[..axis]) } else { 1 };
        let axis_len = shape[axis];
        let inner = if axis + 1 < shape.len() {
            product(&shape[axis + 1..])
        } else {
            1
        };
        let Some(chunk_bytes) = inner.checked_mul(itemsize) else {
            return raise_exception::<_>(_py, "OverflowError", "repeat_axis byte size overflow");
        };
        let Some(src_axis_bytes) = axis_len.checked_mul(chunk_bytes) else {
            return raise_exception::<_>(_py, "OverflowError", "repeat_axis byte size overflow");
        };
        let Some(out_axis_len) = axis_len.checked_mul(repeats) else {
            return raise_exception::<_>(_py, "OverflowError", "repeat_axis output shape overflow");
        };
        let Some(out_axis_bytes) = out_axis_len.checked_mul(chunk_bytes) else {
            return raise_exception::<_>(_py, "OverflowError", "repeat_axis byte size overflow");
        };
        let Some(out_len) = outer.checked_mul(out_axis_bytes) else {
            return raise_exception::<_>(
                _py,
                "OverflowError",
                "repeat_axis output byte size overflow",
            );
        };
        let mut out = vec![0u8; out_len];
        let src = unsafe { std::slice::from_raw_parts(x_view.ptr, required) };
        for outer_idx in 0..outer {
            let src_outer = outer_idx * src_axis_bytes;
            let dst_outer = outer_idx * out_axis_bytes;
            for axis_idx in 0..axis_len {
                let src_base = src_outer + axis_idx * chunk_bytes;
                let chunk = &src[src_base..src_base + chunk_bytes];
                let dst_base = dst_outer + axis_idx * repeats * chunk_bytes;
                for repeat_idx in 0..repeats {
                    let dst = dst_base + repeat_idx * chunk_bytes;
                    out[dst..dst + chunk_bytes].copy_from_slice(chunk);
                }
            }
        }

        let out_ptr = alloc_bytearray(_py, &out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_tensor_from_buffer(
    tensor_class_bits: u64,
    buffer_bits: u64,
    shape_bits: u64,
    dtype_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (shape_bits, owns_shape_bits) = match normalize_shape_bits(_py, shape_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let tensor_bits = match unsafe {
            build_tensor_instance(_py, tensor_class_bits, buffer_bits, shape_bits, dtype_bits)
        } {
            Ok(bits) => bits,
            Err(bits) => {
                if owns_shape_bits {
                    crate::dec_ref_bits(_py, shape_bits);
                }
                return bits;
            }
        };
        if owns_shape_bits {
            crate::dec_ref_bits(_py, shape_bits);
        }
        tensor_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_buffer_to_list(buffer_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let count = match parse_usize_arg(_py, count_bits, "count") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if std::env::var("MOLT_TRACE_GPU_BUFFER_TO_LIST").as_deref() == Ok("1") {
            eprintln!("molt gpu buffer_to_list count={}", count);
        }
        let data_bits = match unsafe { object_attr_bits(_py, buffer_bits, b"_data", "_data") } {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let format_bits =
            match unsafe { object_attr_bits(_py, buffer_bits, b"_format_char", "_format_char") } {
                Ok(bits) => bits,
                Err(bits) => return bits,
            };
        let format = match parse_format(_py, format_bits, "format_char") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let data_view = match bytes_like_view(_py, data_bits, "_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let Some(required_len) = count.checked_mul(format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "buffer list size overflow");
        };
        if data_view.len < required_len {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "Buffer payload too small for requested count and format",
            );
        }
        let mut values = Vec::with_capacity(count);
        for index in 0..count {
            let bits = match format {
                ScalarFormat::F32 | ScalarFormat::F64 => {
                    MoltObject::from_float(unsafe { read_scalar(data_view.ptr, index, format) })
                        .bits()
                }
                ScalarFormat::I64 => MoltObject::from_int(unsafe {
                    read_scalar(data_view.ptr, index, format) as i64
                })
                .bits(),
            };
            values.push(bits);
        }
        let list_ptr =
            crate::object::builders::alloc_list_with_capacity_owned(_py, &values, values.len());
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_linear_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    weight_data_bits: u64,
    weight_format_bits: u64,
    outer_bits: u64,
    in_features_bits: u64,
    out_features_bits: u64,
    out_format_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let trace_linear = std::env::var("MOLT_TRACE_GPU_LINEAR").as_deref() == Ok("1");
        let x_format = match parse_format(_py, x_format_bits, "x_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let weight_format = match parse_format(_py, weight_format_bits, "weight_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out_format = match parse_format(_py, out_format_bits, "out_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };

        let outer = match parse_usize_arg(_py, outer_bits, "outer") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let in_features = match parse_usize_arg(_py, in_features_bits, "in_features") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out_features = match parse_usize_arg(_py, out_features_bits, "out_features") {
            Ok(value) => value,
            Err(bits) => return bits,
        };

        let x_view = match bytes_like_view(_py, x_data_bits, "x_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let weight_view = match bytes_like_view(_py, weight_data_bits, "weight_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };

        let Some(x_required) = outer
            .checked_mul(in_features)
            .and_then(|n| n.checked_mul(x_format.itemsize()))
        else {
            return raise_exception::<_>(_py, "OverflowError", "x_data shape overflow");
        };
        let Some(weight_required) = out_features
            .checked_mul(in_features)
            .and_then(|n| n.checked_mul(weight_format.itemsize()))
        else {
            return raise_exception::<_>(_py, "OverflowError", "weight_data shape overflow");
        };
        let Some(out_len) = outer
            .checked_mul(out_features)
            .and_then(|n| n.checked_mul(out_format.itemsize()))
        else {
            return raise_exception::<_>(_py, "OverflowError", "output shape overflow");
        };
        if trace_linear {
            eprintln!(
                "molt gpu linear outer={} in_features={} out_features={} x_itemsize={} weight_itemsize={} out_itemsize={} x_bytes={} weight_bytes={} out_bytes={}",
                outer,
                in_features,
                out_features,
                x_format.itemsize(),
                weight_format.itemsize(),
                out_format.itemsize(),
                x_view.len,
                weight_view.len,
                out_len
            );
        }

        if x_view.len < x_required {
            return raise_exception::<_>(_py, "ValueError", "x_data buffer is too small");
        }
        if weight_view.len < weight_required {
            return raise_exception::<_>(_py, "ValueError", "weight_data buffer is too small");
        }

        #[cfg(target_arch = "wasm32")]
        if requested_gpu_backend() == Some(GpuBackend::WebGpu) {
            let browser_result: Result<u64, u64> = (|| {
                let element_ty = webgpu_linear_element_type(x_format, weight_format, out_format)
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                let x_bytes = bytes_like_view_to_webgpu_bytes(x_view, x_format)
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                let weight_bytes = bytes_like_view_to_webgpu_bytes(weight_view, weight_format)
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                let mut out_webgpu = vec![0u8; outer * out_features * 4];
                let outer_i32 = i32::try_from(outer).map_err(|_| {
                    raise_exception::<u64>(_py, "OverflowError", "outer exceeds i32")
                })?;
                let in_features_i32 = i32::try_from(in_features).map_err(|_| {
                    raise_exception::<u64>(_py, "OverflowError", "in_features exceeds i32")
                })?;
                let out_features_i32 = i32::try_from(out_features).map_err(|_| {
                    raise_exception::<u64>(_py, "OverflowError", "out_features exceeds i32")
                })?;
                let outer_bytes = outer_i32.to_le_bytes();
                let in_features_bytes = in_features_i32.to_le_bytes();
                let out_features_bytes = out_features_i32.to_le_bytes();
                let workgroup_size = 64u32;
                let total_threads = outer.checked_mul(out_features).ok_or_else(|| {
                    raise_exception::<u64>(_py, "OverflowError", "gpu linear thread count overflow")
                })?;
                let grid = if total_threads == 0 {
                    0
                } else {
                    u32::try_from(
                        (total_threads + workgroup_size as usize - 1) / workgroup_size as usize,
                    )
                    .map_err(|_| {
                        raise_exception::<u64>(_py, "OverflowError", "gpu linear grid exceeds u32")
                    })?
                };
                let source =
                    render_webgpu_linear_source("linear_contiguous", element_ty, workgroup_size);
                dispatch_browser_webgpu_bindings(
                    _py,
                    source.as_str(),
                    "linear_contiguous",
                    vec![
                        serde_json::json!({"binding": 0, "name": "x", "kind": "buffer", "access": "read", "ptr": x_bytes.as_ptr() as usize as u32, "len": x_bytes.len() as u32}),
                        serde_json::json!({"binding": 1, "name": "weight", "kind": "buffer", "access": "read", "ptr": weight_bytes.as_ptr() as usize as u32, "len": weight_bytes.len() as u32}),
                        serde_json::json!({"binding": 2, "name": "out", "kind": "buffer", "access": "read_write", "ptr": out_webgpu.as_mut_ptr() as usize as u32, "len": out_webgpu.len() as u32}),
                        serde_json::json!({"binding": 3, "name": "outer", "kind": "scalar", "access": "read", "ptr": outer_bytes.as_ptr() as usize as u32, "len": outer_bytes.len() as u32}),
                        serde_json::json!({"binding": 4, "name": "in_features", "kind": "scalar", "access": "read", "ptr": in_features_bytes.as_ptr() as usize as u32, "len": in_features_bytes.len() as u32}),
                        serde_json::json!({"binding": 5, "name": "out_features", "kind": "scalar", "access": "read", "ptr": out_features_bytes.as_ptr() as usize as u32, "len": out_features_bytes.len() as u32}),
                    ],
                    grid,
                    workgroup_size,
                )?;
                let rebuilt = rebuild_host_bytes_from_gpu32_output(
                    _py,
                    out_format,
                    outer * out_features,
                    out_webgpu.as_slice(),
                )?;
                let out_ptr = alloc_bytearray(_py, rebuilt.as_slice());
                if out_ptr.is_null() {
                    return Err(MoltObject::none().bits());
                }
                Ok(MoltObject::from_ptr(out_ptr).bits())
            })();
            return match browser_result {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }

        let mut out = vec![0u8; out_len];
        if x_format == ScalarFormat::F32
            && weight_format == ScalarFormat::F32
            && out_format == ScalarFormat::F32
        {
            unsafe {
                linear_rows_f32(
                    x_view.ptr,
                    weight_view.ptr,
                    out.as_mut_ptr(),
                    outer,
                    in_features,
                    0,
                    out_features,
                );
            }
        } else {
            for batch in 0..outer {
                let x_off = batch * in_features;
                let out_off = batch * out_features;
                for out_idx in 0..out_features {
                    let w_off = out_idx * in_features;
                    let mut acc = 0.0f64;
                    for k in 0..in_features {
                        let x = unsafe { read_scalar(x_view.ptr, x_off + k, x_format) };
                        let w = unsafe { read_scalar(weight_view.ptr, w_off + k, weight_format) };
                        acc += x * w;
                    }
                    unsafe { write_scalar(out.as_mut_ptr(), out_off + out_idx, out_format, acc) };
                }
            }
        }

        let out_ptr = alloc_bytearray(_py, &out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        if trace_linear {
            eprintln!("molt gpu linear done out_bytes={}", out.len());
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_linear_split_last_dim_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    weight_data_bits: u64,
    weight_format_bits: u64,
    outer_bits: u64,
    in_features_bits: u64,
    split_sizes_bits: u64,
    out_format_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let x_format = match parse_format(_py, x_format_bits, "x_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let weight_format = match parse_format(_py, weight_format_bits, "weight_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out_format = match parse_format(_py, out_format_bits, "out_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let outer = match parse_usize_arg(_py, outer_bits, "outer") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let in_features = match parse_usize_arg(_py, in_features_bits, "in_features") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let split_sizes = match parse_shape(_py, split_sizes_bits, "split_sizes") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(out_features) = split_sizes
            .iter()
            .try_fold(0usize, |acc, size| acc.checked_add(*size))
        else {
            return raise_exception::<_>(_py, "OverflowError", "split_sizes overflow");
        };

        let x_view = match bytes_like_view(_py, x_data_bits, "x_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let weight_view = match bytes_like_view(_py, weight_data_bits, "weight_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };

        let Some(x_required) = outer
            .checked_mul(in_features)
            .and_then(|n| n.checked_mul(x_format.itemsize()))
        else {
            return raise_exception::<_>(_py, "OverflowError", "x_data shape overflow");
        };
        let Some(weight_required) = out_features
            .checked_mul(in_features)
            .and_then(|n| n.checked_mul(weight_format.itemsize()))
        else {
            return raise_exception::<_>(_py, "OverflowError", "weight_data shape overflow");
        };
        if x_view.len < x_required {
            return raise_exception::<_>(_py, "ValueError", "x_data buffer is too small");
        }
        if weight_view.len < weight_required {
            return raise_exception::<_>(_py, "ValueError", "weight_data buffer is too small");
        }

        #[cfg(target_arch = "wasm32")]
        if requested_gpu_backend() == Some(GpuBackend::WebGpu) {
            let browser_result: Result<u64, u64> = (|| {
                let element_ty = webgpu_linear_element_type(x_format, weight_format, out_format)
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                let x_bytes = bytes_like_view_to_webgpu_bytes(x_view, x_format)
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                let weight_bytes = bytes_like_view_to_webgpu_bytes(weight_view, weight_format)
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                let mut out_webgpu = vec![0u8; outer * out_features * 4];
                let outer_i32 = i32::try_from(outer).map_err(|_| {
                    raise_exception::<u64>(_py, "OverflowError", "outer exceeds i32")
                })?;
                let in_features_i32 = i32::try_from(in_features).map_err(|_| {
                    raise_exception::<u64>(_py, "OverflowError", "in_features exceeds i32")
                })?;
                let out_features_i32 = i32::try_from(out_features).map_err(|_| {
                    raise_exception::<u64>(_py, "OverflowError", "out_features exceeds i32")
                })?;
                let outer_bytes = outer_i32.to_le_bytes();
                let in_features_bytes = in_features_i32.to_le_bytes();
                let out_features_bytes = out_features_i32.to_le_bytes();
                let workgroup_size = 64u32;
                let total_threads = outer.checked_mul(out_features).ok_or_else(|| {
                    raise_exception::<u64>(_py, "OverflowError", "gpu linear thread count overflow")
                })?;
                let grid = if total_threads == 0 {
                    0
                } else {
                    u32::try_from(
                        (total_threads + workgroup_size as usize - 1) / workgroup_size as usize,
                    )
                    .map_err(|_| {
                        raise_exception::<u64>(_py, "OverflowError", "gpu linear grid exceeds u32")
                    })?
                };
                let source = render_webgpu_linear_source(
                    "linear_split_last_dim",
                    element_ty,
                    workgroup_size,
                );
                dispatch_browser_webgpu_bindings(
                    _py,
                    source.as_str(),
                    "linear_split_last_dim",
                    vec![
                        serde_json::json!({"binding": 0, "name": "x", "kind": "buffer", "access": "read", "ptr": x_bytes.as_ptr() as usize as u32, "len": x_bytes.len() as u32}),
                        serde_json::json!({"binding": 1, "name": "weight", "kind": "buffer", "access": "read", "ptr": weight_bytes.as_ptr() as usize as u32, "len": weight_bytes.len() as u32}),
                        serde_json::json!({"binding": 2, "name": "out", "kind": "buffer", "access": "read_write", "ptr": out_webgpu.as_mut_ptr() as usize as u32, "len": out_webgpu.len() as u32}),
                        serde_json::json!({"binding": 3, "name": "outer", "kind": "scalar", "access": "read", "ptr": outer_bytes.as_ptr() as usize as u32, "len": outer_bytes.len() as u32}),
                        serde_json::json!({"binding": 4, "name": "in_features", "kind": "scalar", "access": "read", "ptr": in_features_bytes.as_ptr() as usize as u32, "len": in_features_bytes.len() as u32}),
                        serde_json::json!({"binding": 5, "name": "out_features", "kind": "scalar", "access": "read", "ptr": out_features_bytes.as_ptr() as usize as u32, "len": out_features_bytes.len() as u32}),
                    ],
                    grid,
                    workgroup_size,
                )?;
                let mut out_bits = Vec::with_capacity(split_sizes.len());
                let mut prefix = 0usize;
                for &size in &split_sizes {
                    let mut part_gpu = vec![0u8; outer * size * 4];
                    for batch in 0..outer {
                        let src_start = (batch * out_features + prefix) * 4;
                        let src_end = src_start + size * 4;
                        let dst_start = batch * size * 4;
                        let dst_end = dst_start + size * 4;
                        part_gpu[dst_start..dst_end]
                            .copy_from_slice(&out_webgpu[src_start..src_end]);
                    }
                    let rebuilt = rebuild_host_bytes_from_gpu32_output(
                        _py,
                        out_format,
                        outer * size,
                        part_gpu.as_slice(),
                    )?;
                    let out_ptr = alloc_bytearray(_py, rebuilt.as_slice());
                    if out_ptr.is_null() {
                        return Err(MoltObject::none().bits());
                    }
                    out_bits.push(MoltObject::from_ptr(out_ptr).bits());
                    prefix += size;
                }
                let tuple_ptr = alloc_tuple(_py, out_bits.as_slice());
                if tuple_ptr.is_null() {
                    return Err(MoltObject::none().bits());
                }
                Ok(MoltObject::from_ptr(tuple_ptr).bits())
            })();
            return match browser_result {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }

        let mut outputs: Vec<Vec<u8>> = Vec::with_capacity(split_sizes.len());
        for &size in &split_sizes {
            let Some(out_len) = outer
                .checked_mul(size)
                .and_then(|n| n.checked_mul(out_format.itemsize()))
            else {
                return raise_exception::<_>(_py, "OverflowError", "output shape overflow");
            };
            outputs.push(vec![0u8; out_len]);
        }

        if x_format == ScalarFormat::F32
            && weight_format == ScalarFormat::F32
            && out_format == ScalarFormat::F32
        {
            let out_ptrs: Vec<*mut u8> = outputs.iter_mut().map(|out| out.as_mut_ptr()).collect();
            unsafe {
                linear_split_last_dim_f32(
                    x_view.ptr,
                    weight_view.ptr,
                    out_ptrs.as_slice(),
                    outer,
                    in_features,
                    split_sizes.as_slice(),
                );
            }
        } else {
            let mut prefix = 0usize;
            for (part_idx, &part_size) in split_sizes.iter().enumerate() {
                for batch in 0..outer {
                    let x_off = batch * in_features;
                    let out_off = batch * part_size;
                    for out_idx in 0..part_size {
                        let w_off = (prefix + out_idx) * in_features;
                        let mut acc = 0.0f64;
                        for k in 0..in_features {
                            let x = unsafe { read_scalar(x_view.ptr, x_off + k, x_format) };
                            let w =
                                unsafe { read_scalar(weight_view.ptr, w_off + k, weight_format) };
                            acc += x * w;
                        }
                        unsafe {
                            write_scalar(
                                outputs[part_idx].as_mut_ptr(),
                                out_off + out_idx,
                                out_format,
                                acc,
                            )
                        };
                    }
                }
                prefix += part_size;
            }
        }

        let mut out_bits = Vec::with_capacity(outputs.len());
        for out in outputs {
            let out_ptr = alloc_bytearray(_py, &out);
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            out_bits.push(MoltObject::from_ptr(out_ptr).bits());
        }
        let tuple_ptr = alloc_tuple(_py, out_bits.as_slice());
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_linear_squared_relu_gate_interleaved_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    weight_data_bits: u64,
    weight_format_bits: u64,
    outer_bits: u64,
    in_features_bits: u64,
    out_format_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let x_format = match parse_format(_py, x_format_bits, "x_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let weight_format = match parse_format(_py, weight_format_bits, "weight_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out_format = match parse_format(_py, out_format_bits, "out_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let outer = match parse_usize_arg(_py, outer_bits, "outer") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let in_features = match parse_usize_arg(_py, in_features_bits, "in_features") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let x_view = match bytes_like_view(_py, x_data_bits, "x_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let weight_view = match bytes_like_view(_py, weight_data_bits, "weight_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };

        let Some(x_required) = outer
            .checked_mul(in_features)
            .and_then(|n| n.checked_mul(x_format.itemsize()))
        else {
            return raise_exception::<_>(_py, "OverflowError", "x_data shape overflow");
        };
        if x_view.len < x_required {
            return raise_exception::<_>(_py, "ValueError", "x_data buffer is too small");
        }
        if in_features == 0 {
            return raise_exception::<_>(_py, "ValueError", "in_features must be positive");
        }
        let row_bytes = in_features * weight_format.itemsize();
        if row_bytes == 0 || weight_view.len % row_bytes != 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "weight_data byte length must be an even multiple of row width",
            );
        }
        let out_features = weight_view.len / row_bytes;
        if out_features % 2 != 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "interleaved gate weight output dimension must be even",
            );
        }
        let hidden = out_features / 2;
        let Some(out_len) = outer
            .checked_mul(hidden)
            .and_then(|n| n.checked_mul(out_format.itemsize()))
        else {
            return raise_exception::<_>(_py, "OverflowError", "output shape overflow");
        };

        #[cfg(target_arch = "wasm32")]
        if requested_gpu_backend() == Some(GpuBackend::WebGpu) {
            let browser_result: Result<u64, u64> = (|| {
                if x_format == ScalarFormat::I64
                    || weight_format == ScalarFormat::I64
                    || out_format == ScalarFormat::I64
                {
                    return Err(raise_exception::<u64>(
                        _py,
                        "RuntimeError",
                        "browser webgpu squared-relu gate fast path currently supports float formats only",
                    ));
                }
                let x_bytes = bytes_like_view_to_webgpu_bytes(x_view, x_format)
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                let weight_bytes = bytes_like_view_to_webgpu_bytes(weight_view, weight_format)
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                let mut out_webgpu = vec![0u8; outer * hidden * 4];
                let outer_i32 = i32::try_from(outer).map_err(|_| {
                    raise_exception::<u64>(_py, "OverflowError", "outer exceeds i32")
                })?;
                let in_features_i32 = i32::try_from(in_features).map_err(|_| {
                    raise_exception::<u64>(_py, "OverflowError", "in_features exceeds i32")
                })?;
                let hidden_i32 = i32::try_from(hidden).map_err(|_| {
                    raise_exception::<u64>(_py, "OverflowError", "hidden exceeds i32")
                })?;
                let outer_bytes = outer_i32.to_le_bytes();
                let in_features_bytes = in_features_i32.to_le_bytes();
                let hidden_bytes = hidden_i32.to_le_bytes();
                let workgroup_size = 64u32;
                let total_threads = outer.checked_mul(hidden).ok_or_else(|| {
                    raise_exception::<u64>(_py, "OverflowError", "gpu gate thread count overflow")
                })?;
                let grid = if total_threads == 0 {
                    0
                } else {
                    u32::try_from(
                        (total_threads + workgroup_size as usize - 1) / workgroup_size as usize,
                    )
                    .map_err(|_| {
                        raise_exception::<u64>(_py, "OverflowError", "gpu gate grid exceeds u32")
                    })?
                };
                let source = render_webgpu_linear_squared_relu_gate_source(
                    "linear_squared_relu_gate_interleaved",
                    workgroup_size,
                );
                dispatch_browser_webgpu_bindings(
                    _py,
                    source.as_str(),
                    "linear_squared_relu_gate_interleaved",
                    vec![
                        serde_json::json!({"binding": 0, "name": "x", "kind": "buffer", "access": "read", "ptr": x_bytes.as_ptr() as usize as u32, "len": x_bytes.len() as u32}),
                        serde_json::json!({"binding": 1, "name": "weight", "kind": "buffer", "access": "read", "ptr": weight_bytes.as_ptr() as usize as u32, "len": weight_bytes.len() as u32}),
                        serde_json::json!({"binding": 2, "name": "out", "kind": "buffer", "access": "read_write", "ptr": out_webgpu.as_mut_ptr() as usize as u32, "len": out_webgpu.len() as u32}),
                        serde_json::json!({"binding": 3, "name": "outer", "kind": "scalar", "access": "read", "ptr": outer_bytes.as_ptr() as usize as u32, "len": outer_bytes.len() as u32}),
                        serde_json::json!({"binding": 4, "name": "in_features", "kind": "scalar", "access": "read", "ptr": in_features_bytes.as_ptr() as usize as u32, "len": in_features_bytes.len() as u32}),
                        serde_json::json!({"binding": 5, "name": "hidden", "kind": "scalar", "access": "read", "ptr": hidden_bytes.as_ptr() as usize as u32, "len": hidden_bytes.len() as u32}),
                    ],
                    grid,
                    workgroup_size,
                )?;
                let rebuilt = rebuild_host_bytes_from_gpu32_output(
                    _py,
                    out_format,
                    outer * hidden,
                    out_webgpu.as_slice(),
                )?;
                let out_ptr = alloc_bytearray(_py, rebuilt.as_slice());
                if out_ptr.is_null() {
                    return Err(MoltObject::none().bits());
                }
                Ok(MoltObject::from_ptr(out_ptr).bits())
            })();
            return match browser_result {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }
        let mut out = vec![0u8; out_len];

        if x_format == ScalarFormat::F32
            && weight_format == ScalarFormat::F32
            && out_format == ScalarFormat::F32
        {
            unsafe {
                linear_squared_relu_gate_interleaved_f32(
                    x_view.ptr,
                    weight_view.ptr,
                    out.as_mut_ptr(),
                    outer,
                    in_features,
                    hidden,
                );
            }
        } else {
            for batch in 0..outer {
                let x_off = batch * in_features;
                let out_off = batch * hidden;
                for hidden_idx in 0..hidden {
                    let gate_off = (2 * hidden_idx) * in_features;
                    let up_off = (2 * hidden_idx + 1) * in_features;
                    let mut gate = 0.0f64;
                    let mut up = 0.0f64;
                    for k in 0..in_features {
                        let x = unsafe { read_scalar(x_view.ptr, x_off + k, x_format) };
                        let gate_w =
                            unsafe { read_scalar(weight_view.ptr, gate_off + k, weight_format) };
                        let up_w =
                            unsafe { read_scalar(weight_view.ptr, up_off + k, weight_format) };
                        gate += x * gate_w;
                        up += x * up_w;
                    }
                    let relu = gate.max(0.0);
                    unsafe {
                        write_scalar(
                            out.as_mut_ptr(),
                            out_off + hidden_idx,
                            out_format,
                            relu * relu * up,
                        )
                    };
                }
            }
        }

        let out_ptr = alloc_bytearray(_py, &out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_broadcast_binary_contiguous(
    a_data_bits: u64,
    a_format_bits: u64,
    a_shape_bits: u64,
    b_data_bits: u64,
    b_format_bits: u64,
    b_shape_bits: u64,
    op_code_bits: u64,
    out_format_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let a_format = match parse_format(_py, a_format_bits, "a_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let b_format = match parse_format(_py, b_format_bits, "b_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out_format = match parse_format(_py, out_format_bits, "out_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let a_shape = match parse_shape(_py, a_shape_bits, "a_shape") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let b_shape = match parse_shape(_py, b_shape_bits, "b_shape") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(op_code) = to_i64(obj_from_bits(op_code_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "op_code must be an integer");
        };
        let a_view = match bytes_like_view(_py, a_data_bits, "a_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let b_view = match bytes_like_view(_py, b_data_bits, "b_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };

        let a_elems = product(&a_shape);
        let b_elems = product(&b_shape);
        let Some(a_required) = a_elems.checked_mul(a_format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "a_data shape overflow");
        };
        let Some(b_required) = b_elems.checked_mul(b_format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "b_data shape overflow");
        };
        if a_view.len < a_required {
            return raise_exception::<_>(_py, "ValueError", "a_data buffer is too small");
        }
        if b_view.len < b_required {
            return raise_exception::<_>(_py, "ValueError", "b_data buffer is too small");
        }

        let out_ndim = a_shape.len().max(b_shape.len());
        let mut a_padded = vec![1usize; out_ndim - a_shape.len()];
        a_padded.extend_from_slice(&a_shape);
        let mut b_padded = vec![1usize; out_ndim - b_shape.len()];
        b_padded.extend_from_slice(&b_shape);
        let mut out_shape = Vec::with_capacity(out_ndim);
        for (&a_dim, &b_dim) in a_padded.iter().zip(b_padded.iter()) {
            if a_dim == b_dim {
                out_shape.push(a_dim);
            } else if a_dim == 1 {
                out_shape.push(b_dim);
            } else if b_dim == 1 {
                out_shape.push(a_dim);
            } else {
                return raise_exception::<_>(_py, "ValueError", "Cannot broadcast input shapes");
            }
        }
        let out_elems = product(&out_shape);
        let out_strides = strides(&out_shape);
        let a_strides = strides(&a_padded);
        let b_strides = strides(&b_padded);
        let Some(out_len) = out_elems.checked_mul(out_format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "output shape overflow");
        };
        let mut out = vec![0u8; out_len];

        for out_index in 0..out_elems {
            let mut rem = out_index;
            let mut a_index = 0usize;
            let mut b_index = 0usize;
            for axis in 0..out_ndim {
                let stride = out_strides[axis];
                let coord = if stride == 0 { 0 } else { rem / stride };
                rem %= stride.max(1);
                if a_padded[axis] != 1 {
                    a_index += coord * a_strides[axis];
                }
                if b_padded[axis] != 1 {
                    b_index += coord * b_strides[axis];
                }
            }
            let a = unsafe { read_scalar(a_view.ptr, a_index, a_format) };
            let b = unsafe { read_scalar(b_view.ptr, b_index, b_format) };
            let value = match apply_binary_op(_py, op_code, a, b) {
                Ok(value) => value,
                Err(bits) => return bits,
            };
            unsafe { write_scalar(out.as_mut_ptr(), out_index, out_format, value) };
        }

        let out_ptr = alloc_bytearray(_py, &out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_matmul_contiguous(
    a_data_bits: u64,
    a_format_bits: u64,
    a_shape_bits: u64,
    b_data_bits: u64,
    b_format_bits: u64,
    b_shape_bits: u64,
    out_format_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let a_format = match parse_format(_py, a_format_bits, "a_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let b_format = match parse_format(_py, b_format_bits, "b_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out_format = match parse_format(_py, out_format_bits, "out_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let a_shape = match parse_shape(_py, a_shape_bits, "a_shape") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let b_shape = match parse_shape(_py, b_shape_bits, "b_shape") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if a_shape.len() < 2 || b_shape.len() < 2 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "matmul requires tensors with at least 2 dimensions",
            );
        }

        let a_rows = a_shape[a_shape.len() - 2];
        let a_cols = a_shape[a_shape.len() - 1];
        let b_rows = b_shape[b_shape.len() - 2];
        let b_cols = b_shape[b_shape.len() - 1];
        if a_cols != b_rows {
            return raise_exception::<_>(_py, "ValueError", "matmul dimension mismatch");
        }

        let a_view = match bytes_like_view(_py, a_data_bits, "a_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let b_view = match bytes_like_view(_py, b_data_bits, "b_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };

        let a_elems = product(&a_shape);
        let b_elems = product(&b_shape);
        let Some(a_required) = a_elems.checked_mul(a_format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "a_data shape overflow");
        };
        let Some(b_required) = b_elems.checked_mul(b_format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "b_data shape overflow");
        };
        if a_view.len < a_required {
            return raise_exception::<_>(_py, "ValueError", "a_data buffer is too small");
        }
        if b_view.len < b_required {
            return raise_exception::<_>(_py, "ValueError", "b_data buffer is too small");
        }

        let a_batch_shape = &a_shape[..a_shape.len() - 2];
        let b_batch_shape = &b_shape[..b_shape.len() - 2];
        let out_batch_ndim = a_batch_shape.len().max(b_batch_shape.len());
        let mut padded_a_batch_shape = vec![1usize; out_batch_ndim - a_batch_shape.len()];
        padded_a_batch_shape.extend_from_slice(a_batch_shape);
        let mut padded_b_batch_shape = vec![1usize; out_batch_ndim - b_batch_shape.len()];
        padded_b_batch_shape.extend_from_slice(b_batch_shape);
        let mut out_batch_shape = Vec::with_capacity(out_batch_ndim);
        for (&a_dim, &b_dim) in padded_a_batch_shape.iter().zip(padded_b_batch_shape.iter()) {
            if a_dim == b_dim {
                out_batch_shape.push(a_dim);
            } else if a_dim == 1 {
                out_batch_shape.push(b_dim);
            } else if b_dim == 1 {
                out_batch_shape.push(a_dim);
            } else {
                return raise_exception::<_>(_py, "ValueError", "matmul batch shape mismatch");
            }
        }
        let batch_count = if out_batch_shape.is_empty() {
            1
        } else {
            product(&out_batch_shape)
        };
        let Some(out_elems) = batch_count
            .checked_mul(a_rows)
            .and_then(|n| n.checked_mul(b_cols))
        else {
            return raise_exception::<_>(_py, "OverflowError", "output shape overflow");
        };
        let Some(out_len) = out_elems.checked_mul(out_format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "output byte size overflow");
        };

        let mut out = vec![0u8; out_len];
        if a_format == ScalarFormat::F32
            && b_format == ScalarFormat::F32
            && out_format == ScalarFormat::F32
        {
            if unsafe { matmul_f32(a_view.ptr, b_view.ptr, out.as_mut_ptr(), &a_shape, &b_shape) }
                .is_err()
            {
                return raise_exception::<_>(_py, "ValueError", "matmul batch shape mismatch");
            }
        } else {
            let a_stride = a_rows * a_cols;
            let b_stride = b_rows * b_cols;
            let a_batch_strides = if padded_a_batch_shape.is_empty() {
                vec![]
            } else {
                strides(&padded_a_batch_shape)
            };
            let b_batch_strides = if padded_b_batch_shape.is_empty() {
                vec![]
            } else {
                strides(&padded_b_batch_shape)
            };
            let out_batch_strides = if out_batch_shape.is_empty() {
                vec![]
            } else {
                strides(&out_batch_shape)
            };
            for batch in 0..batch_count {
                let mut rem = batch;
                let mut a_batch_index = 0usize;
                let mut b_batch_index = 0usize;
                for axis in 0..out_batch_strides.len() {
                    let stride = out_batch_strides[axis];
                    let coord = if stride == 0 { 0 } else { rem / stride };
                    rem %= stride.max(1);
                    if padded_a_batch_shape[axis] != 1 {
                        a_batch_index += coord * a_batch_strides[axis];
                    }
                    if padded_b_batch_shape[axis] != 1 {
                        b_batch_index += coord * b_batch_strides[axis];
                    }
                }
                let a_off = a_batch_index * a_stride;
                let b_off = b_batch_index * b_stride;
                let out_off = batch * a_rows * b_cols;
                for i in 0..a_rows {
                    for j in 0..b_cols {
                        let mut acc = 0.0f64;
                        for k in 0..a_cols {
                            let a = unsafe {
                                read_scalar(a_view.ptr, a_off + i * a_cols + k, a_format)
                            };
                            let b = unsafe {
                                read_scalar(b_view.ptr, b_off + k * b_cols + j, b_format)
                            };
                            acc += a * b;
                        }
                        unsafe {
                            write_scalar(
                                out.as_mut_ptr(),
                                out_off + i * b_cols + j,
                                out_format,
                                acc,
                            )
                        };
                    }
                }
            }
        }

        let out_ptr = alloc_bytearray(_py, &out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_rope_apply_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    cos_data_bits: u64,
    sin_data_bits: u64,
    freq_dim_bits: u64,
    batch_bits: u64,
    seq_bits: u64,
    heads_bits: u64,
    dim_bits: u64,
    seq_len_bits: u64,
    out_format_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let x_format = match parse_format(_py, x_format_bits, "x_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out_format = match parse_format(_py, out_format_bits, "out_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let freq_dim = match parse_usize_arg(_py, freq_dim_bits, "freq_dim") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let batch = match parse_usize_arg(_py, batch_bits, "batch") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let seq = match parse_usize_arg(_py, seq_bits, "seq") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let heads = match parse_usize_arg(_py, heads_bits, "heads") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let dim = match parse_usize_arg(_py, dim_bits, "dim") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let seq_len = match parse_usize_arg(_py, seq_len_bits, "seq_len") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if dim % 2 != 0 {
            return raise_exception::<_>(_py, "ValueError", "dim must be even");
        }

        let x_view = match bytes_like_view(_py, x_data_bits, "x_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let cos_view = match bytes_like_view(_py, cos_data_bits, "cos_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let sin_view = match bytes_like_view(_py, sin_data_bits, "sin_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };

        let Some(total_elems) = batch
            .checked_mul(seq)
            .and_then(|n| n.checked_mul(heads))
            .and_then(|n| n.checked_mul(dim))
        else {
            return raise_exception::<_>(_py, "OverflowError", "rope tensor shape overflow");
        };
        let Some(x_required) = total_elems.checked_mul(x_format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "x_data shape overflow");
        };
        let Some(freq_required) = seq_len.checked_mul(freq_dim).and_then(|n| n.checked_mul(4))
        else {
            return raise_exception::<_>(_py, "OverflowError", "freq buffer shape overflow");
        };
        let Some(out_len) = total_elems.checked_mul(out_format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "rope output shape overflow");
        };

        if x_view.len < x_required {
            return raise_exception::<_>(_py, "ValueError", "x_data buffer is too small");
        }
        if cos_view.len < freq_required {
            return raise_exception::<_>(_py, "ValueError", "cos_data buffer is too small");
        }
        if sin_view.len < freq_required {
            return raise_exception::<_>(_py, "ValueError", "sin_data buffer is too small");
        }

        let mut out = vec![0u8; out_len];
        if x_format == ScalarFormat::F32 && out_format == ScalarFormat::F32 {
            unsafe {
                rope_apply_f32(
                    x_view.ptr,
                    cos_view.ptr,
                    sin_view.ptr,
                    out.as_mut_ptr(),
                    batch,
                    seq,
                    heads,
                    dim,
                    freq_dim,
                    seq_len,
                );
            }
        } else {
            let half = dim / 2;
            let max_seq = seq.min(seq_len);
            for b in 0..batch {
                for s in 0..seq {
                    let freq_base = s * freq_dim;
                    for h in 0..heads {
                        let base = ((b * seq + s) * heads + h) * dim;
                        if s >= max_seq {
                            for i in 0..dim {
                                let x = unsafe { read_scalar(x_view.ptr, base + i, x_format) };
                                unsafe { write_scalar(out.as_mut_ptr(), base + i, out_format, x) };
                            }
                            continue;
                        }
                        for i in 0..half {
                            let (cos_v, sin_v) = if i < freq_dim {
                                (
                                    unsafe {
                                        read_scalar(cos_view.ptr, freq_base + i, ScalarFormat::F32)
                                    },
                                    unsafe {
                                        read_scalar(sin_view.ptr, freq_base + i, ScalarFormat::F32)
                                    },
                                )
                            } else {
                                (1.0f64, 0.0f64)
                            };
                            let x0 = unsafe { read_scalar(x_view.ptr, base + i, x_format) };
                            let x1 = if i + half < dim {
                                unsafe { read_scalar(x_view.ptr, base + i + half, x_format) }
                            } else {
                                0.0
                            };
                            unsafe {
                                write_scalar(
                                    out.as_mut_ptr(),
                                    base + i,
                                    out_format,
                                    x0 * cos_v - x1 * sin_v,
                                );
                            }
                            if i + half < dim {
                                unsafe {
                                    write_scalar(
                                        out.as_mut_ptr(),
                                        base + i + half,
                                        out_format,
                                        x0 * sin_v + x1 * cos_v,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        let out_ptr = alloc_bytearray(_py, &out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_permute_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    shape_bits: u64,
    dims_bits: u64,
    out_format_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let x_format = match parse_format(_py, x_format_bits, "x_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out_format = match parse_format(_py, out_format_bits, "out_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if x_format != out_format {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "permute requires matching input/output formats",
            );
        }
        let shape = match parse_shape(_py, shape_bits, "shape") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let dims = match parse_shape(_py, dims_bits, "dims") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Err(bits) = validate_permutation(_py, &dims, shape.len()) {
            return bits;
        }
        let x_view = match bytes_like_view(_py, x_data_bits, "x_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let total_elems = product(&shape);
        let itemsize = x_format.itemsize();
        let Some(required) = total_elems.checked_mul(itemsize) else {
            return raise_exception::<_>(_py, "OverflowError", "permute shape overflow");
        };
        if x_view.len < required {
            return raise_exception::<_>(_py, "ValueError", "x_data buffer is too small");
        }
        let out_shape: Vec<usize> = dims.iter().map(|&dim| shape[dim]).collect();
        let old_strides = strides(&shape);
        let new_strides = strides(&out_shape);
        let mut out = vec![0u8; required];
        let src = unsafe { std::slice::from_raw_parts(x_view.ptr, required) };
        for old_index in 0..total_elems {
            let mut rem = old_index;
            let mut coords = vec![0usize; shape.len()];
            for axis in 0..shape.len() {
                let stride = old_strides[axis];
                coords[axis] = if stride == 0 { 0 } else { rem / stride };
                rem %= stride.max(1);
            }
            let mut new_index = 0usize;
            for (new_axis, &old_axis) in dims.iter().enumerate() {
                new_index += coords[old_axis] * new_strides[new_axis];
            }
            let src_base = old_index * itemsize;
            let dst_base = new_index * itemsize;
            out[dst_base..dst_base + itemsize].copy_from_slice(&src[src_base..src_base + itemsize]);
        }

        let out_ptr = alloc_bytearray(_py, &out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_softmax_last_axis_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    shape_bits: u64,
    out_format_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let x_format = match parse_format(_py, x_format_bits, "x_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out_format = match parse_format(_py, out_format_bits, "out_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let shape = match parse_shape(_py, shape_bits, "shape") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if shape.is_empty() {
            let mut out = vec![0u8; out_format.itemsize()];
            unsafe { write_scalar(out.as_mut_ptr(), 0, out_format, 1.0) };
            let out_ptr = alloc_bytearray(_py, &out);
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(out_ptr).bits();
        }
        let x_view = match bytes_like_view(_py, x_data_bits, "x_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let total_elems = product(&shape);
        let Some(required) = total_elems.checked_mul(x_format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "softmax shape overflow");
        };
        if x_view.len < required {
            return raise_exception::<_>(_py, "ValueError", "x_data buffer is too small");
        }
        let axis_len = *shape.last().unwrap_or(&1);
        let outer = if axis_len == 0 {
            0
        } else {
            total_elems / axis_len
        };
        let mut out = vec![0u8; total_elems * out_format.itemsize()];
        if x_format == ScalarFormat::F32 && out_format == ScalarFormat::F32 {
            unsafe { softmax_last_axis_f32(x_view.ptr, out.as_mut_ptr(), outer, axis_len) };
        } else {
            for row in 0..outer {
                let base = row * axis_len;
                let mut max_val = f64::NEG_INFINITY;
                for i in 0..axis_len {
                    let value = unsafe { read_scalar(x_view.ptr, base + i, x_format) };
                    if value > max_val {
                        max_val = value;
                    }
                }
                let mut sum = 0.0f64;
                for i in 0..axis_len {
                    let value = unsafe { read_scalar(x_view.ptr, base + i, x_format) };
                    let exp_v = (value - max_val).exp();
                    unsafe { write_scalar(out.as_mut_ptr(), base + i, out_format, exp_v) };
                    sum += exp_v;
                }
                for i in 0..axis_len {
                    let exp_v = unsafe { read_scalar(out.as_ptr(), base + i, out_format) };
                    unsafe { write_scalar(out.as_mut_ptr(), base + i, out_format, exp_v / sum) };
                }
            }
        }
        let out_ptr = alloc_bytearray(_py, &out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_rms_norm_last_axis_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    shape_bits: u64,
    eps_bits: u64,
    out_format_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let x_format = match parse_format(_py, x_format_bits, "x_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out_format = match parse_format(_py, out_format_bits, "out_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let x_view = match bytes_like_view(_py, x_data_bits, "x_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let shape = match parse_shape(_py, shape_bits, "shape") {
            Ok(shape) => shape,
            Err(bits) => return bits,
        };
        let Some(eps) = to_f64(obj_from_bits(eps_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "eps must be a float");
        };
        if shape.is_empty() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "rms_norm requires a tensor with at least 1 dimension",
            );
        }
        let axis_len = shape[shape.len() - 1];
        if axis_len == 0 {
            return raise_exception::<_>(_py, "ValueError", "rms_norm last axis must be non-empty");
        }
        let total_elems = product(&shape);
        if x_view.len != total_elems * x_format.itemsize() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "x_data byte length does not match shape",
            );
        }
        let outer = total_elems / axis_len;
        let mut out = vec![0u8; total_elems * out_format.itemsize()];
        if x_format == ScalarFormat::F32 && out_format == ScalarFormat::F32 {
            unsafe {
                rms_norm_last_axis_f32(x_view.ptr, out.as_mut_ptr(), outer, axis_len, eps as f32)
            };
        } else {
            let axis_len_f64 = axis_len as f64;
            for row in 0..outer {
                let base = row * axis_len;
                let mut sumsq = 0.0f64;
                for i in 0..axis_len {
                    let value = unsafe { read_scalar(x_view.ptr, base + i, x_format) };
                    sumsq += value * value;
                }
                let scale = 1.0f64 / ((sumsq / axis_len_f64) + eps).sqrt();
                for i in 0..axis_len {
                    let value = unsafe { read_scalar(x_view.ptr, base + i, x_format) };
                    unsafe { write_scalar(out.as_mut_ptr(), base + i, out_format, value * scale) };
                }
            }
        }
        let out_ptr = alloc_bytearray(_py, &out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_squared_relu_gate_interleaved_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    shape_bits: u64,
    out_format_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let x_format = match parse_format(_py, x_format_bits, "x_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out_format = match parse_format(_py, out_format_bits, "out_format") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let x_view = match bytes_like_view(_py, x_data_bits, "x_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let shape = match parse_shape(_py, shape_bits, "shape") {
            Ok(shape) => shape,
            Err(bits) => return bits,
        };
        if shape.is_empty() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "squared_relu_gate_interleaved requires a tensor with at least 1 dimension",
            );
        }
        let axis_len = shape[shape.len() - 1];
        if axis_len % 2 != 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "squared_relu_gate_interleaved last axis must be even",
            );
        }
        let total_elems = product(&shape);
        if x_view.len != total_elems * x_format.itemsize() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "x_data byte length does not match shape",
            );
        }
        let outer = if axis_len == 0 {
            0
        } else {
            total_elems / axis_len
        };
        let out_elems = outer * (axis_len / 2);
        let mut out = vec![0u8; out_elems * out_format.itemsize()];
        if x_format == ScalarFormat::F32 && out_format == ScalarFormat::F32 {
            unsafe {
                squared_relu_gate_interleaved_f32(x_view.ptr, out.as_mut_ptr(), outer, axis_len)
            };
        } else {
            let hidden = axis_len / 2;
            for row in 0..outer {
                let in_base = row * axis_len;
                let out_base = row * hidden;
                for i in 0..hidden {
                    let gate = unsafe { read_scalar(x_view.ptr, in_base + 2 * i, x_format) };
                    let up = unsafe { read_scalar(x_view.ptr, in_base + 2 * i + 1, x_format) };
                    let relu = gate.max(0.0);
                    unsafe {
                        write_scalar(out.as_mut_ptr(), out_base + i, out_format, relu * relu * up)
                    };
                }
            }
        }
        let out_ptr = alloc_bytearray(_py, &out);
        if out_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}
