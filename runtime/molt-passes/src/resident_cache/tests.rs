use super::*;
use std::collections::VecDeque;
use std::sync::Arc;

#[test]
fn zero_outputs_still_cost_metadata_and_respect_the_budget() {
    let mut cache = ResidentCache::new(Some(1024));
    for key in 0..10_000 {
        cache.insert(key, (), 0);
        assert!(cache.bytes() <= 1024);
    }
    assert!(
        cache.len() < 1024,
        "zero-byte payloads cannot make entries free"
    );
    assert!(!cache.is_empty());
}

#[test]
fn exact_boundary_oversize_replacement_and_overflow() {
    let mut probe = ResidentCache::new(None);
    probe.insert(1_u32, (), 17);
    let weight = probe.bytes();
    let mut cache = ResidentCache::new(Some(weight));
    cache.insert(1_u32, (), 17);
    assert_eq!(cache.len(), 1);
    cache.insert(2, (), 18);
    assert!(
        cache.peek(&1).is_some(),
        "oversize must not flush unrelated work"
    );
    assert_eq!(cache.bytes(), weight);
    cache.insert(1, (), 18);
    assert!(
        cache.is_empty(),
        "inadmissible replacement retires its prior value"
    );
    cache.insert(3, (), usize::MAX);
    assert_eq!(cache.bytes(), 0, "overflow cannot bypass admission");
    let mut disabled = ResidentCache::new(Some(0));
    disabled.insert(1, (), 0);
    assert!(disabled.is_empty());
}

#[test]
fn eviction_releases_key_and_value_owners_immediately() {
    let mut cache = ResidentCache::new(None);
    let key = Arc::<str>::from("old");
    let value = Arc::<[u8]>::from([1, 2, 3].as_slice());
    let key_weak = Arc::downgrade(&key);
    let value_weak = Arc::downgrade(&value);
    cache.insert(key, value, 64);
    cache.max_bytes = Some(cache.bytes());
    cache.insert(Arc::from("new"), Arc::from([4, 5, 6].as_slice()), 64);
    assert!(key_weak.upgrade().is_none());
    assert!(value_weak.upgrade().is_none());
    cache.remove("new");
    assert_eq!(
        cache.capacity(),
        0,
        "empty caches retain no table/free-node capacity"
    );
    assert_eq!(cache.bytes(), 0);
}

#[test]
fn recency_matches_an_independent_sequence_model() {
    let mut probe = ResidentCache::new(None);
    probe.insert(0_u32, 0_u64, 0);
    let base = probe.bytes();
    let budget = base * 7 + 13;
    let mut cache = ResidentCache::new(Some(budget));
    let mut model = VecDeque::<(u32, u64, usize)>::new();
    let mut random = 0x4d6f_6c74_u64;
    for step in 0..20_000_u64 {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        let key = (random % 29) as u32;
        match (random >> 8) % 3 {
            0 => {
                let expected = model.iter().position(|entry| entry.0 == key).map(|index| {
                    let entry = model.remove(index).unwrap();
                    model.push_back(entry);
                    entry.1
                });
                assert_eq!(cache.get(&key).copied(), expected);
            }
            1 => {
                let expected = model
                    .iter()
                    .position(|entry| entry.0 == key)
                    .and_then(|index| model.remove(index))
                    .map(|entry| entry.1);
                assert_eq!(cache.remove(&key), expected);
            }
            _ => {
                let allocation = ((random >> 16) % (budget as u64 + 1)) as usize;
                if let Some(index) = model.iter().position(|entry| entry.0 == key) {
                    model.remove(index);
                }
                let charge = base + allocation;
                if charge <= budget {
                    while model.iter().map(|entry| entry.2).sum::<usize>() + charge > budget {
                        model.pop_front();
                    }
                    model.push_back((key, step, charge));
                }
                cache.insert(key, step, allocation);
            }
        }
        assert_eq!(cache.len(), model.len());
        assert_eq!(
            cache.bytes(),
            model.iter().map(|entry| entry.2).sum::<usize>()
        );
        let actual = cache
            .entries
            .iter()
            .map(|(key, entry)| (*key, entry.value))
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            model
                .iter()
                .map(|entry| (entry.0, entry.1))
                .collect::<Vec<_>>()
        );
    }
}
