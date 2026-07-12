use std::io;

pub(crate) const STDLIB_PARTITION_MANIFEST_SCHEMA: &str = "stdlib-partition-v1";

pub(crate) fn update_fnv1a64(mut hash: u64, bytes: &[u8]) -> u64 {
    const FNV_PRIME: u64 = 0x100000001b3;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

pub(crate) fn shared_stdlib_partition_manifest(
    stdlib_funcs: &[molt_backend::FunctionIR],
) -> io::Result<String> {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    let mut funcs: Vec<&molt_backend::FunctionIR> = stdlib_funcs.iter().collect();
    funcs.sort_by(|left, right| left.name.cmp(&right.name));

    let mut names: Vec<String> = Vec::with_capacity(funcs.len());
    let mut body_hash = FNV_OFFSET;
    for func in funcs {
        names.push(func.name.clone());
        body_hash = update_fnv1a64(body_hash, func.name.as_bytes());
        body_hash = update_fnv1a64(body_hash, &[0]);
        let body = serde_json::to_vec(&serde_json::json!({
            "name": &func.name,
            "params": &func.params,
            "ops": &func.ops,
            "param_types": &func.param_types,
            "source_file": &func.source_file,
            "is_extern": func.is_extern,
        }))
        .map_err(io::Error::other)?;
        body_hash = update_fnv1a64(body_hash, &body);
        body_hash = update_fnv1a64(body_hash, &[0xff]);
    }

    serde_json::to_string(&serde_json::json!({
        "schema": STDLIB_PARTITION_MANIFEST_SCHEMA,
        "function_count": names.len(),
        "functions": names,
        "body_hash": format!("{body_hash:016x}"),
    }))
    .map_err(io::Error::other)
}
