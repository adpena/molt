use crate::bridge::*;
use abi::api::{abstract_number, buffer, numbers, strings};
use abi::bridge::GLOBAL_BRIDGE;
use blake2b_simd::{Params as Blake2bParams, State as Blake2bState};
use blake2s_simd::{Params as Blake2sParams, State as Blake2sState};
use digest::{Digest, ExtendableOutput, Update, XofReader};
#[cfg(feature = "legacy")]
use md4::Md4;
use md5::Md5;
use molt_runtime_core::native_custody::{
    ByteInput, NativeObjectOwner, cpython_abi as abi, exact_bytes, retire_native_reference,
};
use molt_runtime_core::prelude::*;
#[cfg(feature = "legacy")]
use ripemd::Ripemd160;
use sha1::Sha1;
use sha2::{Sha224, Sha256, Sha384, Sha512, Sha512_224, Sha512_256};
use sha3::{Sha3_224, Sha3_256, Sha3_384, Sha3_512};
use shake::{Shake128, Shake256};
use std::ffi::{c_long, c_ulong, c_ulonglong};

#[derive(Clone)]
pub enum HashKind {
    #[cfg(feature = "legacy")]
    Md4(Md4),
    Md5(Md5),
    #[cfg(feature = "legacy")]
    Ripemd160(Ripemd160),
    Sha1(Sha1),
    Sha224(Sha224),
    Sha256(Sha256),
    Sha384(Sha384),
    Sha512(Sha512),
    Sha512_224(Sha512_224),
    Sha512_256(Sha512_256),
    Sha3_224(Sha3_224),
    Sha3_256(Sha3_256),
    Sha3_384(Sha3_384),
    Sha3_512(Sha3_512),
    Shake128(Shake128),
    Shake256(Shake256),
    Blake2b(Blake2bState),
    Blake2s(Blake2sState),
}

#[derive(Clone)]
pub struct HashHandle {
    pub kind: HashKind,
    pub name: &'static str,
    pub digest_size: usize,
    pub block_size: usize,
    pub is_xof: bool,
}

impl HashHandle {
    pub fn update(&mut self, data: &[u8]) {
        self.kind.update(data);
    }

    pub fn finalize_bytes(&self, length: Option<usize>) -> Result<Vec<u8>, HashError> {
        self.kind.clone().finalize_bytes(length)
    }
}

#[derive(Debug)]
pub enum HashError {
    XofLengthMissing,
}

impl HashKind {
    fn update(&mut self, data: &[u8]) {
        match self {
            #[cfg(feature = "legacy")]
            HashKind::Md4(hasher) => {
                Digest::update(hasher, data);
            }
            HashKind::Md5(hasher) => {
                Digest::update(hasher, data);
            }
            #[cfg(feature = "legacy")]
            HashKind::Ripemd160(hasher) => {
                Digest::update(hasher, data);
            }
            HashKind::Sha1(hasher) => {
                Digest::update(hasher, data);
            }
            HashKind::Sha224(hasher) => {
                Digest::update(hasher, data);
            }
            HashKind::Sha256(hasher) => {
                Digest::update(hasher, data);
            }
            HashKind::Sha384(hasher) => {
                Digest::update(hasher, data);
            }
            HashKind::Sha512(hasher) => {
                Digest::update(hasher, data);
            }
            HashKind::Sha512_224(hasher) => {
                Digest::update(hasher, data);
            }
            HashKind::Sha512_256(hasher) => {
                Digest::update(hasher, data);
            }
            HashKind::Sha3_224(hasher) => {
                Digest::update(hasher, data);
            }
            HashKind::Sha3_256(hasher) => {
                Digest::update(hasher, data);
            }
            HashKind::Sha3_384(hasher) => {
                Digest::update(hasher, data);
            }
            HashKind::Sha3_512(hasher) => {
                Digest::update(hasher, data);
            }
            HashKind::Shake128(hasher) => {
                Update::update(hasher, data);
            }
            HashKind::Shake256(hasher) => {
                Update::update(hasher, data);
            }
            HashKind::Blake2b(state) => {
                state.update(data);
            }
            HashKind::Blake2s(state) => {
                state.update(data);
            }
        }
    }

