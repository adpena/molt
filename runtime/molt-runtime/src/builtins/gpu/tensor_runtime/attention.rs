use super::*;

fn normalized_hadamard_in_place(values: &mut [f32]) {
    let size = values.len();
    let mut span = 1usize;
    while span < size {
        let step = span * 2;
        let mut start = 0usize;
        while start < size {
            let stop = start + span;
            let mut index = start;
            while index < stop {
                let left = values[index];
                let right = values[index + span];
                values[index] = left + right;
                values[index + span] = left - right;
                index += 1;
            }
            start += step;
        }
        span = step;
    }
    let scale = 1.0f32 / (size as f32).sqrt();
    for value in values.iter_mut() {
        *value *= scale;
    }
}

fn hadamard_apply_with_signs(values: &[f32], signs: &[f32]) -> Vec<f32> {
    let mut out: Vec<f32> = values
        .iter()
        .zip(signs.iter())
        .map(|(value, sign)| *value * *sign)
        .collect();
    normalized_hadamard_in_place(out.as_mut_slice());
    out
}

fn hadamard_invert_with_signs(values: &[f32], signs: &[f32]) -> Vec<f32> {
    let mut out = values.to_vec();
    normalized_hadamard_in_place(out.as_mut_slice());
    for (value, sign) in out.iter_mut().zip(signs.iter()) {
        *value *= *sign;
    }
    out
}

fn decode_float_sequence_bits(_py: &PyToken<'_>, bits: u64, label: &str) -> Result<Vec<f32>, u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("{label} must be a list or tuple"),
        ));
    };
    let type_id = unsafe { object_type_id(ptr) };
    if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("{label} must be a list or tuple"),
        ));
    }
    let elems = unsafe { seq_vec_ref(ptr) };
    let mut out = Vec::with_capacity(elems.len());
    for &elem_bits in elems.iter() {
        let elem = obj_from_bits(elem_bits);
        let value = if let Some(value) = to_f64(elem) {
            value as f32
        } else if let Some(value) = to_i64(elem) {
            value as f32
        } else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                &format!("{label} elements must be numbers"),
            ));
        };
        out.push(value);
    }
    Ok(out)
}

fn decode_u64_sequence_bits(_py: &PyToken<'_>, bits: u64, label: &str) -> Result<Vec<u64>, u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("{label} must be a list or tuple"),
        ));
    };
    let type_id = unsafe { object_type_id(ptr) };
    if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("{label} must be a list or tuple"),
        ));
    }
    Ok(unsafe { seq_vec_ref(ptr) }.to_vec())
}

fn require_attr_bits(
    _py: &PyToken<'_>,
    target_bits: u64,
    attr_name_bits: u64,
    attr_name: &str,
) -> Result<u64, u64> {
    let missing = crate::missing_bits(_py);
    let bits = crate::molt_getattr_builtin(target_bits, attr_name_bits, missing);
    if bits == missing {
        return Err(raise_exception::<u64>(
            _py,
            "AttributeError",
            &format!("object is missing required attribute {attr_name}"),
        ));
    }
    Ok(bits)
}

fn decode_i64_attr(
    _py: &PyToken<'_>,
    target_bits: u64,
    attr_name_bits: u64,
    attr_name: &str,
) -> Result<i64, u64> {
    let bits = require_attr_bits(_py, target_bits, attr_name_bits, attr_name)?;
    let Some(value) = to_i64(obj_from_bits(bits)) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("{attr_name} must be an integer"),
        ));
    };
    Ok(value)
}

fn decode_rotation_signs_from_codec(
    _py: &PyToken<'_>,
    codec_bits: u64,
    rotation_attr_bits: u64,
    signs_attr_bits: u64,
    rotation_name: &str,
) -> Result<Vec<f32>, u64> {
    let rotation_bits = require_attr_bits(_py, codec_bits, rotation_attr_bits, rotation_name)?;
    let signs_bits = require_attr_bits(_py, rotation_bits, signs_attr_bits, "signs")?;
    decode_float_sequence_bits(_py, signs_bits, "rotation signs")
}

fn decode_mask_value(
    mask: &(TensorRuntimeView, Vec<usize>, Vec<usize>),
    batch_index: usize,
    head_index: usize,
    query_index: usize,
    key_index: usize,
) -> f32 {
    let (mask_view, mask_shape, mask_strides) = mask;
    let b = if mask_shape[0] == 1 { 0 } else { batch_index };
    let h = if mask_shape[1] == 1 { 0 } else { head_index };
    let q = if mask_shape[2] == 1 { 0 } else { query_index };
    let k = if mask_shape[3] == 1 { 0 } else { key_index };
    let elem_index =
        b * mask_strides[0] + h * mask_strides[1] + q * mask_strides[2] + k * mask_strides[3];
    read_float_buffer_value(
        mask_view.buffer.data_view,
        mask_view.buffer.format,
        elem_index,
    )
}

fn read_float_buffer_value(view: ByteView, format: ScalarFormat, index: usize) -> f32 {
    match format {
        ScalarFormat::F32 => unsafe { (view.ptr.add(index * 4) as *const f32).read_unaligned() },
        ScalarFormat::F64 => unsafe {
            (view.ptr.add(index * 8) as *const f64).read_unaligned() as f32
        },
        ScalarFormat::I64 => unsafe {
            (view.ptr.add(index * 8) as *const i64).read_unaligned() as f32
        },
    }
}

