use super::*;

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_linear(x_bits: u64, weight_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (x, x_shape) = match unsafe { tensor_runtime_view(_py, x_bits, "x") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let (weight, weight_shape) =
            match unsafe { tensor_runtime_view(_py, weight_bits, "weight") } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        if weight_shape.len() != 2 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!("linear weight must be 2D, got {:?}", weight_shape),
            );
        }
        if x_shape.is_empty() {
            return raise_exception::<_>(_py, "ValueError", "linear input must be at least 1D");
        }
        let in_features = *x_shape.last().unwrap_or(&0);
        let out_features = weight_shape[0];
        let weight_in = weight_shape[1];
        if in_features != weight_in {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!(
                    "Linear shape mismatch: {:?} with weight {:?}",
                    x_shape, weight_shape
                ),
            );
        }
        let outer = if x_shape.len() > 1 {
            product(&x_shape[..x_shape.len() - 1])
        } else {
            1
        };
        let out_shape = if x_shape.len() > 1 {
            let mut dims = x_shape[..x_shape.len() - 1].to_vec();
            dims.push(out_features);
            dims
        } else {
            vec![out_features]
        };
        let (out_format_bits, out_format, owns_out_format, result_dtype_bits) =
            match unsafe { promoted_result_format_bits(_py, &x, &weight) } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let out_data_bits = molt_gpu_linear_contiguous(
            x.buffer.data_bits,
            x.buffer.format_bits,
            weight.buffer.data_bits,
            weight.buffer.format_bits,
            MoltObject::from_int(outer as i64).bits(),
            MoltObject::from_int(in_features as i64).bits(),
            MoltObject::from_int(out_features as i64).bits(),
            out_format_bits,
        );
        if crate::exception_pending(_py) {
            if owns_out_format {
                crate::dec_ref_bits(_py, out_format_bits);
            }
            return out_data_bits;
        }
        let out_shape_bits = match alloc_tuple_bits_from_usize(_py, out_shape.as_slice()) {
            Ok(bits) => bits,
            Err(bits) => {
                if owns_out_format {
                    crate::dec_ref_bits(_py, out_format_bits);
                }
                return bits;
            }
        };
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                x.class_bits,
                x.buffer.class_bits,
                out_data_bits,
                result_dtype_bits,
                outer * out_features,
                out_format_bits,
                out_format.itemsize(),
                out_shape_bits,
                result_dtype_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, out_data_bits);
        crate::dec_ref_bits(_py, out_shape_bits);
        if owns_out_format {
            crate::dec_ref_bits(_py, out_format_bits);
        }
        tensor_bits
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_linear_split_last_dim(
    x_bits: u64,
    weight_bits: u64,
    split_sizes_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (x, x_shape) = match unsafe { tensor_runtime_view(_py, x_bits, "x") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let (weight, weight_shape) =
            match unsafe { tensor_runtime_view(_py, weight_bits, "weight") } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        if weight_shape.len() != 2 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!("linear weight must be 2D, got {:?}", weight_shape),
            );
        }
        if x_shape.is_empty() {
            return raise_exception::<_>(_py, "ValueError", "linear input must be at least 1D");
        }
        let split_sizes = match parse_shape(_py, split_sizes_bits, "split_sizes") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let in_features = *x_shape.last().unwrap_or(&0);
        let out_features = weight_shape[0];
        let weight_in = weight_shape[1];
        if in_features != weight_in {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!(
                    "Linear shape mismatch: {:?} with weight {:?}",
                    x_shape, weight_shape
                ),
            );
        }
        if split_sizes.iter().copied().sum::<usize>() != out_features {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!(
                    "split sizes {:?} do not match projected dimension {}",
                    split_sizes, out_features
                ),
            );
        }
        let outer = if x_shape.len() > 1 {
            product(&x_shape[..x_shape.len() - 1])
        } else {
            1
        };
        let prefix_shape = if x_shape.len() > 1 {
            x_shape[..x_shape.len() - 1].to_vec()
        } else {
            Vec::new()
        };
        let (out_format_bits, out_format, owns_out_format, result_dtype_bits) =
            match unsafe { promoted_result_format_bits(_py, &x, &weight) } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let out_parts_bits = molt_gpu_linear_split_last_dim_contiguous(
            x.buffer.data_bits,
            x.buffer.format_bits,
            weight.buffer.data_bits,
            weight.buffer.format_bits,
            MoltObject::from_int(outer as i64).bits(),
            MoltObject::from_int(in_features as i64).bits(),
            split_sizes_bits,
            out_format_bits,
        );
        if crate::exception_pending(_py) {
            if owns_out_format {
                crate::dec_ref_bits(_py, out_format_bits);
            }
            return out_parts_bits;
        }
        let Some(out_parts_ptr) = obj_from_bits(out_parts_bits).as_ptr() else {
            if owns_out_format {
                crate::dec_ref_bits(_py, out_format_bits);
            }
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "linear split helper did not return a tuple",
            );
        };
        let part_data_bits = unsafe { seq_vec_ref(out_parts_ptr) };
        if part_data_bits.len() != split_sizes.len() {
            if owns_out_format {
                crate::dec_ref_bits(_py, out_format_bits);
            }
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "intrinsic returned wrong split count",
            );
        }
        let mut tensors = Vec::with_capacity(split_sizes.len());
        for (idx, &part_size) in split_sizes.iter().enumerate() {
            let mut dims = prefix_shape.clone();
            dims.push(part_size);
            let shape_bits = match alloc_tuple_bits_from_usize(_py, dims.as_slice()) {
                Ok(bits) => bits,
                Err(bits) => {
                    if owns_out_format {
                        crate::dec_ref_bits(_py, out_format_bits);
                    }
                    return bits;
                }
            };
            let tensor_bits = match unsafe {
                build_tensor_from_data_bits(
                    _py,
                    x.class_bits,
                    x.buffer.class_bits,
                    part_data_bits[idx],
                    result_dtype_bits,
                    outer * part_size,
                    out_format_bits,
                    out_format.itemsize(),
                    shape_bits,
                    result_dtype_bits,
                )
            } {
                Ok(bits) => bits,
                Err(bits) => {
                    crate::dec_ref_bits(_py, shape_bits);
                    if owns_out_format {
                        crate::dec_ref_bits(_py, out_format_bits);
                    }
                    return bits;
                }
            };
            crate::dec_ref_bits(_py, shape_bits);
            tensors.push(tensor_bits);
        }
        let tuple_ptr = alloc_tuple(_py, tensors.as_slice());
        if owns_out_format {
            crate::dec_ref_bits(_py, out_format_bits);
        }
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_linear_squared_relu_gate_interleaved(
    x_bits: u64,
    weight_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (x, x_shape) = match unsafe { tensor_runtime_view(_py, x_bits, "x") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let (weight, weight_shape) =
            match unsafe { tensor_runtime_view(_py, weight_bits, "weight") } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        if weight_shape.len() != 2 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!("linear weight must be 2D, got {:?}", weight_shape),
            );
        }
        if x_shape.is_empty() {
            return raise_exception::<_>(_py, "ValueError", "linear input must be at least 1D");
        }
        let in_features = *x_shape.last().unwrap_or(&0);
        let out_features = weight_shape[0];
        let weight_in = weight_shape[1];
        if in_features != weight_in {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!(
                    "Linear shape mismatch: {:?} with weight {:?}",
                    x_shape, weight_shape
                ),
            );
        }
        if out_features % 2 != 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!(
                    "interleaved gate weight output dimension must be even, got {}",
                    out_features
                ),
            );
        }
        let outer = if x_shape.len() > 1 {
            product(&x_shape[..x_shape.len() - 1])
        } else {
            1
        };
        let hidden = out_features / 2;
        let out_shape = if x_shape.len() > 1 {
            let mut dims = x_shape[..x_shape.len() - 1].to_vec();
            dims.push(hidden);
            dims
        } else {
            vec![hidden]
        };
        let (out_format_bits, out_format, owns_out_format, result_dtype_bits) =
            match unsafe { promoted_result_format_bits(_py, &x, &weight) } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let out_data_bits = molt_gpu_linear_squared_relu_gate_interleaved_contiguous(
            x.buffer.data_bits,
            x.buffer.format_bits,
            weight.buffer.data_bits,
            weight.buffer.format_bits,
            MoltObject::from_int(outer as i64).bits(),
            MoltObject::from_int(in_features as i64).bits(),
            out_format_bits,
        );
        if crate::exception_pending(_py) {
            if owns_out_format {
                crate::dec_ref_bits(_py, out_format_bits);
            }
            return out_data_bits;
        }
        let out_shape_bits = match alloc_tuple_bits_from_usize(_py, out_shape.as_slice()) {
            Ok(bits) => bits,
            Err(bits) => {
                if owns_out_format {
                    crate::dec_ref_bits(_py, out_format_bits);
                }
                return bits;
            }
        };
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                x.class_bits,
                x.buffer.class_bits,
                out_data_bits,
                result_dtype_bits,
                outer * hidden,
                out_format_bits,
                out_format.itemsize(),
                out_shape_bits,
                result_dtype_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, out_data_bits);
        crate::dec_ref_bits(_py, out_shape_bits);
        if owns_out_format {
            crate::dec_ref_bits(_py, out_format_bits);
        }
        tensor_bits
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_permute_dims(x_bits: u64, dims_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (x, x_shape) = match unsafe { tensor_runtime_view(_py, x_bits, "x") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let normalized_dims = match normalize_permute_dims(_py, dims_bits, x_shape.len()) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if x_shape.len() <= 1 {
            return match unsafe {
                build_tensor_instance(_py, x.class_bits, x.buffer_bits, x.shape_bits, x.dtype_bits)
            } {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }
        let normalized_dims_bits =
            match alloc_tuple_bits_from_usize(_py, normalized_dims.as_slice()) {
                Ok(bits) => bits,
                Err(bits) => return bits,
            };
        let out_data_bits = molt_gpu_permute_contiguous(
            x.buffer.data_bits,
            x.buffer.format_bits,
            x.shape_bits,
            normalized_dims_bits,
            x.buffer.format_bits,
        );
        crate::dec_ref_bits(_py, normalized_dims_bits);
        if crate::exception_pending(_py) {
            return out_data_bits;
        }
        let out_shape: Vec<usize> = normalized_dims.iter().map(|&dim| x_shape[dim]).collect();
        let out_shape_bits = match alloc_tuple_bits_from_usize(_py, out_shape.as_slice()) {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, out_data_bits);
                return bits;
            }
        };
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                x.class_bits,
                x.buffer.class_bits,
                out_data_bits,
                x.buffer.element_type_bits,
                if x_shape.is_empty() {
                    1
                } else {
                    product(&x_shape)
                },
                x.buffer.format_bits,
                x.buffer.format.itemsize(),
                out_shape_bits,
                x.dtype_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, out_data_bits);
        crate::dec_ref_bits(_py, out_shape_bits);
        tensor_bits
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_softmax_last_axis(x_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (x, x_shape) = match unsafe { tensor_runtime_view(_py, x_bits, "x") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let shape_bits = match alloc_tuple_bits_from_usize(_py, x_shape.as_slice()) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let out_data_bits = molt_gpu_softmax_last_axis_contiguous(
            x.buffer.data_bits,
            x.buffer.format_bits,
            shape_bits,
            x.buffer.format_bits,
        );
        if crate::exception_pending(_py) {
            crate::dec_ref_bits(_py, shape_bits);
            return out_data_bits;
        }
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                x.class_bits,
                x.buffer.class_bits,
                out_data_bits,
                x.buffer.element_type_bits,
                x.buffer.size,
                x.buffer.format_bits,
                x.buffer.format.itemsize(),
                shape_bits,
                x.dtype_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, out_data_bits);
        crate::dec_ref_bits(_py, shape_bits);
        tensor_bits
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_reshape_view(x_bits: u64, shape_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (x, x_shape) = match unsafe { tensor_runtime_view(_py, x_bits, "x") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let total_size = if x_shape.is_empty() {
            1
        } else {
            product(&x_shape)
        };
        let mut dims = match normalize_reshape_dims(_py, shape_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut neg_idx = None;
        let mut known = 1i64;
        for (idx, dim) in dims.iter().copied().enumerate() {
            if dim == -1 {
                if neg_idx.is_some() {
                    return raise_exception::<_>(_py, "ValueError", "Only one dimension can be -1");
                }
                neg_idx = Some(idx);
            } else {
                known = known.saturating_mul(dim);
            }
        }
        if let Some(idx) = neg_idx {
            if known == 0 {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "integer division or modulo by zero",
                );
            }
            dims[idx] = (total_size as i64) / known;
        }
        let mut final_shape = Vec::with_capacity(dims.len());
        for dim in dims.iter().copied() {
            let value = usize::try_from(dim).map_err(|_| {
                raise_exception::<u64>(
                    _py,
                    "ValueError",
                    &format!(
                        "Cannot reshape tensor of size {} into shape {:?}",
                        total_size, dims
                    ),
                )
            });
            match value {
                Ok(value) => final_shape.push(value),
                Err(bits) => return bits,
            }
        }
        if product(final_shape.as_slice()) != total_size {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!(
                    "Cannot reshape tensor of size {} into shape {:?}",
                    total_size, final_shape
                ),
            );
        }
        let final_shape_bits = match alloc_tuple_bits_from_usize(_py, final_shape.as_slice()) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let tensor_bits = match unsafe {
            build_tensor_instance(
                _py,
                x.class_bits,
                x.buffer_bits,
                final_shape_bits,
                x.dtype_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, final_shape_bits);
        tensor_bits
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_data_list(x_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (x, x_shape) = match unsafe { tensor_runtime_view(_py, x_bits, "x") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let size = if x_shape.is_empty() {
            1
        } else {
            product(&x_shape)
        };
        molt_gpu_buffer_to_list(x.buffer_bits, MoltObject::from_int(size as i64).bits())
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_take_rows(
    x_bits: u64,
    indices_bits: u64,
    allow_negative_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let trace_take_rows = std::env::var("MOLT_TRACE_GPU_TAKE_ROWS").as_deref() == Ok("1");
        let (x, x_shape) = match unsafe { tensor_runtime_view(_py, x_bits, "x") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if x_shape.is_empty() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "take_rows requires a tensor with at least 1 dimension",
            );
        }
        let indices_tensor_bits = match unsafe { ensure_tensor_object_bits(_py, indices_bits) } {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let (indices, indices_shape) =
            match unsafe { tensor_runtime_view(_py, indices_tensor_bits, "indices") } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let row_count = if indices_shape.is_empty() {
            1
        } else {
            product(&indices_shape)
        };
        let rows_list_bits = molt_gpu_buffer_to_list(
            indices.buffer_bits,
            MoltObject::from_int(row_count as i64).bits(),
        );
        if crate::exception_pending(_py) {
            return rows_list_bits;
        }
        let Some(rows_list_ptr) = obj_from_bits(rows_list_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "indices tensor did not materialize to a list",
            );
        };
        let row_shape = &x_shape[1..];
        let row_size = if row_shape.is_empty() {
            1
        } else {
            product(row_shape)
        };
        let width = row_size * x.buffer.format.itemsize();
        let expected_bytes = x.buffer.size * x.buffer.format.itemsize();
        if trace_take_rows {
            eprintln!(
                "molt gpu take_rows x_shape={:?} indices_shape={:?} row_count={} row_size={} width={} x_size={} x_bytes={}",
                x_shape, indices_shape, row_count, row_size, width, x.buffer.size, expected_bytes
            );
        }
        if x.buffer.data_view.len < expected_bytes {
            return raise_exception::<_>(_py, "ValueError", "x_data buffer is too small");
        }
        let allow_negative = crate::is_truthy(_py, obj_from_bits(allow_negative_bits));
        let rows = unsafe { seq_vec_ref(rows_list_ptr) };
        if trace_take_rows {
            let preview: Vec<i64> = rows
                .iter()
                .take(8)
                .map(|&bits| crate::to_i64(obj_from_bits(bits)).unwrap_or(i64::MIN))
                .collect();
            eprintln!(
                "molt gpu take_rows rows_len={} rows_preview={:?} allow_negative={}",
                rows.len(),
                preview,
                allow_negative
            );
        }
        let src = unsafe { std::slice::from_raw_parts(x.buffer.data_view.ptr, expected_bytes) };
        let mut out = vec![0u8; rows.len() * width];
        for (out_row, &raw_idx_bits) in rows.iter().enumerate() {
            let raw_idx_obj = obj_from_bits(raw_idx_bits);
            let idx = if let Some(value) = to_i64(raw_idx_obj) {
                value
            } else if let Some(value) = to_f64(raw_idx_obj) {
                let idx = value as i64;
                if (idx as f64) != value {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        &format!("take_rows indices must be integers, got {:?}", value),
                    );
                }
                idx
            } else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "take_rows indices must be integers",
                );
            };
            let mut resolved = idx;
            if resolved < 0 && allow_negative {
                resolved += x_shape[0] as i64;
            }
            if resolved < 0 || resolved >= x_shape[0] as i64 {
                return raise_exception::<_>(
                    _py,
                    "IndexError",
                    &format!(
                        "Index {} out of range for axis 0 with size {}",
                        idx, x_shape[0]
                    ),
                );
            }
            let src_start = resolved as usize * width;
            let dst_start = out_row * width;
            out[dst_start..dst_start + width].copy_from_slice(&src[src_start..src_start + width]);
        }
        if trace_take_rows {
            eprintln!(
                "molt gpu take_rows copied_rows={} out_bytes={}",
                rows.len(),
                out.len()
            );
        }
        let out_data_ptr = alloc_bytearray(_py, out.as_slice());
        if out_data_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let mut out_shape = indices_shape.clone();
        out_shape.extend_from_slice(row_shape);
        let out_shape_bits = match alloc_tuple_bits_from_usize(_py, out_shape.as_slice()) {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, MoltObject::from_ptr(out_data_ptr).bits());
                return bits;
            }
        };
        let out_data_bits = MoltObject::from_ptr(out_data_ptr).bits();
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                x.class_bits,
                x.buffer.class_bits,
                out_data_bits,
                x.buffer.element_type_bits,
                rows.len() * row_size,
                x.buffer.format_bits,
                x.buffer.format.itemsize(),
                out_shape_bits,
                x.dtype_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, out_data_bits);
        crate::dec_ref_bits(_py, out_shape_bits);
        tensor_bits
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_concat_first_dim(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (a, a_shape) = match unsafe { tensor_runtime_view(_py, a_bits, "a") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let (b, b_shape) = match unsafe { tensor_runtime_view(_py, b_bits, "b") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if a_shape.is_empty() || b_shape.is_empty() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "concat_first_dim requires tensors with at least 1 dimension",
            );
        }
        if a_shape.len() != b_shape.len() {
            return raise_exception::<_>(_py, "ValueError", "concat_first_dim rank mismatch");
        }
        if a_shape[1..] != b_shape[1..] {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "concat_first_dim trailing shape mismatch",
            );
        }
        if a.buffer.format != b.buffer.format {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "concat_first_dim requires matching buffer formats",
            );
        }
        let a_required = a.buffer.size * a.buffer.format.itemsize();
        let b_required = b.buffer.size * b.buffer.format.itemsize();
        if a.buffer.data_view.len < a_required {
            return raise_exception::<_>(_py, "ValueError", "a buffer is too small");
        }
        if b.buffer.data_view.len < b_required {
            return raise_exception::<_>(_py, "ValueError", "b buffer is too small");
        }
        let mut out = vec![0u8; a_required + b_required];
        let a_src = unsafe { std::slice::from_raw_parts(a.buffer.data_view.ptr, a_required) };
        let b_src = unsafe { std::slice::from_raw_parts(b.buffer.data_view.ptr, b_required) };
        out[..a_required].copy_from_slice(a_src);
        out[a_required..].copy_from_slice(b_src);
        let out_data_ptr = alloc_bytearray(_py, out.as_slice());
        if out_data_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let mut out_shape = a_shape[..1].to_vec();
        out_shape[0] += b_shape[0];
        out_shape.extend_from_slice(&a_shape[1..]);
        let out_shape_bits = match alloc_tuple_bits_from_usize(_py, out_shape.as_slice()) {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, MoltObject::from_ptr(out_data_ptr).bits());
                return bits;
            }
        };
        let out_data_bits = MoltObject::from_ptr(out_data_ptr).bits();
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                a.class_bits,
                a.buffer.class_bits,
                out_data_bits,
                a.buffer.element_type_bits,
                a.buffer.size + b.buffer.size,
                a.buffer.format_bits,
                a.buffer.format.itemsize(),
                out_shape_bits,
                a.dtype_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, out_data_bits);
        crate::dec_ref_bits(_py, out_shape_bits);
        tensor_bits
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_scatter_rows(
    base_bits: u64,
    indices_bits: u64,
    updates_bits: u64,
    allow_negative_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let trace_scatter_rows = std::env::var("MOLT_TRACE_GPU_SCATTER_ROWS").as_deref() == Ok("1");
        let (base, base_shape) = match unsafe { tensor_runtime_view(_py, base_bits, "base") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let (updates, updates_shape) =
            match unsafe { tensor_runtime_view(_py, updates_bits, "updates") } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        if base_shape.is_empty() || updates_shape.is_empty() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "scatter_rows requires tensors with at least 1 dimension",
            );
        }
        if base_shape.len() != updates_shape.len() {
            return raise_exception::<_>(_py, "ValueError", "scatter_rows rank mismatch");
        }
        if base_shape[1..] != updates_shape[1..] {
            return raise_exception::<_>(_py, "ValueError", "scatter_rows trailing shape mismatch");
        }
        if base.buffer.format != updates.buffer.format {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "scatter_rows requires matching buffer formats",
            );
        }
        let indices_tensor_bits = match unsafe { ensure_tensor_object_bits(_py, indices_bits) } {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let (indices, indices_shape) =
            match unsafe { tensor_runtime_view(_py, indices_tensor_bits, "indices") } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let row_count = if indices_shape.is_empty() {
            1
        } else {
            product(&indices_shape)
        };
        if row_count != updates_shape[0] {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "scatter_rows update row count mismatch",
            );
        }
        let rows_list_bits = molt_gpu_buffer_to_list(
            indices.buffer_bits,
            MoltObject::from_int(row_count as i64).bits(),
        );
        if crate::exception_pending(_py) {
            return rows_list_bits;
        }
        let Some(rows_list_ptr) = obj_from_bits(rows_list_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "indices tensor did not materialize to a list",
            );
        };
        let row_shape = &base_shape[1..];
        let row_size = if row_shape.is_empty() {
            1
        } else {
            product(row_shape)
        };
        let width = row_size * base.buffer.format.itemsize();
        let base_required = base.buffer.size * base.buffer.format.itemsize();
        let updates_required = updates.buffer.size * updates.buffer.format.itemsize();
        if trace_scatter_rows {
            eprintln!(
                "molt gpu scatter_rows base_shape={:?} updates_shape={:?} indices_shape={:?} row_count={} row_size={} width={} base_size={} updates_size={}",
                base_shape,
                updates_shape,
                indices_shape,
                row_count,
                row_size,
                width,
                base.buffer.size,
                updates.buffer.size
            );
        }
        if base.buffer.data_view.len < base_required {
            return raise_exception::<_>(_py, "ValueError", "base buffer is too small");
        }
        if updates.buffer.data_view.len < updates_required {
            return raise_exception::<_>(_py, "ValueError", "updates buffer is too small");
        }
        let allow_negative = crate::is_truthy(_py, obj_from_bits(allow_negative_bits));
        let rows = unsafe { seq_vec_ref(rows_list_ptr) };
        if trace_scatter_rows {
            let preview: Vec<i64> = rows
                .iter()
                .take(8)
                .map(|&bits| crate::to_i64(obj_from_bits(bits)).unwrap_or(i64::MIN))
                .collect();
            eprintln!(
                "molt gpu scatter_rows rows_len={} rows_preview={:?} allow_negative={}",
                rows.len(),
                preview,
                allow_negative
            );
        }
        let base_src =
            unsafe { std::slice::from_raw_parts(base.buffer.data_view.ptr, base_required) };
        let updates_src =
            unsafe { std::slice::from_raw_parts(updates.buffer.data_view.ptr, updates_required) };
        let mut out = base_src.to_vec();
        for (src_row, &raw_idx_bits) in rows.iter().enumerate() {
            let raw_idx_obj = obj_from_bits(raw_idx_bits);
            let idx = if let Some(value) = to_i64(raw_idx_obj) {
                value
            } else if let Some(value) = to_f64(raw_idx_obj) {
                let idx = value as i64;
                if (idx as f64) != value {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        &format!("scatter_rows indices must be integers, got {:?}", value),
                    );
                }
                idx
            } else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "scatter_rows indices must be integers",
                );
            };
            let mut resolved = idx;
            if resolved < 0 && allow_negative {
                resolved += base_shape[0] as i64;
            }
            if resolved < 0 || resolved >= base_shape[0] as i64 {
                return raise_exception::<_>(
                    _py,
                    "IndexError",
                    &format!(
                        "Index {} out of range for axis 0 with size {}",
                        idx, base_shape[0]
                    ),
                );
            }
            let dst_start = resolved as usize * width;
            let src_start = src_row * width;
            out[dst_start..dst_start + width]
                .copy_from_slice(&updates_src[src_start..src_start + width]);
        }
        if trace_scatter_rows {
            eprintln!(
                "molt gpu scatter_rows copied_rows={} out_bytes={}",
                rows.len(),
                out.len()
            );
        }
        let out_data_ptr = alloc_bytearray(_py, out.as_slice());
        if out_data_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let out_data_bits = MoltObject::from_ptr(out_data_ptr).bits();
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                base.class_bits,
                base.buffer.class_bits,
                out_data_bits,
                base.buffer.element_type_bits,
                base.buffer.size,
                base.buffer.format_bits,
                base.buffer.format.itemsize(),
                base.shape_bits,
                base.dtype_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, out_data_bits);
        tensor_bits
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__zeros(shape_bits: u64, dtype_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let tensor_class_bits =
            match unsafe { module_global_bits(_py, b"molt.gpu.tensor", b"Tensor", "Tensor") } {
                Ok(bits) => bits,
                Err(bits) => return bits,
            };
        let buffer_class_bits =
            match unsafe { module_global_bits(_py, b"molt.gpu.tensor", b"Buffer", "Buffer") } {
                Ok(bits) => bits,
                Err(bits) => {
                    crate::dec_ref_bits(_py, tensor_class_bits);
                    return bits;
                }
            };
        let dims_i64 = match parse_i64_sequence_arg(_py, shape_bits, "shape", true) {
            Ok(value) => value,
            Err(bits) => {
                crate::dec_ref_bits(_py, tensor_class_bits);
                crate::dec_ref_bits(_py, buffer_class_bits);
                return bits;
            }
        };
        let mut dims = Vec::with_capacity(dims_i64.len());
        for dim in dims_i64 {
            let value = usize::try_from(dim).map_err(|_| {
                raise_exception::<u64>(_py, "ValueError", "shape dimensions must be non-negative")
            });
            match value {
                Ok(value) => dims.push(value),
                Err(bits) => {
                    crate::dec_ref_bits(_py, tensor_class_bits);
                    crate::dec_ref_bits(_py, buffer_class_bits);
                    return bits;
                }
            }
        }
        let size = product(dims.as_slice());
        let out = vec![0u8; size * 8];
        let data_ptr = alloc_bytearray(_py, out.as_slice());
        if data_ptr.is_null() {
            crate::dec_ref_bits(_py, tensor_class_bits);
            crate::dec_ref_bits(_py, buffer_class_bits);
            return MoltObject::none().bits();
        }
        let data_bits = MoltObject::from_ptr(data_ptr).bits();
        let format_bits = match alloc_string_bits(_py, b"d") {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, tensor_class_bits);
                crate::dec_ref_bits(_py, buffer_class_bits);
                crate::dec_ref_bits(_py, data_bits);
                return bits;
            }
        };
        let shape_tuple_bits = match alloc_tuple_bits_from_usize(_py, dims.as_slice()) {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, tensor_class_bits);
                crate::dec_ref_bits(_py, buffer_class_bits);
                crate::dec_ref_bits(_py, data_bits);
                crate::dec_ref_bits(_py, format_bits);
                return bits;
            }
        };
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                tensor_class_bits,
                buffer_class_bits,
                data_bits,
                crate::builtins::classes::builtin_classes(_py).float,
                size,
                format_bits,
                ScalarFormat::F64.itemsize(),
                shape_tuple_bits,
                dtype_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, tensor_class_bits);
        crate::dec_ref_bits(_py, buffer_class_bits);
        crate::dec_ref_bits(_py, data_bits);
        crate::dec_ref_bits(_py, format_bits);
        crate::dec_ref_bits(_py, shape_tuple_bits);
        tensor_bits
    })
}