    fn finalize_bytes(self, length: Option<usize>) -> Result<Vec<u8>, HashError> {
        match self {
            #[cfg(feature = "legacy")]
            HashKind::Md4(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Md5(hasher) => Ok(hasher.finalize().to_vec()),
            #[cfg(feature = "legacy")]
            HashKind::Ripemd160(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Sha1(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Sha224(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Sha256(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Sha384(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Sha512(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Sha512_224(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Sha512_256(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Sha3_224(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Sha3_256(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Sha3_384(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Sha3_512(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Shake128(hasher) => {
                let Some(length) = length else {
                    return Err(HashError::XofLengthMissing);
                };
                let mut reader = hasher.finalize_xof();
                let mut out = vec![0u8; length];
                reader.read(&mut out);
                Ok(out)
            }
            HashKind::Shake256(hasher) => {
                let Some(length) = length else {
                    return Err(HashError::XofLengthMissing);
                };
                let mut reader = hasher.finalize_xof();
                let mut out = vec![0u8; length];
                reader.read(&mut out);
                Ok(out)
            }
            HashKind::Blake2b(state) => Ok(state.finalize().as_bytes().to_vec()),
            HashKind::Blake2s(state) => Ok(state.finalize().as_bytes().to_vec()),
        }
    }
}

// CPython first checks its exact cached names; only provider spellings then
// receive case-insensitive lookup. The selected provider accepts trailing ASCII
// spaces on those spellings, not leading/other whitespace or cached-only names.
pub fn normalize_hash_name(name: &str) -> Option<&'static str> {
    match name {
        "sha512_224" | "SHA512_224" => return Some("sha512_224"),
        "sha512_256" | "SHA512_256" => return Some("sha512_256"),
        "sha3_224" => return Some("sha3_224"),
        "sha3_256" => return Some("sha3_256"),
        "sha3_384" => return Some("sha3_384"),
        "sha3_512" => return Some("sha3_512"),
        "shake_128" => return Some("shake_128"),
        "shake_256" => return Some("shake_256"),
        "blake2b" => return Some("blake2b"),
        "blake2s" => return Some("blake2s"),
        _ => {}
    }
    let provider_name = name.trim_end_matches(' ');
    [
        ("md4", "md4"),
        ("md5", "md5"),
        ("ripemd160", "ripemd160"),
        ("ripemd-160", "ripemd160"),
        ("sha1", "sha1"),
        ("sha-1", "sha1"),
        ("sha224", "sha224"),
        ("sha-224", "sha224"),
        ("sha256", "sha256"),
        ("sha-256", "sha256"),
        ("sha384", "sha384"),
        ("sha-384", "sha384"),
        ("sha512", "sha512"),
        ("sha-512", "sha512"),
        ("sha-512/224", "sha512_224"),
        ("sha512-224", "sha512_224"),
        ("sha-512/256", "sha512_256"),
        ("sha512-256", "sha512_256"),
        ("sha3-224", "sha3_224"),
        ("sha3-256", "sha3_256"),
        ("sha3-384", "sha3_384"),
        ("sha3-512", "sha3_512"),
        ("shake-128", "shake_128"),
        ("shake128", "shake_128"),
        ("shake-256", "shake_256"),
        ("shake256", "shake_256"),
    ]
    .into_iter()
    .find_map(|(alias, canonical)| {
        provider_name
            .eq_ignore_ascii_case(alias)
            .then_some(canonical)
    })
}

// The builtin HMAC stage selects a subset of the existing hash codecs. Its
// HACL spellings project to those same canonical names; provider aliases do
// not participate in this separate CPython 3.14 name lookup.
pub(crate) fn builtin_hmac_name(name: &str) -> Option<&'static str> {
    let name = match name {
        "sha2_224" => "sha224",
        "sha2_256" => "sha256",
        "sha2_384" => "sha384",
        "sha2_512" => "sha512",
        "blake2b_32" => "blake2b",
        "blake2s_32" => "blake2s",
        name => name,
    };
    normalize_hash_name(name).filter(|canonical| {
        name == *canonical
            && !matches!(
                *canonical,
                "md4" | "ripemd160" | "sha512_224" | "sha512_256" | "shake_128" | "shake_256"
            )
    })
}

pub(crate) fn byte_input<'py>(py: &'py PyToken, bits: u64) -> Result<ByteInput<'py>, u64> {
    unsafe { ByteInput::acquire(py, bits) }.map_err(|_| MoltObject::none().bits())
}

pub(crate) fn clinic_input<'py>(
    py: &'py PyToken,
    bits: u64,
    function: &str,
    argument: &str,
) -> Result<ByteInput<'py>, u64> {
    let input = byte_input(py, bits)?;
    // 3.13+ Clinic relies on the SIMPLE exporter contract; 3.12 explicitly
    // rejects a malformed provider's noncontiguous descriptor after acquire.
    if !rt_target_at_least(3, 13) && !input.is_c_contiguous() {
        return Err(raise_exception::<u64>(
            py,
            "TypeError",
            &format!(
                "{function}() argument '{argument}' must be contiguous buffer, not {}",
                type_name(py, obj_from_bits(bits))
            ),
        ));
    }
    Ok(input)
}

// hashlib's GET_BUFFER_VIEW contract deliberately has a different admission
// diagnostic and rank restriction from Clinic's ordinary y* arguments.
pub(crate) fn hash_input<'py>(py: &'py PyToken, bits: u64) -> Result<ByteInput<'py>, u64> {
    if !unsafe { exact_bytes(py, bits) }
        .map_err(|_| MoltObject::none().bits())?
        .is_some()
    {
        let object = unsafe {
            NativeObjectOwner::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits))
        };
        if object.as_ptr().is_null() {
            retire_native_reference(|| drop(object));
            return Err(MoltObject::none().bits());
        }
        if unsafe { strings::PyUnicode_Check(object.as_ptr()) } != 0 {
            return Err(raise_exception::<u64>(
                py,
                "TypeError",
                "Strings must be encoded before hashing",
            ));
        }
        if unsafe { buffer::PyObject_CheckBuffer(object.as_ptr()) } == 0 {
            return Err(raise_exception::<u64>(
                py,
                "TypeError",
                "object supporting the buffer API required",
            ));
        }
    }
    let input = byte_input(py, bits)?;
    if input.ndim() > 1 {
        return Err(raise_exception::<u64>(
            py,
            "BufferError",
            "Buffer must be single dimension",
        ));
    }
    Ok(input)
}

fn native_number<T>(
    py: &PyToken,
    bits: u64,
    convert: impl FnOnce(*mut abi::abi_types::PyObject) -> T,
) -> Result<T, u64> {
    let object =
        unsafe { NativeObjectOwner::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits)) };
    if object.as_ptr().is_null() {
        retire_native_reference(|| drop(object));
        return Err(MoltObject::none().bits());
    }
    let value = convert(object.as_ptr());
    retire_native_reference(|| drop(object));
    if exception_pending(py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(value)
    }
}

fn c_int_from_bits(py: &PyToken, bits: u64) -> Result<i32, u64> {
    native_number(py, bits, |object| unsafe { numbers::PyLong_AsInt(object) })
}

fn c_long_from_bits(py: &PyToken, bits: u64) -> Result<c_long, u64> {
    native_number(py, bits, |object| unsafe { numbers::PyLong_AsLong(object) })
}

fn c_ulong_from_bits(py: &PyToken, bits: u64) -> Result<u64, u64> {
    native_number(py, bits, |object| unsafe {
        let mut value: c_ulong = 0;
        numbers::_PyLong_UnsignedLong_Converter(object, (&raw mut value).cast());
        value as u64
    })
}

fn c_ulonglong_from_bits(py: &PyToken, bits: u64) -> Result<u64, u64> {
    native_number(py, bits, |object| unsafe {
        let mut value: c_ulonglong = 0;
        numbers::_PyLong_UnsignedLongLong_Converter(object, (&raw mut value).cast());
        value as u64
    })
}

fn ssize_from_bits(py: &PyToken, bits: u64) -> Result<isize, u64> {
    native_number(py, bits, |object| unsafe {
        let index = NativeObjectOwner::from_owned(abstract_number::PyNumber_Index(object));
        if index.as_ptr().is_null() {
            return -1;
        }
        numbers::PyLong_AsSsize_t(index.as_ptr())
    })
}

fn truth_from_bits(py: &PyToken, bits: u64) -> Result<bool, u64> {
    let value = is_truthy(py, obj_from_bits(bits));
    if exception_pending(py) {
        Err(MoltObject::none().bits())
    } else {
        Ok(value)
    }
}

// Capture all Clinic operands before the first conversion can mutate the
// caller's dictionary. These are values, not additional buffer owners.
const HASH_OPTION_NAMES: &[&str] = &[
    "digest_size",
    "key",
    "salt",
    "person",
    "fanout",
    "depth",
    "leaf_size",
    "node_offset",
    "node_depth",
    "inner_size",
    "last_node",
    "usedforsecurity",
];

fn hash_options<'py>(
    py: &'py PyToken,
    bits: u64,
    names: &[&str],
) -> Result<Vec<Option<OwnedRuntimeValue<'py>>>, u64> {
    if obj_from_bits(bits).is_none() {
        return Ok(names.iter().map(|_| None).collect());
    }
    let Some(ptr) = obj_from_bits(bits)
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_DICT })
    else {
        return Err(raise_exception::<u64>(
            py,
            "TypeError",
            "hashlib options must be dict",
        ));
    };
    names
        .iter()
        .map(|name| {
            let key = unsafe { OwnedRuntimeValue::from_owned_bits(py, rt_string_from(name)) };
            if exception_pending(py) {
                return Err(MoltObject::none().bits());
            }
            unsafe { dict_get_in_place(py, ptr, key.bits()) }
                .map(|value| value.map(|bits| OwnedRuntimeValue::retain(py, bits)))
                .map_err(|_| MoltObject::none().bits())
        })
        .collect()
}