fn read_tensor_value_4d(
    tensor: &TensorRuntimeView,
    _shape: &[usize],
    strides: &[usize],
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> f32 {
    let index = a * strides[0] + b * strides[1] + c * strides[2] + d * strides[3];
    read_float_buffer_value(tensor.buffer.data_view, tensor.buffer.format, index)
}

fn read_tensor_value_3d(
    tensor: &TensorRuntimeView,
    strides: &[usize],
    a: usize,
    b: usize,
    c: usize,
) -> f32 {
    let index = a * strides[0] + b * strides[1] + c * strides[2];
    read_float_buffer_value(tensor.buffer.data_view, tensor.buffer.format, index)
}

fn write_float_buffer_value(out: &mut [u8], format: ScalarFormat, index: usize, value: f32) {
    match format {
        ScalarFormat::F32 => unsafe {
            (out.as_mut_ptr().add(index * 4) as *mut f32).write_unaligned(value);
        },
        ScalarFormat::F64 => unsafe {
            (out.as_mut_ptr().add(index * 8) as *mut f64).write_unaligned(value as f64);
        },
        ScalarFormat::I64 => unsafe {
            (out.as_mut_ptr().add(index * 8) as *mut i64).write_unaligned(value as i64);
        },
    }
}

fn kv_head_index(
    query_heads: usize,
    kv_heads: usize,
    query_head_index: usize,
) -> Result<usize, ()> {
    if query_heads == kv_heads {
        return Ok(query_head_index);
    }
    if query_heads < kv_heads || !query_heads.is_multiple_of(kv_heads) {
        return Err(());
    }
    Ok(query_head_index / (query_heads / kv_heads))
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_scaled_dot_product_attention(
    q_bits: u64,
    k_bits: u64,
    v_bits: u64,
    mask_bits: u64,
    scale_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (q, q_shape) = match unsafe { tensor_runtime_view(_py, q_bits, "q") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let (k, k_shape) = match unsafe { tensor_runtime_view(_py, k_bits, "k") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let (v, v_shape) = match unsafe { tensor_runtime_view(_py, v_bits, "v") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if q_shape.len() != 4 || k_shape.len() != 4 || v_shape.len() != 4 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "scaled dot product attention requires rank-4 tensors",
            );
        }

        let batch = q_shape[0];
        let heads = q_shape[1];
        let seq_q = q_shape[2];
        let dim = q_shape[3];
        let seq_k = k_shape[2];
        let value_dim = v_shape[3];
        if k_shape[0] != batch
            || k_shape[1] != heads
            || k_shape[3] != dim
            || v_shape[0] != batch
            || v_shape[1] != heads
            || v_shape[2] != seq_k
        {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "scaled dot product attention shape mismatch",
            );
        }
        if q.buffer.format != ScalarFormat::F32
            || k.buffer.format != ScalarFormat::F32
            || v.buffer.format != ScalarFormat::F32
        {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "scaled dot product attention currently requires f32 tensors",
            );
        }

        let scale = if let Some(value) = to_f64(obj_from_bits(scale_bits)) {
            value as f32
        } else if let Some(value) = to_i64(obj_from_bits(scale_bits)) {
            value as f32
        } else {
            return raise_exception::<_>(_py, "TypeError", "scale must be a float");
        };

        let q_total = product(&q_shape);
        let k_total = product(&k_shape);
        let v_total = product(&v_shape);
        let Some(q_required) = q_total.checked_mul(ScalarFormat::F32.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "q shape overflow");
        };
        let Some(k_required) = k_total.checked_mul(ScalarFormat::F32.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "k shape overflow");
        };
        let Some(v_required) = v_total.checked_mul(ScalarFormat::F32.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "v shape overflow");
        };
        if q.buffer.data_view.len < q_required
            || k.buffer.data_view.len < k_required
            || v.buffer.data_view.len < v_required
        {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "scaled dot product attention input buffer is too small",
            );
        }

        let mask_info = if obj_from_bits(mask_bits).is_none() {
            None
        } else {
            let (mask, mask_shape) = match unsafe { tensor_runtime_view(_py, mask_bits, "mask") } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
            if mask_shape.len() != 4 || mask.buffer.format != ScalarFormat::F32 {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "scaled dot product attention mask must be a rank-4 f32 tensor",
                );
            }
            let expected = [batch, heads, seq_q, seq_k];
            for (dim_value, expected_value) in mask_shape.iter().zip(expected.iter()) {
                if *dim_value != 1 && *dim_value != *expected_value {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "scaled dot product attention mask shape mismatch",
                    );
                }
            }
            let mask_total = product(&mask_shape);
            let Some(mask_required) = mask_total.checked_mul(ScalarFormat::F32.itemsize()) else {
                return raise_exception::<_>(_py, "OverflowError", "mask shape overflow");
            };
            if mask.buffer.data_view.len < mask_required {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "scaled dot product attention mask buffer is too small",
                );
            }
            let mask_strides = strides(&mask_shape);
            Some((mask, mask_shape, mask_strides))
        };

        let Some(out_elems) = batch
            .checked_mul(heads)
            .and_then(|n| n.checked_mul(seq_q))
            .and_then(|n| n.checked_mul(value_dim))
        else {
            return raise_exception::<_>(_py, "OverflowError", "attention output shape overflow");
        };
        if std::env::var("MOLT_TRACE_GPU_SDPA").as_deref() == Ok("1") {
            eprintln!(
                "molt gpu sdpa batch={} heads={} seq_q={} seq_k={} dim={} value_dim={} out_elems={}",
                batch, heads, seq_q, seq_k, dim, value_dim, out_elems
            );
        }

        #[cfg(target_arch = "wasm32")]
        if requested_gpu_backend() == Some(GpuBackend::WebGpu) {
            let browser_result: Result<u64, u64> = (|| {
                let q_bytes =
                    bytes_like_view_to_webgpu_bytes(q.buffer.data_view, ScalarFormat::F32)
                        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                let k_bytes =
                    bytes_like_view_to_webgpu_bytes(k.buffer.data_view, ScalarFormat::F32)
                        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                let v_bytes =
                    bytes_like_view_to_webgpu_bytes(v.buffer.data_view, ScalarFormat::F32)
                        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                let (mask_bytes, has_mask_i32) =
                    if let Some((mask, mask_shape, mask_strides)) = &mask_info {
                        (
                            expand_attention_mask_to_webgpu_bytes(
                                mask,
                                mask_shape.as_slice(),
                                mask_strides.as_slice(),
                                batch,
                                heads,
                                seq_q,
                                seq_k,
                            )
                            .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?,
                            1i32,
                        )
                    } else {
                        (vec![0u8; 4], 0i32)
                    };
                let mut out_webgpu = vec![0u8; out_elems * 4];
                let batch_bytes = i32::try_from(batch)
                    .map_err(|_| raise_exception::<u64>(_py, "OverflowError", "batch exceeds i32"))?
                    .to_le_bytes();
                let heads_bytes = i32::try_from(heads)
                    .map_err(|_| raise_exception::<u64>(_py, "OverflowError", "heads exceeds i32"))?
                    .to_le_bytes();
                let seq_q_bytes = i32::try_from(seq_q)
                    .map_err(|_| raise_exception::<u64>(_py, "OverflowError", "seq_q exceeds i32"))?
                    .to_le_bytes();
                let seq_k_bytes = i32::try_from(seq_k)
                    .map_err(|_| raise_exception::<u64>(_py, "OverflowError", "seq_k exceeds i32"))?
                    .to_le_bytes();
                let dim_bytes = i32::try_from(dim)
                    .map_err(|_| raise_exception::<u64>(_py, "OverflowError", "dim exceeds i32"))?
                    .to_le_bytes();
                let value_dim_bytes = i32::try_from(value_dim)
                    .map_err(|_| {
                        raise_exception::<u64>(_py, "OverflowError", "value_dim exceeds i32")
                    })?
                    .to_le_bytes();
                let scale_bytes = scale.to_le_bytes();
                let has_mask_bytes = has_mask_i32.to_le_bytes();
                let workgroup_size = 64u32;
                let grid = if out_elems == 0 {
                    0
                } else {
                    u32::try_from(
                        (out_elems + workgroup_size as usize - 1) / workgroup_size as usize,
                    )
                    .map_err(|_| {
                        raise_exception::<u64>(_py, "OverflowError", "attention grid exceeds u32")
                    })?
                };
                let source =
                    render_webgpu_attention_source("scaled_dot_product_attention", workgroup_size);
                dispatch_browser_webgpu_bindings(
                    _py,
                    source.as_str(),
                    "scaled_dot_product_attention",
                    vec![
                        serde_json::json!({"binding": 0, "name": "q", "kind": "buffer", "access": "read", "ptr": q_bytes.as_ptr() as usize as u32, "len": q_bytes.len() as u32}),
                        serde_json::json!({"binding": 1, "name": "k", "kind": "buffer", "access": "read", "ptr": k_bytes.as_ptr() as usize as u32, "len": k_bytes.len() as u32}),
                        serde_json::json!({"binding": 2, "name": "v", "kind": "buffer", "access": "read", "ptr": v_bytes.as_ptr() as usize as u32, "len": v_bytes.len() as u32}),
                        serde_json::json!({"binding": 3, "name": "out", "kind": "buffer", "access": "read_write", "ptr": out_webgpu.as_mut_ptr() as usize as u32, "len": out_webgpu.len() as u32}),
                        serde_json::json!({"binding": 4, "name": "mask", "kind": "buffer", "access": "read", "ptr": mask_bytes.as_ptr() as usize as u32, "len": mask_bytes.len() as u32}),
                        serde_json::json!({"binding": 5, "name": "batch", "kind": "scalar", "access": "read", "ptr": batch_bytes.as_ptr() as usize as u32, "len": batch_bytes.len() as u32}),
                        serde_json::json!({"binding": 6, "name": "heads", "kind": "scalar", "access": "read", "ptr": heads_bytes.as_ptr() as usize as u32, "len": heads_bytes.len() as u32}),
                        serde_json::json!({"binding": 7, "name": "seq_q", "kind": "scalar", "access": "read", "ptr": seq_q_bytes.as_ptr() as usize as u32, "len": seq_q_bytes.len() as u32}),
                        serde_json::json!({"binding": 8, "name": "seq_k", "kind": "scalar", "access": "read", "ptr": seq_k_bytes.as_ptr() as usize as u32, "len": seq_k_bytes.len() as u32}),
                        serde_json::json!({"binding": 9, "name": "dim", "kind": "scalar", "access": "read", "ptr": dim_bytes.as_ptr() as usize as u32, "len": dim_bytes.len() as u32}),
                        serde_json::json!({"binding": 10, "name": "value_dim", "kind": "scalar", "access": "read", "ptr": value_dim_bytes.as_ptr() as usize as u32, "len": value_dim_bytes.len() as u32}),
                        serde_json::json!({"binding": 11, "name": "scale", "kind": "scalar", "access": "read", "ptr": scale_bytes.as_ptr() as usize as u32, "len": scale_bytes.len() as u32}),
                        serde_json::json!({"binding": 12, "name": "has_mask", "kind": "scalar", "access": "read", "ptr": has_mask_bytes.as_ptr() as usize as u32, "len": has_mask_bytes.len() as u32}),
                    ],
                    grid,
                    workgroup_size,
                )?;
                let data_ptr = alloc_bytearray(_py, out_webgpu.as_slice());
                if data_ptr.is_null() {
                    return Err(MoltObject::none().bits());
                }
                let data_bits = MoltObject::from_ptr(data_ptr).bits();
                let format_bits = alloc_string_bits(_py, b"f")?;
                let shape_bits =
                    alloc_tuple_bits_from_usize(_py, &[batch, heads, seq_q, value_dim])?;
                let tensor_bits = match unsafe {
                    build_tensor_from_data_bits(
                        _py,
                        q.class_bits,
                        q.buffer.class_bits,
                        data_bits,
                        crate::builtins::classes::builtin_classes(_py).float,
                        out_elems,
                        format_bits,
                        ScalarFormat::F32.itemsize(),
                        shape_bits,
                        crate::builtins::classes::builtin_classes(_py).float,
                    )
                } {
                    Ok(bits) => bits,
                    Err(bits) => bits,
                };
                crate::dec_ref_bits(_py, data_bits);
                crate::dec_ref_bits(_py, format_bits);
                crate::dec_ref_bits(_py, shape_bits);
                Ok(tensor_bits)
            })();
            return match browser_result {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }

        let mut out = vec![0u8; out_elems * ScalarFormat::F32.itemsize()];
        let q_stride = seq_q * dim;
        let k_stride = seq_k * dim;
        let v_stride = seq_k * value_dim;
        let out_stride = seq_q * value_dim;
        for b in 0..batch {
            for h in 0..heads {
                let q_batch_off = (b * heads + h) * q_stride;
                let k_batch_off = (b * heads + h) * k_stride;
                let v_batch_off = (b * heads + h) * v_stride;
                let out_batch_off = (b * heads + h) * out_stride;
                for q_idx in 0..seq_q {
                    let q_base = q_batch_off + q_idx * dim;
                    let mut max_score = f32::NEG_INFINITY;
                    for k_idx in 0..seq_k {
                        let k_base = k_batch_off + k_idx * dim;
                        let mut score = 0.0f32;
                        for d in 0..dim {
                            let qv = unsafe {
                                (q.buffer.data_view.ptr.add((q_base + d) * 4) as *const f32)
                                    .read_unaligned()
                            };
                            let kv = unsafe {
                                (k.buffer.data_view.ptr.add((k_base + d) * 4) as *const f32)
                                    .read_unaligned()
                            };
                            score += qv * kv;
                        }
                        score *= scale;
                        if let Some((mask, mask_shape, mask_strides)) = &mask_info {
                            let mask_index = (if mask_shape[0] == 1 {
                                0
                            } else {
                                b * mask_strides[0]
                            }) + (if mask_shape[1] == 1 {
                                0
                            } else {
                                h * mask_strides[1]
                            }) + (if mask_shape[2] == 1 {
                                0
                            } else {
                                q_idx * mask_strides[2]
                            }) + (if mask_shape[3] == 1 {
                                0
                            } else {
                                k_idx * mask_strides[3]
                            });
                            score += unsafe {
                                (mask.buffer.data_view.ptr.add(mask_index * 4) as *const f32)
                                    .read_unaligned()
                            };
                        }
                        if score > max_score {
                            max_score = score;
                        }
                    }

                    let mut sum = 0.0f32;
                    let mut acc = vec![0.0f32; value_dim];
                    for k_idx in 0..seq_k {
                        let k_base = k_batch_off + k_idx * dim;
                        let mut score = 0.0f32;
                        for d in 0..dim {
                            let qv = unsafe {
                                (q.buffer.data_view.ptr.add((q_base + d) * 4) as *const f32)
                                    .read_unaligned()
                            };
                            let kv = unsafe {
                                (k.buffer.data_view.ptr.add((k_base + d) * 4) as *const f32)
                                    .read_unaligned()
                            };
                            score += qv * kv;
                        }
                        score *= scale;
                        if let Some((mask, mask_shape, mask_strides)) = &mask_info {
                            let mask_index = (if mask_shape[0] == 1 {
                                0
                            } else {
                                b * mask_strides[0]
                            }) + (if mask_shape[1] == 1 {
                                0
                            } else {
                                h * mask_strides[1]
                            }) + (if mask_shape[2] == 1 {
                                0
                            } else {
                                q_idx * mask_strides[2]
                            }) + (if mask_shape[3] == 1 {
                                0
                            } else {
                                k_idx * mask_strides[3]
                            });
                            score += unsafe {
                                (mask.buffer.data_view.ptr.add(mask_index * 4) as *const f32)
                                    .read_unaligned()
                            };
                        }
                        let weight = (score - max_score).exp();
                        sum += weight;
                        let v_base = v_batch_off + k_idx * value_dim;
                        for d in 0..value_dim {
                            let vv = unsafe {
                                (v.buffer.data_view.ptr.add((v_base + d) * 4) as *const f32)
                                    .read_unaligned()
                            };
                            acc[d] += weight * vv;
                        }
                    }

                    let inv_sum = if sum != 0.0 { 1.0 / sum } else { 0.0 };
                    let out_base = out_batch_off + q_idx * value_dim;
                    for d in 0..value_dim {
                        unsafe {
                            (out.as_mut_ptr().add((out_base + d) * 4) as *mut f32)
                                .write_unaligned(acc[d] * inv_sum);
                        }
                    }
                }
            }
        }

        let data_ptr = alloc_bytearray(_py, out.as_slice());
        if data_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let data_bits = MoltObject::from_ptr(data_ptr).bits();
        let format_bits = match alloc_string_bits(_py, b"f") {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, data_bits);
                return bits;
            }
        };
        let shape_bits = match alloc_tuple_bits_from_usize(_py, &[batch, heads, seq_q, value_dim]) {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, data_bits);
                crate::dec_ref_bits(_py, format_bits);
                return bits;
            }
        };
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                q.class_bits,
                q.buffer.class_bits,
                data_bits,
                crate::builtins::classes::builtin_classes(_py).float,
                out_elems,
                format_bits,
                ScalarFormat::F32.itemsize(),
                shape_bits,
                q.dtype_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, data_bits);
        crate::dec_ref_bits(_py, format_bits);
        crate::dec_ref_bits(_py, shape_bits);
        tensor_bits
    })
}

