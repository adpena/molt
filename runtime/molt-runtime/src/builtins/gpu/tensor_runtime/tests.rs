use super::super::{decode_bf16_payload_to_f32_bytes, decode_f16_payload_to_f32_bytes};
#[cfg(all(target_arch = "aarch64", not(miri)))]
use super::{
    linear_dot4_gate_up_interleaved_unaligned, linear_dot4_rows_unaligned,
    linear_dot8_gate_up_interleaved_unaligned, linear_gate_up8_store_unaligned,
};
#[cfg(any(
    all(target_arch = "aarch64", not(miri)),
    target_arch = "x86_64",
    all(target_arch = "wasm32", target_feature = "simd128")
))]
use super::{linear_gate_up4_store_unaligned, linear_rows4_store_ptrs_unaligned};
use super::{
    molt_gpu_broadcast_binary_contiguous, molt_gpu_buffer_to_list, molt_gpu_linear_contiguous,
    molt_gpu_linear_split_last_dim_contiguous,
    molt_gpu_linear_squared_relu_gate_interleaved_contiguous, molt_gpu_matmul_contiguous,
    molt_gpu_repeat_axis_contiguous, molt_gpu_rms_norm_last_axis_contiguous,
    molt_gpu_rope_apply_contiguous, molt_gpu_softmax_last_axis_contiguous,
    molt_gpu_squared_relu_gate_interleaved_contiguous, molt_gpu_tensor__tensor_data_list,
    molt_gpu_tensor__tensor_linear, molt_gpu_tensor__tensor_reshape_view, molt_gpu_tensor__zeros,
    molt_gpu_tensor_from_parts,
};
use crate::{
    MoltObject, alloc_bytes, alloc_class_obj, alloc_string, alloc_tuple, attr_name_bits_from_bytes,
    builtin_classes, bytes_data, bytes_len, dec_ref_bits, obj_from_bits, seq_vec_ref, to_f64,
};

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for value in values {
        out.extend_from_slice(&value.to_ne_bytes());
    }
    out
}

#[test]
fn decode_f16_payload_to_f32_bytes_matches_expected_values() {
    let raw = [0x00_u8, 0x3c_u8, 0x00_u8, 0xc3_u8];
    let decoded = decode_f16_payload_to_f32_bytes(&raw).expect("decode should succeed");
    let values: [f32; 2] = [
        f32::from_le_bytes(decoded[0..4].try_into().expect("first f32")),
        f32::from_le_bytes(decoded[4..8].try_into().expect("second f32")),
    ];
    assert_eq!(values, [1.0, -3.5]);
}

#[test]
fn decode_bf16_payload_to_f32_bytes_matches_expected_values() {
    let raw = [0x80_u8, 0x3f_u8, 0x60_u8, 0xc0_u8];
    let decoded = decode_bf16_payload_to_f32_bytes(&raw).expect("decode should succeed");
    let values: [f32; 2] = [
        f32::from_le_bytes(decoded[0..4].try_into().expect("first f32")),
        f32::from_le_bytes(decoded[4..8].try_into().expect("second f32")),
    ];
    assert_eq!(values, [1.0, -3.5]);
}

fn make_tensor_from_f32(
    _py: &crate::PyToken<'_>,
    tensor_cls_bits: u64,
    buffer_cls_bits: u64,
    values: &[f32],
    shape: &[i64],
) -> u64 {
    let data_ptr = alloc_bytes(_py, &f32_bytes(values));
    let fmt_ptr = alloc_string(_py, b"f");
    let shape_bits: Vec<u64> = shape
        .iter()
        .copied()
        .map(|dim| MoltObject::from_int(dim).bits())
        .collect();
    let shape_ptr = alloc_tuple(_py, shape_bits.as_slice());
    molt_gpu_tensor_from_parts(
        tensor_cls_bits,
        buffer_cls_bits,
        MoltObject::from_ptr(data_ptr).bits(),
        builtin_classes(_py).float,
        MoltObject::from_int(values.len() as i64).bits(),
        MoltObject::from_ptr(fmt_ptr).bits(),
        MoltObject::from_ptr(shape_ptr).bits(),
        builtin_classes(_py).float,
    )
}

fn attr_bits(_py: &crate::PyToken<'_>, obj_bits: u64, name: &[u8]) -> u64 {
    let name_bits = attr_name_bits_from_bytes(_py, name).expect("attr name");
    let value_bits = crate::molt_get_attr_name(obj_bits, name_bits);
    dec_ref_bits(_py, name_bits);
    value_bits
}

