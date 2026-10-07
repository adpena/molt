//! Little-endian element codec shared by the molt-gpu integration tests and
//! benches. Device buffers cross the host boundary as raw bytes; this module is
//! the one host-side encoder and decoder the suites use for inputs and results.
#![allow(
    dead_code,
    reason = "Cargo builds each tests/*.rs and benches/*.rs file as its own crate; \
              each includes this module and uses a subset of it"
)]

fn decode<T, const N: usize>(bytes: &[u8], element: fn([u8; N]) -> T) -> Vec<T> {
    let (elements, trailing) = bytes.as_chunks::<N>();
    assert!(
        trailing.is_empty(),
        "a {}-byte buffer is not a whole number of {N}-byte elements",
        bytes.len()
    );
    elements.iter().map(|&encoded| element(encoded)).collect()
}

pub fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    decode(bytes, f32::from_le_bytes)
}

pub fn bytes_to_i32(bytes: &[u8]) -> Vec<i32> {
    decode(bytes, i32::from_le_bytes)
}

pub fn bytes_to_u32(bytes: &[u8]) -> Vec<u32> {
    decode(bytes, u32::from_le_bytes)
}

pub fn bytes_to_u16(bytes: &[u8]) -> Vec<u16> {
    decode(bytes, u16::from_le_bytes)
}

pub fn f32_to_bytes(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

pub fn i32_to_bytes(values: &[i32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

pub fn u32_to_bytes(values: &[u32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

pub fn u16_to_bytes(values: &[u16]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}