#[unsafe(no_mangle)]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_turboquant_attention_packed(
    q_bits: u64,
    k_bits: u64,
    v_bits: u64,
    mask_bits: u64,
    scale_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let (q, q_shape) = match unsafe { tensor_runtime_view(_py, q_bits, "q") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if q_shape.len() != 4 || !matches!(q.buffer.format, ScalarFormat::F32 | ScalarFormat::F64) {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "turboquant attention expects a rank-4 float query tensor",
            );
        }
        let batch = q_shape[0];
        let query_heads = q_shape[1];
        let query_seq = q_shape[2];
        let dim = q_shape[3];
        if dim == 0 || (dim & (dim - 1)) != 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "turboquant attention currently requires a power-of-two head dimension",
            );
        }

        #[cfg(not(feature = "molt_gpu_cuda"))]
        if requested_gpu_backend() == Some(GpuBackend::Cuda) {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "cuda gpu backend requested but runtime was built without molt_gpu_cuda",
            );
        }
        #[cfg(feature = "molt_gpu_cuda")]
        if requested_gpu_backend() == Some(GpuBackend::Cuda) {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "cuda turboquant attention path is not yet implemented",
            );
        }
        #[cfg(not(feature = "molt_gpu_hip"))]
        if requested_gpu_backend() == Some(GpuBackend::Hip) {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "hip gpu backend requested but runtime was built without molt_gpu_hip",
            );
        }
        #[cfg(feature = "molt_gpu_hip")]
        if requested_gpu_backend() == Some(GpuBackend::Hip) {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "hip turboquant attention path is not yet implemented",
            );
        }

        let missing = crate::missing_bits(_py);
        let Some(kv_cache_name) = attr_name_bits_from_bytes(_py, b"_kv_cache") else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "failed to intern _kv_cache attribute name",
            );
        };
        let Some(role_name) = attr_name_bits_from_bytes(_py, b"_kv_role") else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "failed to intern _kv_role attribute name",
            );
        };
        let Some(codec_name) = attr_name_bits_from_bytes(_py, b"codec") else {
            return raise_exception::<_>(_py, "RuntimeError", "failed to intern codec attribute");
        };
        let Some(runtime_mse_signs_name) = attr_name_bits_from_bytes(_py, b"_runtime_mse_signs")
        else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "failed to intern _runtime_mse_signs attribute",
            );
        };
        let Some(runtime_qjl_signs_name) = attr_name_bits_from_bytes(_py, b"_runtime_qjl_signs")
        else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "failed to intern _runtime_qjl_signs attribute",
            );
        };
        let Some(mse_rotation_name) = attr_name_bits_from_bytes(_py, b"mse_rotation") else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "failed to intern mse_rotation attribute",
            );
        };
        let Some(qjl_rotation_name) = attr_name_bits_from_bytes(_py, b"qjl_rotation") else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "failed to intern qjl_rotation attribute",
            );
        };
        let Some(signs_name) = attr_name_bits_from_bytes(_py, b"signs") else {
            return raise_exception::<_>(_py, "RuntimeError", "failed to intern signs attribute");
        };
        let Some(key_vectors_name) = attr_name_bits_from_bytes(_py, b"_key_vectors") else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "failed to intern _key_vectors attribute",
            );
        };
        let Some(value_vectors_name) = attr_name_bits_from_bytes(_py, b"_value_vectors") else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "failed to intern _value_vectors attribute",
            );
        };
        let Some(runtime_key_mse_rows_name) =
            attr_name_bits_from_bytes(_py, b"_runtime_key_mse_weight_rows")
        else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "failed to intern _runtime_key_mse_weight_rows attribute",
            );
        };
        let Some(runtime_key_sign_rows_name) =
            attr_name_bits_from_bytes(_py, b"_runtime_key_residual_sign_rows")
        else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "failed to intern _runtime_key_residual_sign_rows attribute",
            );
        };
        let Some(runtime_key_scale_rows_name) =
            attr_name_bits_from_bytes(_py, b"_runtime_key_residual_scale_rows")
        else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "failed to intern _runtime_key_residual_scale_rows attribute",
            );
        };
        let Some(runtime_value_rows_name) = attr_name_bits_from_bytes(_py, b"_runtime_value_rows")
        else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "failed to intern _runtime_value_rows attribute",
            );
        };
        let Some(heads_name) = attr_name_bits_from_bytes(_py, b"_heads") else {
            return raise_exception::<_>(_py, "RuntimeError", "failed to intern _heads attribute");
        };
        let Some(batch_name) = attr_name_bits_from_bytes(_py, b"_batch") else {
            return raise_exception::<_>(_py, "RuntimeError", "failed to intern _batch attribute");
        };
        let Some(mse_weights_name) = attr_name_bits_from_bytes(_py, b"mse_weights") else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "failed to intern mse_weights attribute",
            );
        };
        let Some(residual_signs_name) = attr_name_bits_from_bytes(_py, b"residual_signs") else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "failed to intern residual_signs attribute",
            );
        };
        let Some(residual_scale_name) = attr_name_bits_from_bytes(_py, b"residual_scale") else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "failed to intern residual_scale attribute",
            );
        };

        let k_cache_bits = crate::molt_getattr_builtin(k_bits, kv_cache_name, missing);
        let v_cache_bits = crate::molt_getattr_builtin(v_bits, kv_cache_name, missing);
        if k_cache_bits == missing || v_cache_bits == missing || k_cache_bits != v_cache_bits {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "turboquant attention expects matching key/value cache views",
            );
        }

        let k_role_bits = crate::molt_getattr_builtin(k_bits, role_name, missing);
        let v_role_bits = crate::molt_getattr_builtin(v_bits, role_name, missing);
        let Some(k_role) = string_obj_to_owned(obj_from_bits(k_role_bits)) else {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "turboquant attention key view is missing _kv_role",
            );
        };
        let Some(v_role) = string_obj_to_owned(obj_from_bits(v_role_bits)) else {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "turboquant attention value view is missing _kv_role",
            );
        };
        if k_role != "key" || v_role != "value" {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "turboquant attention expects key/value cache view roles",
            );
        }
        let runtime_mse_signs_bits =
            crate::molt_getattr_builtin(k_cache_bits, runtime_mse_signs_name, missing);
        let runtime_qjl_signs_bits =
            crate::molt_getattr_builtin(k_cache_bits, runtime_qjl_signs_name, missing);
        let (mse_signs, qjl_signs) =
            if runtime_mse_signs_bits != missing && runtime_qjl_signs_bits != missing {
                let (mse_signs_tensor, mse_signs_shape) = match unsafe {
                    tensor_runtime_view(_py, runtime_mse_signs_bits, "_runtime_mse_signs")
                } {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                let (qjl_signs_tensor, qjl_signs_shape) = match unsafe {
                    tensor_runtime_view(_py, runtime_qjl_signs_bits, "_runtime_qjl_signs")
                } {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                if mse_signs_shape != vec![dim] || qjl_signs_shape != vec![dim] {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "turboquant runtime sign shadow tensors do not match query head dimension",
                    );
                }
                let mut mse = vec![0.0f32; dim];
                let mut qjl = vec![0.0f32; dim];
                for dim_index in 0..dim {
                    mse[dim_index] = read_float_buffer_value(
                        mse_signs_tensor.buffer.data_view,
                        mse_signs_tensor.buffer.format,
                        dim_index,
                    );
                    qjl[dim_index] = read_float_buffer_value(
                        qjl_signs_tensor.buffer.data_view,
                        qjl_signs_tensor.buffer.format,
                        dim_index,
                    );
                }
                (mse, qjl)
            } else {
                let codec_bits = require_attr_bits(_py, k_cache_bits, codec_name, "codec")
                    .unwrap_or_else(|bits| bits);
                let mse = match decode_rotation_signs_from_codec(
                    _py,
                    codec_bits,
                    mse_rotation_name,
                    signs_name,
                    "mse_rotation",
                ) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                let qjl = match decode_rotation_signs_from_codec(
                    _py,
                    codec_bits,
                    qjl_rotation_name,
                    signs_name,
                    "qjl_rotation",
                ) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                (mse, qjl)
            };
        if mse_signs.len() != dim || qjl_signs.len() != dim {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "turboquant attention rotation signs do not match query head dimension",
            );
        }

        let kv_heads = match decode_i64_attr(_py, k_cache_bits, heads_name, "_heads") {
            Ok(value) => value as usize,
            Err(bits) => return bits,
        };
        let cache_batch = match decode_i64_attr(_py, k_cache_bits, batch_name, "_batch") {
            Ok(value) => value as usize,
            Err(bits) => return bits,
        };
        if cache_batch != batch {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "turboquant attention query batch must match cache batch",
            );
        }
        if query_heads < kv_heads || query_heads % kv_heads != 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "turboquant attention query heads are incompatible with cache heads",
            );
        }

        let scale = if let Some(value) = to_f64(obj_from_bits(scale_bits)) {
            value as f32
        } else if let Some(value) = to_i64(obj_from_bits(scale_bits)) {
            value as f32
        } else {
            return raise_exception::<_>(_py, "TypeError", "scale must be a float");
        };

        let mask_info = if obj_from_bits(mask_bits).is_none() {
            None
        } else {
            let (mask, mask_shape) = match unsafe { tensor_runtime_view(_py, mask_bits, "mask") } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
            if mask_shape.len() != 4
                || !matches!(mask.buffer.format, ScalarFormat::F32 | ScalarFormat::F64)
            {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "turboquant attention mask must be a rank-4 float tensor",
                );
            }
            let expected = [batch, query_heads, query_seq, usize::MAX];
            if mask_shape[0] != 1 && mask_shape[0] != expected[0] {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "turboquant attention mask batch mismatch",
                );
            }
            if mask_shape[1] != 1 && mask_shape[1] != expected[1] {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "turboquant attention mask head mismatch",
                );
            }
            if mask_shape[2] != 1 && mask_shape[2] != expected[2] {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "turboquant attention mask query mismatch",
                );
            }
            Some((mask, mask_shape.clone(), strides(&mask_shape)))
        };

        let runtime_key_mse_bits =
            crate::molt_getattr_builtin(k_cache_bits, runtime_key_mse_rows_name, missing);
        let runtime_key_sign_bits =
            crate::molt_getattr_builtin(k_cache_bits, runtime_key_sign_rows_name, missing);
        let runtime_key_scale_bits =
            crate::molt_getattr_builtin(k_cache_bits, runtime_key_scale_rows_name, missing);
        let runtime_value_bits =
            crate::molt_getattr_builtin(k_cache_bits, runtime_value_rows_name, missing);

        if runtime_key_mse_bits != missing
            && runtime_key_sign_bits != missing
            && runtime_key_scale_bits != missing
            && runtime_value_bits != missing
        {
            #[cfg(all(not(target_arch = "wasm32"), feature = "molt_gpu_webgpu"))]
            if requested_gpu_backend() == Some(GpuBackend::WebGpu)
                && runtime_mse_signs_bits != missing
                && runtime_qjl_signs_bits != missing
            {
                if trace_gpu_backend_enabled() {
                    eprintln!("[molt gpu backend] webgpu turboquant_attention_packed");
                }
                let native_webgpu_result: Result<u64, u64> = (|| {
                    let (mse_signs_tensor, mse_signs_shape) = unsafe {
                        tensor_runtime_view(_py, runtime_mse_signs_bits, "_runtime_mse_signs")
                    }
                    .map_err(|bits| bits)?;
                    let (qjl_signs_tensor, qjl_signs_shape) = unsafe {
                        tensor_runtime_view(_py, runtime_qjl_signs_bits, "_runtime_qjl_signs")
                    }
                    .map_err(|bits| bits)?;
                    let (key_mse, key_mse_shape) = unsafe {
                        tensor_runtime_view(
                            _py,
                            runtime_key_mse_bits,
                            "_runtime_key_mse_weight_rows",
                        )
                    }
                    .map_err(|bits| bits)?;
                    let (key_sign, key_sign_shape) = unsafe {
                        tensor_runtime_view(
                            _py,
                            runtime_key_sign_bits,
                            "_runtime_key_residual_sign_rows",
                        )
                    }
                    .map_err(|bits| bits)?;
                    let (key_scale, key_scale_shape) = unsafe {
                        tensor_runtime_view(
                            _py,
                            runtime_key_scale_bits,
                            "_runtime_key_residual_scale_rows",
                        )
                    }
                    .map_err(|bits| bits)?;
                    let (value_rows, value_rows_shape) = unsafe {
                        tensor_runtime_view(_py, runtime_value_bits, "_runtime_value_rows")
                    }
                    .map_err(|bits| bits)?;
                    if mse_signs_shape != vec![dim]
                        || qjl_signs_shape != vec![dim]
                        || key_mse_shape.len() != 4
                        || key_sign_shape.len() != 4
                        || key_scale_shape.len() != 3
                        || value_rows_shape.len() != 4
                    {
                        return Err(raise_exception::<u64>(
                            _py,
                            "ValueError",
                            "turboquant native webgpu shadow tensor shape mismatch",
                        ));
                    }
                    let seq_k = key_mse_shape[2];

                    let q_total = batch * query_heads * query_seq * dim;
                    let mut rotated_q_bytes = vec![0u8; q_total * 4];
                    let mut query_sketch_bytes = vec![0u8; q_total * 4];
                    let mut query_row = vec![0.0f32; dim];
                    let mse_signs: Vec<f32> = (0..dim)
                        .map(|index| {
                            read_float_buffer_value(
                                mse_signs_tensor.buffer.data_view,
                                mse_signs_tensor.buffer.format,
                                index,
                            )
                        })
                        .collect();
                    let qjl_signs: Vec<f32> = (0..dim)
                        .map(|index| {
                            read_float_buffer_value(
                                qjl_signs_tensor.buffer.data_view,
                                qjl_signs_tensor.buffer.format,
                                index,
                            )
                        })
                        .collect();
                    let q_stride = query_seq * dim;
                    for batch_index in 0..batch {
                        for query_head_index in 0..query_heads {
                            for query_index in 0..query_seq {
                                let q_base = ((batch_index * query_heads + query_head_index)
                                    * q_stride)
                                    + query_index * dim;
                                for dim_index in 0..dim {
                                    query_row[dim_index] = read_float_buffer_value(
                                        q.buffer.data_view,
                                        q.buffer.format,
                                        q_base + dim_index,
                                    );
                                }
                                let rotated_q = hadamard_apply_with_signs(
                                    query_row.as_slice(),
                                    mse_signs.as_slice(),
                                );
                                let query_sketch = hadamard_apply_with_signs(
                                    query_row.as_slice(),
                                    qjl_signs.as_slice(),
                                );
                                for dim_index in 0..dim {
                                    write_float_buffer_value(
                                        rotated_q_bytes.as_mut_slice(),
                                        ScalarFormat::F32,
                                        q_base + dim_index,
                                        rotated_q[dim_index],
                                    );
                                    write_float_buffer_value(
                                        query_sketch_bytes.as_mut_slice(),
                                        ScalarFormat::F32,
                                        q_base + dim_index,
                                        query_sketch[dim_index],
                                    );
                                }
                            }
                        }
                    }

                    let query_pair_len = q_total * 4;
                    let mut query_pair_bytes = vec![0u8; query_pair_len * 2];
                    query_pair_bytes[..query_pair_len].copy_from_slice(rotated_q_bytes.as_slice());
                    query_pair_bytes[query_pair_len..]
                        .copy_from_slice(query_sketch_bytes.as_slice());
                    let key_mse_bytes = bytes_like_view_to_webgpu_bytes(
                        key_mse.buffer.data_view,
                        key_mse.buffer.format,
                    )
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                    let key_sign_bytes = bytes_like_view_to_webgpu_bytes(
                        key_sign.buffer.data_view,
                        key_sign.buffer.format,
                    )
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                    let key_scale_bytes = bytes_like_view_to_webgpu_bytes(
                        key_scale.buffer.data_view,
                        key_scale.buffer.format,
                    )
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                    let value_rows_bytes = bytes_like_view_to_webgpu_bytes(
                        value_rows.buffer.data_view,
                        value_rows.buffer.format,
                    )
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                    let (mask_bytes, has_mask_i32): (Vec<u8>, i32) =
                        if let Some((mask, mask_shape, mask_strides)) = &mask_info {
                            (
                                expand_attention_mask_to_webgpu_bytes(
                                    mask,
                                    mask_shape.as_slice(),
                                    mask_strides.as_slice(),
                                    batch,
                                    query_heads,
                                    query_seq,
                                    seq_k,
                                )
                                .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?,
                                1i32,
                            )
                        } else {
                            (vec![0u8; 4], 0i32)
                        };
                    let out_elems = batch * query_heads * query_seq * dim;
                    let mut out_gpu = vec![0u8; out_elems * 4];
                    let batch_bytes = i32::try_from(batch)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "batch exceeds i32")
                        })?
                        .to_le_bytes();
                    let query_heads_bytes = i32::try_from(query_heads)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "query_heads exceeds i32")
                        })?
                        .to_le_bytes();
                    let kv_heads_bytes = i32::try_from(kv_heads)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "kv_heads exceeds i32")
                        })?
                        .to_le_bytes();
                    let seq_q_bytes = i32::try_from(query_seq)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "seq_q exceeds i32")
                        })?
                        .to_le_bytes();
                    let seq_k_bytes = i32::try_from(seq_k)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "seq_k exceeds i32")
                        })?
                        .to_le_bytes();
                    let dim_bytes = i32::try_from(dim)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "dim exceeds i32")
                        })?
                        .to_le_bytes();
                    let scale_bytes = scale.to_le_bytes();
                    let has_mask_bytes = has_mask_i32.to_le_bytes();
                    let mut params_bytes = Vec::with_capacity(8 * 4);
                    params_bytes.extend_from_slice(&batch_bytes);
                    params_bytes.extend_from_slice(&query_heads_bytes);
                    params_bytes.extend_from_slice(&kv_heads_bytes);
                    params_bytes.extend_from_slice(&seq_q_bytes);
                    params_bytes.extend_from_slice(&seq_k_bytes);
                    params_bytes.extend_from_slice(&dim_bytes);
                    params_bytes.extend_from_slice(&scale_bytes);
                    params_bytes.extend_from_slice(&has_mask_bytes);
                    let workgroup_size = 64u32;
                    let grid = if out_elems == 0 {
                        0
                    } else {
                        u32::try_from(
                            (out_elems + workgroup_size as usize - 1) / workgroup_size as usize,
                        )
                        .map_err(|_| {
                            raise_exception::<u64>(
                                _py,
                                "OverflowError",
                                "turboquant attention grid exceeds u32",
                            )
                        })?
                    };

                    let device = RuntimeWebGpuDevice::new()
                        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                    let pipeline = device
                        .compile_pipeline(
                            "turboquant_attention_packed",
                            &render_webgpu_turboquant_attention_source(
                                "turboquant_attention_packed",
                                workgroup_size,
                            ),
                        )
                        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;

                    let buffer_payloads: [&[u8]; 8] = [
                        query_pair_bytes.as_slice(),
                        key_mse_bytes.as_slice(),
                        key_sign_bytes.as_slice(),
                        key_scale_bytes.as_slice(),
                        value_rows_bytes.as_slice(),
                        out_gpu.as_slice(),
                        mask_bytes.as_slice(),
                        params_bytes.as_slice(),
                    ];
                    let mut owned_buffers: Vec<wgpu::Buffer> = Vec::new();
                    for data in buffer_payloads {
                        let (_, gpu_buf) = device.alloc_buffer(data.len().max(1));
                        if !data.is_empty() {
                            device.copy_to_buffer(&gpu_buf, data);
                        }
                        owned_buffers.push(gpu_buf);
                    }
                    let refs: Vec<&wgpu::Buffer> = owned_buffers.iter().collect();
                    device
                        .dispatch(&pipeline, grid, &refs)
                        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                    out_gpu = device
                        .copy_from_buffer(&owned_buffers[5], out_elems * 4)
                        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;

                    let data_ptr = alloc_bytearray(_py, out_gpu.as_slice());
                    if data_ptr.is_null() {
                        return Err(MoltObject::none().bits());
                    }
                    let data_bits = MoltObject::from_ptr(data_ptr).bits();
                    let format_bits = match alloc_string_bits(
                        _py,
                        match q.buffer.format {
                            ScalarFormat::F32 => b"f",
                            ScalarFormat::F64 => b"d",
                            ScalarFormat::I64 => b"q",
                        },
                    ) {
                        Ok(bits) => bits,
                        Err(bits) => {
                            crate::dec_ref_bits(_py, data_bits);
                            return Err(bits);
                        }
                    };
                    let shape_bits =
                        alloc_tuple_bits_from_usize(_py, &[batch, query_heads, query_seq, dim])?;
                    let out_format = q.buffer.format;
                    let rebuilt = rebuild_host_bytes_from_gpu32_output(
                        _py,
                        out_format,
                        out_elems,
                        out_gpu.as_slice(),
                    )?;
                    let rebuilt_ptr = alloc_bytearray(_py, rebuilt.as_slice());
                    if rebuilt_ptr.is_null() {
                        crate::dec_ref_bits(_py, data_bits);
                        crate::dec_ref_bits(_py, format_bits);
                        crate::dec_ref_bits(_py, shape_bits);
                        return Err(MoltObject::none().bits());
                    }
                    let rebuilt_bits = MoltObject::from_ptr(rebuilt_ptr).bits();
                    let tensor_bits = match unsafe {
                        build_tensor_from_data_bits(
                            _py,
                            q.class_bits,
                            q.buffer.class_bits,
                            rebuilt_bits,
                            crate::builtins::classes::builtin_classes(_py).float,
                            out_elems,
                            format_bits,
                            out_format.itemsize(),
                            shape_bits,
                            q.dtype_bits,
                        )
                    } {
                        Ok(bits) => bits,
                        Err(bits) => bits,
                    };
                    crate::dec_ref_bits(_py, data_bits);
                    crate::dec_ref_bits(_py, rebuilt_bits);
                    crate::dec_ref_bits(_py, format_bits);
                    crate::dec_ref_bits(_py, shape_bits);
                    Ok(tensor_bits)
                })();
                return match native_webgpu_result {
                    Ok(bits) => bits,
                    Err(bits) => bits,
                };
            }

            #[cfg(all(target_os = "macos", feature = "molt_gpu_metal"))]
            if requested_gpu_backend() == Some(GpuBackend::Metal)
                && runtime_mse_signs_bits != missing
                && runtime_qjl_signs_bits != missing
            {
                if trace_gpu_backend_enabled() {
                    eprintln!("[molt gpu backend] metal turboquant_attention_packed");
                }
                let metal_result: Result<u64, u64> = (|| {
                    let (mse_signs_tensor, mse_signs_shape) = unsafe {
                        tensor_runtime_view(_py, runtime_mse_signs_bits, "_runtime_mse_signs")
                    }
                    .map_err(|bits| bits)?;
                    let (qjl_signs_tensor, qjl_signs_shape) = unsafe {
                        tensor_runtime_view(_py, runtime_qjl_signs_bits, "_runtime_qjl_signs")
                    }
                    .map_err(|bits| bits)?;
                    let (key_mse, key_mse_shape) = unsafe {
                        tensor_runtime_view(
                            _py,
                            runtime_key_mse_bits,
                            "_runtime_key_mse_weight_rows",
                        )
                    }
                    .map_err(|bits| bits)?;
                    let (key_sign, key_sign_shape) = unsafe {
                        tensor_runtime_view(
                            _py,
                            runtime_key_sign_bits,
                            "_runtime_key_residual_sign_rows",
                        )
                    }
                    .map_err(|bits| bits)?;
                    let (key_scale, key_scale_shape) = unsafe {
                        tensor_runtime_view(
                            _py,
                            runtime_key_scale_bits,
                            "_runtime_key_residual_scale_rows",
                        )
                    }
                    .map_err(|bits| bits)?;
                    let (value_rows, value_rows_shape) = unsafe {
                        tensor_runtime_view(_py, runtime_value_bits, "_runtime_value_rows")
                    }
                    .map_err(|bits| bits)?;
                    if mse_signs_shape != vec![dim]
                        || qjl_signs_shape != vec![dim]
                        || key_mse_shape.len() != 4
                        || key_sign_shape.len() != 4
                        || key_scale_shape.len() != 3
                        || value_rows_shape.len() != 4
                    {
                        return Err(raise_exception::<u64>(
                            _py,
                            "ValueError",
                            "turboquant metal shadow tensor shape mismatch",
                        ));
                    }
                    let seq_k = key_mse_shape[2];

                    let q_total = batch * query_heads * query_seq * dim;
                    let mut rotated_q_bytes = vec![0u8; q_total * 4];
                    let mut query_sketch_bytes = vec![0u8; q_total * 4];
                    let mut query_row = vec![0.0f32; dim];
                    let mse_signs: Vec<f32> = (0..dim)
                        .map(|index| {
                            read_float_buffer_value(
                                mse_signs_tensor.buffer.data_view,
                                mse_signs_tensor.buffer.format,
                                index,
                            )
                        })
                        .collect();
                    let qjl_signs: Vec<f32> = (0..dim)
                        .map(|index| {
                            read_float_buffer_value(
                                qjl_signs_tensor.buffer.data_view,
                                qjl_signs_tensor.buffer.format,
                                index,
                            )
                        })
                        .collect();
                    let q_stride = query_seq * dim;
                    for batch_index in 0..batch {
                        for query_head_index in 0..query_heads {
                            for query_index in 0..query_seq {
                                let q_base = ((batch_index * query_heads + query_head_index)
                                    * q_stride)
                                    + query_index * dim;
                                for dim_index in 0..dim {
                                    query_row[dim_index] = read_float_buffer_value(
                                        q.buffer.data_view,
                                        q.buffer.format,
                                        q_base + dim_index,
                                    );
                                }
                                let rotated_q = hadamard_apply_with_signs(
                                    query_row.as_slice(),
                                    mse_signs.as_slice(),
                                );
                                let query_sketch = hadamard_apply_with_signs(
                                    query_row.as_slice(),
                                    qjl_signs.as_slice(),
                                );
                                for dim_index in 0..dim {
                                    write_float_buffer_value(
                                        rotated_q_bytes.as_mut_slice(),
                                        ScalarFormat::F32,
                                        q_base + dim_index,
                                        rotated_q[dim_index],
                                    );
                                    write_float_buffer_value(
                                        query_sketch_bytes.as_mut_slice(),
                                        ScalarFormat::F32,
                                        q_base + dim_index,
                                        query_sketch[dim_index],
                                    );
                                }
                            }
                        }
                    }

                    let key_mse_bytes = bytes_like_view_to_webgpu_bytes(
                        key_mse.buffer.data_view,
                        key_mse.buffer.format,
                    )
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                    let key_sign_bytes = bytes_like_view_to_webgpu_bytes(
                        key_sign.buffer.data_view,
                        key_sign.buffer.format,
                    )
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                    let key_scale_bytes = bytes_like_view_to_webgpu_bytes(
                        key_scale.buffer.data_view,
                        key_scale.buffer.format,
                    )
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                    let value_rows_bytes = bytes_like_view_to_webgpu_bytes(
                        value_rows.buffer.data_view,
                        value_rows.buffer.format,
                    )
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                    let q_rotated_bytes = rotated_q_bytes;
                    let q_sketch_bytes = query_sketch_bytes;
                    let (mask_bytes, has_mask_i32): (Vec<u8>, i32) =
                        if let Some((mask, mask_shape, mask_strides)) = &mask_info {
                            (
                                expand_attention_mask_to_webgpu_bytes(
                                    mask,
                                    mask_shape.as_slice(),
                                    mask_strides.as_slice(),
                                    batch,
                                    query_heads,
                                    query_seq,
                                    seq_k,
                                )
                                .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?,
                                1i32,
                            )
                        } else {
                            (vec![0u8; 4], 0i32)
                        };
                    let out_elems = batch * query_heads * query_seq * dim;
                    let mut out_gpu = vec![0u8; out_elems * 4];
                    let batch_bytes = i32::try_from(batch)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "batch exceeds i32")
                        })?
                        .to_le_bytes();
                    let query_heads_bytes = i32::try_from(query_heads)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "query_heads exceeds i32")
                        })?
                        .to_le_bytes();
                    let kv_heads_bytes = i32::try_from(kv_heads)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "kv_heads exceeds i32")
                        })?
                        .to_le_bytes();
                    let seq_q_bytes = i32::try_from(query_seq)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "seq_q exceeds i32")
                        })?
                        .to_le_bytes();
                    let seq_k_bytes = i32::try_from(seq_k)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "seq_k exceeds i32")
                        })?
                        .to_le_bytes();
                    let dim_bytes = i32::try_from(dim)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "dim exceeds i32")
                        })?
                        .to_le_bytes();
                    let scale_bytes = scale.to_le_bytes();
                    let has_mask_bytes = has_mask_i32.to_le_bytes();

                    let device = RuntimeMetalDevice::new()
                        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                    let pipeline = device
                        .compile_pipeline(
                            "turboquant_attention_packed",
                            &render_metal_turboquant_attention_source(
                                "turboquant_attention_packed",
                            ),
                        )
                        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;

                    let buffer_payloads: [&[u8]; 16] = [
                        q_rotated_bytes.as_slice(),
                        q_sketch_bytes.as_slice(),
                        key_mse_bytes.as_slice(),
                        key_sign_bytes.as_slice(),
                        key_scale_bytes.as_slice(),
                        value_rows_bytes.as_slice(),
                        out_gpu.as_slice(),
                        mask_bytes.as_slice(),
                        batch_bytes.as_slice(),
                        query_heads_bytes.as_slice(),
                        kv_heads_bytes.as_slice(),
                        seq_q_bytes.as_slice(),
                        seq_k_bytes.as_slice(),
                        dim_bytes.as_slice(),
                        scale_bytes.as_slice(),
                        has_mask_bytes.as_slice(),
                    ];
                    let mut owned_buffers: Vec<MetalBuffer> = Vec::new();
                    for data in buffer_payloads {
                        let metal_buf = device.alloc_buffer(data.len().max(1));
                        if !data.is_empty() {
                            device.copy_to_buffer(&metal_buf, data);
                        }
                        owned_buffers.push(metal_buf);
                    }
                    let refs: Vec<&MetalBuffer> = owned_buffers.iter().collect();
                    device
                        .dispatch(&pipeline, out_elems, &refs)
                        .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                    out_gpu = device.copy_from_buffer(&owned_buffers[6], out_elems * 4);

                    let data_ptr = alloc_bytearray(_py, out_gpu.as_slice());
                    if data_ptr.is_null() {
                        return Err(MoltObject::none().bits());
                    }
                    let data_bits = MoltObject::from_ptr(data_ptr).bits();
                    let format_bits = match alloc_string_bits(
                        _py,
                        match q.buffer.format {
                            ScalarFormat::F32 => b"f",
                            ScalarFormat::F64 => b"d",
                            ScalarFormat::I64 => b"q",
                        },
                    ) {
                        Ok(bits) => bits,
                        Err(bits) => {
                            crate::dec_ref_bits(_py, data_bits);
                            return Err(bits);
                        }
                    };
                    let shape_bits =
                        alloc_tuple_bits_from_usize(_py, &[batch, query_heads, query_seq, dim])?;
                    let out_format = q.buffer.format;
                    let rebuilt = rebuild_host_bytes_from_gpu32_output(
                        _py,
                        out_format,
                        out_elems,
                        out_gpu.as_slice(),
                    )?;
                    let rebuilt_ptr = alloc_bytearray(_py, rebuilt.as_slice());
                    if rebuilt_ptr.is_null() {
                        crate::dec_ref_bits(_py, data_bits);
                        crate::dec_ref_bits(_py, format_bits);
                        crate::dec_ref_bits(_py, shape_bits);
                        return Err(MoltObject::none().bits());
                    }
                    let rebuilt_bits = MoltObject::from_ptr(rebuilt_ptr).bits();
                    let tensor_bits = match unsafe {
                        build_tensor_from_data_bits(
                            _py,
                            q.class_bits,
                            q.buffer.class_bits,
                            rebuilt_bits,
                            crate::builtins::classes::builtin_classes(_py).float,
                            out_elems,
                            format_bits,
                            out_format.itemsize(),
                            shape_bits,
                            q.dtype_bits,
                        )
                    } {
                        Ok(bits) => bits,
                        Err(bits) => bits,
                    };
                    crate::dec_ref_bits(_py, data_bits);
                    crate::dec_ref_bits(_py, rebuilt_bits);
                    crate::dec_ref_bits(_py, format_bits);
                    crate::dec_ref_bits(_py, shape_bits);
                    Ok(tensor_bits)
                })();
                return match metal_result {
                    Ok(bits) => bits,
                    Err(bits) => bits,
                };
            }

            #[cfg(target_arch = "wasm32")]
            if requested_gpu_backend() == Some(GpuBackend::WebGpu)
                && runtime_mse_signs_bits != missing
                && runtime_qjl_signs_bits != missing
            {
                let browser_result: Result<u64, u64> = (|| {
                    let (mse_signs_tensor, mse_signs_shape) = unsafe {
                        tensor_runtime_view(_py, runtime_mse_signs_bits, "_runtime_mse_signs")
                    }
                    .map_err(|bits| bits)?;
                    let (qjl_signs_tensor, qjl_signs_shape) = unsafe {
                        tensor_runtime_view(_py, runtime_qjl_signs_bits, "_runtime_qjl_signs")
                    }
                    .map_err(|bits| bits)?;
                    let (key_mse, key_mse_shape) = unsafe {
                        tensor_runtime_view(
                            _py,
                            runtime_key_mse_bits,
                            "_runtime_key_mse_weight_rows",
                        )
                    }
                    .map_err(|bits| bits)?;
                    let (key_sign, key_sign_shape) = unsafe {
                        tensor_runtime_view(
                            _py,
                            runtime_key_sign_bits,
                            "_runtime_key_residual_sign_rows",
                        )
                    }
                    .map_err(|bits| bits)?;
                    let (key_scale, key_scale_shape) = unsafe {
                        tensor_runtime_view(
                            _py,
                            runtime_key_scale_bits,
                            "_runtime_key_residual_scale_rows",
                        )
                    }
                    .map_err(|bits| bits)?;
                    let (value_rows, value_rows_shape) = unsafe {
                        tensor_runtime_view(_py, runtime_value_bits, "_runtime_value_rows")
                    }
                    .map_err(|bits| bits)?;
                    if mse_signs_shape != vec![dim]
                        || qjl_signs_shape != vec![dim]
                        || key_mse_shape.len() != 4
                        || key_sign_shape.len() != 4
                        || key_scale_shape.len() != 3
                        || value_rows_shape.len() != 4
                    {
                        return Err(raise_exception::<u64>(
                            _py,
                            "ValueError",
                            "turboquant browser shadow tensor shape mismatch",
                        ));
                    }
                    let seq_k = key_mse_shape[2];

                    let q_total = batch * query_heads * query_seq * dim;
                    let mut rotated_q_bytes = vec![0u8; q_total * 4];
                    let mut query_sketch_bytes = vec![0u8; q_total * 4];
                    let mut query_row = vec![0.0f32; dim];
                    let mse_signs: Vec<f32> = (0..dim)
                        .map(|index| {
                            read_float_buffer_value(
                                mse_signs_tensor.buffer.data_view,
                                mse_signs_tensor.buffer.format,
                                index,
                            )
                        })
                        .collect();
                    let qjl_signs: Vec<f32> = (0..dim)
                        .map(|index| {
                            read_float_buffer_value(
                                qjl_signs_tensor.buffer.data_view,
                                qjl_signs_tensor.buffer.format,
                                index,
                            )
                        })
                        .collect();
                    let q_stride = query_seq * dim;
                    for batch_index in 0..batch {
                        for query_head_index in 0..query_heads {
                            for query_index in 0..query_seq {
                                let q_base = ((batch_index * query_heads + query_head_index)
                                    * q_stride)
                                    + query_index * dim;
                                for dim_index in 0..dim {
                                    query_row[dim_index] = read_float_buffer_value(
                                        q.buffer.data_view,
                                        q.buffer.format,
                                        q_base + dim_index,
                                    );
                                }
                                let rotated_q = hadamard_apply_with_signs(
                                    query_row.as_slice(),
                                    mse_signs.as_slice(),
                                );
                                let query_sketch = hadamard_apply_with_signs(
                                    query_row.as_slice(),
                                    qjl_signs.as_slice(),
                                );
                                for dim_index in 0..dim {
                                    write_float_buffer_value(
                                        rotated_q_bytes.as_mut_slice(),
                                        ScalarFormat::F32,
                                        q_base + dim_index,
                                        rotated_q[dim_index],
                                    );
                                    write_float_buffer_value(
                                        query_sketch_bytes.as_mut_slice(),
                                        ScalarFormat::F32,
                                        q_base + dim_index,
                                        query_sketch[dim_index],
                                    );
                                }
                            }
                        }
                    }

                    let query_pair_len = q_total * 4;
                    let mut query_pair_bytes = vec![0u8; query_pair_len * 2];
                    query_pair_bytes[..query_pair_len].copy_from_slice(rotated_q_bytes.as_slice());
                    query_pair_bytes[query_pair_len..]
                        .copy_from_slice(query_sketch_bytes.as_slice());
                    let key_mse_bytes = bytes_like_view_to_webgpu_bytes(
                        key_mse.buffer.data_view,
                        key_mse.buffer.format,
                    )
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                    let key_sign_bytes = bytes_like_view_to_webgpu_bytes(
                        key_sign.buffer.data_view,
                        key_sign.buffer.format,
                    )
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                    let key_scale_bytes = bytes_like_view_to_webgpu_bytes(
                        key_scale.buffer.data_view,
                        key_scale.buffer.format,
                    )
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                    let value_rows_bytes = bytes_like_view_to_webgpu_bytes(
                        value_rows.buffer.data_view,
                        value_rows.buffer.format,
                    )
                    .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?;
                    let (mask_bytes, has_mask_i32) =
                        if let Some((mask, mask_shape, mask_strides)) = &mask_info {
                            (
                                expand_attention_mask_to_webgpu_bytes(
                                    mask,
                                    mask_shape.as_slice(),
                                    mask_strides.as_slice(),
                                    batch,
                                    query_heads,
                                    query_seq,
                                    seq_k,
                                )
                                .map_err(|msg| raise_exception::<u64>(_py, "RuntimeError", &msg))?,
                                1i32,
                            )
                        } else {
                            (vec![0u8; 4], 0i32)
                        };
                    let out_elems = batch * query_heads * query_seq * dim;
                    let mut out_webgpu = vec![0u8; out_elems * 4];
                    let batch_bytes = i32::try_from(batch)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "batch exceeds i32")
                        })?
                        .to_le_bytes();
                    let query_heads_bytes = i32::try_from(query_heads)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "query_heads exceeds i32")
                        })?
                        .to_le_bytes();
                    let kv_heads_bytes = i32::try_from(kv_heads)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "kv_heads exceeds i32")
                        })?
                        .to_le_bytes();
                    let seq_q_bytes = i32::try_from(query_seq)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "seq_q exceeds i32")
                        })?
                        .to_le_bytes();
                    let seq_k_bytes = i32::try_from(seq_k)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "seq_k exceeds i32")
                        })?
                        .to_le_bytes();
                    let dim_bytes = i32::try_from(dim)
                        .map_err(|_| {
                            raise_exception::<u64>(_py, "OverflowError", "dim exceeds i32")
                        })?
                        .to_le_bytes();
                    let scale_bytes = scale.to_le_bytes();
                    let has_mask_bytes = has_mask_i32.to_le_bytes();
                    let mut params_bytes = Vec::with_capacity(8 * 4);
                    params_bytes.extend_from_slice(&batch_bytes);
                    params_bytes.extend_from_slice(&query_heads_bytes);
                    params_bytes.extend_from_slice(&kv_heads_bytes);
                    params_bytes.extend_from_slice(&seq_q_bytes);
                    params_bytes.extend_from_slice(&seq_k_bytes);
                    params_bytes.extend_from_slice(&dim_bytes);
                    params_bytes.extend_from_slice(&scale_bytes);
                    params_bytes.extend_from_slice(&has_mask_bytes);
                    let workgroup_size = 64u32;
                    let grid = if out_elems == 0 {
                        0
                    } else {
                        u32::try_from(
                            (out_elems + workgroup_size as usize - 1) / workgroup_size as usize,
                        )
                        .map_err(|_| {
                            raise_exception::<u64>(
                                _py,
                                "OverflowError",
                                "turboquant attention grid exceeds u32",
                            )
                        })?
                    };
                    let source = render_webgpu_turboquant_attention_source(
                        "turboquant_attention_packed",
                        workgroup_size,
                    );
                    dispatch_browser_webgpu_bindings(
                        _py,
                        source.as_str(),
                        "turboquant_attention_packed",
                        vec![
                            serde_json::json!({"binding": 0, "name": "query_pair", "kind": "buffer", "access": "read", "ptr": query_pair_bytes.as_ptr() as usize as u32, "len": query_pair_bytes.len() as u32}),
                            serde_json::json!({"binding": 1, "name": "key_mse", "kind": "buffer", "access": "read", "ptr": key_mse_bytes.as_ptr() as usize as u32, "len": key_mse_bytes.len() as u32}),
                            serde_json::json!({"binding": 2, "name": "key_sign", "kind": "buffer", "access": "read", "ptr": key_sign_bytes.as_ptr() as usize as u32, "len": key_sign_bytes.len() as u32}),
                            serde_json::json!({"binding": 3, "name": "key_scale", "kind": "buffer", "access": "read", "ptr": key_scale_bytes.as_ptr() as usize as u32, "len": key_scale_bytes.len() as u32}),
                            serde_json::json!({"binding": 4, "name": "value_rows", "kind": "buffer", "access": "read", "ptr": value_rows_bytes.as_ptr() as usize as u32, "len": value_rows_bytes.len() as u32}),
                            serde_json::json!({"binding": 5, "name": "out", "kind": "buffer", "access": "read_write", "ptr": out_webgpu.as_mut_ptr() as usize as u32, "len": out_webgpu.len() as u32}),
                            serde_json::json!({"binding": 6, "name": "mask", "kind": "buffer", "access": "read", "ptr": mask_bytes.as_ptr() as usize as u32, "len": mask_bytes.len() as u32}),
                            serde_json::json!({"binding": 7, "name": "params", "kind": "buffer", "access": "read", "ptr": params_bytes.as_ptr() as usize as u32, "len": params_bytes.len() as u32}),
                        ],
                        grid,
                        workgroup_size,
                    )?;
                    let data_ptr = alloc_bytearray(_py, out_webgpu.as_slice());
                    if data_ptr.is_null() {
                        return Err(MoltObject::none().bits());
                    }
                    let data_bits = MoltObject::from_ptr(data_ptr).bits();
                    let format_bits = alloc_string_bits(_py, b"f")?;
                    let shape_bits =
                        alloc_tuple_bits_from_usize(_py, &[batch, query_heads, query_seq, dim])?;
                    let tensor_bits = match unsafe {
                        build_tensor_from_data_bits(
                            _py,
                            q.class_bits,
                            q.buffer.class_bits,
                            data_bits,
                            crate::builtins::classes::builtin_classes(_py).float,
                            out_elems,
                            format_bits,
                            ScalarFormat::F32.itemsize(),
                            shape_bits,
                            crate::builtins::classes::builtin_classes(_py).float,
                        )
                    } {
                        Ok(bits) => bits,
                        Err(bits) => bits,
                    };
                    crate::dec_ref_bits(_py, data_bits);
                    crate::dec_ref_bits(_py, format_bits);
                    crate::dec_ref_bits(_py, shape_bits);
                    Ok(tensor_bits)
                })();
                return match browser_result {
                    Ok(bits) => bits,
                    Err(bits) => bits,
                };
            }

            let (key_mse, key_mse_shape) = match unsafe {
                tensor_runtime_view(_py, runtime_key_mse_bits, "_runtime_key_mse_weight_rows")
            } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
            let (key_sign, key_sign_shape) = match unsafe {
                tensor_runtime_view(
                    _py,
                    runtime_key_sign_bits,
                    "_runtime_key_residual_sign_rows",
                )
            } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
            let (key_scale, key_scale_shape) = match unsafe {
                tensor_runtime_view(
                    _py,
                    runtime_key_scale_bits,
                    "_runtime_key_residual_scale_rows",
                )
            } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
            let (value_rows, value_rows_shape) = match unsafe {
                tensor_runtime_view(_py, runtime_value_bits, "_runtime_value_rows")
            } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
            if key_mse_shape.len() != 4
                || key_sign_shape.len() != 4
                || key_scale_shape.len() != 3
                || value_rows_shape.len() != 4
            {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "turboquant runtime shadow tensors have invalid rank",
                );
            }
            let seq_k = key_mse_shape[2];
            if key_mse_shape != key_sign_shape
                || key_mse_shape[0] != batch
                || key_mse_shape[1] != kv_heads
                || key_mse_shape[3] != dim
                || key_scale_shape != vec![batch, kv_heads, seq_k]
                || value_rows_shape != key_mse_shape
            {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "turboquant runtime shadow tensor shape mismatch",
                );
            }
            let key_mse_strides = strides(&key_mse_shape);
            let key_sign_strides = strides(&key_sign_shape);
            let key_scale_strides = strides(&key_scale_shape);
            let value_rows_strides = strides(&value_rows_shape);

            let out_format = q.buffer.format;
            let out_elems = batch * query_heads * query_seq * dim;
            let mut out = vec![0u8; out_elems * out_format.itemsize()];
            let q_stride = query_seq * dim;
            let out_stride = query_seq * dim;

            for batch_index in 0..batch {
                for query_head_index in 0..query_heads {
                    let kv_head_index = match kv_head_index(query_heads, kv_heads, query_head_index)
                    {
                        Ok(value) => value,
                        Err(()) => {
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                "turboquant attention query heads are incompatible with cache heads",
                            );
                        }
                    };
                    for query_index in 0..query_seq {
                        let q_base = ((batch_index * query_heads + query_head_index) * q_stride)
                            + query_index * dim;
                        let mut query_row = vec![0.0f32; dim];
                        for dim_index in 0..dim {
                            query_row[dim_index] = read_float_buffer_value(
                                q.buffer.data_view,
                                q.buffer.format,
                                q_base + dim_index,
                            );
                        }
                        let rotated_query =
                            hadamard_apply_with_signs(query_row.as_slice(), mse_signs.as_slice());
                        let query_sketch =
                            hadamard_apply_with_signs(query_row.as_slice(), qjl_signs.as_slice());

                        let mut logits = vec![0.0f32; seq_k];
                        let mut max_logit = f32::NEG_INFINITY;
                        for row_index in 0..seq_k {
                            let mut score = 0.0f32;
                            for dim_index in 0..dim {
                                score += rotated_query[dim_index]
                                    * read_tensor_value_4d(
                                        &key_mse,
                                        key_mse_shape.as_slice(),
                                        key_mse_strides.as_slice(),
                                        batch_index,
                                        kv_head_index,
                                        row_index,
                                        dim_index,
                                    );
                            }
                            let mut residual = 0.0f32;
                            for dim_index in 0..dim {
                                residual += query_sketch[dim_index]
                                    * read_tensor_value_4d(
                                        &key_sign,
                                        key_sign_shape.as_slice(),
                                        key_sign_strides.as_slice(),
                                        batch_index,
                                        kv_head_index,
                                        row_index,
                                        dim_index,
                                    );
                            }
                            score += residual
                                * read_tensor_value_3d(
                                    &key_scale,
                                    key_scale_strides.as_slice(),
                                    batch_index,
                                    kv_head_index,
                                    row_index,
                                );
                            score *= scale;
                            if let Some(mask) = &mask_info {
                                let mask_shape = &mask.1;
                                if mask_shape[3] != 1 && mask_shape[3] != seq_k {
                                    return raise_exception::<_>(
                                        _py,
                                        "ValueError",
                                        "turboquant attention mask key width mismatch",
                                    );
                                }
                                score += decode_mask_value(
                                    mask,
                                    batch_index,
                                    query_head_index,
                                    query_index,
                                    row_index,
                                );
                            }
                            logits[row_index] = score;
                            if score > max_logit {
                                max_logit = score;
                            }
                        }

                        let mut exp_sum = 0.0f32;
                        let mut probs = vec![0.0f32; seq_k];
                        for row_index in 0..seq_k {
                            let value = (logits[row_index] - max_logit).exp();
                            probs[row_index] = value;
                            exp_sum += value;
                        }
                        if exp_sum == 0.0 {
                            exp_sum = 1.0;
                        }

                        let mut out_row = vec![0.0f32; dim];
                        for row_index in 0..seq_k {
                            let weight = probs[row_index] / exp_sum;
                            for dim_index in 0..dim {
                                out_row[dim_index] += weight
                                    * read_tensor_value_4d(
                                        &value_rows,
                                        value_rows_shape.as_slice(),
                                        value_rows_strides.as_slice(),
                                        batch_index,
                                        kv_head_index,
                                        row_index,
                                        dim_index,
                                    );
                            }
                        }

                        let out_base = ((batch_index * query_heads + query_head_index)
                            * out_stride)
                            + query_index * dim;
                        for dim_index in 0..dim {
                            write_float_buffer_value(
                                out.as_mut_slice(),
                                out_format,
                                out_base + dim_index,
                                out_row[dim_index],
                            );
                        }
                    }
                }
            }

            let data_ptr = alloc_bytearray(_py, out.as_slice());
            if data_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let data_bits = MoltObject::from_ptr(data_ptr).bits();
            let format_bits = match alloc_string_bits(
                _py,
                match out_format {
                    ScalarFormat::F32 => b"f",
                    ScalarFormat::F64 => b"d",
                    ScalarFormat::I64 => b"q",
                },
            ) {
                Ok(bits) => bits,
                Err(bits) => {
                    crate::dec_ref_bits(_py, data_bits);
                    return bits;
                }
            };
            let shape_bits =
                match alloc_tuple_bits_from_usize(_py, &[batch, query_heads, query_seq, dim]) {
                    Ok(bits) => bits,
                    Err(bits) => {
                        crate::dec_ref_bits(_py, data_bits);
                        crate::dec_ref_bits(_py, format_bits);
                        return bits;
                    }
                };
            let tensor_bits = match unsafe {
                build_tensor_from_data_bits(
                    _py,
                    q.class_bits,
                    q.buffer.class_bits,
                    data_bits,
                    crate::builtins::classes::builtin_classes(_py).float,
                    out_elems,
                    format_bits,
                    out_format.itemsize(),
                    shape_bits,
                    crate::builtins::classes::builtin_classes(_py).float,
                )
            } {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
            crate::dec_ref_bits(_py, data_bits);
            crate::dec_ref_bits(_py, format_bits);
            crate::dec_ref_bits(_py, shape_bits);
            return tensor_bits;
        }

        let key_batches_bits =
            match require_attr_bits(_py, k_cache_bits, key_vectors_name, "_key_vectors") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let value_batches_bits =
            match require_attr_bits(_py, k_cache_bits, value_vectors_name, "_value_vectors") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let key_batches = match decode_u64_sequence_bits(_py, key_batches_bits, "_key_vectors") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let value_batches =
            match decode_u64_sequence_bits(_py, value_batches_bits, "_value_vectors") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        if key_batches.len() != batch || value_batches.len() != batch {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "turboquant cache batch structure mismatch",
            );
        }

        let out_format = q.buffer.format;
        let out_elems = batch * query_heads * query_seq * dim;
        let mut out = vec![0u8; out_elems * out_format.itemsize()];
        let q_stride = query_seq * dim;
        let out_stride = query_seq * dim;

        for batch_index in 0..batch {
            let key_heads =
                match decode_u64_sequence_bits(_py, key_batches[batch_index], "key head list") {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
            let value_heads = match decode_u64_sequence_bits(
                _py,
                value_batches[batch_index],
                "value head list",
            ) {
                Ok(value) => value,
                Err(bits) => return bits,
            };
            if key_heads.len() != kv_heads || value_heads.len() != kv_heads {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "turboquant cache head structure mismatch",
                );
            }

            for query_head_index in 0..query_heads {
                let kv_head_index = match kv_head_index(query_heads, kv_heads, query_head_index) {
                    Ok(value) => value,
                    Err(()) => {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "turboquant attention query heads are incompatible with cache heads",
                        );
                    }
                };
                let key_rows = match decode_u64_sequence_bits(
                    _py,
                    key_heads[kv_head_index],
                    "encoded key rows",
                ) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                let value_rows = match decode_u64_sequence_bits(
                    _py,
                    value_heads[kv_head_index],
                    "encoded value rows",
                ) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                if key_rows.len() != value_rows.len() {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "turboquant key/value row count mismatch",
                    );
                }
                let seq_k = key_rows.len();

                for query_index in 0..query_seq {
                    let q_base = ((batch_index * query_heads + query_head_index) * q_stride)
                        + query_index * dim;
                    let mut query_row = vec![0.0f32; dim];
                    for dim_index in 0..dim {
                        query_row[dim_index] = read_float_buffer_value(
                            q.buffer.data_view,
                            q.buffer.format,
                            q_base + dim_index,
                        );
                    }
                    let rotated_query =
                        hadamard_apply_with_signs(query_row.as_slice(), mse_signs.as_slice());
                    let query_sketch =
                        hadamard_apply_with_signs(query_row.as_slice(), qjl_signs.as_slice());

                    let mut logits = vec![0.0f32; seq_k];
                    let mut max_logit = f32::NEG_INFINITY;
                    for (row_index, &encoded_bits) in key_rows.iter().enumerate() {
                        let mse_bits = match require_attr_bits(
                            _py,
                            encoded_bits,
                            mse_weights_name,
                            "mse_weights",
                        ) {
                            Ok(value) => value,
                            Err(bits) => return bits,
                        };
                        let mse_weights =
                            match decode_float_sequence_bits(_py, mse_bits, "mse_weights") {
                                Ok(value) => value,
                                Err(bits) => return bits,
                            };
                        let residual_sign_bits = match require_attr_bits(
                            _py,
                            encoded_bits,
                            residual_signs_name,
                            "residual_signs",
                        ) {
                            Ok(value) => value,
                            Err(bits) => return bits,
                        };
                        let residual_signs = match decode_float_sequence_bits(
                            _py,
                            residual_sign_bits,
                            "residual_signs",
                        ) {
                            Ok(value) => value,
                            Err(bits) => return bits,
                        };
                        let residual_scale_bits = match require_attr_bits(
                            _py,
                            encoded_bits,
                            residual_scale_name,
                            "residual_scale",
                        ) {
                            Ok(value) => value,
                            Err(bits) => return bits,
                        };
                        let residual_scale =
                            if let Some(value) = to_f64(obj_from_bits(residual_scale_bits)) {
                                value as f32
                            } else if let Some(value) = to_i64(obj_from_bits(residual_scale_bits)) {
                                value as f32
                            } else {
                                return raise_exception::<_>(
                                    _py,
                                    "TypeError",
                                    "residual_scale must be numeric",
                                );
                            };
                        if mse_weights.len() != dim || residual_signs.len() != dim {
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                "turboquant encoded row dimension mismatch",
                            );
                        }
                        let mut score = 0.0f32;
                        for dim_index in 0..dim {
                            score += rotated_query[dim_index] * mse_weights[dim_index];
                        }
                        let mut residual = 0.0f32;
                        for dim_index in 0..dim {
                            residual += query_sketch[dim_index] * residual_signs[dim_index];
                        }
                        score += residual * residual_scale;
                        score *= scale;
                        if let Some(mask) = &mask_info {
                            let mask_shape = &mask.1;
                            if mask_shape[3] != 1 && mask_shape[3] != seq_k {
                                return raise_exception::<_>(
                                    _py,
                                    "ValueError",
                                    "turboquant attention mask key width mismatch",
                                );
                            }
                            score += decode_mask_value(
                                mask,
                                batch_index,
                                query_head_index,
                                query_index,
                                row_index,
                            );
                        }
                        logits[row_index] = score;
                        if logits[row_index] > max_logit {
                            max_logit = logits[row_index];
                        }
                    }

                    let mut exp_sum = 0.0f32;
                    let mut probs = vec![0.0f32; seq_k];
                    for row_index in 0..seq_k {
                        let value = (logits[row_index] - max_logit).exp();
                        probs[row_index] = value;
                        exp_sum += value;
                    }
                    if exp_sum == 0.0 {
                        exp_sum = 1.0;
                    }

                    let mut out_row = vec![0.0f32; dim];
                    for (row_index, &encoded_bits) in value_rows.iter().enumerate() {
                        let mse_bits = match require_attr_bits(
                            _py,
                            encoded_bits,
                            mse_weights_name,
                            "mse_weights",
                        ) {
                            Ok(value) => value,
                            Err(bits) => return bits,
                        };
                        let mse_weights =
                            match decode_float_sequence_bits(_py, mse_bits, "mse_weights") {
                                Ok(value) => value,
                                Err(bits) => return bits,
                            };
                        let residual_sign_bits = match require_attr_bits(
                            _py,
                            encoded_bits,
                            residual_signs_name,
                            "residual_signs",
                        ) {
                            Ok(value) => value,
                            Err(bits) => return bits,
                        };
                        let residual_signs = match decode_float_sequence_bits(
                            _py,
                            residual_sign_bits,
                            "residual_signs",
                        ) {
                            Ok(value) => value,
                            Err(bits) => return bits,
                        };
                        let residual_scale_bits = match require_attr_bits(
                            _py,
                            encoded_bits,
                            residual_scale_name,
                            "residual_scale",
                        ) {
                            Ok(value) => value,
                            Err(bits) => return bits,
                        };
                        let residual_scale =
                            if let Some(value) = to_f64(obj_from_bits(residual_scale_bits)) {
                                value as f32
                            } else if let Some(value) = to_i64(obj_from_bits(residual_scale_bits)) {
                                value as f32
                            } else {
                                return raise_exception::<_>(
                                    _py,
                                    "TypeError",
                                    "residual_scale must be numeric",
                                );
                            };
                        let base = hadamard_invert_with_signs(
                            mse_weights.as_slice(),
                            mse_signs.as_slice(),
                        );
                        let residual_rot: Vec<f32> = residual_signs
                            .iter()
                            .map(|value| *value * residual_scale)
                            .collect();
                        let residual = hadamard_invert_with_signs(
                            residual_rot.as_slice(),
                            qjl_signs.as_slice(),
                        );
                        let weight = probs[row_index] / exp_sum;
                        for dim_index in 0..dim {
                            out_row[dim_index] += weight * (base[dim_index] + residual[dim_index]);
                        }
                    }

                    let out_base = ((batch_index * query_heads + query_head_index) * out_stride)
                        + query_index * dim;
                    for dim_index in 0..dim {
                        write_float_buffer_value(
                            out.as_mut_slice(),
                            out_format,
                            out_base + dim_index,
                            out_row[dim_index],
                        );
                    }
                }
            }
        }

        let data_ptr = alloc_bytearray(_py, out.as_slice());
        if data_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let data_bits = MoltObject::from_ptr(data_ptr).bits();
        let format_bits = match alloc_string_bits(
            _py,
            match out_format {
                ScalarFormat::F32 => b"f",
                ScalarFormat::F64 => b"d",
                ScalarFormat::I64 => b"q",
            },
        ) {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, data_bits);
                return bits;
            }
        };
        let shape_bits =
            match alloc_tuple_bits_from_usize(_py, &[batch, query_heads, query_seq, dim]) {
                Ok(bits) => bits,
                Err(bits) => {
                    crate::dec_ref_bits(_py, data_bits);
                    crate::dec_ref_bits(_py, format_bits);
                    return bits;
                }
            };
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                q.class_bits,
                q.buffer.class_bits,
                data_bits,
                crate::builtins::classes::builtin_classes(_py).float,
                out_elems,
                format_bits,
                out_format.itemsize(),
                shape_bits,
                crate::builtins::classes::builtin_classes(_py).float,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, data_bits);
        crate::dec_ref_bits(_py, format_bits);
        crate::dec_ref_bits(_py, shape_bits);
        tensor_bits
    })
}
