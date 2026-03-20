//! Snapshot artifact for fast cold-start on edge platforms.
//! Captures post-init runtime state for restore instead of re-execution.

/// Header metadata for a `molt.snapshot` artifact.
///
/// The header is serialised as JSON alongside an optional binary blob
/// containing WASM linear memory + globals captured after deterministic
/// init completes.
#[derive(Clone, Debug)]
pub struct SnapshotHeader {
    pub snapshot_version: u32,
    pub abi_version: String,
    pub target_profile: String,
    pub module_hash: String,
    pub mount_plan: Vec<MountEntry>,
    pub capability_manifest: Vec<String>,
    pub determinism_stamp: String,
    pub init_state_size: u64,
    /// Integrity hash computed from all other header fields.
    /// TODO(v0.2): Compute as SHA-256 of the canonical JSON representation
    /// (excluding this field) when serializing, and verify on deserialize.
    /// For v0.1, `validate_against` already checks `module_hash` which
    /// provides primary tamper detection against the deployment manifest.
    pub integrity_hash: Option<String>,
}

/// A single mount recorded in the snapshot header.
#[derive(Clone, Debug)]
pub struct MountEntry {
    pub path: String,
    pub mount_type: String, // "bundle", "tmp", "dev"
    pub hash: Option<String>,
    pub quota_mb: Option<u32>,
}

// ---------------------------------------------------------------------------
// serde_json manual serialization (serde_json is already a dependency;
// serde derive is NOT, so we write the conversion by hand).
// ---------------------------------------------------------------------------

impl SnapshotHeader {
    /// Validate that a snapshot matches the expected module and ABI.
    pub fn validate_against(&self, module_hash: &str, abi_version: &str) -> Result<(), String> {
        if self.module_hash != module_hash {
            return Err(format!(
                "snapshot module hash mismatch: expected {module_hash}, got {}",
                self.module_hash
            ));
        }
        if self.abi_version != abi_version {
            return Err(format!(
                "snapshot ABI version mismatch: expected {abi_version}, got {}",
                self.abi_version
            ));
        }
        Ok(())
    }

    /// Serialize to a `serde_json::Value`.
    pub fn to_json(&self) -> serde_json::Value {
        let mut obj = serde_json::json!({
            "snapshot_version": self.snapshot_version,
            "abi_version": self.abi_version,
            "target_profile": self.target_profile,
            "module_hash": self.module_hash,
            "mount_plan": self.mount_plan.iter().map(|m| m.to_json()).collect::<Vec<_>>(),
            "capability_manifest": self.capability_manifest,
            "determinism_stamp": self.determinism_stamp,
            "init_state_size": self.init_state_size,
        });
        if let Some(ref hash) = self.integrity_hash {
            obj.as_object_mut()
                .unwrap()
                .insert("integrity_hash".into(), serde_json::Value::String(hash.clone()));
        }
        obj
    }

    /// Deserialize from a `serde_json::Value`.
    pub fn from_json(v: &serde_json::Value) -> Result<Self, String> {
        let obj = v.as_object().ok_or("expected JSON object")?;
        Ok(Self {
            snapshot_version: obj
                .get("snapshot_version")
                .and_then(|v| v.as_u64())
                .ok_or("missing snapshot_version")? as u32,
            abi_version: obj
                .get("abi_version")
                .and_then(|v| v.as_str())
                .ok_or("missing abi_version")?
                .to_string(),
            target_profile: obj
                .get("target_profile")
                .and_then(|v| v.as_str())
                .ok_or("missing target_profile")?
                .to_string(),
            module_hash: obj
                .get("module_hash")
                .and_then(|v| v.as_str())
                .ok_or("missing module_hash")?
                .to_string(),
            mount_plan: obj
                .get("mount_plan")
                .and_then(|v| v.as_array())
                .ok_or("missing mount_plan")?
                .iter()
                .map(MountEntry::from_json)
                .collect::<Result<Vec<_>, _>>()?,
            capability_manifest: obj
                .get("capability_manifest")
                .and_then(|v| v.as_array())
                .ok_or("missing capability_manifest")?
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            determinism_stamp: obj
                .get("determinism_stamp")
                .and_then(|v| v.as_str())
                .ok_or("missing determinism_stamp")?
                .to_string(),
            init_state_size: obj
                .get("init_state_size")
                .and_then(|v| v.as_u64())
                .ok_or("missing init_state_size")?,
            integrity_hash: obj
                .get("integrity_hash")
                .and_then(|v| v.as_str())
                .map(String::from),
        })
    }
}

