use super::*;
use std::hash::BuildHasher;

#[test]
fn scratch_membership_preserves_typed_node_domains() {
    // The same payload denotes three different API targets. Compare insertion,
    // duplicate detection and membership with the standard keyed hasher.
    let mut expected = HashSet::new();
    let mut actual = HashSet::with_hasher(ScratchAddressHash::default());
    for word in (0..4096u64).map(|value| (value << 32) | 0x1234) {
        for key in [
            GcApiTarget::Node(GcNode::Native(word as usize)),
            GcApiTarget::Node(GcNode::Runtime(PtrSlot(std::ptr::without_provenance_mut(
                word as usize,
            )))),
            GcApiTarget::Inline(word),
        ] {
            assert_eq!(actual.insert(key), expected.insert(key));
            assert!(!actual.insert(key));
        }
    }
    assert_eq!(actual.len(), expected.len());
    assert!(expected.iter().all(|key| actual.contains(key)));
    actual.clear();
    assert!(actual.is_empty());
}

#[test]
fn scratch_hash_spreads_high_payload_bits_on_narrow_targets() {
    // API targets include arbitrary boxed words, not only pointer-width
    // addresses. Truncating each word to usize makes every key here collide
    // on wasm32. Check distribution, independently of the mixer's exact output.
    let hasher = ScratchAddressHash::default();
    let mut buckets = [0usize; 64];
    for value in 0..4096u64 {
        let hash = hasher.hash_one(GcApiTarget::Inline(value << 32));
        buckets[(hash & 63) as usize] += 1;
    }
    assert!(buckets.iter().filter(|&&count| count != 0).count() >= 48);
    assert!(buckets.into_iter().max().unwrap() <= 256);
}
