//! Identity of bytes read by the successful complete-frame decoder.
use sha2::{Digest, Sha256};
use std::io::{self, Read};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContentIdentity {
    pub(crate) sha256: String,
    pub(crate) size: u64,
}
impl ContentIdentity {
    pub(crate) fn of_bytes(bytes: &[u8]) -> Self {
        Self {
            sha256: molt_ir::content_digest::bytes_to_lower_hex(&Sha256::digest(bytes)),
            size: bytes.len() as u64,
        }
    }
}
pub(crate) struct ContentReader<R> {
    reader: R,
    digest: Sha256,
    size: u64,
}
impl<R: Read> ContentReader<R> {
    pub(crate) fn new(reader: R) -> Self {
        Self {
            reader,
            digest: Sha256::new(),
            size: 0,
        }
    }
    pub(crate) fn identity(self) -> ContentIdentity {
        ContentIdentity {
            sha256: molt_ir::content_digest::bytes_to_lower_hex(&self.digest.finalize()),
            size: self.size,
        }
    }
}
impl<R: Read> Read for ContentReader<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.reader.read(bytes)?;
        self.size = self
            .size
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("input byte count overflow"))?;
        self.digest.update(&bytes[..count]);
        Ok(count)
    }
}

/// Input to a successful addressed-program assembly. `lease` observes only the
/// actual frame read, which may omit resident bodies. The ordered digests name
/// every validated function assembled for codegen, without reserializing IR.
#[cfg(molt_backend_daemon)]
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct AddressedInputIdentity {
    schema: &'static str,
    pub(crate) lease: ContentIdentity,
    pub(crate) function_digests: String,
}
#[cfg(molt_backend_daemon)]
impl AddressedInputIdentity {
    pub(crate) fn new(lease: ContentIdentity, function_digests: String) -> Self {
        Self {
            schema: "molt.addressed-program-input.v1",
            lease,
            function_digests,
        }
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        self.lease.sha256.capacity() + self.function_digests.capacity()
    }
    #[cfg(test)]
    pub(crate) fn fixture() -> Self {
        Self::new(ContentIdentity::of_bytes(b"fixture lease"), String::new())
    }
}
