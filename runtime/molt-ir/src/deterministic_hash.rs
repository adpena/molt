//! The compiler's one hash map and hash set.
//!
//! `std::collections::HashMap` seeds its hasher randomly per process, so two
//! compiler processes iterate one map in different orders. Any pass whose
//! output follows that order (drop placement, block walks, poll placement)
//! then emits different code for identical input, and the content-addressed
//! native object cache refused the second emission (HF-852). These aliases
//! use the deterministic Fx hash that rustc uses: one input, one iteration
//! order, in every process. `clippy.toml` in each compiler crate disallows
//! the std types so a random seed cannot come back.
pub type HashMap<K, V> = rustc_hash::FxHashMap<K, V>;
pub type HashSet<T> = rustc_hash::FxHashSet<T>;
