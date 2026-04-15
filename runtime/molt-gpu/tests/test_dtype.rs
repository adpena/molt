use molt_gpu::dtype::DType;

#[test]
fn test_size_bytes() {
    assert_eq!(DType::Bool.size_bytes(), 1);
    assert_eq!(DType::Float16.size_bytes(), 2);
    assert_eq!(DType::Float32.size_bytes(), 4);
    assert_eq!(DType::Float64.size_bytes(), 8);
    assert_eq!(DType::Int64.size_bytes(), 8);
}

#[test]
fn test_type_categories() {
    assert!(DType::Float32.is_float());
    assert!(!DType::Int32.is_float());
    assert!(DType::Int32.is_signed_int());
    assert!(!DType::UInt32.is_signed_int());
    assert!(DType::UInt32.is_unsigned_int());
    assert!(DType::Bool.is_unsigned_int());
}

#[test]
fn test_metal_narrowing() {
    assert_eq!(DType::Float64.narrow_metal(), DType::Float32);
    assert_eq!(DType::Float32.narrow_metal(), DType::Float32);
    assert_eq!(DType::Int64.narrow_metal(), DType::Int64);
}

#[test]
fn test_webgpu_narrowing() {
    assert_eq!(DType::Float64.narrow_webgpu(), DType::Float32);
    assert_eq!(DType::Int64.narrow_webgpu(), DType::Int32);
    assert_eq!(DType::UInt64.narrow_webgpu(), DType::UInt32);
    assert_eq!(DType::Int32.narrow_webgpu(), DType::Int32);
}

#[test]
fn test_msl_types() {
    assert_eq!(DType::Float32.msl_type(), "float");
    assert_eq!(DType::Float64.msl_type(), "float"); // narrowed
    assert_eq!(DType::Int32.msl_type(), "int");
    assert_eq!(DType::Bool.msl_type(), "bool");
    assert_eq!(DType::Float16.msl_type(), "half");
}