#[derive(Clone)]
struct Blake2Options {
    digest_size: usize,
    key: Vec<u8>,
    salt: Vec<u8>,
    person: Vec<u8>,
    fanout: u8,
    depth: u8,
    leaf_size: u32,
    node_offset: u64,
    node_depth: u8,
    inner_size: usize,
    last_node: bool,
}

fn parse_blake2_options<'py>(
    py: &'py PyToken,
    values: &[Option<OwnedRuntimeValue<'py>>],
    max_digest: usize,
    max_key: usize,
    max_salt: usize,
    max_person: usize,
    inputs: &mut [Option<ByteInput<'py>>; 3],
) -> Result<Blake2Options, u64> {
    let integer = |index: usize, default| match &values[index] {
        Some(value) => c_int_from_bits(py, value.bits()),
        None => Ok(default),
    };
    let digest_size = integer(0, max_digest as i32)?;
    for index in 0..3 {
        if let Some(value) = &values[index + 1] {
            inputs[index] = Some(clinic_input(
                py,
                value.bits(),
                if max_digest == 64 {
                    "blake2b"
                } else {
                    "blake2s"
                },
                ["key", "salt", "person"][index],
            )?);
        }
    }
    let fanout = integer(4, 1)?;
    let depth = integer(5, 1)?;
    let leaf_size = match &values[6] {
        Some(value) => c_ulong_from_bits(py, value.bits())?,
        None => 0,
    };
    let node_offset = match &values[7] {
        Some(value) => c_ulonglong_from_bits(py, value.bits())?,
        None => 0,
    };
    let node_depth = integer(8, 0)?;
    let inner_size = integer(9, 0)?;
    let last_node = match &values[10] {
        Some(value) => truth_from_bits(py, value.bits())?,
        None => false,
    };
    if let Some(value) = &values[11] {
        truth_from_bits(py, value.bits())?;
    }
    // No slices survive conversions. Exports stay live until initial data has
    // been consumed, including the successful constructor's release callbacks.
    let bytes = |index: usize| {
        inputs[index]
            .as_ref()
            .map(|input| unsafe { input.bytes(py) })
            .unwrap_or(&[])
    };
    if digest_size < 1 || digest_size as usize > max_digest {
        let message = if rt_target_at_least(3, 14) {
            format!(
                "digest_size for {} must be between 1 and {max_digest} bytes, here it is {digest_size}",
                if max_digest == 64 {
                    "Blake2b"
                } else {
                    "Blake2s"
                }
            )
        } else {
            format!("digest_size must be between 1 and {max_digest} bytes")
        };
        return Err(raise_exception::<u64>(py, "ValueError", &message));
    }
    if bytes(1).len() > max_salt {
        return Err(raise_exception::<u64>(
            py,
            "ValueError",
            &format!("maximum salt length is {max_salt} bytes"),
        ));
    }
    if bytes(2).len() > max_person {
        return Err(raise_exception::<u64>(
            py,
            "ValueError",
            &format!("maximum person length is {max_person} bytes"),
        ));
    }
    if !(0..=255).contains(&fanout) {
        return Err(raise_exception::<u64>(
            py,
            "ValueError",
            "fanout must be between 0 and 255",
        ));
    }
    if !(1..=255).contains(&depth) {
        return Err(raise_exception::<u64>(
            py,
            "ValueError",
            "depth must be between 1 and 255",
        ));
    }
    if leaf_size > u32::MAX as u64 {
        return Err(raise_exception::<u64>(
            py,
            "OverflowError",
            "leaf_size is too large",
        ));
    }
    if max_digest == 32 && node_offset > 0xffff_ffff_ffff {
        return Err(raise_exception::<u64>(
            py,
            "OverflowError",
            "node_offset is too large",
        ));
    }
    if !(0..=255).contains(&node_depth) {
        return Err(raise_exception::<u64>(
            py,
            "ValueError",
            "node_depth must be between 0 and 255",
        ));
    }
    if inner_size < 0 || inner_size as usize > max_digest {
        return Err(raise_exception::<u64>(
            py,
            "ValueError",
            &format!("inner_size must be between 0 and is {max_digest}"),
        ));
    }
    if bytes(0).len() > max_key {
        return Err(raise_exception::<u64>(
            py,
            "ValueError",
            &format!("maximum key length is {max_key} bytes"),
        ));
    }
    Ok(Blake2Options {
        digest_size: digest_size as usize,
        key: bytes(0).to_vec(),
        salt: bytes(1).to_vec(),
        person: bytes(2).to_vec(),
        fanout: fanout as u8,
        depth: depth as u8,
        leaf_size: leaf_size as u32,
        node_offset,
        node_depth: node_depth as u8,
        inner_size: inner_size as usize,
        last_node,
    })
}

pub fn build_hash_handle<'py>(
    _py: &'py PyToken,
    name: &str,
    values: &[Option<OwnedRuntimeValue<'py>>],
    inputs: &mut [Option<ByteInput<'py>>; 3],
    unsupported_class: Option<u64>,
) -> Result<HashHandle, u64> {
    let normalized = normalize_hash_name(name);
    match normalized.unwrap_or("") {
        #[cfg(feature = "legacy")]
        "md4" => Ok(HashHandle {
            kind: HashKind::Md4(Md4::new()),
            name: "md4",
            digest_size: 16,
            block_size: 64,
            is_xof: false,
        }),
        "md5" => Ok(HashHandle {
            kind: HashKind::Md5(Md5::new()),
            name: "md5",
            digest_size: 16,
            block_size: 64,
            is_xof: false,
        }),
        #[cfg(feature = "legacy")]
        "ripemd160" => Ok(HashHandle {
            kind: HashKind::Ripemd160(Ripemd160::new()),
            name: "ripemd160",
            digest_size: 20,
            block_size: 64,
            is_xof: false,
        }),
        "sha1" => Ok(HashHandle {
            kind: HashKind::Sha1(Sha1::new()),
            name: "sha1",
            digest_size: 20,
            block_size: 64,
            is_xof: false,
        }),
        "sha224" => Ok(HashHandle {
            kind: HashKind::Sha224(Sha224::new()),
            name: "sha224",
            digest_size: 28,
            block_size: 64,
            is_xof: false,
        }),
        "sha256" => Ok(HashHandle {
            kind: HashKind::Sha256(Sha256::new()),
            name: "sha256",
            digest_size: 32,
            block_size: 64,
            is_xof: false,
        }),
        "sha384" => Ok(HashHandle {
            kind: HashKind::Sha384(Sha384::new()),
            name: "sha384",
            digest_size: 48,
            block_size: 128,
            is_xof: false,
        }),
        "sha512" => Ok(HashHandle {
            kind: HashKind::Sha512(Sha512::new()),
            name: "sha512",
            digest_size: 64,
            block_size: 128,
            is_xof: false,
        }),
        "sha512_224" => Ok(HashHandle {
            kind: HashKind::Sha512_224(Sha512_224::new()),
            name: "sha512_224",
            digest_size: 28,
            block_size: 128,
            is_xof: false,
        }),
        "sha512_256" => Ok(HashHandle {
            kind: HashKind::Sha512_256(Sha512_256::new()),
            name: "sha512_256",
            digest_size: 32,
            block_size: 128,
            is_xof: false,
        }),
        "sha3_224" => Ok(HashHandle {
            kind: HashKind::Sha3_224(Sha3_224::new()),
            name: "sha3_224",
            digest_size: 28,
            block_size: 144,
            is_xof: false,
        }),
        "sha3_256" => Ok(HashHandle {
            kind: HashKind::Sha3_256(Sha3_256::new()),
            name: "sha3_256",
            digest_size: 32,
            block_size: 136,
            is_xof: false,
        }),
        "sha3_384" => Ok(HashHandle {
            kind: HashKind::Sha3_384(Sha3_384::new()),
            name: "sha3_384",
            digest_size: 48,
            block_size: 104,
            is_xof: false,
        }),
        "sha3_512" => Ok(HashHandle {
            kind: HashKind::Sha3_512(Sha3_512::new()),
            name: "sha3_512",
            digest_size: 64,
            block_size: 72,
            is_xof: false,
        }),
        "shake_128" => Ok(HashHandle {
            kind: HashKind::Shake128(Shake128::default()),
            name: "shake_128",
            digest_size: 0,
            block_size: 168,
            is_xof: true,
        }),
        "shake_256" => Ok(HashHandle {
            kind: HashKind::Shake256(Shake256::default()),
            name: "shake_256",
            digest_size: 0,
            block_size: 136,
            is_xof: true,
        }),
        "blake2b" => {
            let opts = parse_blake2_options(_py, values, 64, 64, 16, 16, inputs)?;
            let mut params = Blake2bParams::new();
            params.hash_length(opts.digest_size);
            if !opts.key.is_empty() {
                params.key(&opts.key);
            }
            if !opts.salt.is_empty() {
                params.salt(&opts.salt);
            }
            if !opts.person.is_empty() {
                params.personal(&opts.person);
            }
            params.fanout(opts.fanout);
            params.max_depth(opts.depth);
            params.max_leaf_length(opts.leaf_size);
            params.node_offset(opts.node_offset);
            params.node_depth(opts.node_depth);
            params.inner_hash_length(opts.inner_size);
            params.last_node(opts.last_node);
            Ok(HashHandle {
                kind: HashKind::Blake2b(params.to_state()),
                name: "blake2b",
                digest_size: opts.digest_size,
                block_size: 128,
                is_xof: false,
            })
        }
        "blake2s" => {
            let opts = parse_blake2_options(_py, values, 32, 32, 8, 8, inputs)?;
            let mut params = Blake2sParams::new();
            params.hash_length(opts.digest_size);
            if !opts.key.is_empty() {
                params.key(&opts.key);
            }
            if !opts.salt.is_empty() {
                params.salt(&opts.salt);
            }
            if !opts.person.is_empty() {
                params.personal(&opts.person);
            }
            params.fanout(opts.fanout);
            params.max_depth(opts.depth);
            params.max_leaf_length(opts.leaf_size);
            params.node_offset(opts.node_offset);
            params.node_depth(opts.node_depth);
            params.inner_hash_length(opts.inner_size);
            params.last_node(opts.last_node);
            Ok(HashHandle {
                kind: HashKind::Blake2s(params.to_state()),
                name: "blake2s",
                digest_size: opts.digest_size,
                block_size: 64,
                is_xof: false,
            })
        }
        _ => Err(match unsupported_class {
            Some(class) => molt_runtime_core::rt_raise_class(
                _py,
                class,
                "[digital envelope routines] unsupported",
            ),
            None => {
                raise_exception::<u64>(_py, "ValueError", &format!("unsupported hash type {name}"))
            }
        }),
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_hash_new(
    name_bits: u64,
    data_bits: u64,
    options_bits: u64,
    unsupported_error_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        // Retain the declared completion class before option/exporter callbacks.
        let unsupported_error = OwnedRuntimeValue::retain(_py, unsupported_error_bits);
        let unsupported_class = (!obj_from_bits(unsupported_error.bits()).is_none())
            .then_some(unsupported_error.bits());
        let name_obj = obj_from_bits(name_bits);
        let name = string_obj_to_owned(name_obj);
        let blake = matches!(name.as_deref(), Some("blake2b" | "blake2s"));
        // 3.12 EVP_new's Clinic wrapper converts security before its impl
        // validates the name. BLAKE's public named constructors have their own
        // option protocol; private EVP BLAKE still follows EVP's ordering.
        let provider_role = unsupported_class.is_some() || !blake;
        let legacy_provider = !rt_target_at_least(3, 13) && provider_role;
        let early_options = if legacy_provider {
            let values = match hash_options(_py, options_bits, &["usedforsecurity"]) {
                Ok(values) => values,
                Err(bits) => return bits,
            };
            if let Some(value) = &values[0] {
                if let Err(bits) = truth_from_bits(_py, value.bits()) {
                    return bits;
                }
            }
            Some(values)
        } else {
            None
        };
        let Some(name) = name else {
            if legacy_provider {
                return raise_exception::<u64>(_py, "TypeError", "name must be a string");
            }
            let actual_type = if name_obj.is_none() {
                "None".to_owned()
            } else {
                type_name(_py, name_obj).to_string()
            };
            return raise_exception::<u64>(
                _py,
                "TypeError",
                &format!("new() argument 'name' must be str, not {actual_type}"),
            );
        };
        if name.contains('\0') {
            return if legacy_provider {
                raise_exception::<u64>(_py, "TypeError", "name must be a string")
            } else {
                raise_exception::<u64>(_py, "ValueError", "embedded null character")
            };
        }
        let mut option_values = if let Some(values) = early_options {
            values
        } else {
            match hash_options(
                _py,
                options_bits,
                if provider_role {
                    &["usedforsecurity"]
                } else {
                    HASH_OPTION_NAMES
                },
            ) {
                Ok(values) => values,
                Err(bits) => return bits,
            }
        };
        let mut option_inputs = [None, None, None];
        let mut data = None;
        if provider_role {
            if let Some(value) = option_values[0].as_ref().filter(|_| !legacy_provider) {
                if let Err(bits) = truth_from_bits(_py, value.bits()) {
                    return bits;
                }
            }
            data = Some(match hash_input(_py, data_bits) {
                Ok(input) => input,
                Err(bits) => return bits,
            });
        }
        if provider_role && blake {
            // Private EVP BLAKE has already converted security and acquired data.
            // Reuse the same state builder with builtin defaults, never replaying
            // the security callback or moving data acquisition behind construction.
            option_values.clear();
            option_values.resize_with(HASH_OPTION_NAMES.len(), || None);
        }
        let mut handle = match build_hash_handle(
            _py,
            &name,
            &option_values,
            &mut option_inputs,
            unsupported_class,
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if !provider_role {
            data = Some(match hash_input(_py, data_bits) {
                Ok(input) => input,
                Err(bits) => return bits,
            });
        }
        handle.update(unsafe { data.as_ref().unwrap().bytes(_py) });
        let name = handle.name;
        let digest_size = handle.digest_size;
        let block_size = handle.block_size;
        let handle = unsafe { OwnedRuntimeValue::from_owned_bits(_py, new_hash_owner(handle)) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let name = unsafe { OwnedRuntimeValue::from_owned_bits(_py, rt_string_from(name)) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let size = unsafe {
            OwnedRuntimeValue::from_owned_bits(_py, int_bits_from_i64(_py, digest_size as i64))
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let block_size = unsafe {
            OwnedRuntimeValue::from_owned_bits(_py, int_bits_from_i64(_py, block_size as i64))
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        rt_tuple(&[handle.bits(), name.bits(), size.bits(), block_size.bits()])
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_hash_update(handle_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(owner) = hash_owner(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid hash handle");
        };
        let data = match hash_input(_py, data_bits) {
            Ok(slice) => slice,
            Err(bits) => return bits,
        };
        {
            let mut handle = owner.lock().unwrap_or_else(|poison| poison.into_inner());
            handle.update(unsafe { data.bytes(_py) });
        }
        MoltObject::none().bits()
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_hash_copy(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(owner) = hash_owner(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid hash handle");
        };
        let copy = owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        new_hash_owner(copy)
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_hash_digest(handle_bits: u64, length_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(owner) = hash_owner(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid hash handle");
        };
        let length_obj = obj_from_bits(length_bits);
        let is_xof = owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .is_xof;
        let length = if is_xof {
            if length_obj.is_none() {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "digest() missing required argument 'length' (pos 1)",
                );
            }
            let val = match ssize_from_bits(_py, length_bits) {
                Ok(val) => val,
                Err(bits) => return bits,
            };
            if val < 0 {
                return raise_exception::<u64>(
                    _py,
                    "SystemError",
                    "Negative size passed to PyBytes_FromStringAndSize",
                );
            }
            Some(val as usize)
        } else {
            if !length_obj.is_none() {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "HASH.digest() takes no arguments (1 given)",
                );
            }
            None
        };
        let finalized = {
            let handle = owner.lock().unwrap_or_else(|poison| poison.into_inner());
            handle.finalize_bytes(length)
        };
        let out = match finalized {
            Ok(bytes) => bytes,
            Err(HashError::XofLengthMissing) => {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "digest() missing required argument 'length' (pos 1)",
                );
            }
        };
        let ptr = alloc_bytes(_py, &out);
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}
fn pbkdf2_digest_size(name: &str) -> Option<usize> {
    match name {
        #[cfg(feature = "legacy")]
        "md4" => Some(16),
        "md5" => Some(16),
        #[cfg(feature = "legacy")]
        "ripemd160" => Some(20),
        "sha1" => Some(20),
        "sha224" => Some(28),
        "sha256" => Some(32),
        "sha384" => Some(48),
        "sha512" => Some(64),
        "sha512_224" => Some(28),
        "sha512_256" => Some(32),
        "sha3_224" => Some(28),
        "sha3_256" => Some(32),
        "sha3_384" => Some(48),
        "sha3_512" => Some(64),
        _ => None,
    }
}

fn checked_input_length(py: &PyToken, input: &ByteInput<'_>, name: &str) -> Result<(), u64> {
    if unsafe { input.bytes(py) }.len() > i32::MAX as usize {
        return Err(raise_exception::<u64>(
            py,
            "OverflowError",
            &format!("{name} is too long."),
        ));
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pbkdf2_hmac(
    name_bits: u64,
    password_bits: u64,
    salt_bits: u64,
    iterations_bits: u64,
    dklen_bits: u64,
    unsupported_error_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let unsupported_error = OwnedRuntimeValue::retain(_py, unsupported_error_bits);
        let name_obj = obj_from_bits(name_bits);
        let Some(name) = string_obj_to_owned(name_obj) else {
            let type_name = type_name(_py, name_obj);
            let msg = format!("pbkdf2_hmac() argument 'hash_name' must be str, not {type_name}");
            return raise_exception::<u64>(_py, "TypeError", &msg);
        };
        if name.contains('\0') {
            return raise_exception::<u64>(_py, "ValueError", "embedded null character");
        }
        // Clinic owns password then salt and retires them in that same order.
        let mut inputs = [None, None];
        for (index, bits) in [password_bits, salt_bits].into_iter().enumerate() {
            inputs[index] = Some(
                match clinic_input(_py, bits, "pbkdf2_hmac", ["password", "salt"][index]) {
                    Ok(input) => input,
                    Err(bits) => return bits,
                },
            );
        }
        let iterations = match c_long_from_bits(_py, iterations_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(normalized) = normalize_hash_name(&name) else {
            return molt_runtime_core::rt_raise_class(
                _py,
                unsupported_error.bits(),
                "[digital envelope routines] unsupported",
            );
        };
        let Some(default_dklen) = pbkdf2_digest_size(normalized) else {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "[digital envelope routines] unsupported",
            );
        };
        for (input, name) in inputs.iter().zip(["password", "salt"]) {
            if let Err(bits) = checked_input_length(_py, input.as_ref().unwrap(), name) {
                return bits;
            }
        }
        if iterations < 1 {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "iteration value must be greater than 0.",
            );
        }
        if iterations > i32::MAX as c_long {
            return raise_exception::<u64>(_py, "OverflowError", "iteration value is too great.");
        }
        let rounds = iterations as u32;
        let dklen = if obj_from_bits(dklen_bits).is_none() {
            default_dklen as c_long
        } else {
            match c_long_from_bits(_py, dklen_bits) {
                Ok(value) => value,
                Err(bits) => return bits,
            }
        };
        if dklen < 1 {
            return raise_exception::<u64>(_py, "ValueError", "key length must be greater than 0.");
        }
        if dklen > i32::MAX as c_long {
            return raise_exception::<u64>(_py, "OverflowError", "key length is too great.");
        }
        let dklen = dklen as usize;
        let mut out = Vec::new();
        if out.try_reserve_exact(dklen).is_err() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        out.resize(dklen, 0);
        let password = unsafe { inputs[0].as_ref().unwrap().bytes(_py) };
        let salt = unsafe { inputs[1].as_ref().unwrap().bytes(_py) };
        match normalized {
            #[cfg(feature = "legacy")]
            "md4" => pbkdf2::pbkdf2_hmac::<Md4>(password, salt, rounds, &mut out),
            "md5" => pbkdf2::pbkdf2_hmac::<Md5>(password, salt, rounds, &mut out),
            #[cfg(feature = "legacy")]
            "ripemd160" => pbkdf2::pbkdf2_hmac::<Ripemd160>(password, salt, rounds, &mut out),
            "sha1" => pbkdf2::pbkdf2_hmac::<Sha1>(password, salt, rounds, &mut out),
            "sha224" => pbkdf2::pbkdf2_hmac::<Sha224>(password, salt, rounds, &mut out),
            "sha256" => pbkdf2::pbkdf2_hmac::<Sha256>(password, salt, rounds, &mut out),
            "sha384" => pbkdf2::pbkdf2_hmac::<Sha384>(password, salt, rounds, &mut out),
            "sha512" => pbkdf2::pbkdf2_hmac::<Sha512>(password, salt, rounds, &mut out),
            "sha512_224" => pbkdf2::pbkdf2_hmac::<Sha512_224>(password, salt, rounds, &mut out),
            "sha512_256" => pbkdf2::pbkdf2_hmac::<Sha512_256>(password, salt, rounds, &mut out),
            "sha3_224" => {
                pbkdf2::pbkdf2::<hmac::SimpleHmac<Sha3_224>>(password, salt, rounds, &mut out)
                    .expect("HMAC can be initialized with any key length")
            }
            "sha3_256" => {
                pbkdf2::pbkdf2::<hmac::SimpleHmac<Sha3_256>>(password, salt, rounds, &mut out)
                    .expect("HMAC can be initialized with any key length")
            }
            "sha3_384" => {
                pbkdf2::pbkdf2::<hmac::SimpleHmac<Sha3_384>>(password, salt, rounds, &mut out)
                    .expect("HMAC can be initialized with any key length")
            }
            "sha3_512" => {
                pbkdf2::pbkdf2::<hmac::SimpleHmac<Sha3_512>>(password, salt, rounds, &mut out)
                    .expect("HMAC can be initialized with any key length")
            }
            _ => {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "[digital envelope routines] unsupported",
                );
            }
        }
        let ptr = alloc_bytes(_py, &out);
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

fn scrypt_legacy_unsigned(py: &PyToken, bits: u64, name: &str) -> Result<u64, u64> {
    native_number(py, bits, |object| unsafe {
        let value = numbers::PyLong_AsUnsignedLong(object);
        if !abi::api::errors::PyErr_Occurred().is_null() {
            let message =
                std::ffi::CString::new(format!("{name} is required and must be an unsigned int"))
                    .unwrap();
            abi::api::errors::PyErr_SetString(
                (&raw mut abi::abi_types::PyExc_TypeError).cast(),
                message.as_ptr(),
            );
        }
        value as u64
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_scrypt(
    password_bits: u64,
    salt_bits: u64,
    n_bits: u64,
    r_bits: u64,
    p_bits: u64,
    maxmem_bits: u64,
    dklen_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let mut inputs = [None, None];
        for (index, bits) in [password_bits, salt_bits].into_iter().enumerate() {
            inputs[index] = Some(
                match clinic_input(_py, bits, "scrypt", ["password", "salt"][index]) {
                    Ok(input) => input,
                    Err(bits) => return bits,
                },
            );
        }
        let modern = rt_target_at_least(3, 14);
        let mut nrp = [0u64; 3];
        for (index, (bits, name)) in [(n_bits, "n"), (r_bits, "r"), (p_bits, "p")]
            .into_iter()
            .enumerate()
        {
            if modern {
                nrp[index] = match c_ulong_from_bits(_py, bits) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
            } else {
                let is_int = match native_number(_py, bits, |object| unsafe {
                    numbers::PyLong_Check(object) != 0
                }) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                if !is_int {
                    return raise_exception::<u64>(
                        _py,
                        "TypeError",
                        &format!(
                            "scrypt() argument '{name}' must be int, not {}",
                            type_name(_py, obj_from_bits(bits))
                        ),
                    );
                }
            }
        }
        let maxmem = match c_long_from_bits(_py, maxmem_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let dklen = match c_long_from_bits(_py, dklen_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        for (input, name) in inputs.iter().zip(["password", "salt"]) {
            if let Err(bits) = checked_input_length(_py, input.as_ref().unwrap(), name) {
                return bits;
            }
        }
        if !modern {
            nrp[0] = match scrypt_legacy_unsigned(_py, n_bits, "n") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        }
        let n = nrp[0];
        if n < 2 || !n.is_power_of_two() {
            return raise_exception::<u64>(_py, "ValueError", "n must be a power of 2.");
        }
        if !modern {
            nrp[1] = match scrypt_legacy_unsigned(_py, r_bits, "r") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
            nrp[2] = match scrypt_legacy_unsigned(_py, p_bits, "p") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        }
        let [_, r, p] = nrp;
        if !(0..=i32::MAX as c_long).contains(&maxmem) {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "maxmem must be positive and smaller than 2147483647",
            );
        }
        if dklen < 1 || dklen > i32::MAX as c_long {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "dklen must be greater than 0 and smaller than 2147483647",
            );
        }
        if r > u32::MAX as u64 || p > u32::MAX as u64 {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "[digital envelope routines] parameter too large",
            );
        }
        if r == 0 || p == 0 {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "Invalid parameter combination for n, r, p, maxmem.",
            );
        }
        if u128::from(r) * u128::from(p) >= (1u128 << 30) || (r < 4 && n >= (1u64 << (16 * r))) {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "[digital envelope routines] memory limit exceeded",
            );
        }
        let log_n = (64 - n.leading_zeros() - 1) as u8;
        let params = match scrypt::Params::new(log_n, r as u32, p as u32) {
            Ok(val) => val,
            Err(_) => {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "Invalid parameter combination for n, r, p, maxmem.",
                );
            }
        };
        // OpenSSL's default memory ceiling is 32 MiB. Count in a wider
        // domain before allocation; neither saturation nor a signed cast may
        // turn an oversized request into an admitted one.
        let limit = if maxmem == 0 {
            32u128 * 1024 * 1024
        } else {
            maxmem as u128
        };
        let required = 128u128 * u128::from(r) * (u128::from(n) + u128::from(p) + 2);
        if required > limit {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "[digital envelope routines] memory limit exceeded",
            );
        }
        let dklen = dklen as usize;
        let mut out = Vec::new();
        if out.try_reserve_exact(dklen).is_err() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        out.resize(dklen, 0);
        let password = unsafe { inputs[0].as_ref().unwrap().bytes(_py) };
        let salt = unsafe { inputs[1].as_ref().unwrap().bytes(_py) };
        if scrypt::scrypt(password, salt, &params, &mut out).is_err() {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "Invalid parameter combination for n, r, p, maxmem.",
            );
        }
        let ptr = alloc_bytes(_py, &out);
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[cfg(test)]
mod name_admission_tests {
    use super::{builtin_hmac_name, normalize_hash_name};

    #[test]
    fn provider_names_keep_cache_case_and_whitespace_boundaries() {
        for (name, expected) in [
            ("SHA-512/224", "sha512_224"),
            ("sha512-256", "sha512_256"),
            ("SHA512_224", "sha512_224"),
            ("SHA256 ", "sha256"),
            ("SHA3-256", "sha3_256"),
            ("sha3_256", "sha3_256"),
            ("blake2b", "blake2b"),
        ] {
            assert_eq!(normalize_hash_name(name), Some(expected), "{name:?}");
        }
        for name in [
            "sha512/224",
            "sha512/256",
            "Sha512_224",
            "SHA3_256",
            "BLAKE2B",
            " sha256",
            "sha256\t",
            "sha256\n",
            "sha256\u{a0}",
            "sha3_256 ",
            "sha512_224 ",
            "blake2b ",
        ] {
            assert_eq!(normalize_hash_name(name), None, "{name:?}");
        }
    }

    #[test]
    fn builtin_hmac_names_are_a_distinct_exact_codec_subset() {
        assert_eq!(builtin_hmac_name("sha3_256"), Some("sha3_256"));
        assert_eq!(builtin_hmac_name("sha2_256"), Some("sha256"));
        assert_eq!(builtin_hmac_name("blake2b_32"), Some("blake2b"));
        for name in [
            "SHA3_256",
            "sha3-256",
            "sha256 ",
            "md4",
            "ripemd160",
            "sha512_224",
            "shake_128",
        ] {
            assert_eq!(builtin_hmac_name(name), None, "{name:?}");
        }
    }
}
