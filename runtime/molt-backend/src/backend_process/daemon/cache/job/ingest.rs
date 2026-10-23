use super::super::super::origin::{DaemonCacheEntry, DaemonJobOrigin};
use super::super::super::protocol::DaemonCacheKey;
use super::super::entries::insert_daemon_cache_entries;
use super::super::shared::DaemonCache;
use crate::backend_process::input::ContentIdentity;
use crate::backend_process::input::content::AddressedInputIdentity;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

pub(crate) fn capture_native_output(
    cache: &DaemonCache,
    output_path: &Path,
    cache_key: Option<&Arc<DaemonCacheKey>>,
    function_cache_key: Option<&Arc<DaemonCacheKey>>,
    input: AddressedInputIdentity,
    codegen: super::super::super::origin::CodegenOrigin,
    emitted_output: ContentIdentity,
) -> Result<(Vec<String>, DaemonJobOrigin), String> {
    let mut file =
        molt_artifact_publish::open_regular_artifact(output_path).map_err(|e| e.to_string())?;
    let expected = file.metadata().map_err(|e| e.to_string())?.len();
    let budget = cache.max_bytes().unwrap_or(usize::MAX);
    let retain = (cache_key.is_some() || function_cache_key.is_some()) && expected <= budget as u64;
    let mut retained = retain.then(Vec::new);
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let count = match file.read(&mut buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result.map_err(|e| e.to_string())?,
        };
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .ok_or("daemon output byte count overflow")?;
        if size > expected {
            return Err("daemon output changed during capture".into());
        }
        digest.update(&buffer[..count]);
        if let Some(bytes) = &mut retained {
            bytes.extend_from_slice(&buffer[..count]);
        }
    }
    if size != expected {
        return Err("daemon output changed during capture".into());
    }
    let observed_output = ContentIdentity {
        sha256: molt_ir::content_digest::bytes_to_lower_hex(&digest.finalize()),
        size,
    };
    if observed_output != emitted_output {
        return Err("daemon output changed after emission".into());
    }
    let origin = DaemonJobOrigin::new(input, emitted_output, codegen);
    if let Some(bytes) = retained {
        insert_daemon_cache_entries(
            cache,
            cache_key,
            function_cache_key,
            Arc::new(DaemonCacheEntry::new(
                Arc::from(bytes.into_boxed_slice()),
                origin.clone(),
            )),
        );
    }
    let mut warnings = Vec::new();
    if (cache_key.is_some() || function_cache_key.is_some()) && expected > budget as u64 {
        let warning = format!(
            "skipped daemon memory cache for '{}' ({} bytes exceeds cache budget)",
            output_path.display(),
            expected
        );
        eprintln!("MOLT_BACKEND(daemon): warning: {warning}");
        warnings.push(warning);
    }
    Ok((warnings, origin))
}

#[cfg(test)]
mod tests {

    #[cfg(feature = "native-backend")]
    #[test]
    fn daemon_native_capture_rejects_same_length_post_emission_substitution() {
        use super::{DaemonCache, capture_native_output};
        use crate::backend_process::daemon::origin::CodegenOrigin;
        use crate::backend_process::input::ContentIdentity;
        use crate::backend_process::input::content::AddressedInputIdentity;
        let output = molt_ir::debug_artifacts::prepare_unique_debug_artifact_path(
            "daemon-origin-substitution",
        )
        .unwrap();
        let _scope = molt_backend::backend_environment::enter(std::sync::Arc::default());
        let mut job = crate::backend_process::daemon::protocol::DaemonJobRequest::from_json_value(
            &serde_json::json!({"id":"capture", "is_wasm":false, "native_output_kind":"object",
                "output":output.to_string_lossy(), "cache_key":"captured-output", "probe_cache_only":true,
                "function_digests":""}), "job"
        ).unwrap();
        let key = job.take_artifact_cache_keys().unwrap().module.unwrap();
        let positive_cache = DaemonCache::new(None);
        let cache = DaemonCache::new(None);
        let emitted = ContentIdentity::of_bytes(b"original");
        std::fs::write(&output, b"original").unwrap();
        let (_, origin) = capture_native_output(
            &positive_cache,
            &output,
            Some(&key),
            None,
            AddressedInputIdentity::fixture(),
            CodegenOrigin::in_process(),
            emitted.clone(),
        )
        .unwrap();
        assert_eq!(origin.output, emitted);
        assert_eq!(positive_cache.get_entry(&key).unwrap().origin, origin);
        // Same length defeats a mere metadata/size check at the actual consumer.
        std::fs::write(&output, b"replaced").unwrap();
        let error = capture_native_output(
            &cache,
            &output,
            Some(&key),
            None,
            AddressedInputIdentity::fixture(),
            CodegenOrigin::in_process(),
            emitted,
        )
        .unwrap_err();
        assert!(error.contains("changed after emission"));
        assert!(
            cache.get_entry(&key).is_none(),
            "substitution cannot enter the cache"
        );
        assert_eq!(cache.usage(), (0, 0));
        std::fs::remove_file(output).unwrap();
    }
}
