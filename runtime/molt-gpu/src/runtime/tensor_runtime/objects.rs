use super::*;

#[derive(Copy, Clone)]
pub(super) struct BufferRuntimeView {
    pub(super) class_bits: u64,
    pub(super) data_bits: u64,
    pub(super) data_view: ByteView,
    pub(super) element_type_bits: u64,
    pub(super) format_bits: u64,
    pub(super) format: ScalarFormat,
    pub(super) size: usize,
}

#[derive(Copy, Clone)]
pub(super) struct TensorRuntimeView {
    pub(super) class_bits: u64,
    pub(super) buffer_bits: u64,
    pub(super) buffer: BufferRuntimeView,
    pub(super) shape_bits: u64,
    pub(super) dtype_bits: u64,
}

pub(super) unsafe fn buffer_runtime_view(
    _py: &PyToken,
    buffer_bits: u64,
    role: &str,
) -> Result<BufferRuntimeView, u64> {
    let Some(buffer_ptr) = obj_from_bits(buffer_bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be a Buffer instance"),
        ));
    };
    let class_bits = unsafe { object_class_bits(buffer_ptr) };
    if obj_from_bits(class_bits).is_none() {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be a Buffer instance"),
        ));
    }
    let data_bits = unsafe { object_attr_bits(_py, buffer_bits, b"_data", "_data") }?;
    let element_type_bits =
        unsafe { object_attr_bits(_py, buffer_bits, b"_element_type", "_element_type") }?;
    let size_bits = unsafe { object_attr_bits(_py, buffer_bits, b"_size", "_size") }?;
    let format_bits =
        unsafe { object_attr_bits(_py, buffer_bits, b"_format_char", "_format_char") }?;
    let size = parse_usize_arg(_py, size_bits, "buffer._size")?;
    let format = parse_format(_py, format_bits, "buffer._format_char")?;
    let data_view = bytes_like_view(_py, data_bits, "buffer._data")?;
    Ok(BufferRuntimeView {
        class_bits,
        data_bits,
        data_view,
        element_type_bits,
        format_bits,
        format,
        size,
    })
}

pub(super) unsafe fn tensor_runtime_view(
    _py: &PyToken,
    tensor_bits: u64,
    role: &str,
) -> Result<(TensorRuntimeView, Vec<usize>), u64> {
    let Some(tensor_ptr) = obj_from_bits(tensor_bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be a Tensor instance"),
        ));
    };
    let class_bits = unsafe { object_class_bits(tensor_ptr) };
    if obj_from_bits(class_bits).is_none() {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be a Tensor instance"),
        ));
    }
    let buffer_bits = unsafe { object_attr_bits(_py, tensor_bits, b"_buf", "_buf") }?;
    let shape_bits = unsafe { object_attr_bits(_py, tensor_bits, b"_shape", "_shape") }?;
    let dtype_bits = unsafe { object_attr_bits(_py, tensor_bits, b"_dtype", "_dtype") }?;
    let shape = parse_shape(_py, shape_bits, "tensor._shape")?;
    let buffer = unsafe { buffer_runtime_view(_py, buffer_bits, "tensor._buf") }?;
    Ok((
        TensorRuntimeView {
            class_bits,
            buffer_bits,
            buffer,
            shape_bits,
            dtype_bits,
        },
        shape,
    ))
}

pub(super) fn alloc_string_bits(_py: &PyToken, value: &[u8]) -> Result<u64, u64> {
    let ptr = alloc_string(_py, value);
    if ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(ptr).bits())
    }
}

pub(super) fn alloc_tuple_bits_from_usize(_py: &PyToken, dims: &[usize]) -> Result<u64, u64> {
    let bits: Vec<u64> = dims
        .iter()
        .copied()
        .map(|dim| MoltObject::from_int(dim as i64).bits())
        .collect();
    let ptr = alloc_tuple(_py, bits.as_slice());
    if ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(ptr).bits())
    }
}