fn install_gpu_tensor_module(_py: &crate::PyToken<'_>, tensor_cls_bits: u64, buffer_cls_bits: u64) {
    let root_name_ptr = alloc_string(_py, b"molt");
    let gpu_name_ptr = alloc_string(_py, b"molt.gpu");
    let tensor_name_ptr = alloc_string(_py, b"molt.gpu.tensor");
    let root_name_bits = MoltObject::from_ptr(root_name_ptr).bits();
    let gpu_name_bits = MoltObject::from_ptr(gpu_name_ptr).bits();
    let tensor_name_bits_full = MoltObject::from_ptr(tensor_name_ptr).bits();
    let root_module_bits = crate::builtins::modules::molt_module_new(root_name_bits);
    let gpu_module_bits = crate::builtins::modules::molt_module_new(gpu_name_bits);
    let tensor_module_bits = crate::builtins::modules::molt_module_new(tensor_name_bits_full);
    assert!(!crate::exception_pending(_py));

    let gpu_attr_bits = attr_name_bits_from_bytes(_py, b"gpu").expect("gpu attr");
    let tensor_attr_bits = attr_name_bits_from_bytes(_py, b"tensor").expect("tensor attr");
    let tensor_name_bits = attr_name_bits_from_bytes(_py, b"Tensor").expect("Tensor attr");
    let buffer_name_bits = attr_name_bits_from_bytes(_py, b"Buffer").expect("Buffer attr");
    crate::builtins::modules::molt_module_set_attr(
        root_module_bits,
        gpu_attr_bits,
        gpu_module_bits,
    );
    crate::builtins::modules::molt_module_set_attr(
        gpu_module_bits,
        tensor_attr_bits,
        tensor_module_bits,
    );
    crate::builtins::modules::molt_module_set_attr(
        tensor_module_bits,
        tensor_name_bits,
        tensor_cls_bits,
    );
    crate::builtins::modules::molt_module_set_attr(
        tensor_module_bits,
        buffer_name_bits,
        buffer_cls_bits,
    );
    crate::builtins::modules::molt_module_cache_set(root_name_bits, root_module_bits);
    crate::builtins::modules::molt_module_cache_set(gpu_name_bits, gpu_module_bits);
    crate::builtins::modules::molt_module_cache_set(tensor_name_bits_full, tensor_module_bits);
    dec_ref_bits(_py, gpu_attr_bits);
    dec_ref_bits(_py, tensor_attr_bits);
    dec_ref_bits(_py, tensor_name_bits);
    dec_ref_bits(_py, buffer_name_bits);
    dec_ref_bits(_py, root_name_bits);
    dec_ref_bits(_py, gpu_name_bits);
    dec_ref_bits(_py, tensor_name_bits_full);
    dec_ref_bits(_py, root_module_bits);
    dec_ref_bits(_py, gpu_module_bits);
    dec_ref_bits(_py, tensor_module_bits);
    assert!(!crate::exception_pending(_py));
}

#[cfg(all(target_arch = "aarch64", not(miri)))]
#[test]
fn linear_dot4_rows_unaligned_matches_scalar_rows() {
    let x = [1.5f32, -2.0, 0.5, 3.0, -1.0, 4.0];
    let weights = [
        0.25f32, 1.0, -0.5, 2.0, 0.0, 1.5, -1.0, 0.5, 0.75, -0.25, 1.25, 0.0, 2.0, -0.5, 1.0, 0.0,
        -1.5, 0.5, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0,
    ];
    let mut x_bytes = vec![0u8];
    x_bytes.extend_from_slice(&f32_bytes(&x));
    let mut weight_bytes = vec![0u8, 0u8, 0u8];
    weight_bytes.extend_from_slice(&f32_bytes(&weights));
    let row_offsets = [0usize, 6, 12, 18];

    let got = unsafe {
        linear_dot4_rows_unaligned(
            x_bytes[1..].as_ptr(),
            0,
            weight_bytes[3..].as_ptr(),
            row_offsets,
            x.len(),
        )
    };

    for (row_idx, row_off) in row_offsets.into_iter().enumerate() {
        let expected = x
            .iter()
            .zip(weights[row_off..row_off + x.len()].iter())
            .map(|(lhs, rhs)| lhs * rhs)
            .sum::<f32>();
        assert!(
            (got[row_idx] - expected).abs() < 1e-5,
            "row {row_idx} mismatch: got {}, expected {expected}",
            got[row_idx]
        );
    }
}

#[cfg(any(
    all(target_arch = "aarch64", not(miri)),
    target_arch = "x86_64",
    all(target_arch = "wasm32", target_feature = "simd128")
))]
#[test]
fn linear_rows4_store_unaligned_matches_scalar_rows() {
    let x = [1.5f32, -2.0, 0.5, 3.0, -1.0, 4.0];
    let weights = [
        0.25f32, 1.0, -0.5, 2.0, 0.0, 1.5, -1.0, 0.5, 0.75, -0.25, 1.25, 0.0, 2.0, -0.5, 1.0, 0.0,
        -1.5, 0.5, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0,
    ];
    let row_offsets = [0usize, 6, 12, 18];
    let mut x_bytes = vec![0u8];
    x_bytes.extend_from_slice(&f32_bytes(&x));
    let mut weight_bytes = vec![0u8, 0u8, 0u8];
    weight_bytes.extend_from_slice(&f32_bytes(&weights));
    let mut out_bytes = [0u8; 4 * 4 + 1];

    unsafe {
        linear_rows4_store_ptrs_unaligned(
            x_bytes[1..].as_ptr(),
            [
                weight_bytes[3 + row_offsets[0] * 4..].as_ptr(),
                weight_bytes[3 + row_offsets[1] * 4..].as_ptr(),
                weight_bytes[3 + row_offsets[2] * 4..].as_ptr(),
                weight_bytes[3 + row_offsets[3] * 4..].as_ptr(),
            ],
            [
                out_bytes[1..].as_mut_ptr(),
                out_bytes[5..].as_mut_ptr(),
                out_bytes[9..].as_mut_ptr(),
                out_bytes[13..].as_mut_ptr(),
            ],
            x.len(),
        );
    }

    let got = out_bytes[1..]
        .chunks_exact(4)
        .map(|chunk| f32::from_ne_bytes(chunk.try_into().unwrap()))
        .collect::<Vec<_>>();
    let mut expected = Vec::new();
    for row_off in row_offsets {
        expected.push(
            x.iter()
                .zip(weights[row_off..row_off + x.len()].iter())
                .map(|(lhs, rhs)| lhs * rhs)
                .sum::<f32>(),
        );
    }
    assert_eq!(got.len(), expected.len());
    for (idx, (lhs, rhs)) in got.iter().zip(expected.iter()).enumerate() {
        assert!(
            (lhs - rhs).abs() < 1e-5,
            "idx {idx}: got {lhs}, expected {rhs}"
        );
    }
}

