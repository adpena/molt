use crate::builtins::containers::dict_len;
use crate::builtins::numbers::index_bigint_from_obj;
use crate::*;
use blake2b_simd::{Params as Blake2bParams, State as Blake2bState};
use blake2s_simd::{Params as Blake2sParams, State as Blake2sState};
use digest::{Digest, ExtendableOutput, Update, XofReader};
use md5::Md5;
use num_traits::ToPrimitive;
use sha1::Sha1;
use sha2::{Sha224, Sha256, Sha384, Sha512};
use sha3::{Sha3_224, Sha3_256, Sha3_384, Sha3_512, Shake128, Shake256};

#[derive(Clone)]
pub(crate) enum HashKind {
    Md5(Md5),
    Sha1(Sha1),
    Sha224(Sha224),
    Sha256(Sha256),
    Sha384(Sha384),
    Sha512(Sha512),
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
pub(crate) struct HashHandle {
    pub(crate) kind: HashKind,
    pub(crate) name: &'static str,
    pub(crate) digest_size: usize,
    pub(crate) block_size: usize,
    pub(crate) is_xof: bool,
}

impl HashHandle {
    pub(crate) fn update(&mut self, data: &[u8]) {
        self.kind.update(data);
    }

    pub(crate) fn finalize_bytes(&self, length: Option<usize>) -> Result<Vec<u8>, HashError> {
        self.kind.clone().finalize_bytes(length)
    }
}

#[derive(Debug)]
pub(crate) enum HashError {
    XofLengthMissing,
    XofLengthNegative,
}

impl HashKind {
    fn update(&mut self, data: &[u8]) {
        match self {
            HashKind::Md5(hasher) => {
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
            HashKind::Md5(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Sha1(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Sha224(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Sha256(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Sha384(hasher) => Ok(hasher.finalize().to_vec()),
            HashKind::Sha512(hasher) => Ok(hasher.finalize().to_vec()),
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

pub(crate) fn normalize_hash_name(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    match lower.as_str() {
        "sha-1" => "sha1".to_string(),
        "sha-224" => "sha224".to_string(),
        "sha-256" => "sha256".to_string(),
        "sha-384" => "sha384".to_string(),
        "sha-512" => "sha512".to_string(),
        "sha3-224" => "sha3_224".to_string(),
        "sha3-256" => "sha3_256".to_string(),
        "sha3-384" => "sha3_384".to_string(),
        "sha3-512" => "sha3_512".to_string(),
        "shake-128" | "shake128" => "shake_128".to_string(),
        "shake-256" | "shake256" => "shake_256".to_string(),
        _ => lower,
    }
}

fn hash_handle_from_bits(bits: u64) -> Option<&'static mut HashHandle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { &mut *(ptr as *mut HashHandle) })
}

fn bytes_like_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<&'static [u8], u64> {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "object supporting the buffer API required",
        ));
    };
    unsafe {
        if object_type_id(ptr) == TYPE_ID_STRING {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "Strings must be encoded before hashing",
            ));
        }
        if let Some(slice) = bytes_like_slice(ptr) {
            return Ok(slice);
        }
    }
    Err(raise_exception::<u64>(
        _py,
        "TypeError",
        "object supporting the buffer API required",
    ))
}

fn bytes_like_required(
    _py: &PyToken<'_>,
    bits: u64,
    label: &str,
) -> Result<&'static [u8], u64> {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        let msg = format!("a bytes-like object is required, not '{label}'");
        return Err(raise_exception::<u64>(_py, "TypeError", &msg));
    };
    unsafe {
        if let Some(slice) = bytes_like_slice(ptr) {
            return Ok(slice);
        }
    }
    let type_name = type_name(_py, obj);
    let msg = format!("a bytes-like object is required, not '{type_name}'");
    Err(raise_exception::<u64>(_py, "TypeError", &msg))
}

fn int_from_bits(
    _py: &PyToken<'_>,
    bits: u64,
    name: &str,
) -> Result<i64, u64> {
    let obj = obj_from_bits(bits);
    let type_name = type_name(_py, obj);
    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
    let val = index_i64_from_obj(_py, bits, &msg);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(val)
}

