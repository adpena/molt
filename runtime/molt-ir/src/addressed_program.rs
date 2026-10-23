//! Content-addressed backend IR transport (backend daemon protocol 4, HF-516).
//!
//! A lease names its program as an ordered list of function digests and ships
//! the bodies its receiver lacks. A digest is SHA-256 over
//! [`FUNCTION_DIGEST_DOMAIN`] and the exact MessagePack bytes of one body; the
//! CLI's `molt.cli.function_records.function_body_digest` is the producer.
//! The receiver hashes every shipped body, so a body can only fill the slot its
//! own bytes name. One-shot receivers require every body; the backend daemon
//! supplies the rest from its resident store and asks again for the bodies it
//! no longer holds, as Bazel's `FindMissingBlobs` and `Execute` do.
//!
//! Validation splits along the same line: each admitted body passes
//! [`validate_function_transport_contract`] once, and every assembled program
//! passes [`validate_program_transport_contract`].

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha256};

use crate::code_slots::CodeSlotBindings;
use crate::content_digest::bytes_to_lower_hex;
use crate::ir::{
    BackendIrDocument, FunctionIR, ModuleRegistryIR, OpIR, PgoProfileIR, SimpleIR,
    validate_function_transport_contract, validate_module_slots,
    validate_program_transport_contract,
};

/// Domain tag of a function body digest. The CLI's
/// `molt.cli.function_records._FUNCTION_DIGEST_DOMAIN` spells the same bytes;
/// both sides pin the digest of an empty body to one literal in their tests.
pub const FUNCTION_DIGEST_DOMAIN: &[u8] = b"molt.function-body.msgpack.v1\0";

const DIGEST_BYTES: usize = 32;

/// The identity of one function body: SHA-256 over the domain tag and the
/// exact transported bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FunctionDigest([u8; DIGEST_BYTES]);

impl FunctionDigest {
    pub fn of_body(body: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(FUNCTION_DIGEST_DOMAIN);
        hasher.update(body);
        Self(hasher.finalize().into())
    }

    pub fn as_bytes(&self) -> &[u8; DIGEST_BYTES] {
        &self.0
    }

    /// Split `32·N` concatenated digest bytes, in order.
    pub fn split_concatenated(bytes: &[u8]) -> Result<Vec<Self>, String> {
        if !bytes.len().is_multiple_of(DIGEST_BYTES) {
            return Err(format!(
                "function digest list has {} bytes, not a multiple of {DIGEST_BYTES}",
                bytes.len()
            ));
        }
        Ok(bytes
            .as_chunks::<DIGEST_BYTES>()
            .0
            .iter()
            .map(|chunk| Self(*chunk))
            .collect())
    }

    /// Parse `64·N` concatenated lowercase hex digits, in order.
    pub fn parse_concatenated_hex(text: &str) -> Result<Vec<Self>, String> {
        let hex = text.as_bytes();
        if !hex.len().is_multiple_of(2 * DIGEST_BYTES) {
            return Err(format!(
                "function digest hex has {} digits, not a multiple of {}",
                hex.len(),
                2 * DIGEST_BYTES
            ));
        }
        fn nibble(digit: u8) -> Result<u8, String> {
            match digit {
                b'0'..=b'9' => Ok(digit - b'0'),
                b'a'..=b'f' => Ok(digit - b'a' + 10),
                _ => Err(format!(
                    "function digest hex contains {:?}; lowercase hex is required",
                    char::from(digit)
                )),
            }
        }
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        for [high, low] in hex.as_chunks::<2>().0 {
            bytes.push((nibble(*high)? << 4) | nibble(*low)?);
        }
        Self::split_concatenated(&bytes)
    }
}

impl fmt::Debug for FunctionDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "FunctionDigest({self})")
    }
}

impl fmt::Display for FunctionDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&bytes_to_lower_hex(&self.0))
    }
}

/// One MessagePack `bin` value, owned.
struct WireBytes(Vec<u8>);