#[cfg(any(
    all(target_arch = "aarch64", not(miri)),
    target_arch = "x86_64",
    all(target_arch = "wasm32", target_feature = "simd128")
))]
#[test]
fn linear_gate_up4_store_unaligned_matches_reference_outputs() {
    let x = [0.25f32, -1.0, 2.5, 0.5, -0.75, 1.25];
    let weights = [
        1.0f32, 0.0, 0.5, -1.0, 0.25, 1.5, -0.5, 2.0, 0.0, 0.25, 1.0, -1.5, 0.75, -0.5, 1.5, 0.0,
        -1.0, 0.5, 1.25, 0.0, -0.75, 2.0, 0.5, -0.25, -1.5, 0.25, 1.0, 0.5, -0.25, 2.0, 0.0, 1.5,
        -0.5, 1.25, 0.75, -1.0, 0.5, -1.25, 0.0, 0.75, 1.5, 0.25, 2.0, 0.5, -1.0, 0.0, -0.5, 1.0,
    ];
    let mut x_bytes = vec![0u8, 0u8];
    x_bytes.extend_from_slice(&f32_bytes(&x));
    let mut weight_bytes = vec![0u8];
    weight_bytes.extend_from_slice(&f32_bytes(&weights));
    let mut out_bytes = [0u8; 4 * 4 + 3];

    unsafe {
        linear_gate_up4_store_unaligned(
            x_bytes[2..].as_ptr(),
            0,
            weight_bytes[1..].as_ptr(),
            0,
            x.len(),
            out_bytes[3..].as_mut_ptr(),
        );
    }

    let got = out_bytes[3..]
        .chunks_exact(4)
        .map(|chunk| f32::from_ne_bytes(chunk.try_into().unwrap()))
        .collect::<Vec<_>>();
    let mut expected = Vec::new();
    for hidden_idx in 0..4usize {
        let gate_off = (2 * hidden_idx) * x.len();
        let up_off = (2 * hidden_idx + 1) * x.len();
        let gate = x
            .iter()
            .zip(weights[gate_off..gate_off + x.len()].iter())
            .map(|(lhs, rhs)| lhs * rhs)
            .sum::<f32>();
        let up = x
            .iter()
            .zip(weights[up_off..up_off + x.len()].iter())
            .map(|(lhs, rhs)| lhs * rhs)
            .sum::<f32>();
        let relu = gate.max(0.0);
        expected.push(relu * relu * up);
    }
    assert_eq!(got.len(), expected.len());
    for (idx, (lhs, rhs)) in got.iter().zip(expected.iter()).enumerate() {
        assert!(
            (lhs - rhs).abs() < 1e-5,
            "idx {idx}: got {lhs}, expected {rhs}"
        );
    }
}

#[cfg(all(target_arch = "aarch64", not(miri)))]
#[test]
fn linear_dot4_gate_up_interleaved_unaligned_matches_scalar_rows() {
    let x = [0.25f32, -1.0, 2.5, 0.5, -0.75, 1.25];
    let weights = [
        1.0f32, 0.0, 0.5, -1.0, 0.25, 1.5, -0.5, 2.0, 0.0, 0.25, 1.0, -1.5, 0.75, -0.5, 1.5, 0.0,
        -1.0, 0.5, 1.25, 0.0, -0.75, 2.0, 0.5, -0.25, -1.5, 0.25, 1.0, 0.5, -0.25, 2.0, 0.0, 1.5,
        -0.5, 1.25, 0.75, -1.0, 0.5, -1.25, 0.0, 0.75, 1.5, 0.25, 2.0, 0.5, -1.0, 0.0, -0.5, 1.0,
    ];
    let mut x_bytes = vec![0u8, 0u8];
    x_bytes.extend_from_slice(&f32_bytes(&x));
    let mut weight_bytes = vec![0u8];
    weight_bytes.extend_from_slice(&f32_bytes(&weights));

    let (gates, ups) = unsafe {
        linear_dot4_gate_up_interleaved_unaligned(
            x_bytes[2..].as_ptr(),
            0,
            weight_bytes[1..].as_ptr(),
            0,
            x.len(),
        )
    };

    for hidden_idx in 0..4usize {
        let gate_off = (2 * hidden_idx) * x.len();
        let up_off = (2 * hidden_idx + 1) * x.len();
        let expected_gate = x
            .iter()
            .zip(weights[gate_off..gate_off + x.len()].iter())
            .map(|(lhs, rhs)| lhs * rhs)
            .sum::<f32>();
        let expected_up = x
            .iter()
            .zip(weights[up_off..up_off + x.len()].iter())
            .map(|(lhs, rhs)| lhs * rhs)
            .sum::<f32>();
        assert!(
            (gates[hidden_idx] - expected_gate).abs() < 1e-5,
            "gate {hidden_idx} mismatch: got {}, expected {expected_gate}",
            gates[hidden_idx]
        );
        assert!(
            (ups[hidden_idx] - expected_up).abs() < 1e-5,
            "up {hidden_idx} mismatch: got {}, expected {expected_up}",
            ups[hidden_idx]
        );
    }
}

#[cfg(all(target_arch = "aarch64", not(miri)))]
#[test]
fn linear_dot8_gate_up_interleaved_unaligned_matches_scalar_rows() {
    let x = [0.5f32, -1.0, 1.5, 2.0, -0.25, 0.75];
    let mut weights = Vec::new();
    for hidden_idx in 0..8usize {
        for k in 0..x.len() {
            weights.push((hidden_idx as f32 + 1.0) * (k as f32 - 1.5));
        }
        for k in 0..x.len() {
            weights.push((hidden_idx as f32 + 0.5) * (2.0 - k as f32));
        }
    }
    let mut x_bytes = vec![0u8];
    x_bytes.extend_from_slice(&f32_bytes(&x));
    let mut weight_bytes = vec![0u8, 0u8, 0u8];
    weight_bytes.extend_from_slice(&f32_bytes(&weights));

    let (gates, ups) = unsafe {
        linear_dot8_gate_up_interleaved_unaligned(
            x_bytes[1..].as_ptr(),
            0,
            weight_bytes[3..].as_ptr(),
            0,
            x.len(),
        )
    };

    for hidden_idx in 0..8usize {
        let gate_off = (2 * hidden_idx) * x.len();
        let up_off = (2 * hidden_idx + 1) * x.len();
        let expected_gate = x
            .iter()
            .zip(weights[gate_off..gate_off + x.len()].iter())
            .map(|(lhs, rhs)| lhs * rhs)
            .sum::<f32>();
        let expected_up = x
            .iter()
            .zip(weights[up_off..up_off + x.len()].iter())
            .map(|(lhs, rhs)| lhs * rhs)
            .sum::<f32>();
        assert!(
            (gates[hidden_idx] - expected_gate).abs() < 1e-5,
            "gate {hidden_idx} mismatch: got {}, expected {expected_gate}",
            gates[hidden_idx]
        );
        assert!(
            (ups[hidden_idx] - expected_up).abs() < 1e-5,
            "up {hidden_idx} mismatch: got {}, expected {expected_up}",
            ups[hidden_idx]
        );
    }
}