pub(super) fn parse_i64_sequence_arg(
    _py: &PyToken,
    bits: u64,
    role: &str,
    allow_scalar_int: bool,
) -> Result<Vec<i64>, u64> {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        return if allow_scalar_int {
            to_i64(obj).map(|value| vec![value]).ok_or_else(|| {
                raise_exception::<_>(
                    _py,
                    "TypeError",
                    &format!("{role} must be a tuple or list of ints"),
                )
            })
        } else {
            Err(raise_exception::<_>(
                _py,
                "TypeError",
                &format!("{role} must be a tuple or list of ints"),
            ))
        };
    };
    let ty = unsafe { object_type_id(ptr) };
    if ty != TYPE_ID_TUPLE && ty != TYPE_ID_LIST {
        return if allow_scalar_int {
            to_i64(obj).map(|value| vec![value]).ok_or_else(|| {
                raise_exception::<_>(
                    _py,
                    "TypeError",
                    &format!("{role} must be a tuple or list of ints"),
                )
            })
        } else {
            Err(raise_exception::<_>(
                _py,
                "TypeError",
                &format!("{role} must be a tuple or list of ints"),
            ))
        };
    }

    let decode = |elems: &[u64]| {
        let mut out = Vec::with_capacity(elems.len());
        for &elem_bits in elems {
            let value = to_i64(obj_from_bits(elem_bits))?;
            out.push(value);
        }
        Some(out)
    };

    // Preserve the accepted ``((dims...),)`` call shape without exporting a
    // borrowed edge from a mutable outer list. The pin keeps the sole item
    // alive while the inner sequence is inspected and decoded.
    let nested_item = if unsafe { seq_access::len(ptr) } == 1 {
        unsafe { seq_access::pin_item(_py, ptr, 0) }
    } else {
        None
    };
    let decoded = if let Some(item) = nested_item.as_ref() {
        let inner = obj_from_bits(item.bits());
        if let Some(inner_ptr) = inner.as_ptr() {
            let inner_ty = unsafe { object_type_id(inner_ptr) };
            if inner_ty == TYPE_ID_TUPLE || inner_ty == TYPE_ID_LIST {
                unsafe { seq_access::with_borrowed(inner_ptr, decode) }
            } else {
                unsafe { seq_access::with_borrowed(ptr, decode) }
            }
        } else {
            unsafe { seq_access::with_borrowed(ptr, decode) }
        }
    } else {
        unsafe { seq_access::with_borrowed(ptr, decode) }
    };
    decoded.ok_or_else(|| {
        raise_exception::<_>(_py, "TypeError", &format!("{role} must contain integers"))
    })
}

pub(super) fn normalize_permute_dims(
    _py: &PyToken,
    dims_bits: u64,
    ndim: usize,
) -> Result<Vec<usize>, u64> {
    let raw_dims = parse_i64_sequence_arg(_py, dims_bits, "dims", false)?;
    if raw_dims.len() != ndim {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "permute dims must match tensor rank",
        ));
    }
    let mut normalized = Vec::with_capacity(raw_dims.len());
    for raw_dim in raw_dims {
        let mut dim = raw_dim;
        if dim < 0 {
            dim += ndim as i64;
        }
        if dim < 0 || dim >= ndim as i64 {
            return Err(raise_exception::<_>(
                _py,
                "ValueError",
                &format!("permute dim {raw_dim} out of range for ndim={ndim}"),
            ));
        }
        normalized.push(dim as usize);
    }
    validate_permutation(_py, normalized.as_slice(), ndim)?;
    Ok(normalized)
}

pub(super) fn normalize_reshape_dims(_py: &PyToken, shape_bits: u64) -> Result<Vec<i64>, u64> {
    parse_i64_sequence_arg(_py, shape_bits, "shape", true)
}

