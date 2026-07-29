use super::super::publish::bytes_to_lower_hex;
use sha2::{Digest, Sha256};
use std::io::{self, Write};

const STDLIB_PARTITION_MANIFEST_SCHEMA: &str = "stdlib-partition-v2-exact-linkage-abi";

struct DigestWriter<'a>(&'a mut Sha256);

impl Write for DigestWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn shared_stdlib_partition_manifest(
    stdlib_funcs: &[molt_backend::FunctionIR],
    module_context: &molt_backend::NativeBackendModuleContext,
) -> io::Result<String> {
    let mut funcs: Vec<&molt_backend::FunctionIR> = stdlib_funcs.iter().collect();
    funcs.sort_by(|left, right| left.name.cmp(&right.name));

    let mut names: Vec<String> = Vec::with_capacity(funcs.len());
    let mut content_hash = Sha256::new();
    for func in funcs {
        names.push(func.name.clone());
        let linkage_abi = module_context
            .function_linkage_abi(&func.name)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "missing exact native linkage ABI for stdlib function `{}`",
                        func.name
                    ),
                )
            })?;

        // Hash the complete versioned FunctionIR contract rather than a manual
        // field projection, so future ABI-relevant fields cannot silently fall
        // outside cache admission. Per-record digests provide fixed framing
        // without allocating duplicate O(IR) serialization buffers.
        let mut function_hash = Sha256::new();
        molt_backend::ir::write_function_ir_contract(func, &mut DigestWriter(&mut function_hash))
            .map_err(io::Error::other)?;
        let mut linkage_hash = Sha256::new();
        let mut serializer =
            rmp_serde::Serializer::new(DigestWriter(&mut linkage_hash)).with_struct_map();
        serde::Serialize::serialize(linkage_abi, &mut serializer).map_err(io::Error::other)?;

        content_hash.update(b"function-ir\0");
        content_hash.update(function_hash.finalize());
        content_hash.update(b"native-linkage-abi\0");
        content_hash.update(linkage_hash.finalize());
    }

    serde_json::to_string(&serde_json::json!({
        "schema": STDLIB_PARTITION_MANIFEST_SCHEMA,
        "function_count": names.len(),
        "functions": names,
        "content_sha256": bytes_to_lower_hex(content_hash.finalize().as_ref()),
    }))
    .map_err(io::Error::other)
}