#[cfg(all(target_arch = "aarch64", not(miri)))]
#[test]
fn linear_gate_up8_store_unaligned_matches_reference_outputs() {
    let x = [0.5f32, -1.0, 1.5, 2.0, -0.25, 0.75];
    let mut weights = Vec::new();
    for hidden_idx in 0..8usize {
        for k in 0..x.len() {
            weights.push((hidden_idx as f32 + 1.0) * (k as f32 - 1.5));
        }
        for k in 0..x.len() {
            weights.push((hidden_idx as f32 + 0.5) * (2.0 - k as f32));
        }
    }
    let mut x_bytes = vec![0u8];
    x_bytes.extend_from_slice(&f32_bytes(&x));
    let mut weight_bytes = vec![0u8, 0u8, 0u8];
    weight_bytes.extend_from_slice(&f32_bytes(&weights));
    let mut out_bytes = [0u8; 8 * 4 + 3];

    unsafe {
        linear_gate_up8_store_unaligned(
            x_bytes[1..].as_ptr(),
            0,
            weight_bytes[3..].as_ptr(),
            0,
            x.len(),
            out_bytes[3..].as_mut_ptr(),
        );
    }

    let got = out_bytes[3..]
        .chunks_exact(4)
        .map(|chunk| f32::from_ne_bytes(chunk.try_into().unwrap()))
        .collect::<Vec<_>>();
    let mut expected = Vec::new();
    for hidden_idx in 0..8usize {
        let gate_off = (2 * hidden_idx) * x.len();
        let up_off = (2 * hidden_idx + 1) * x.len();
        let gate = x
            .iter()
            .zip(weights[gate_off..gate_off + x.len()].iter())
            .map(|(lhs, rhs)| lhs * rhs)
            .sum::<f32>();
        let up = x
            .iter()
            .zip(weights[up_off..up_off + x.len()].iter())
            .map(|(lhs, rhs)| lhs * rhs)
            .sum::<f32>();
        let relu = gate.max(0.0);
        expected.push(relu * relu * up);
    }
    assert_eq!(got.len(), expected.len());
    for (idx, (lhs, rhs)) in got.iter().zip(expected.iter()).enumerate() {
        assert!(
            (lhs - rhs).abs() < 1e-5,
            "idx {idx}: got {lhs}, expected {rhs}"
        );
    }
}

#[test]
fn gpu_tensor_from_parts_wraps_tensor_and_buffer_objects() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let tensor_name_ptr = alloc_string(_py, b"Tensor");
        let buffer_name_ptr = alloc_string(_py, b"Buffer");
        let tensor_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(tensor_name_ptr).bits());
        let buffer_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(buffer_name_ptr).bits());
        let data_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
        let fmt_ptr = alloc_string(_py, b"f");
        let shape_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(2).bits(),
            ],
        );

        let out_bits = molt_gpu_tensor_from_parts(
            MoltObject::from_ptr(tensor_cls_ptr).bits(),
            MoltObject::from_ptr(buffer_cls_ptr).bits(),
            MoltObject::from_ptr(data_ptr).bits(),
            builtin_classes(_py).float,
            MoltObject::from_int(4).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(shape_ptr).bits(),
            builtin_classes(_py).float,
        );
        assert!(!crate::exception_pending(_py));
        let tensor_ptr = obj_from_bits(out_bits)
            .as_ptr()
            .expect("tensor_from_parts should return a tensor object");

        let buf_name_bits = attr_name_bits_from_bytes(_py, b"_buf").expect("_buf attr");
        let shape_name_bits = attr_name_bits_from_bytes(_py, b"_shape").expect("_shape attr");
        let format_name_bits =
            attr_name_bits_from_bytes(_py, b"_format_char").expect("_format_char attr");
        let itemsize_name_bits =
            attr_name_bits_from_bytes(_py, b"_itemsize").expect("_itemsize attr");

        let buffer_bits = crate::molt_get_attr_name(out_bits, buf_name_bits);
        let shape_bits = crate::molt_get_attr_name(out_bits, shape_name_bits);
        let format_bits = crate::molt_get_attr_name(buffer_bits, format_name_bits);
        let itemsize_bits = crate::molt_get_attr_name(buffer_bits, itemsize_name_bits);

        dec_ref_bits(_py, buf_name_bits);
        dec_ref_bits(_py, shape_name_bits);
        dec_ref_bits(_py, format_name_bits);
        dec_ref_bits(_py, itemsize_name_bits);

        assert_eq!(
            unsafe { crate::object_type_id(tensor_ptr) },
            crate::TYPE_ID_OBJECT
        );
        let shape_ptr = obj_from_bits(shape_bits)
            .as_ptr()
            .expect("tensor shape should be a tuple");
        let dims = unsafe { seq_vec_ref(shape_ptr) };
        assert_eq!(dims.len(), 2);
        assert_eq!(crate::to_i64(obj_from_bits(dims[0])), Some(2));
        assert_eq!(crate::to_i64(obj_from_bits(dims[1])), Some(2));
        assert_eq!(
            crate::string_obj_to_owned(obj_from_bits(format_bits)).as_deref(),
            Some("f")
        );
        assert_eq!(crate::to_i64(obj_from_bits(itemsize_bits)), Some(4));
    });
}