fn bigint_from_bits(
    _py: &PyToken<'_>,
    bits: u64,
    name: &str,
    overflow: &str,
) -> Result<i64, u64> {
    let obj = obj_from_bits(bits);
    let type_name = type_name(_py, obj);
    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
    let Some(value) = index_bigint_from_obj(_py, bits, &msg) else {
        return Err(MoltObject::none().bits());
    };
    if let Some(i) = value.to_i64() {
        return Ok(i);
    }
    Err(raise_exception::<u64>(_py, "OverflowError", overflow))
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

fn parse_blake2_options(
    _py: &PyToken<'_>,
    options_bits: u64,
    max_digest: usize,
    max_key: usize,
    max_salt: usize,
    max_person: usize,
) -> Result<Blake2Options, u64> {
    let mut opts = Blake2Options {
        digest_size: max_digest,
        key: Vec::new(),
        salt: Vec::new(),
        person: Vec::new(),
        fanout: 1,
        depth: 1,
        leaf_size: 0,
        node_offset: 0,
        node_depth: 0,
        inner_size: 0,
        last_node: false,
    };
    let obj = obj_from_bits(options_bits);
    if obj.is_none() {
        return Ok(opts);
    }
    let Some(ptr) = obj.as_ptr() else {
        return Ok(opts);
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "hashlib options must be dict",
            ));
        }
        if dict_len(ptr) == 0 {
            return Ok(opts);
        }
    }
    let get_opt = |key: &str| -> Option<u64> {
        let key_ptr = alloc_string(_py, key.as_bytes());
        if key_ptr.is_null() {
            return None;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let value = unsafe { dict_get_in_place(_py, ptr, key_bits) };
        dec_ref_bits(_py, key_bits);
        value
    };
    if let Some(bits) = get_opt("digest_size") {
        let val = int_from_bits(_py, bits, "digest_size")?;
        if val < 1 || val as usize > max_digest {
            let msg = format!("digest_size must be between 1 and {max_digest} bytes");
            return Err(raise_exception::<u64>(_py, "ValueError", &msg));
        }
        opts.digest_size = val as usize;
    }
    if let Some(bits) = get_opt("key") {
        let key = bytes_like_required(_py, bits, "key")?.to_vec();
        if key.len() > max_key {
            let msg = format!("maximum key length is {max_key} bytes");
            return Err(raise_exception::<u64>(_py, "ValueError", &msg));
        }
        opts.key = key;
    }
    if let Some(bits) = get_opt("salt") {
        let salt = bytes_like_required(_py, bits, "salt")?.to_vec();
        if salt.len() > max_salt {
            let msg = format!("maximum salt length is {max_salt} bytes");
            return Err(raise_exception::<u64>(_py, "ValueError", &msg));
        }
        opts.salt = salt;
    }
    if let Some(bits) = get_opt("person") {
        let person = bytes_like_required(_py, bits, "person")?.to_vec();
        if person.len() > max_person {
            let msg = format!("maximum person length is {max_person} bytes");
            return Err(raise_exception::<u64>(_py, "ValueError", &msg));
        }
        opts.person = person;
    }
    if let Some(bits) = get_opt("fanout") {
        let val = int_from_bits(_py, bits, "fanout")?;
        if !(0..=255).contains(&val) {
            return Err(raise_exception::<u64>(
                _py,
                "ValueError",
                "fanout must be between 0 and 255",
            ));
        }
        opts.fanout = val as u8;
    }
    if let Some(bits) = get_opt("depth") {
        let val = int_from_bits(_py, bits, "depth")?;
        if !(1..=255).contains(&val) {
            return Err(raise_exception::<u64>(
                _py,
                "ValueError",
                "depth must be between 1 and 255",
            ));
        }
        opts.depth = val as u8;
    }
    if let Some(bits) = get_opt("leaf_size") {
        let val = int_from_bits(_py, bits, "leaf_size")?;
        if val < 0 {
            return Err(raise_exception::<u64>(
                _py,
                "ValueError",
                "value must be positive",
            ));
        }
        opts.leaf_size = val as u32;
    }
    if let Some(bits) = get_opt("node_offset") {
        let val = int_from_bits(_py, bits, "node_offset")?;
        if val < 0 {
            return Err(raise_exception::<u64>(
                _py,
                "ValueError",
                "value must be positive",
            ));
        }
        opts.node_offset = val as u64;
    }
    if let Some(bits) = get_opt("node_depth") {
        let val = int_from_bits(_py, bits, "node_depth")?;
        if !(0..=255).contains(&val) {
            return Err(raise_exception::<u64>(
                _py,
                "ValueError",
                "node_depth must be between 0 and 255",
            ));
        }
        opts.node_depth = val as u8;
    }
    if let Some(bits) = get_opt("inner_size") {
        let val = int_from_bits(_py, bits, "inner_size")?;
        if val < 0 || val as usize > max_digest {
            let msg = format!("inner_size must be between 0 and {max_digest}");
            return Err(raise_exception::<u64>(_py, "ValueError", &msg));
        }
        opts.inner_size = val as usize;
    }
    if let Some(bits) = get_opt("last_node") {
        opts.last_node = is_truthy(_py, obj_from_bits(bits));
    }
    Ok(opts)
}