impl<'de> Deserialize<'de> for WireBytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = WireBytes;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a MessagePack bin value")
            }
            fn visit_bytes<E: serde::de::Error>(self, bytes: &[u8]) -> Result<WireBytes, E> {
                Ok(WireBytes(bytes.to_vec()))
            }
            fn visit_byte_buf<E: serde::de::Error>(self, bytes: Vec<u8>) -> Result<WireBytes, E> {
                Ok(WireBytes(bytes))
            }
        }
        deserializer.deserialize_byte_buf(Visitor)
    }
}

/// One MessagePack `bin` value, borrowed.
struct WireBytesRef<'a>(&'a [u8]);

impl Serialize for WireBytesRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.0)
    }
}

/// The lease map. Keys are in lexical order, as the CLI writes them.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LeaseWire {
    #[serde(default)]
    code_slots: CodeSlotBindings,
    function_bodies: Vec<WireBytes>,
    function_digests: WireBytes,
    #[serde(default)]
    module_registry: Option<ModuleRegistryIR>,
    #[serde(default)]
    profile: Option<PgoProfileIR>,
}

#[derive(Serialize)]
struct LeaseWireRef<'a> {
    code_slots: &'a CodeSlotBindings,
    function_bodies: Vec<WireBytesRef<'a>>,
    function_digests: WireBytesRef<'a>,
    module_registry: &'a Option<ModuleRegistryIR>,
    profile: &'a Option<PgoProfileIR>,
}

/// Why a lease could not become a program.
#[derive(Debug, PartialEq, Eq)]
pub enum ProgramAssemblyError {
    /// Program positions whose digest has neither a shipped body nor a
    /// resident one; the sender ships them and asks again.
    MissingFunctions(Vec<usize>),
    /// The assembled program fails the cross-function transport contract.
    Invalid(String),
}

impl fmt::Display for ProgramAssemblyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFunctions(positions) => write!(
                formatter,
                "IR lease lacks {} function bodies (first at program position {})",
                positions.len(),
                positions.first().copied().unwrap_or_default()
            ),
            Self::Invalid(reason) => formatter.write_str(reason),
        }
    }
}

/// A decoded lease: the program order, the admitted shipped bodies and the
/// program header. Every shipped body matches its digest and passed
/// [`validate_function_transport_contract`].
pub struct AddressedProgram {
    digests: Vec<FunctionDigest>,
    shipped: HashMap<FunctionDigest, Arc<FunctionIR>>,
    profile: Option<PgoProfileIR>,
    module_registry: Option<ModuleRegistryIR>,
    code_slots: CodeSlotBindings,
}

impl AddressedProgram {
    /// Decode one lease and require that the reader holds nothing after it.
    /// The lease is read whole, then decoded from memory: one buffered read
    /// rather than one reader call per MessagePack marker.
    pub fn from_msgpack_reader(reader: &mut impl std::io::Read) -> Result<Self, String> {
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        Self::from_msgpack_bytes(&bytes)
    }

    /// Decode one lease that is exactly `bytes`.
    pub fn from_msgpack_bytes(bytes: &[u8]) -> Result<Self, String> {
        let mut deserializer = rmp_serde::Deserializer::new(std::io::Cursor::new(bytes));
        let wire = LeaseWire::deserialize(&mut deserializer).map_err(|error| error.to_string())?;
        if usize::try_from(deserializer.position()).ok() != Some(bytes.len()) {
            return Err("trailing bytes after the IR value".to_string());
        }
        Self::admit(wire)
    }