#[test]
fn gpu_repeat_axis_contiguous_f32_roundtrip() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let data_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
        let fmt_ptr = alloc_string(_py, b"f");
        let shape_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(2).bits(),
            ],
        );

        let out_bits = molt_gpu_repeat_axis_contiguous(
            MoltObject::from_ptr(data_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(shape_ptr).bits(),
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(3).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
        );
        let out_ptr = obj_from_bits(out_bits)
            .as_ptr()
            .expect("repeat intrinsic should return bytes");
        let out = unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
        let values = out
            .chunks_exact(4)
            .map(|chunk| f32::from_ne_bytes(chunk.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            vec![1.0, 2.0, 1.0, 2.0, 1.0, 2.0, 3.0, 4.0, 3.0, 4.0, 3.0, 4.0]
        );
    });
}

#[test]
fn gpu_buffer_to_list_f32_roundtrip() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let tensor_name_ptr = alloc_string(_py, b"Tensor");
        let buffer_name_ptr = alloc_string(_py, b"Buffer");
        let tensor_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(tensor_name_ptr).bits());
        let buffer_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(buffer_name_ptr).bits());
        let data_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
        let fmt_ptr = alloc_string(_py, b"f");
        let shape_ptr = alloc_tuple(_py, &[MoltObject::from_int(4).bits()]);

        let tensor_bits = molt_gpu_tensor_from_parts(
            MoltObject::from_ptr(tensor_cls_ptr).bits(),
            MoltObject::from_ptr(buffer_cls_ptr).bits(),
            MoltObject::from_ptr(data_ptr).bits(),
            builtin_classes(_py).float,
            MoltObject::from_int(4).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(shape_ptr).bits(),
            builtin_classes(_py).float,
        );
        assert!(!crate::exception_pending(_py));

        let buf_name_bits = attr_name_bits_from_bytes(_py, b"_buf").expect("_buf attr");
        let buffer_bits = crate::molt_get_attr_name(tensor_bits, buf_name_bits);
        dec_ref_bits(_py, buf_name_bits);

        let list_bits = molt_gpu_buffer_to_list(buffer_bits, MoltObject::from_int(4).bits());
        assert!(!crate::exception_pending(_py));
        let list_ptr = obj_from_bits(list_bits)
            .as_ptr()
            .expect("buffer_to_list should return a list");
        let elems = unsafe { seq_vec_ref(list_ptr) };
        let values: Vec<f64> = elems
            .iter()
            .copied()
            .map(|bits| to_f64(obj_from_bits(bits)).expect("float element"))
            .collect();
        assert_eq!(values, vec![1.0, 2.0, 3.0, 4.0]);
    });
}

#[test]
fn gpu_module_tensor_linear_wrapper_roundtrip() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let tensor_name_ptr = alloc_string(_py, b"Tensor");
        let buffer_name_ptr = alloc_string(_py, b"Buffer");
        let tensor_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(tensor_name_ptr).bits());
        let buffer_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(buffer_name_ptr).bits());
        let tensor_cls_bits = MoltObject::from_ptr(tensor_cls_ptr).bits();
        let buffer_cls_bits = MoltObject::from_ptr(buffer_cls_ptr).bits();

        let x_bits = make_tensor_from_f32(
            _py,
            tensor_cls_bits,
            buffer_cls_bits,
            &[1.0, 2.0, 3.0, 4.0],
            &[2, 2],
        );
        let weight_bits = make_tensor_from_f32(
            _py,
            tensor_cls_bits,
            buffer_cls_bits,
            &[5.0, 6.0, 7.0, 8.0, 9.0, 10.0],
            &[3, 2],
        );

        let out_bits = molt_gpu_tensor__tensor_linear(x_bits, weight_bits);
        assert!(!crate::exception_pending(_py));

        let out_shape_bits = attr_bits(_py, out_bits, b"_shape");
        let out_shape_ptr = obj_from_bits(out_shape_bits).as_ptr().expect("shape tuple");
        let out_dims = unsafe { seq_vec_ref(out_shape_ptr) };
        assert_eq!(crate::to_i64(obj_from_bits(out_dims[0])), Some(2));
        assert_eq!(crate::to_i64(obj_from_bits(out_dims[1])), Some(3));

        let out_buf_bits = attr_bits(_py, out_bits, b"_buf");
        let list_bits = molt_gpu_buffer_to_list(out_buf_bits, MoltObject::from_int(6).bits());
        let list_ptr = obj_from_bits(list_bits).as_ptr().expect("list");
        let elems = unsafe { seq_vec_ref(list_ptr) };
        let values: Vec<f64> = elems
            .iter()
            .copied()
            .map(|bits| to_f64(obj_from_bits(bits)).expect("float element"))
            .collect();
        assert_eq!(values, vec![17.0, 23.0, 29.0, 39.0, 53.0, 67.0]);
    });
}