pub(crate) fn build_hash_handle(
    _py: &PyToken<'_>,
    name: &str,
    options_bits: u64,
) -> Result<HashHandle, u64> {
    let normalized = normalize_hash_name(name);
    match normalized.as_str() {
        "md5" => Ok(HashHandle {
            kind: HashKind::Md5(Md5::new()),
            name: "md5",
            digest_size: 16,
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
            let opts = parse_blake2_options(_py, options_bits, 64, 64, 16, 16)?;
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
            let opts = parse_blake2_options(_py, options_bits, 32, 32, 8, 8)?;
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
        _ => Err(raise_exception::<u64>(
            _py,
            "ValueError",
            &format!("unsupported hash type {name}"),
        )),
    }
}

#[no_mangle]
pub extern "C" fn molt_hash_new(name_bits: u64, data_bits: u64, options_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name) = string_obj_to_owned(name_obj) else {
            return raise_exception::<u64>(_py, "TypeError", "hash name must be str");
        };
        let mut handle = match build_hash_handle(_py, &name, options_bits) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let data = match bytes_like_from_bits(_py, data_bits) {
            Ok(slice) => slice,
            Err(bits) => return bits,
        };
        if !data.is_empty() {
            handle.update(data);
        }
        let ptr = Box::into_raw(Box::new(handle)) as *mut u8;
        bits_from_ptr(ptr)
    })
}

#[no_mangle]
pub extern "C" fn molt_hash_update(handle_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = hash_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid hash handle");
        };
        let data = match bytes_like_from_bits(_py, data_bits) {
            Ok(slice) => slice,
            Err(bits) => return bits,
        };
        if !data.is_empty() {
            handle.update(data);
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_hash_copy(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = hash_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid hash handle");
        };
        let copy = handle.clone();
        let ptr = Box::into_raw(Box::new(copy)) as *mut u8;
        bits_from_ptr(ptr)
    })
}

#[no_mangle]
pub extern "C" fn molt_hash_digest(handle_bits: u64, length_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(handle) = hash_handle_from_bits(handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "invalid hash handle");
        };
        let length_obj = obj_from_bits(length_bits);
        let length = if handle.is_xof {
            if length_obj.is_none() {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "digest() missing required argument 'length' (pos 1)",
                );
            }
            let val = match int_from_bits(_py, length_bits, "length") {
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
        let out = match handle.finalize_bytes(length) {
            Ok(bytes) => bytes,
            Err(HashError::XofLengthMissing) => {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "digest() missing required argument 'length' (pos 1)",
                )
            }
            Err(HashError::XofLengthNegative) => {
                return raise_exception::<u64>(
                    _py,
                    "SystemError",
                    "Negative size passed to PyBytes_FromStringAndSize",
                )
            }
        };
        let ptr = alloc_bytes(_py, &out);
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_hash_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(handle_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        let _ = unsafe { Box::from_raw(ptr as *mut HashHandle) };
        MoltObject::none().bits()
    })
}

fn pbkdf2_digest_size(name: &str) -> Option<usize> {
    match name {
        "md5" => Some(16),
        "sha1" => Some(20),
        "sha224" => Some(28),
        "sha256" => Some(32),
        "sha384" => Some(48),
        "sha512" => Some(64),
        "sha3_224" => Some(28),
        "sha3_256" => Some(32),
        "sha3_384" => Some(48),
        "sha3_512" => Some(64),
        _ => None,
    }
}

fn pbkdf2_rounds(_py: &PyToken<'_>, bits: u64) -> Result<u32, u64> {
    let value = bigint_from_bits(
        _py,
        bits,
        "iterations",
        "Python int too large to convert to C long",
    )?;
    if value <= 0 {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "iteration value must be greater than 0.",
        ));
    }
    if value > u32::MAX as i64 {
        return Err(raise_exception::<u64>(
            _py,
            "OverflowError",
            "iteration value is too great.",
        ));
    }
    Ok(value as u32)
}

fn pbkdf2_dklen(_py: &PyToken<'_>, bits: u64) -> Result<usize, u64> {
    let value = bigint_from_bits(
        _py,
        bits,
        "dklen",
        "Python int too large to convert to C long",
    )?;
    if value <= 0 {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "key length must be greater than 0.",
        ));
    }
    if value > u32::MAX as i64 {
        return Err(raise_exception::<u64>(
            _py,
            "OverflowError",
            "key length is too great.",
        ));
    }
    Ok(value as usize)
}

