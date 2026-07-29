use std::collections::BTreeMap;
use std::io::{self, Read, Seek, SeekFrom};

const MAX_CACHED_IDENTITIES: usize = 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageCacheKey {
    stable_file_id: String,
    mutation_token: String,
}

impl ImageCacheKey {
    pub fn new(stable_file_id: String, mutation_token: String) -> Self {
        Self {
            stable_file_id,
            mutation_token,
        }
    }

    pub fn stable_file_id(&self) -> &str {
        &self.stable_file_id
    }

    pub fn mutation_token(&self) -> &str {
        &self.mutation_token
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CacheEntry {
    mutation_token: String,
    sha256: String,
}

/// Per-supervision executable digest cache.
///
/// Stable OS file identity alone is insufficient because an admitted derived
/// image may be mutated in place.  A cached digest is reusable only while the
/// platform mutation token is identical; a changed token replaces the entry
/// after hashing the already-open executable handle.
#[derive(Debug, Default)]
pub struct ImageHashCache {
    entries: BTreeMap<String, CacheEntry>,
}

impl ImageHashCache {
    pub fn digest<R, F>(
        &mut self,
        key: &ImageCacheKey,
        reader: &mut R,
        revalidate: F,
    ) -> io::Result<String>
    where
        R: Read + Seek,
        F: FnOnce(&R) -> io::Result<ImageCacheKey>,
    {
        if let Some(entry) = self.entries.get(&key.stable_file_id)
            && entry.mutation_token == key.mutation_token
        {
            ensure_stable(key, &revalidate(reader)?)?;
            return Ok(entry.sha256.clone());
        }
        reader.seek(SeekFrom::Start(0))?;
        let sha256 = crate::sha256_reader(reader)?;
        ensure_stable(key, &revalidate(reader)?)?;
        if self.entries.len() >= MAX_CACHED_IDENTITIES
            && !self.entries.contains_key(&key.stable_file_id)
            && let Some(oldest_key) = self.entries.keys().next().cloned()
        {
            self.entries.remove(&oldest_key);
        }
        self.entries.insert(
            key.stable_file_id.clone(),
            CacheEntry {
                mutation_token: key.mutation_token.clone(),
                sha256: sha256.clone(),
            },
        );
        Ok(sha256)
    }
}

fn ensure_stable(before: &ImageCacheKey, after: &ImageCacheKey) -> io::Result<()> {
    if before == after {
        Ok(())
    } else {
        Err(io::Error::other(
            "executable identity or mutation token changed while hashing",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Seek};

    struct CountingReader {
        inner: Cursor<Vec<u8>>,
        reads: usize,
    }

    impl CountingReader {
        fn new(value: &[u8]) -> Self {
            Self {
                inner: Cursor::new(value.to_vec()),
                reads: 0,
            }
        }
    }

    impl Read for CountingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.reads += 1;
            self.inner.read(buffer)
        }
    }

    impl Seek for CountingReader {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.inner.seek(position)
        }
    }

    #[test]
    fn exact_os_identity_and_mutation_token_reuse_digest() {
        let mut cache = ImageHashCache::default();
        let mut first = CountingReader::new(b"same image");
        let key = ImageCacheKey::new("device:inode".to_owned(), "size:mtime:ctime".to_owned());
        let expected = cache.digest(&key, &mut first, |_| Ok(key.clone())).unwrap();
        assert!(first.reads > 0);

        let mut second = CountingReader::new(b"different bytes must not be read");
        let cached = cache
            .digest(&key, &mut second, |_| Ok(key.clone()))
            .unwrap();
        assert_eq!(cached, expected);
        assert_eq!(second.reads, 0);
    }

    #[test]
    fn mutation_token_change_forces_rehash_for_same_os_identity() {
        let mut cache = ImageHashCache::default();
        let mut before = CountingReader::new(b"before");
        let old_key = ImageCacheKey::new("device:inode".to_owned(), "token-1".to_owned());
        let old = cache
            .digest(&old_key, &mut before, |_| Ok(old_key.clone()))
            .unwrap();

        let mut after = CountingReader::new(b"after");
        let new_key = ImageCacheKey::new("device:inode".to_owned(), "token-2".to_owned());
        let new = cache
            .digest(&new_key, &mut after, |_| Ok(new_key.clone()))
            .unwrap();
        assert_ne!(new, old);
        assert!(after.reads > 0);
    }

    #[test]
    fn identity_change_during_hash_is_rejected_and_not_cached() {
        let mut cache = ImageHashCache::default();
        let key = ImageCacheKey::new("device:inode".to_owned(), "token-1".to_owned());
        let changed = ImageCacheKey::new("device:inode".to_owned(), "token-2".to_owned());
        let mut reader = CountingReader::new(b"racing image");
        let error = cache
            .digest(&key, &mut reader, |_| Ok(changed.clone()))
            .unwrap_err();
        assert!(error.to_string().contains("changed while hashing"));

        let mut stable = CountingReader::new(b"racing image");
        cache
            .digest(&key, &mut stable, |_| Ok(key.clone()))
            .unwrap();
        assert!(stable.reads > 0);
    }

    #[test]
    fn adversarial_identity_churn_keeps_cache_bounded() {
        let mut cache = ImageHashCache::default();
        for index in 0..(MAX_CACHED_IDENTITIES + 128) {
            let key = ImageCacheKey::new(format!("device:{index}"), "token".to_owned());
            let mut reader = CountingReader::new(index.to_string().as_bytes());
            cache
                .digest(&key, &mut reader, |_| Ok(key.clone()))
                .unwrap();
        }
        assert_eq!(cache.entries.len(), MAX_CACHED_IDENTITIES);
    }
}