#[test]
fn gpu_module_tensor_reshape_view_wrapper_reuses_buffer() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let tensor_name_ptr = alloc_string(_py, b"Tensor");
        let buffer_name_ptr = alloc_string(_py, b"Buffer");
        let tensor_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(tensor_name_ptr).bits());
        let buffer_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(buffer_name_ptr).bits());
        let tensor_cls_bits = MoltObject::from_ptr(tensor_cls_ptr).bits();
        let buffer_cls_bits = MoltObject::from_ptr(buffer_cls_ptr).bits();
        install_gpu_tensor_module(_py, tensor_cls_bits, buffer_cls_bits);

        let tensor_bits = make_tensor_from_f32(
            _py,
            tensor_cls_bits,
            buffer_cls_bits,
            &[1.0, 2.0, 3.0, 4.0],
            &[4],
        );
        let shape_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(2).bits(),
            ],
        );

        let reshaped_bits = molt_gpu_tensor__tensor_reshape_view(
            tensor_bits,
            MoltObject::from_ptr(shape_ptr).bits(),
        );
        assert!(!crate::exception_pending(_py));

        let original_buf_bits = attr_bits(_py, tensor_bits, b"_buf");
        let reshaped_buf_bits = attr_bits(_py, reshaped_bits, b"_buf");
        assert_eq!(reshaped_buf_bits, original_buf_bits);

        let reshaped_shape_bits = attr_bits(_py, reshaped_bits, b"_shape");
        let reshaped_shape_ptr = obj_from_bits(reshaped_shape_bits)
            .as_ptr()
            .expect("shape tuple");
        let dims = unsafe { seq_vec_ref(reshaped_shape_ptr) };
        assert_eq!(crate::to_i64(obj_from_bits(dims[0])), Some(2));
        assert_eq!(crate::to_i64(obj_from_bits(dims[1])), Some(2));
    });
}

#[test]
fn gpu_module_tensor_data_list_and_zeros_wrappers_roundtrip() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let tensor_name_ptr = alloc_string(_py, b"Tensor");
        let buffer_name_ptr = alloc_string(_py, b"Buffer");
        let tensor_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(tensor_name_ptr).bits());
        let buffer_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(buffer_name_ptr).bits());
        let tensor_cls_bits = MoltObject::from_ptr(tensor_cls_ptr).bits();
        let buffer_cls_bits = MoltObject::from_ptr(buffer_cls_ptr).bits();
        install_gpu_tensor_module(_py, tensor_cls_bits, buffer_cls_bits);

        let tensor_bits = make_tensor_from_f32(
            _py,
            tensor_cls_bits,
            buffer_cls_bits,
            &[1.0, 2.0, 3.0, 4.0],
            &[2, 2],
        );
        let list_bits = molt_gpu_tensor__tensor_data_list(tensor_bits);
        assert!(!crate::exception_pending(_py));
        let list_ptr = obj_from_bits(list_bits).as_ptr().expect("list");
        let elems = unsafe { seq_vec_ref(list_ptr) };
        let values: Vec<f64> = elems
            .iter()
            .copied()
            .map(|bits| to_f64(obj_from_bits(bits)).expect("float element"))
            .collect();
        assert_eq!(values, vec![1.0, 2.0, 3.0, 4.0]);

        let zero_shape_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(3).bits(),
            ],
        );
        let zeros_bits = molt_gpu_tensor__zeros(
            MoltObject::from_ptr(zero_shape_ptr).bits(),
            builtin_classes(_py).float,
        );
        assert!(!crate::exception_pending(_py));
        let zero_shape_bits = attr_bits(_py, zeros_bits, b"_shape");
        let zero_shape_ptr = obj_from_bits(zero_shape_bits)
            .as_ptr()
            .expect("shape tuple");
        let zero_dims = unsafe { seq_vec_ref(zero_shape_ptr) };
        assert_eq!(crate::to_i64(obj_from_bits(zero_dims[0])), Some(2));
        assert_eq!(crate::to_i64(obj_from_bits(zero_dims[1])), Some(3));

        let zero_buf_bits = attr_bits(_py, zeros_bits, b"_buf");
        let zero_list_bits = molt_gpu_buffer_to_list(zero_buf_bits, MoltObject::from_int(6).bits());
        let zero_list_ptr = obj_from_bits(zero_list_bits).as_ptr().expect("zero list");
        let zero_values: Vec<f64> = unsafe { seq_vec_ref(zero_list_ptr) }
            .iter()
            .copied()
            .map(|bits| to_f64(obj_from_bits(bits)).expect("float element"))
            .collect();
        assert_eq!(zero_values, vec![0.0; 6]);
    });
}

#[test]
fn gpu_linear_contiguous_f32_roundtrip() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let x_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
        let w_ptr = alloc_bytes(_py, &f32_bytes(&[5.0, 6.0, 7.0, 8.0, 9.0, 10.0]));
        let fmt_ptr = alloc_string(_py, b"f");
        assert!(!x_ptr.is_null());
        assert!(!w_ptr.is_null());
        assert!(!fmt_ptr.is_null());

        let out_bits = molt_gpu_linear_contiguous(
            MoltObject::from_ptr(x_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(w_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_int(2).bits(),
            MoltObject::from_int(2).bits(),
            MoltObject::from_int(3).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
        );
        let out_ptr = obj_from_bits(out_bits)
            .as_ptr()
            .expect("linear intrinsic should return bytes");
        let out = unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
        let mut values = Vec::new();
        for chunk in out.chunks_exact(4) {
            values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        assert_eq!(values, vec![17.0, 23.0, 29.0, 39.0, 53.0, 67.0]);
    });
}

