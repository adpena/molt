use molt_gpu::dtype::{DType, MXFP4_BLOCK_SIZE, MXFP8_BLOCK_SIZE};

#[test]
fn test_mxfp8_size_bytes() {
    assert_eq!(DType::MxFP8.size_bytes(), 1);
}

#[test]
fn test_mxfp4_size_bytes() {
    assert_eq!(DType::MxFP4.size_bytes(), 1);
}

#[test]
fn test_mxfp_is_float() {
    assert!(DType::MxFP8.is_float());
    assert!(DType::MxFP4.is_float());
}

#[test]
fn test_mxfp_not_int() {
    assert!(!DType::MxFP8.is_int());
    assert!(!DType::MxFP4.is_int());
    assert!(!DType::MxFP8.is_signed_int());
    assert!(!DType::MxFP4.is_unsigned_int());
}

#[test]
fn test_mxfp_is_mxfp() {
    assert!(DType::MxFP8.is_mxfp());
    assert!(DType::MxFP4.is_mxfp());
    assert!(!DType::Float32.is_mxfp());
    assert!(!DType::Int32.is_mxfp());
}

#[test]
fn test_mxfp_element_bits() {
    assert_eq!(DType::MxFP8.mxfp_element_bits(), 8);
    assert_eq!(DType::MxFP4.mxfp_element_bits(), 4);
    assert_eq!(DType::Float32.mxfp_element_bits(), 0);
}

#[test]
fn test_mxfp_block_size() {
    assert_eq!(DType::MxFP8.mxfp_block_size(), MXFP8_BLOCK_SIZE);
    assert_eq!(DType::MxFP4.mxfp_block_size(), MXFP4_BLOCK_SIZE);
    assert_eq!(MXFP8_BLOCK_SIZE, 16);
    assert_eq!(MXFP4_BLOCK_SIZE, 32);
    assert_eq!(DType::Float32.mxfp_block_size(), 0);
}

#[test]
fn test_mxfp_block_bytes() {
    // MXFP8: 16 elements * 1 byte + 1 byte exponent = 17 bytes
    assert_eq!(DType::MxFP8.mxfp_block_bytes(), 17);
    // MXFP4: 32 elements * 0.5 bytes + 1 byte exponent = 17 bytes
    assert_eq!(DType::MxFP4.mxfp_block_bytes(), 17);
    assert_eq!(DType::Float32.mxfp_block_bytes(), 0);
}

#[test]
fn test_mxfp_msl_type() {
    assert_eq!(DType::MxFP8.msl_type(), "uchar");
    assert_eq!(DType::MxFP4.msl_type(), "uchar");
}

#[test]
fn test_mxfp_cuda_type() {
    assert_eq!(DType::MxFP8.cuda_type(), "unsigned char");
    assert_eq!(DType::MxFP4.cuda_type(), "unsigned char");
}

#[test]
fn test_mxfp_hip_type() {
    assert_eq!(DType::MxFP8.hip_type(), "unsigned char");
    assert_eq!(DType::MxFP4.hip_type(), "unsigned char");
}

#[test]
fn test_mxfp_opencl_type() {
    assert_eq!(DType::MxFP8.opencl_type(), "uchar");
    assert_eq!(DType::MxFP4.opencl_type(), "uchar");
}

#[test]
fn test_mxfp_wgsl_type() {
    assert_eq!(DType::MxFP8.wgsl_type(), "u32");
    assert_eq!(DType::MxFP4.wgsl_type(), "u32");
}

#[test]
fn test_mxfp_glsl_type() {
    // MXFP narrowed to UInt32 by narrow_webgl2, then glsl_type returns "uint".
    assert_eq!(DType::MxFP8.glsl_type(), "uint");
    assert_eq!(DType::MxFP4.glsl_type(), "uint");
}

#[test]
fn test_mxfp_narrow_metal() {
    // MXFP types are kept as-is (not narrowed).
    assert_eq!(DType::MxFP8.narrow_metal(), DType::MxFP8);
    assert_eq!(DType::MxFP4.narrow_metal(), DType::MxFP4);
}

#[test]
fn test_mxfp_narrow_webgpu() {
    // MXFP types are kept as-is (not narrowed).
    assert_eq!(DType::MxFP8.narrow_webgpu(), DType::MxFP8);
    assert_eq!(DType::MxFP4.narrow_webgpu(), DType::MxFP4);
}

#[test]
fn test_mxfp_narrow_webgl2() {
    // MXFP types are narrowed to UInt32 in WebGL2.
    assert_eq!(DType::MxFP8.narrow_webgl2(), DType::UInt32);
    assert_eq!(DType::MxFP4.narrow_webgl2(), DType::UInt32);
}

#[test]
fn test_mxfp_narrow_opencl() {
    // MXFP types are kept as-is (not narrowed).
    assert_eq!(DType::MxFP8.narrow_opencl(true), DType::MxFP8);
    assert_eq!(DType::MxFP4.narrow_opencl(false), DType::MxFP4);
}

#[test]
fn test_mxfp_equality_and_hash() {
    use std::collections::HashSet;
    let mut set = HashSet::new();
    set.insert(DType::MxFP8);
    set.insert(DType::MxFP4);
    set.insert(DType::Float32);
    assert_eq!(set.len(), 3);
    assert!(set.contains(&DType::MxFP8));
    assert!(set.contains(&DType::MxFP4));
}

#[test]
fn test_mxfp_debug_format() {
    assert_eq!(format!("{:?}", DType::MxFP8), "MxFP8");
    assert_eq!(format!("{:?}", DType::MxFP4), "MxFP4");
}
