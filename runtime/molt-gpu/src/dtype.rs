//! Data types for GPU tensors.
//!
//! Maps 1:1 to tinygrad's dtypes. Each backend narrows unsupported types
//! (e.g., WebGPU: f64->f32, i64->i32; Metal: f64->f32).

/// Element data type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DType {
    Bool,
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float16,
    BFloat16,
    Float32,
    Float64,
    /// Microscaling FP8 (OCP MX spec v1.0).
    ///
    /// Block-based format: each block of 32 elements shares a single 8-bit
    /// E8M0 shared exponent (bias 127). Individual elements use the FP8
    /// (E4M3 or E5M2) element format.
    /// Total per-element cost: 8 bits data + amortized exponent overhead.
    MxFP8,
    /// Microscaling FP4 (OCP MX spec v1.0).
    ///
    /// Block-based format: each block of 32 elements shares a single 8-bit
    /// E8M0 shared exponent (bias 127). Individual elements use the FP4
    /// (E2M1) element format.
    /// Total per-element cost: 4 bits data + amortized exponent overhead.
    MxFP4,
}

/// Block size for MXFP8: 32 elements share one 8-bit exponent.
///
/// Per OCP Microscaling Specification v1.0 (Table 1), ALL concrete MX
/// formats (MXFP8, MXFP6, MXFP4, MXINT8) use a block size of 32.
pub const MXFP8_BLOCK_SIZE: usize = 32;

/// Block size for MXFP4: 32 elements share one 8-bit exponent.
///
/// Per OCP Microscaling Specification v1.0 (Table 1), ALL concrete MX
/// formats use a block size of 32.
pub const MXFP4_BLOCK_SIZE: usize = 32;

impl DType {
    /// Size in bytes of one element.
    #[inline(always)]
    ///
    /// For MXFP types, this returns the per-element data size (1 byte for
    /// MXFP8, 1 byte for MXFP4 — 4 bits packed into bytes). The shared
    /// exponent overhead is accounted for separately via `mxfp_block_bytes()`.
    pub fn size_bytes(self) -> usize {
        match self {
            Self::Bool | Self::Int8 | Self::UInt8 | Self::MxFP8 => 1,
            Self::Int16 | Self::UInt16 | Self::Float16 | Self::BFloat16 => 2,
            Self::Int32 | Self::UInt32 | Self::Float32 => 4,
            Self::Int64 | Self::UInt64 | Self::Float64 => 8,
            // MXFP4: 4 bits per element, but minimum addressable unit is 1 byte.
            // Use mxfp_element_bits() for sub-byte precision.
            Self::MxFP4 => 1,
        }
    }

    /// Whether this is a floating-point type.
    #[inline(always)]
    pub fn is_float(self) -> bool {
        matches!(
            self,
            Self::Float16
                | Self::BFloat16
                | Self::Float32
                | Self::Float64
                | Self::MxFP8
                | Self::MxFP4
        )
    }

    /// Whether this is a signed integer type.
    pub fn is_signed_int(self) -> bool {
        matches!(self, Self::Int8 | Self::Int16 | Self::Int32 | Self::Int64)
    }

    /// Whether this is an unsigned integer type (including Bool).
    pub fn is_unsigned_int(self) -> bool {
        matches!(
            self,
            Self::Bool | Self::UInt8 | Self::UInt16 | Self::UInt32 | Self::UInt64
        )
    }

    /// Whether this is any integer type.
    pub fn is_int(self) -> bool {
        self.is_signed_int() || self.is_unsigned_int()
    }

    /// Whether this is an MXFP (Microscaling Floating Point) type.
    pub fn is_mxfp(self) -> bool {
        matches!(self, Self::MxFP8 | Self::MxFP4)
    }

    /// Number of bits per element for MXFP types. Returns 0 for non-MXFP types.
    pub fn mxfp_element_bits(self) -> usize {
        match self {
            Self::MxFP8 => 8,
            Self::MxFP4 => 4,
            _ => 0,
        }
    }