#[test]
fn gpu_linear_split_last_dim_contiguous_f32_roundtrip() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let x_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
        let w_ptr = alloc_bytes(
            _py,
            &f32_bytes(&[1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 0.0, 0.0, 2.0]),
        );
        let fmt_ptr = alloc_string(_py, b"f");
        let sizes_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(3).bits(),
            ],
        );

        let out_bits = molt_gpu_linear_split_last_dim_contiguous(
            MoltObject::from_ptr(x_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(w_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_int(2).bits(),
            MoltObject::from_int(2).bits(),
            MoltObject::from_ptr(sizes_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
        );
        let out_ptr = obj_from_bits(out_bits)
            .as_ptr()
            .expect("linear split intrinsic should return tuple");
        let parts = unsafe { crate::seq_vec_ref(out_ptr) };
        assert_eq!(parts.len(), 2);

        let left_ptr = obj_from_bits(parts[0]).as_ptr().expect("left bytes");
        let left = unsafe { std::slice::from_raw_parts(bytes_data(left_ptr), bytes_len(left_ptr)) };
        let right_ptr = obj_from_bits(parts[1]).as_ptr().expect("right bytes");
        let right =
            unsafe { std::slice::from_raw_parts(bytes_data(right_ptr), bytes_len(right_ptr)) };

        let mut left_values = Vec::new();
        for chunk in left.chunks_exact(4) {
            left_values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        let mut right_values = Vec::new();
        for chunk in right.chunks_exact(4) {
            right_values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }

        assert_eq!(left_values, vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(right_values, vec![3.0, 2.0, 4.0, 7.0, 6.0, 8.0]);
    });
}

#[test]
fn gpu_linear_split_last_dim_contiguous_f32_three_way_wider_roundtrip() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let x_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0]));
        let w_ptr = alloc_bytes(
            _py,
            &f32_bytes(&[
                1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 2.0, 0.0, 1.0, 0.0,
                2.0, 1.0,
            ]),
        );
        let fmt_ptr = alloc_string(_py, b"f");
        let sizes_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(3).bits(),
            ],
        );

        let out_bits = molt_gpu_linear_split_last_dim_contiguous(
            MoltObject::from_ptr(x_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(w_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(3).bits(),
            MoltObject::from_ptr(sizes_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
        );
        let out_ptr = obj_from_bits(out_bits)
            .as_ptr()
            .expect("linear split intrinsic should return tuple");
        let parts = unsafe { crate::seq_vec_ref(out_ptr) };
        assert_eq!(parts.len(), 3);

        let decode = |bits: u64| {
            let ptr = obj_from_bits(bits).as_ptr().expect("bytes");
            let bytes = unsafe { std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr)) };
            bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_ne_bytes(chunk.try_into().unwrap()))
                .collect::<Vec<_>>()
        };

        assert_eq!(decode(parts[0]), vec![1.0, 2.0]);
        assert_eq!(decode(parts[1]), vec![3.0]);
        assert_eq!(decode(parts[2]), vec![6.0, 5.0, 7.0]);
    });
}

#[test]
fn gpu_linear_squared_relu_gate_interleaved_contiguous_f32_roundtrip() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let x_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
        let w_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 0.0]));
        let fmt_ptr = alloc_string(_py, b"f");

        let out_bits = molt_gpu_linear_squared_relu_gate_interleaved_contiguous(
            MoltObject::from_ptr(x_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(w_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_int(2).bits(),
            MoltObject::from_int(2).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
        );
        let out_ptr = obj_from_bits(out_bits)
            .as_ptr()
            .expect("linear squared relu gate intrinsic should return bytes");
        let out = unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
        let mut values = Vec::new();
        for chunk in out.chunks_exact(4) {
            values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        assert_eq!(values, vec![2.0, 18.0, 36.0, 294.0]);
    });
}

#[test]
fn gpu_linear_squared_relu_gate_interleaved_contiguous_f32_wide_roundtrip() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let x_ptr = alloc_bytes(_py, &f32_bytes(&[2.0, 3.0]));
        let w_ptr = alloc_bytes(
            _py,
            &f32_bytes(&[
                1.0, 0.0, 0.0, 1.0, 2.0, 0.0, 0.0, 1.0, 3.0, 0.0, 0.0, 1.0, 4.0, 0.0, 0.0, 1.0,
                5.0, 0.0, 0.0, 1.0,
            ]),
        );
        let fmt_ptr = alloc_string(_py, b"f");

        let out_bits = molt_gpu_linear_squared_relu_gate_interleaved_contiguous(
            MoltObject::from_ptr(x_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(w_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(2).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
        );
        let out_ptr = obj_from_bits(out_bits)
            .as_ptr()
            .expect("linear squared relu gate intrinsic should return bytes");
        let out = unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
        let mut values = Vec::new();
        for chunk in out.chunks_exact(4) {
            values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        assert_eq!(values, vec![12.0, 48.0, 108.0, 192.0, 300.0]);
    });
}

#[test]
fn gpu_linear_squared_relu_gate_interleaved_contiguous_f32_wider_roundtrip() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let x_ptr = alloc_bytes(_py, &f32_bytes(&[2.0, 3.0]));
        let w_ptr = alloc_bytes(
            _py,
            &f32_bytes(&[
                1.0, 0.0, 0.0, 1.0, 2.0, 0.0, 0.0, 1.0, 3.0, 0.0, 0.0, 1.0, 4.0, 0.0, 0.0, 1.0,
                5.0, 0.0, 0.0, 1.0, 6.0, 0.0, 0.0, 1.0, 7.0, 0.0, 0.0, 1.0, 8.0, 0.0, 0.0, 1.0,
                9.0, 0.0, 0.0, 1.0,
            ]),
        );
        let fmt_ptr = alloc_string(_py, b"f");

        let out_bits = molt_gpu_linear_squared_relu_gate_interleaved_contiguous(
            MoltObject::from_ptr(x_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(w_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(2).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
        );
        let out_ptr = obj_from_bits(out_bits)
            .as_ptr()
            .expect("linear squared relu gate intrinsic should return bytes");
        let out = unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
        let mut values = Vec::new();
        for chunk in out.chunks_exact(4) {
            values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        assert_eq!(
            values,
            vec![12.0, 48.0, 108.0, 192.0, 300.0, 432.0, 588.0, 768.0, 972.0]
        );
    });
}

#[test]
fn gpu_broadcast_binary_contiguous_f32_roundtrip() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let a_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
        let b_ptr = alloc_bytes(_py, &f32_bytes(&[10.0, 20.0]));
        let fmt_ptr = alloc_string(_py, b"f");
        let a_shape_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(2).bits(),
            ],
        );
        let b_shape_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
            ],
        );

        let out_bits = molt_gpu_broadcast_binary_contiguous(
            MoltObject::from_ptr(a_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(a_shape_ptr).bits(),
            MoltObject::from_ptr(b_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(b_shape_ptr).bits(),
            MoltObject::from_int(0).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
        );
        let out_ptr = obj_from_bits(out_bits)
            .as_ptr()
            .expect("broadcast intrinsic should return bytes-like");
        let out = unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
        let mut values = Vec::new();
        for chunk in out.chunks_exact(4) {
            values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        assert_eq!(values, vec![11.0, 22.0, 13.0, 24.0]);
    });
}