impl MountEntry {
    pub fn to_json(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        m.insert("path".into(), serde_json::Value::String(self.path.clone()));
        m.insert(
            "mount_type".into(),
            serde_json::Value::String(self.mount_type.clone()),
        );
        if let Some(ref h) = self.hash {
            m.insert("hash".into(), serde_json::Value::String(h.clone()));
        }
        if let Some(q) = self.quota_mb {
            m.insert("quota_mb".into(), serde_json::json!(q));
        }
        serde_json::Value::Object(m)
    }

    pub fn from_json(v: &serde_json::Value) -> Result<Self, String> {
        let obj = v.as_object().ok_or("mount entry must be an object")?;
        Ok(Self {
            path: obj
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or("missing mount path")?
                .to_string(),
            mount_type: obj
                .get("mount_type")
                .and_then(|v| v.as_str())
                .ok_or("missing mount_type")?
                .to_string(),
            hash: obj.get("hash").and_then(|v| v.as_str()).map(String::from),
            quota_mb: obj
                .get("quota_mb")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32),
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_header() -> SnapshotHeader {
        SnapshotHeader {
            snapshot_version: 1,
            abi_version: "0.1.0".into(),
            target_profile: "wasm_worker_cloudflare".into(),
            module_hash: "sha256:abc123".into(),
            mount_plan: vec![
                MountEntry {
                    path: "/bundle".into(),
                    mount_type: "bundle".into(),
                    hash: Some("sha256:def456".into()),
                    quota_mb: None,
                },
                MountEntry {
                    path: "/tmp".into(),
                    mount_type: "tmp".into(),
                    hash: None,
                    quota_mb: Some(32),
                },
            ],
            capability_manifest: vec![
                "fs.bundle.read".into(),
                "fs.tmp.read".into(),
                "fs.tmp.write".into(),
            ],
            determinism_stamp: "2026-03-20T00:00:00Z".into(),
            init_state_size: 0,
            integrity_hash: None,
        }
    }

    #[test]
    fn round_trip_json() {
        let header = sample_header();
        let json = header.to_json();
        let restored = SnapshotHeader::from_json(&json).unwrap();
        assert_eq!(restored.snapshot_version, 1);
        assert_eq!(restored.abi_version, "0.1.0");
        assert_eq!(restored.target_profile, "wasm_worker_cloudflare");
        assert_eq!(restored.module_hash, "sha256:abc123");
        assert_eq!(restored.mount_plan.len(), 2);
        assert_eq!(restored.mount_plan[0].path, "/bundle");
        assert_eq!(
            restored.mount_plan[0].hash.as_deref(),
            Some("sha256:def456")
        );
        assert_eq!(restored.mount_plan[1].quota_mb, Some(32));
        assert_eq!(restored.capability_manifest.len(), 3);
        assert_eq!(restored.determinism_stamp, "2026-03-20T00:00:00Z");
        assert_eq!(restored.init_state_size, 0);
    }

    #[test]
    fn validate_against_ok() {
        let header = sample_header();
        assert!(header.validate_against("sha256:abc123", "0.1.0").is_ok());
    }

    #[test]
    fn validate_against_hash_mismatch() {
        let header = sample_header();
        let err = header
            .validate_against("sha256:wrong", "0.1.0")
            .unwrap_err();
        assert!(err.contains("module hash mismatch"));
    }

    #[test]
    fn validate_against_abi_mismatch() {
        let header = sample_header();
        let err = header
            .validate_against("sha256:abc123", "0.2.0")
            .unwrap_err();
        assert!(err.contains("ABI version mismatch"));
    }

    #[test]
    fn mount_entry_optional_fields() {
        let entry = MountEntry {
            path: "/dev".into(),
            mount_type: "dev".into(),
            hash: None,
            quota_mb: None,
        };
        let json = entry.to_json();
        assert!(json.get("hash").is_none());
        assert!(json.get("quota_mb").is_none());
        let restored = MountEntry::from_json(&json).unwrap();
        assert!(restored.hash.is_none());
        assert!(restored.quota_mb.is_none());
    }

    #[test]
    fn snapshot_header_json_string_round_trip() {
        let header = sample_header();
        let json_string = serde_json::to_string_pretty(&header.to_json()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_string).unwrap();
        let restored = SnapshotHeader::from_json(&parsed).unwrap();
        assert_eq!(restored.snapshot_version, header.snapshot_version);
        assert_eq!(restored.module_hash, header.module_hash);
    }
}
