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