#[test]
fn gpu_matmul_contiguous_f32_roundtrip() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let a_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
        let b_ptr = alloc_bytes(_py, &f32_bytes(&[5.0, 6.0, 7.0, 8.0]));
        let fmt_ptr = alloc_string(_py, b"f");
        let a_shape_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(2).bits(),
            ],
        );
        let b_shape_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(2).bits(),
            ],
        );

        let out_bits = molt_gpu_matmul_contiguous(
            MoltObject::from_ptr(a_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(a_shape_ptr).bits(),
            MoltObject::from_ptr(b_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(b_shape_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
        );
        let out_ptr = obj_from_bits(out_bits)
            .as_ptr()
            .expect("matmul intrinsic should return bytes");
        let out = unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
        let mut values = Vec::new();
        for chunk in out.chunks_exact(4) {
            values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        assert_eq!(values, vec![19.0, 22.0, 43.0, 50.0]);
    });
}

#[test]
fn gpu_rope_apply_contiguous_f32_roundtrip() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let x_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
        let cos_ptr = alloc_bytes(_py, &f32_bytes(&[0.0, 1.0]));
        let sin_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 0.0]));
        let fmt_ptr = alloc_string(_py, b"f");

        let out_bits = molt_gpu_rope_apply_contiguous(
            MoltObject::from_ptr(x_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(cos_ptr).bits(),
            MoltObject::from_ptr(sin_ptr).bits(),
            MoltObject::from_int(2).bits(),
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(4).bits(),
            MoltObject::from_int(1).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
        );
        let out_ptr = obj_from_bits(out_bits)
            .as_ptr()
            .expect("rope intrinsic should return bytes-like");
        let out = unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
        let mut values = Vec::new();
        for chunk in out.chunks_exact(4) {
            values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        assert_eq!(values, vec![-3.0, 2.0, 1.0, 4.0]);
    });
}

#[test]
fn gpu_rope_apply_contiguous_rejects_odd_dim() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let x_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0]));
        let cos_ptr = alloc_bytes(_py, &f32_bytes(&[1.0]));
        let sin_ptr = alloc_bytes(_py, &f32_bytes(&[0.0]));
        let fmt_ptr = alloc_string(_py, b"f");

        let out_bits = molt_gpu_rope_apply_contiguous(
            MoltObject::from_ptr(x_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(cos_ptr).bits(),
            MoltObject::from_ptr(sin_ptr).bits(),
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(1).bits(),
            MoltObject::from_int(3).bits(),
            MoltObject::from_int(1).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
        );

        assert!(crate::exception_pending(_py));
        let _ = out_bits;
        let _ = crate::molt_exception_clear();
    });
}

#[test]
fn gpu_softmax_last_axis_contiguous_f32_roundtrip() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let x_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
        let fmt_ptr = alloc_string(_py, b"f");
        let shape_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(2).bits(),
            ],
        );

        let out_bits = molt_gpu_softmax_last_axis_contiguous(
            MoltObject::from_ptr(x_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(shape_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
        );
        let out_ptr = obj_from_bits(out_bits)
            .as_ptr()
            .expect("softmax intrinsic should return bytes");
        let out = unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
        let mut values = Vec::new();
        for chunk in out.chunks_exact(4) {
            values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        assert!((values[0] + values[1] - 1.0).abs() < 1e-6);
        assert!((values[2] + values[3] - 1.0).abs() < 1e-6);
    });
}

#[test]
fn gpu_rms_norm_last_axis_contiguous_f32_roundtrip() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let x_ptr = alloc_bytes(_py, &f32_bytes(&[3.0, 4.0, 0.0, 5.0]));
        let fmt_ptr = alloc_string(_py, b"f");
        let shape_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(2).bits(),
            ],
        );

        let out_bits = molt_gpu_rms_norm_last_axis_contiguous(
            MoltObject::from_ptr(x_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(shape_ptr).bits(),
            MoltObject::from_float(0.0).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
        );
        let out_ptr = obj_from_bits(out_bits)
            .as_ptr()
            .expect("rms_norm intrinsic should return bytes");
        let out = unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
        let mut values = Vec::new();
        for chunk in out.chunks_exact(4) {
            values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        assert!((values[0] - 0.84852815).abs() < 1e-6);
        assert!((values[1] - 1.1313709).abs() < 1e-6);
        assert!((values[2] - 0.0).abs() < 1e-6);
        assert!((values[3] - std::f32::consts::SQRT_2).abs() < 1e-6);
    });
}

#[test]
fn gpu_squared_relu_gate_interleaved_contiguous_f32_roundtrip() {
    let _guard = crate::TEST_MUTEX.lock().unwrap();
    crate::with_gil_entry_nopanic!(_py, {
        let x_ptr = alloc_bytes(
            _py,
            &f32_bytes(&[1.0, 10.0, -2.0, 20.0, 3.0, 30.0, 4.0, 40.0]),
        );
        let fmt_ptr = alloc_string(_py, b"f");
        let shape_ptr = alloc_tuple(
            _py,
            &[
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(8).bits(),
            ],
        );

        let out_bits = molt_gpu_squared_relu_gate_interleaved_contiguous(
            MoltObject::from_ptr(x_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(shape_ptr).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
        );
        let out_ptr = obj_from_bits(out_bits)
            .as_ptr()
            .expect("squared relu gate intrinsic should return bytes");
        let out = unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
        let mut values = Vec::new();
        for chunk in out.chunks_exact(4) {
            values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        assert_eq!(values, vec![10.0, 0.0, 270.0, 640.0]);
    });
}