    /// Block size for MXFP types (number of elements sharing one exponent).
    /// Returns 0 for non-MXFP types.
    pub fn mxfp_block_size(self) -> usize {
        match self {
            Self::MxFP8 => MXFP8_BLOCK_SIZE,
            Self::MxFP4 => MXFP4_BLOCK_SIZE,
            _ => 0,
        }
    }

    /// Total bytes for one MXFP block: data bytes + 1 byte shared exponent.
    /// Returns 0 for non-MXFP types.
    pub fn mxfp_block_bytes(self) -> usize {
        match self {
            Self::MxFP8 => {
                // 32 elements * 1 byte each + 1 byte exponent = 33 bytes
                MXFP8_BLOCK_SIZE + 1
            }
            Self::MxFP4 => {
                // 32 elements * 0.5 bytes each + 1 byte exponent = 17 bytes
                MXFP4_BLOCK_SIZE / 2 + 1
            }
            _ => 0,
        }
    }

    /// MSL type name for this dtype.
    pub fn msl_type(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int8 => "char",
            Self::Int16 => "short",
            Self::Int32 => "int",
            Self::Int64 => "long",
            Self::UInt8 => "uchar",
            Self::UInt16 => "ushort",
            Self::UInt32 => "uint",
            Self::UInt64 => "ulong",
            Self::Float16 => "half",
            Self::BFloat16 => "bfloat",
            Self::Float32 => "float",
            Self::Float64 => "float", // Metal lacks f64 — narrowed to f32
            // MXFP types are stored as uchar (data) + uchar (shared exponent).
            // Dequantization happens in the kernel body, not the type system.
            Self::MxFP8 | Self::MxFP4 => "uchar",
        }
    }

    /// Narrow this dtype to what the Metal backend supports.
    /// Metal lacks Float64. MXFP types are kept as-is (dequantized in kernel).
    pub fn narrow_metal(self) -> DType {
        match self {
            Self::Float64 => Self::Float32,
            other => other,
        }
    }

    /// Narrow this dtype to what the WebGPU backend supports.
    /// WebGPU lacks Float64 and Int64/UInt64. MXFP kept as-is (dequantized in kernel).
    pub fn narrow_webgpu(self) -> DType {
        match self {
            Self::Float64 => Self::Float32,
            Self::Int64 => Self::Int32,
            Self::UInt64 => Self::UInt32,
            other => other,
        }
    }

    /// WGSL type name for this dtype (post-narrowing).
    pub fn wgsl_type(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int8 => "i32",  // WGSL has no 8-bit types; narrow to i32
            Self::Int16 => "i32", // WGSL has no 16-bit int; narrow to i32
            Self::Int32 => "i32",
            Self::Int64 => "i32",  // narrowed by narrow_webgpu
            Self::UInt8 => "u32",  // WGSL has no 8-bit types; narrow to u32
            Self::UInt16 => "u32", // WGSL has no 16-bit uint; narrow to u32
            Self::UInt32 => "u32",
            Self::UInt64 => "u32", // narrowed by narrow_webgpu
            Self::Float16 => "f16",
            Self::BFloat16 => "f32", // WGSL has no bf16; narrow to f32
            Self::Float32 => "f32",
            Self::Float64 => "f32", // narrowed by narrow_webgpu
            // MXFP stored as u32 in WGSL (packed bytes); dequantized in kernel.
            Self::MxFP8 | Self::MxFP4 => "u32",
        }
    }

    /// CUDA C type name for this dtype.
    pub fn cuda_type(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int8 => "char",
            Self::Int16 => "short",
            Self::Int32 => "int",
            Self::Int64 => "long long",
            Self::UInt8 => "unsigned char",
            Self::UInt16 => "unsigned short",
            Self::UInt32 => "unsigned int",
            Self::UInt64 => "unsigned long long",
            Self::Float16 => "half",
            Self::BFloat16 => "nv_bfloat16",
            Self::Float32 => "float",
            Self::Float64 => "double",
            // MXFP stored as unsigned char; dequantized in kernel body.
            Self::MxFP8 | Self::MxFP4 => "unsigned char",
        }
    }

    /// Narrow this dtype to what the WebGL2 backend supports.
    /// WebGL2 shader math: only float (32-bit highp), int (32-bit), uint (32-bit), bool.
    /// No f64, i64, u64, f16, bf16, i8, u8, i16, u16 in shader math.
    /// (Textures can transport sub-32-bit data, but all shader arithmetic
    /// operates on f32/i32/u32 only.)
    /// MXFP types are narrowed to UInt32 for storage; dequantization is in the kernel.
    pub fn narrow_webgl2(self) -> DType {
        match self {
            Self::Float64 => Self::Float32,
            Self::Int64 => Self::Int32,
            Self::UInt64 => Self::UInt32,
            Self::BFloat16 => Self::Float32,
            Self::Float16 => Self::Float32,
            Self::Int8 | Self::Int16 => Self::Int32,
            Self::UInt8 | Self::UInt16 => Self::UInt32,
            Self::MxFP8 | Self::MxFP4 => Self::UInt32,
            other => other,
        }
    }

    /// GLSL ES 3.0 type name for this dtype (post-narrowing via `narrow_webgl2`).
    pub fn glsl_type(self) -> &'static str {
        let narrowed = self.narrow_webgl2();
        match narrowed {
            Self::Bool => "bool",
            Self::Int32 => "int",
            Self::UInt32 => "uint",
            Self::Float32 => "float",
            _ => unreachable!(
                "narrow_webgl2 should have reduced all types to f32/i32/u32/bool, got {:?}",
                narrowed
            ),
        }
    }

    /// Narrow this dtype to what the OpenCL backend supports.
    ///
    /// OpenCL supports i64 natively. f64 requires the `cl_khr_fp64` extension.
    /// BFloat16 has no OpenCL equivalent and is always narrowed to Float32.
    /// Float16 is supported via `cl_khr_fp16` but is available on most modern
    /// devices, so we do not narrow it here (the renderer handles the pragma).
    /// MXFP types are kept as-is (dequantized in kernel).
    pub fn narrow_opencl(self, has_fp64: bool) -> DType {
        match self {
            Self::Float64 if !has_fp64 => Self::Float32,
            Self::BFloat16 => Self::Float32, // no bf16 in OpenCL
            other => other,
        }
    }

    /// OpenCL C type name for this dtype.
    ///
    /// Should be called on a dtype that has already been narrowed via
    /// `narrow_opencl`. BFloat16 will panic as it must be narrowed first.
    pub fn opencl_type(self) -> &'static str {
        match self {
            Self::Bool => "int", // OpenCL kernels use int for boolean values
            Self::Int8 => "char",
            Self::Int16 => "short",
            Self::Int32 => "int",
            Self::Int64 => "long",
            Self::UInt8 => "uchar",
            Self::UInt16 => "ushort",
            Self::UInt32 => "uint",
            Self::UInt64 => "ulong",
            Self::Float16 => "half",
            Self::BFloat16 => {
                panic!("BFloat16 must be narrowed to Float32 before OpenCL type mapping")
            }
            Self::Float32 => "float",
            Self::Float64 => "double",
            // MXFP stored as uchar; dequantized in kernel body.
            Self::MxFP8 | Self::MxFP4 => "uchar",
        }
    }

    /// HIP C type name for this dtype.
    pub fn hip_type(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int8 => "char",
            Self::Int16 => "short",
            Self::Int32 => "int",
            Self::Int64 => "long long",
            Self::UInt8 => "unsigned char",
            Self::UInt16 => "unsigned short",
            Self::UInt32 => "unsigned int",
            Self::UInt64 => "unsigned long long",
            Self::Float16 => "half",
            Self::BFloat16 => "hip_bfloat16",
            Self::Float32 => "float",
            Self::Float64 => "double",
            // MXFP stored as unsigned char; dequantized in kernel body.
            Self::MxFP8 | Self::MxFP4 => "unsigned char",
        }
    }
}