#[no_mangle]
pub extern "C" fn molt_pbkdf2_hmac(
    name_bits: u64,
    password_bits: u64,
    salt_bits: u64,
    iterations_bits: u64,
    dklen_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name) = string_obj_to_owned(name_obj) else {
            let type_name = type_name(_py, name_obj);
            let msg = format!(
                "pbkdf2_hmac() argument 'hash_name' must be str, not {type_name}"
            );
            return raise_exception::<u64>(_py, "TypeError", &msg);
        };
        let normalized = normalize_hash_name(&name);
        let Some(default_dklen) = pbkdf2_digest_size(normalized.as_str()) else {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "[digital envelope routines] unsupported",
            );
        };
        let password = match bytes_like_required(_py, password_bits, "password") {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let salt = match bytes_like_required(_py, salt_bits, "salt") {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let rounds = match pbkdf2_rounds(_py, iterations_bits) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let dklen = if obj_from_bits(dklen_bits).is_none() {
            default_dklen
        } else {
            match pbkdf2_dklen(_py, dklen_bits) {
                Ok(val) => val,
                Err(bits) => return bits,
            }
        };
        let mut out = Vec::new();
        if out.try_reserve_exact(dklen).is_err() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        out.resize(dklen, 0);
        match normalized.as_str() {
            "md5" => pbkdf2::pbkdf2_hmac::<Md5>(password, salt, rounds, &mut out),
            "sha1" => pbkdf2::pbkdf2_hmac::<Sha1>(password, salt, rounds, &mut out),
            "sha224" => pbkdf2::pbkdf2_hmac::<Sha224>(password, salt, rounds, &mut out),
            "sha256" => pbkdf2::pbkdf2_hmac::<Sha256>(password, salt, rounds, &mut out),
            "sha384" => pbkdf2::pbkdf2_hmac::<Sha384>(password, salt, rounds, &mut out),
            "sha512" => pbkdf2::pbkdf2_hmac::<Sha512>(password, salt, rounds, &mut out),
            "sha3_224" => pbkdf2::pbkdf2_hmac::<Sha3_224>(password, salt, rounds, &mut out),
            "sha3_256" => pbkdf2::pbkdf2_hmac::<Sha3_256>(password, salt, rounds, &mut out),
            "sha3_384" => pbkdf2::pbkdf2_hmac::<Sha3_384>(password, salt, rounds, &mut out),
            "sha3_512" => pbkdf2::pbkdf2_hmac::<Sha3_512>(password, salt, rounds, &mut out),
            _ => {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "[digital envelope routines] unsupported",
                )
            }
        }
        let ptr = alloc_bytes(_py, &out);
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

fn scrypt_int_required(
    _py: &PyToken<'_>,
    bits: u64,
    name: &str,
) -> Result<u64, u64> {
    let value = bigint_from_bits(
        _py,
        bits,
        name,
        "Python int too large to convert to C long",
    )?;
    if value < 0 {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("{name} is required and must be an unsigned int"),
        ));
    }
    Ok(value as u64)
}

#[no_mangle]
pub extern "C" fn molt_scrypt(
    password_bits: u64,
    salt_bits: u64,
    n_bits: u64,
    r_bits: u64,
    p_bits: u64,
    maxmem_bits: u64,
    dklen_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let password = match bytes_like_required(_py, password_bits, "password") {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let salt = match bytes_like_required(_py, salt_bits, "salt") {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let n = match scrypt_int_required(_py, n_bits, "n") {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let r = match scrypt_int_required(_py, r_bits, "r") {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let p = match scrypt_int_required(_py, p_bits, "p") {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        let maxmem = match bigint_from_bits(
            _py,
            maxmem_bits,
            "maxmem",
            "Python int too large to convert to C long",
        ) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        if maxmem < 0 || maxmem > 2_147_483_647 {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "maxmem must be positive and smaller than 2147483647",
            );
        }
        let dklen = match bigint_from_bits(
            _py,
            dklen_bits,
            "dklen",
            "Python int too large to convert to C long",
        ) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        if dklen <= 0 || dklen > 2_147_483_647 {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "dklen must be greater than 0 and smaller than 2147483647",
            );
        }
        if n <= 1 || (n & (n - 1)) != 0 {
            return raise_exception::<u64>(_py, "ValueError", "n must be a power of 2.");
        }
        if r == 0 || p == 0 {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "Invalid parameter combination for n, r, p, maxmem.",
            );
        }
        let log_n = (64 - (n as u64).leading_zeros() - 1) as u8;
        let params = match scrypt::Params::new(log_n, r as u32, p as u32, 32) {
            Ok(val) => val,
            Err(_) => {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "Invalid parameter combination for n, r, p, maxmem.",
                )
            }
        };
        if maxmem > 0 {
            let r128 = r.saturating_mul(128);
            let required = r128
                .saturating_mul(n.saturating_add(p).saturating_add(1));
            if required as i64 > maxmem {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "[digital envelope routines] memory limit exceeded",
                );
            }
        }
        let dklen = dklen as usize;
        let mut out = Vec::new();
        if out.try_reserve_exact(dklen).is_err() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        out.resize(dklen, 0);
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
