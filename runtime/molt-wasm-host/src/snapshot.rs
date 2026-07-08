use super::*;

/// Compute the SHA-256 hash of a WASM module file for snapshot validation.
pub(super) fn compute_module_hash(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let hash = Sha256::digest(&bytes);
    let hex = hash
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("sha256:{hex}"))
}

/// Capture a snapshot of WASM linear memory after init completes.
pub(super) fn capture_snapshot(
    store: &mut Store<HostState>,
    instance: &wasmtime::Instance,
    header: &SnapshotHeader,
    output_path: &Path,
) -> Result<()> {
    // Get the memory export â€” try "molt_memory" first, then "memory"
    let memory = instance
        .get_memory(&mut *store, "molt_memory")
        .or_else(|| instance.get_memory(&mut *store, "memory"))
        .ok_or_else(|| anyhow::anyhow!("molt_memory export not found"))?;

    // Read the entire linear memory
    let data = memory.data(&store);
    let memory_bytes = data.to_vec();

    // Write header + blob
    let header_json = serde_json::to_string_pretty(&header.to_json())?;
    let mut file = std::fs::File::create(output_path)?;
    // Write header length (4 bytes LE) + header JSON + memory blob
    let header_bytes = header_json.as_bytes();
    file.write_all(&(header_bytes.len() as u32).to_le_bytes())?;
    file.write_all(header_bytes)?;
    file.write_all(&memory_bytes)?;
    debug_log(|| {
        format!(
            "snapshot captured: header={}B memory={}B -> {:?}",
            header_bytes.len(),
            memory_bytes.len(),
            output_path
        )
    });
    Ok(())
}

/// Restore a snapshot of WASM linear memory, skipping init if successful.
pub(super) fn restore_snapshot(
    store: &mut Store<HostState>,
    instance: &wasmtime::Instance,
    snapshot_path: &Path,
    expected_module_hash: &str,
) -> Result<bool> {
    if !snapshot_path.exists() {
        return Ok(false);
    }
    let data = std::fs::read(snapshot_path)?;
    if data.len() < 4 {
        return Ok(false);
    }
    // Read header length
    let header_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if data.len() < 4 + header_len {
        bail!(
            "snapshot file truncated: expected at least {} bytes, got {}",
            4 + header_len,
            data.len()
        );
    }
    let header_json = std::str::from_utf8(&data[4..4 + header_len])?;
    let header_value: serde_json::Value = serde_json::from_str(header_json)?;
    let header = SnapshotHeader::from_json(&header_value)
        .map_err(|e| anyhow::anyhow!("snapshot header parse error: {e}"))?;

    // Validate
    header
        .validate_against(expected_module_hash, "0.1.0")
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    header
        .verify_integrity()
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Restore memory
    let memory = instance
        .get_memory(&mut *store, "molt_memory")
        .or_else(|| instance.get_memory(&mut *store, "memory"))
        .ok_or_else(|| anyhow::anyhow!("molt_memory export not found"))?;
    let memory_blob = &data[4 + header_len..];
    let mem_data = memory.data_mut(&mut *store);
    if memory_blob.len() > mem_data.len() {
        bail!(
            "snapshot memory blob ({} bytes) exceeds linear memory ({} bytes)",
            memory_blob.len(),
            mem_data.len()
        );
    }
    mem_data[..memory_blob.len()].copy_from_slice(memory_blob);

    debug_log(|| {
        format!(
            "snapshot restored: header={}B memory={}B from {:?}",
            header_len,
            memory_blob.len(),
            snapshot_path
        )
    });
    Ok(true) // skip molt_main
}
