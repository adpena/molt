use molt_ir::content_digest::bytes_to_lower_hex;
use molt_ir::ir::FunctionIrContractDigest;
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
    contracts: Option<&[Option<FunctionIrContractDigest>]>,
    module_context: &molt_backend::NativeBackendModuleContext,
) -> io::Result<String> {
    let mut funcs: Vec<_> = stdlib_funcs
        .iter()
        .enumerate()
        .filter(|(_, func)| !func.is_extern)
        .collect();
    funcs.sort_by(|(_, left), (_, right)| left.name.cmp(&right.name));

    let table = std::cell::OnceCell::new();
    let mut names: Vec<String> = Vec::with_capacity(funcs.len());
    let mut content_hash = Sha256::new();
    for (row, func) in funcs {
        names.push(func.name.clone());
        let contract = if let Some(contracts) = contracts {
            contracts.get(row).copied().flatten().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("missing body contract for stdlib function `{}`", func.name),
                )
            })?
        } else {
            // Without carrier-cache keys, only hash the bodies this partition publishes.
            let table = table.get_or_init(|| molt_ir::FunctionTable::of(stdlib_funcs));
            FunctionIrContractDigest::of(func, table).map_err(io::Error::other)?
        };
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

        let mut linkage_hash = Sha256::new();
        let mut serializer =
            rmp_serde::Serializer::new(DigestWriter(&mut linkage_hash)).with_struct_map();
        serde::Serialize::serialize(linkage_abi, &mut serializer).map_err(io::Error::other)?;

        content_hash.update(b"function-ir\0");
        content_hash.update(contract.as_bytes());
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