pub(super) unsafe fn module_global_bits(
    _py: &PyToken,
    module_name: &[u8],
    attr_name: &[u8],
    attr_label: &str,
) -> Result<u64, u64> {
    let module_name_bits = alloc_string_bits(_py, module_name)?;
    let mut module_bits = molt_module_cache_get(module_name_bits);
    if obj_from_bits(module_bits).is_none() {
        module_bits = molt_module_import(module_name_bits);
    }
    dec_ref_bits(_py, module_name_bits);
    if exception_pending(_py) && obj_from_bits(module_bits).as_ptr().is_some() {
        let _ = molt_exception_clear();
    }
    if exception_pending(_py) {
        return Err(module_bits);
    }
    let attr_bits =
        attr_name_bits_from_bytes(_py, attr_name).ok_or_else(|| MoltObject::none().bits())?;
    let missing = missing_bits(_py);
    let value_bits = molt_getattr_builtin(module_bits, attr_bits, missing);
    dec_ref_bits(_py, attr_bits);
    dec_ref_bits(_py, module_bits);
    if exception_pending(_py) && !is_missing_bits(_py, value_bits) {
        let _ = molt_exception_clear();
    }
    if exception_pending(_py) {
        return Err(value_bits);
    }
    if is_missing_bits(_py, value_bits) {
        return Err(raise_exception::<_>(
            _py,
            "AttributeError",
            &format!(
                "module {:?} has no attribute {:?}",
                String::from_utf8_lossy(module_name),
                attr_label
            ),
        ));
    }
    Ok(value_bits)
}

pub(super) unsafe fn ensure_tensor_object_bits(_py: &PyToken, value_bits: u64) -> Result<u64, u64> {
    let tensor_class_bits =
        unsafe { module_global_bits(_py, b"molt.gpu.tensor", b"Tensor", "Tensor") }?;
    let is_tensor_bits = molt_isinstance(value_bits, tensor_class_bits);
    if exception_pending(_py) {
        dec_ref_bits(_py, tensor_class_bits);
        return Err(is_tensor_bits);
    }
    let is_tensor = is_truthy(_py, obj_from_bits(is_tensor_bits));
    dec_ref_bits(_py, is_tensor_bits);
    if is_tensor {
        dec_ref_bits(_py, tensor_class_bits);
        return Ok(value_bits);
    }
    let tensor_bits = unsafe { call_callable1(_py, tensor_class_bits, value_bits) };
    dec_ref_bits(_py, tensor_class_bits);
    if exception_pending(_py) {
        return Err(tensor_bits);
    }
    Ok(tensor_bits)
}

pub(super) unsafe fn promoted_result_format_bits(
    _py: &PyToken,
    x: &TensorRuntimeView,
    weight: &TensorRuntimeView,
) -> Result<(u64, ScalarFormat, bool, u64), u64> {
    let float_bits = builtin_float(_py);
    if x.dtype_bits == float_bits && weight.dtype_bits == float_bits {
        if x.buffer.element_type_bits == float_bits
            && weight.buffer.element_type_bits == float_bits
            && x.buffer.format == ScalarFormat::F32
            && weight.buffer.format == ScalarFormat::F32
        {
            return Ok((
                alloc_string_bits(_py, b"f")?,
                ScalarFormat::F32,
                true,
                x.dtype_bits,
            ));
        }
        return Ok((
            alloc_string_bits(_py, b"d")?,
            ScalarFormat::F64,
            true,
            x.dtype_bits,
        ));
    }
    Ok((x.buffer.format_bits, x.buffer.format, false, x.dtype_bits))
}

pub(super) unsafe fn build_tensor_from_data_bits(
    _py: &PyToken,
    tensor_class_bits: u64,
    buffer_class_bits: u64,
    data_bits: u64,
    element_type_bits: u64,
    size: usize,
    format_bits: u64,
    itemsize: usize,
    shape_bits: u64,
    dtype_bits: u64,
) -> Result<u64, u64> {
    let buffer_bits = unsafe {
        build_buffer_instance(
            _py,
            buffer_class_bits,
            data_bits,
            element_type_bits,
            size,
            format_bits,
            itemsize,
        )
    }?;
    let tensor_bits = unsafe {
        build_tensor_instance(_py, tensor_class_bits, buffer_bits, shape_bits, dtype_bits)
    };
    dec_ref_bits(_py, buffer_bits);
    tensor_bits
}