    fn admit(wire: LeaseWire) -> Result<Self, String> {
        let LeaseWire {
            code_slots,
            function_bodies,
            function_digests,
            module_registry,
            profile,
        } = wire;
        let digests = FunctionDigest::split_concatenated(&function_digests.0)?;
        drop(function_digests);
        let mut listed = HashSet::with_capacity(digests.len());
        for digest in &digests {
            if !listed.insert(*digest) {
                return Err(format!(
                    "function digest {digest} appears twice in the program"
                ));
            }
        }
        let mut shipped = HashMap::with_capacity(function_bodies.len());
        for WireBytes(body) in function_bodies {
            let digest = FunctionDigest::of_body(&body);
            if !listed.contains(&digest) {
                return Err(format!(
                    "shipped function body {digest} is not in the program's digest list"
                ));
            }
            let function: FunctionIR = rmp_serde::from_slice(&body)
                .map_err(|error| format!("function body {digest}: {error}"))?;
            drop(body);
            validate_function_transport_contract(&function)
                .map_err(|error| format!("invalid SimpleIR contract: {error}"))?;
            if shipped.insert(digest, Arc::new(function)).is_some() {
                return Err(format!("function body {digest} is shipped twice"));
            }
        }
        Ok(Self {
            digests,
            shipped,
            profile,
            module_registry,
            code_slots,
        })
    }

    /// The program's functions, in order.
    pub fn digests(&self) -> &[FunctionDigest] {
        &self.digests
    }

    pub fn ships(&self, digest: &FunctionDigest) -> bool {
        self.shipped.contains_key(digest)
    }

    /// The admitted shipped bodies. A store that keeps them clones these
    /// handles before [`Self::assemble`].
    pub fn shipped(&self) -> impl Iterator<Item = (FunctionDigest, &Arc<FunctionIR>)> {
        self.shipped
            .iter()
            .map(|(digest, function)| (*digest, function))
    }

    /// Compose the program in order. `resident(position, digest)` supplies a
    /// function the lease does not ship. A body whose handle is not shared
    /// moves into the program; a shared one is cloned.
    pub fn assemble(
        self,
        mut resident: impl FnMut(usize, &FunctionDigest) -> Option<Arc<FunctionIR>>,
    ) -> Result<BackendIrDocument, ProgramAssemblyError> {
        let Self {
            digests,
            mut shipped,
            profile,
            module_registry,
            code_slots,
        } = self;
        let mut handles = Vec::with_capacity(digests.len());
        let mut missing = Vec::new();
        for (position, digest) in digests.iter().enumerate() {
            match shipped
                .remove(digest)
                .or_else(|| resident(position, digest))
            {
                Some(function) => handles.push(function),
                None => missing.push(position),
            }
        }
        if !missing.is_empty() {
            return Err(ProgramAssemblyError::MissingFunctions(missing));
        }
        let functions: Vec<FunctionIR> = handles.into_iter().map(Arc::unwrap_or_clone).collect();
        let ir = SimpleIR { functions, profile };
        validate_program_transport_contract(&ir).map_err(|error| {
            ProgramAssemblyError::Invalid(format!("invalid SimpleIR contract: {error}"))
        })?;
        code_slots
            .validate_functions(&ir.functions)
            .map_err(|error| ProgramAssemblyError::Invalid(error.to_string()))?;
        validate_module_slots(module_registry.as_ref(), &ir.functions)
            .map_err(|error| ProgramAssemblyError::Invalid(error.to_string()))?;
        Ok(BackendIrDocument {
            ir,
            module_registry,
            code_slots,
        })
    }

    /// Assemble a lease that must ship every body (one-shot backends, the fact
    /// graph and `molt-jit`, which keep no store).
    pub fn into_complete_document(self) -> Result<BackendIrDocument, String> {
        self.assemble(|_, _| None)
            .map_err(|error| error.to_string())
    }
}

/// Encode `document` as a lease that ships every body. Rust producers and test
/// fixtures use it; the CLI's `molt.cli.backend_ir_transport` writes the same
/// map. Each body is its named-field MessagePack encoding.
pub fn encode_complete_lease(document: &BackendIrDocument) -> Result<Vec<u8>, String> {
    encode_lease(document, |_| true)
}

