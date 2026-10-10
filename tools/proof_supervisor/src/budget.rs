//! Shared admission bounds projected from protocol.json; no runtime overrides.
use crate::*;
use serde::Serialize;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The caller's existing value may be large; this caps only additional wire
/// storage owned by the supervisor, before every extension/allocation.
pub struct BoundedBuffer {
    bytes: Vec<u8>,
    limit: usize,
}
impl BoundedBuffer {
    pub fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }
    pub fn reset(&mut self, limit: usize) {
        self.bytes.clear();
        self.limit = limit;
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
    pub fn len(&self) -> usize {
        self.bytes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
    pub fn into_vec(self) -> Vec<u8> {
        self.bytes
    }
}
impl Write for BoundedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| io::Error::other("protocol serialization byte budget exceeded"))?;
        if next > self.bytes.capacity() {
            self.bytes
                .try_reserve_exact(
                    next.max(self.bytes.capacity().saturating_mul(2))
                        .min(self.limit)
                        - self.bytes.len(),
                )
                .map_err(|_| io::Error::other("protocol serialization reservation refused"))?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct CountWriter {
    count: usize,
    limit: usize,
}
impl Write for CountWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.count = self
            .count
            .checked_add(bytes.len())
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| io::Error::other("protocol encoded-byte budget exceeded"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub fn encoded_size(value: &impl Serialize, limit: usize) -> Result<usize, String> {
    let mut sink = CountWriter { count: 0, limit };
    serde_json::to_writer(&mut sink, value).map_err(|e| e.to_string())?;
    Ok(sink.count)
}
pub fn encode(value: &impl Serialize, limit: usize) -> Result<Vec<u8>, String> {
    let mut sink = BoundedBuffer::new(limit);
    serde_json::to_writer(&mut sink, value).map_err(|e| e.to_string())?;
    Ok(sink.into_vec())
}
pub fn copy_string(value: &str) -> Result<String, String> {
    let mut result = String::new();
    result
        .try_reserve_exact(value.len())
        .map_err(|_| "string reservation refused".to_owned())?;
    result.push_str(value);
    Ok(result)
}
pub fn copy_path(value: &Path) -> Result<PathBuf, String> {
    let mut result = std::ffi::OsString::new();
    result
        .try_reserve_exact(value.as_os_str().len())
        .map_err(|_| "path reservation refused".to_owned())?;
    result.push(value);
    Ok(result.into())
}
pub fn image_size(image: &FileIdentity) -> Result<usize, String> {
    path_bound(&image.path)?;
    bound(image.sha256.len(), 64, "image SHA-256")?;
    bound(
        image.file_id.len(),
        BUDGET_FILE_ID_UTF8_BYTES,
        "image file identity",
    )?;
    for role in &image.roles {
        bound(role.len(), BUDGET_ROLE_UTF8_BYTES, "image role")?;
    }
    encoded_size(&image.roles, BUDGET_ONE_IMAGE_ROLES_JSON_BYTES)?;
    encoded_size(image, BUDGET_EVENT_RECORD_BYTES - 1024)
}
/// No owned copies or formatting of caller payloads precede this preflight.
/// The event-wire bound is separate from retained diagnostic truncation.
pub fn event_shape(event: &ProcessEvent) -> Result<(), String> {
    bound(
        event.stable_process_id.len(),
        BUDGET_STABLE_PROCESS_ID_UTF8_BYTES,
        "stable process identity",
    )?;
    match &event.event {
        ProcessEventKind::Exec { image } | ProcessEventKind::InitialImage { image } => {
            image_size(image)?;
        }
        ProcessEventKind::Fork {
            image: Some(image), ..
        } => {
            image_size(image)?;
        }
        _ => {}
    }
    encoded_size(event, BUDGET_EVENT_RECORD_BYTES - 1)?;
    Ok(())
}

pub fn copy_image(image: &FileIdentity) -> Result<FileIdentity, String> {
    image_size(image)?;
    let mut roles = Vec::new();
    roles
        .try_reserve_exact(image.roles.len())
        .map_err(|_| "image roles reservation refused".to_owned())?;
    for role in &image.roles {
        roles.push(copy_string(role)?);
    }
    Ok(FileIdentity {
        path: copy_path(&image.path)?,
        file_id: copy_string(&image.file_id)?,
        size_bytes: image.size_bytes,
        sha256: copy_string(&image.sha256)?,
        class: image.class.clone(),
        roles,
    })
}
pub fn bound(size: usize, limit: usize, label: &str) -> Result<(), String> {
    if size > limit {
        Err(format!("{label} exceeds protocol budget {limit}"))
    } else {
        Ok(())
    }
}
pub fn path_bound(path: &Path) -> Result<(), String> {
    let value = path
        .to_str()
        .ok_or_else(|| "protocol path is not UTF-8".to_owned())?;
    bound(value.len(), BUDGET_PATH_UTF8_BYTES, "path")
}
pub fn policy_shape(policy: &Policy) -> Result<(), String> {
    bound(policy.nonce.len(), BUDGET_NONCE_UTF8_BYTES, "nonce")?;
    bound(
        policy.command.len(),
        BUDGET_COMMAND_ELEMENTS,
        "command elements",
    )?;
    bound(
        policy.environment.len(),
        BUDGET_ENVIRONMENT_ENTRIES,
        "environment entries",
    )?;
    bound(
        policy.fixed_images.len(),
        BUDGET_FIXED_IMAGE_ROWS,
        "fixed-image rows",
    )?;
    bound(
        policy.derived_roots.len(),
        BUDGET_DERIVED_ROOTS,
        "derived roots",
    )?;
    bound(policy.root_role.len(), BUDGET_ROLE_UTF8_BYTES, "root role")?;
    path_bound(&policy.cwd)?;
    for image in &policy.fixed_images {
        bound(image.role.len(), BUDGET_ROLE_UTF8_BYTES, "fixed role")?;
        path_bound(&image.path)?;
    }
    for root in &policy.derived_roots {
        bound(root.role.len(), BUDGET_ROLE_UTF8_BYTES, "derived role")?;
        path_bound(&root.path)?;
    }
    encoded_size(policy, BUDGET_CANONICAL_POLICY_BYTES)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn serializer_refuses_before_buffer_extension() {
        let original = "\u{0}\n\"\\é𐐀".repeat(32);
        let oracle = serde_json::to_vec(&original).unwrap();
        for limit in [0, 1, oracle.len() - 1, oracle.len()] {
            let mut sink = BoundedBuffer::new(limit);
            let result = serde_json::to_writer(&mut sink, &original);
            assert!(sink.len() <= limit);
            assert_eq!(result.is_ok(), limit == oracle.len());
            assert!(oracle.starts_with(sink.as_slice()));
        }
    }
    #[test]
    fn policy_preflight_rejects_escaped_scalar_and_field_count_before_construction() {
        let too_long_nonce = "\u{0001}".repeat(BUDGET_NONCE_UTF8_BYTES + 1);
        let wire = serde_json::to_vec(&serde_json::json!({"nonce": too_long_nonce})).unwrap();
        // This object intentionally lacks required fields: bounded preflight
        // must refuse its decoded scalar before typed Policy construction.
        assert!(decode_policy(&wire).unwrap_err().contains("scalar budget"));
        let wire = serde_json::to_vec(
            &serde_json::json!({"derived_roots": vec![(); BUDGET_DERIVED_ROOTS + 1]}),
        )
        .unwrap();
        assert!(
            decode_policy(&wire)
                .unwrap_err()
                .contains("sequence budget")
        );
        assert!(
            decode_policy(br#"{"environment":{"A":"1","A":"2"}}"#)
                .unwrap_err()
                .contains("duplicate")
        );
    }

    #[test]
    fn giant_library_value_does_not_grow_owned_wire_storage() {
        let value = "x".repeat(BUDGET_EVENT_RECORD_BYTES + 1);
        let mut sink = BoundedBuffer::new(64);
        assert!(serde_json::to_writer(&mut sink, &value).is_err());
        assert!(sink.bytes.capacity() <= 64);
    }
}

/// Inspect JSON container bounds and duplicate keys without building a Value
/// tree. Parser escape scratch is bounded by the admitted raw input size.
#[derive(Clone, Copy)]
struct JsonShape {
    sequence_limit: usize,
    string_limit: usize,
    environment: bool,
}
impl JsonShape {
    fn root() -> Self {
        Self {
            sequence_limit: BUDGET_FIXED_IMAGE_ROWS.max(BUDGET_COMMAND_ELEMENTS),
            string_limit: BUDGET_POLICY_INPUT_BYTES,
            environment: false,
        }
    }
    fn field(key: &str) -> Self {
        let mut shape = Self::root();
        shape.sequence_limit = match key {
            "command" => BUDGET_COMMAND_ELEMENTS,
            "fixed_images" => BUDGET_FIXED_IMAGE_ROWS,
            "derived_roots" => BUDGET_DERIVED_ROOTS,
            _ => shape.sequence_limit,
        };
        shape.string_limit = match key {
            "nonce" => BUDGET_NONCE_UTF8_BYTES,
            "role" | "root_role" => BUDGET_ROLE_UTF8_BYTES,
            "path" | "cwd" => BUDGET_PATH_UTF8_BYTES,
            "sha256" => 64,
            _ => BUDGET_POLICY_INPUT_BYTES,
        };
        shape.environment = key == "environment";
        shape
    }
}
impl<'de> serde::de::DeserializeSeed<'de> for JsonShape {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_any(self)
    }
}
impl<'de> serde::de::Visitor<'de> for JsonShape {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("bounded JSON policy")
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<(), E> {
        if value.len() > self.string_limit {
            return Err(E::custom("policy scalar budget exceeded"));
        }
        Ok(())
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        use serde::de::Error;
        let mut count = 0;
        while seq.next_element_seed(JsonShape::root())?.is_some() {
            count += 1;
            if count > self.sequence_limit {
                return Err(A::Error::custom("policy sequence budget exceeded"));
            }
        }
        Ok(())
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        use serde::de::Error;
        let mut keys = std::collections::HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if keys.len() >= BUDGET_ENVIRONMENT_ENTRIES {
                return Err(A::Error::custom("policy map budget exceeded"));
            }
            keys.try_reserve(1)
                .map_err(|_| A::Error::custom("policy key reservation refused"))?;
            let child = if self.environment {
                JsonShape::root()
            } else {
                JsonShape::field(&key)
            };
            if !keys.insert(key) {
                return Err(A::Error::custom("duplicate policy object key"));
            }
            map.next_value_seed(child)?;
        }
        Ok(())
    }
}
pub fn decode_policy(bytes: &[u8]) -> Result<Policy, String> {
    use serde::de::DeserializeSeed;
    bound(bytes.len(), BUDGET_POLICY_INPUT_BYTES, "policy input")?;
    let mut reader = serde_json::Deserializer::from_slice(bytes);
    JsonShape::root()
        .deserialize(&mut reader)
        .map_err(|e| format!("invalid bounded policy: {e}"))?;
    reader.end().map_err(|e| e.to_string())?;
    let policy: Policy =
        serde_json::from_slice(bytes).map_err(|e| format!("invalid policy: {e}"))?;
    policy_shape(&policy)?;
    Ok(policy)
}

pub fn digest(value: &impl Serialize, limit: usize) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    struct Sink {
        count: usize,
        limit: usize,
        digest: Sha256,
    }
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.count = self
                .count
                .checked_add(bytes.len())
                .filter(|n| *n <= self.limit)
                .ok_or_else(|| io::Error::other("canonical hash byte budget exceeded"))?;
            self.digest.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut sink = Sink {
        count: 0,
        limit,
        digest: Sha256::new(),
    };
    serde_json::to_writer(&mut sink, value).map_err(|e| e.to_string())?;
    Ok(crate::hex_lower(&sink.digest.finalize()))
}