/// Encode `document` as a lease that ships the bodies at the positions `ship`
/// selects, and lists every function's digest in program order.
pub fn encode_lease(
    document: &BackendIrDocument,
    ship: impl Fn(usize) -> bool,
) -> Result<Vec<u8>, String> {
    let bodies: Vec<Vec<u8>> = document
        .ir
        .functions
        .iter()
        .map(|function| rmp_serde::to_vec_named(function).map_err(|error| error.to_string()))
        .collect::<Result<_, _>>()?;
    let digests: Vec<u8> = bodies
        .iter()
        .flat_map(|body| FunctionDigest::of_body(body).0)
        .collect();
    rmp_serde::to_vec_named(&LeaseWireRef {
        code_slots: &document.code_slots,
        function_bodies: bodies
            .iter()
            .enumerate()
            .filter(|(position, _)| ship(*position))
            .map(|(_, body)| WireBytesRef(body))
            .collect(),
        function_digests: WireBytesRef(&digests),
        module_registry: &document.module_registry,
        profile: &document.ir.profile,
    })
    .map_err(|error| error.to_string())
}

/// The digest each body of `document` has in a lease from [`encode_lease`].
pub fn lease_function_digests(document: &BackendIrDocument) -> Result<Vec<FunctionDigest>, String> {
    document
        .ir
        .functions
        .iter()
        .map(|function| {
            rmp_serde::to_vec_named(function)
                .map(|body| FunctionDigest::of_body(&body))
                .map_err(|error| error.to_string())
        })
        .collect()
}

impl FunctionIR {
    /// Transitive heap bytes this function owns, plus its inline size: the
    /// retained-ownership charge of a resident copy. Every field is named, so
    /// a new field fails this weigher until its allocation is counted.
    pub fn retained_bytes(&self) -> usize {
        let Self {
            name,
            return_abi: _,
            params,
            ops,
            param_types,
            source_file,
            is_extern: _,
            execution_context: _,
            codegen_partition: _,
            parameter_custody,
        } = self;
        std::mem::size_of::<Self>()
            + name.capacity()
            + strings_bytes(params)
            + ops.capacity() * std::mem::size_of::<OpIR>()
            + ops.iter().map(op_heap_bytes).sum::<usize>()
            + param_types.as_ref().map_or(0, strings_bytes)
            + source_file.as_ref().map_or(0, String::capacity)
            + parameter_custody.capacity() * std::mem::size_of::<crate::ParameterCustody>()
    }
}

fn strings_bytes(strings: &Vec<String>) -> usize {
    strings.capacity() * std::mem::size_of::<String>()
        + strings.iter().map(String::capacity).sum::<usize>()
}

fn op_heap_bytes(op: &OpIR) -> usize {
    let OpIR {
        kind,
        value: _,
        state_targets,
        f_value: _,
        s_value,
        bytes,
        var,
        args,
        out,
        stack_eligible: _,
        arena_eligible: _,
        defines_del: _,
        bound_local: _,
        task_kind,
        task_closure_size: _,
        container_type,
        native_callable_export,
        native_callable_binding,
        native_callable_symbol,
        native_callable_abi,
        builtin_name,
        runtime_symbol,
        class_definition,
        runtime_requirement_bits: _,
        passes_execution_context: _,
        async_work_poll: _,
        argument_custody,
        type_hint,
        source_op_idx: _,
        col_offset: _,
        end_col_offset: _,
        source_line: _,
        bce_safe: _,
    } = op;
    let text = |value: &Option<String>| value.as_ref().map_or(0, String::capacity);
    kind.capacity()
        + state_targets.as_ref().map_or(0, |targets| {
            targets.capacity() * std::mem::size_of::<(i64, i64)>()
        })
        + text(s_value)
        + bytes.as_ref().map_or(0, Vec::capacity)
        + text(var)
        + args.as_ref().map_or(0, strings_bytes)
        + text(out)
        + text(task_kind)
        + text(container_type)
        + text(native_callable_export)
        + text(native_callable_binding)
        + text(native_callable_symbol)
        + text(native_callable_abi)
        + text(builtin_name)
        + text(runtime_symbol)
        + text(class_definition)
        + argument_custody.as_ref().map_or(0, |custody| {
            custody.capacity() * std::mem::size_of::<crate::ParameterCustody>()
        })
        + text(type_hint)
}

#[cfg(test)]
#[path = "addressed_program_tests.rs"]
mod tests;
