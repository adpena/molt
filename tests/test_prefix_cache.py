"""Tests for prefix caching in kv_cache.py (radix tree / trie)."""

import sys
import os
import math

# Load the PrefixCache and _RadixNode classes directly from kv_cache.py,
# bypassing molt's import system. These classes are pure Python (no
# Tensor/LazyOp dependencies).
_kv_path = os.path.join(
    os.path.dirname(__file__), "..", "src", "molt", "stdlib", "tinygrad", "kv_cache.py"
)
_kv_path = os.path.abspath(_kv_path)

with open(_kv_path) as f:
    _full_source = f.read()

# Extract from the prefix caching section to end of file.
_prefix_marker = "# Prefix Caching via Radix Tree"
_prefix_start = _full_source.index(_prefix_marker)
_prefix_source = _full_source[_prefix_start:]

_ns = {"__builtins__": __builtins__, "math": math}
exec(compile(_prefix_source, _kv_path, "exec"), _ns)

PrefixCache = _ns["PrefixCache"]


def make_kv(d_k, val=1.0):
    """Create a (k_data, v_data) pair with constant values."""
    return ([val] * d_k, [val * 2] * d_k)


D_K = 16  # dimension for test vectors


# --- Basic insert and lookup ---

def test_empty_lookup():
    cache = PrefixCache(d_k=D_K)
    cached_len, kv_blocks = cache.lookup_prefix([1, 2, 3])
    assert cached_len == 0
    assert kv_blocks == []


def test_insert_and_lookup_exact():
    cache = PrefixCache(d_k=D_K)
    tokens = [10, 20, 30]
    kv_blocks = [make_kv(D_K, float(i)) for i in range(3)]
    cache.insert(tokens, kv_blocks)

    cached_len, result = cache.lookup_prefix(tokens)
    assert cached_len == 3
    assert len(result) == 3
    # Verify the actual KV data matches.
    for i in range(3):
        assert result[i][0] == kv_blocks[i][0]
        assert result[i][1] == kv_blocks[i][1]


def test_lookup_prefix_match():
    cache = PrefixCache(d_k=D_K)
    tokens = [10, 20, 30]
    kv_blocks = [make_kv(D_K, float(i)) for i in range(3)]
    cache.insert(tokens, kv_blocks)

    # Lookup with a longer sequence that shares the prefix.
    cached_len, result = cache.lookup_prefix([10, 20, 30, 40, 50])
    assert cached_len == 3
    assert len(result) == 3


def test_lookup_partial_prefix():
    cache = PrefixCache(d_k=D_K)
    tokens = [10, 20, 30]
    kv_blocks = [make_kv(D_K, float(i)) for i in range(3)]
    cache.insert(tokens, kv_blocks)

    # Lookup with only the first 2 tokens.
    cached_len, result = cache.lookup_prefix([10, 20])
    assert cached_len == 2
    assert len(result) == 2


def test_lookup_no_match():
    cache = PrefixCache(d_k=D_K)
    tokens = [10, 20, 30]
    kv_blocks = [make_kv(D_K, float(i)) for i in range(3)]
    cache.insert(tokens, kv_blocks)

    # Completely different prefix.
    cached_len, result = cache.lookup_prefix([99, 98, 97])
    assert cached_len == 0
    assert result == []


def test_lookup_diverges_midway():
    cache = PrefixCache(d_k=D_K)
    tokens = [10, 20, 30]
    kv_blocks = [make_kv(D_K, float(i)) for i in range(3)]
    cache.insert(tokens, kv_blocks)

    # Shares first token, diverges at second.
    cached_len, result = cache.lookup_prefix([10, 99])
    assert cached_len == 1
    assert len(result) == 1


# --- Multiple prefixes ---

def test_two_prefixes_shared_root():
    cache = PrefixCache(d_k=D_K)
    # Two prompts sharing prefix [10, 20]
    cache.insert([10, 20, 30], [make_kv(D_K) for _ in range(3)])
    cache.insert([10, 20, 40], [make_kv(D_K) for _ in range(3)])

    # Both should find the shared prefix.
    len1, _ = cache.lookup_prefix([10, 20, 30])
    assert len1 == 3
    len2, _ = cache.lookup_prefix([10, 20, 40])
    assert len2 == 3

    # The shared prefix [10, 20] should work too.
    len3, _ = cache.lookup_prefix([10, 20])
    assert len3 == 2


def test_disjoint_prefixes():
    cache = PrefixCache(d_k=D_K)
    cache.insert([1, 2, 3], [make_kv(D_K) for _ in range(3)])
    cache.insert([4, 5, 6], [make_kv(D_K) for _ in range(3)])

    len1, _ = cache.lookup_prefix([1, 2, 3])
    assert len1 == 3
    len2, _ = cache.lookup_prefix([4, 5, 6])
    assert len2 == 3
    len3, _ = cache.lookup_prefix([1, 5])
    assert len3 == 1


# --- LRU eviction ---

def test_eviction_at_capacity():
    # Cache with max 3 blocks.
    cache = PrefixCache(d_k=D_K, max_cached_blocks=3)
    cache.insert([1, 2, 3], [make_kv(D_K) for _ in range(3)])
    assert cache.total_blocks == 3

    # Inserting 3 more blocks should evict the 3 oldest.
    cache.insert([4, 5, 6], [make_kv(D_K) for _ in range(3)])
    assert cache.total_blocks <= 3

    # The new prefix should be cached.
    cached_len, _ = cache.lookup_prefix([4, 5, 6])
    assert cached_len == 3


def test_lru_preserves_recently_accessed():
    cache = PrefixCache(d_k=D_K, max_cached_blocks=6)
    cache.insert([1, 2, 3], [make_kv(D_K) for _ in range(3)])
    cache.insert([4, 5, 6], [make_kv(D_K) for _ in range(3)])
    assert cache.total_blocks == 6

    # Access the first prefix to make it more recent.
    cache.lookup_prefix([1, 2, 3])

    # Insert 3 more — should evict the second prefix (older access).
    cache.insert([7, 8, 9], [make_kv(D_K) for _ in range(3)])
    assert cache.total_blocks <= 6

    # First prefix should still be cached (recently accessed).
    len1, _ = cache.lookup_prefix([1, 2, 3])
    assert len1 == 3


# --- Invalidation ---

def test_invalidate_existing():
    cache = PrefixCache(d_k=D_K)
    cache.insert([1, 2, 3], [make_kv(D_K) for _ in range(3)])
    assert cache.total_blocks == 3

    removed = cache.invalidate([1, 2, 3])
    assert removed == 1  # Removes the leaf node and its block.

    # After invalidation, only the prefix [1, 2] should remain.
    cached_len, _ = cache.lookup_prefix([1, 2, 3])
    assert cached_len == 2


def test_invalidate_nonexistent():
    cache = PrefixCache(d_k=D_K)
    cache.insert([1, 2, 3], [make_kv(D_K) for _ in range(3)])
    removed = cache.invalidate([9, 9, 9])
    assert removed == 0
    assert cache.total_blocks == 3


def test_invalidate_subtree():
    cache = PrefixCache(d_k=D_K)
    cache.insert([1, 2, 3], [make_kv(D_K) for _ in range(3)])
    cache.insert([1, 2, 4], [make_kv(D_K) for _ in range(3)])
    assert cache.total_blocks == 4  # shared [1,2] + leaf [3] + leaf [4]

    # Invalidate [1, 2] — removes the node at [1,2] and its children [3] and [4].
    removed = cache.invalidate([1, 2])
    assert removed == 3  # node [2] + children [3] and [4]


# --- Clear ---

def test_clear():
    cache = PrefixCache(d_k=D_K)
    cache.insert([1, 2, 3], [make_kv(D_K) for _ in range(3)])
    cache.insert([4, 5], [make_kv(D_K) for _ in range(2)])
    assert cache.total_blocks == 5

    cache.clear()
    assert cache.total_blocks == 0
    cached_len, _ = cache.lookup_prefix([1, 2, 3])
    assert cached_len == 0


# --- Edge cases ---

def test_empty_token_sequence():
    cache = PrefixCache(d_k=D_K)
    cache.insert([], [])
    cached_len, _ = cache.lookup_prefix([])
    assert cached_len == 0


def test_single_token():
    cache = PrefixCache(d_k=D_K)
    cache.insert([42], [make_kv(D_K)])
    cached_len, kv = cache.lookup_prefix([42])
    assert cached_len == 1
    assert len(kv) == 1


def test_duplicate_insert_idempotent():
    """Inserting the same prefix twice should not duplicate blocks."""
    cache = PrefixCache(d_k=D_K)
    tokens = [1, 2, 3]
    kv = [make_kv(D_K) for _ in range(3)]
    cache.insert(tokens, kv)
    assert cache.total_blocks == 3

    cache.insert(tokens, kv)  # duplicate insert
    assert cache.total_blocks == 3  # no duplicates


def test_validation_mismatched_lengths():
    cache = PrefixCache(d_k=D_K)
    try:
        cache.insert([1, 2], [make_kv(D_K)])  # 2 tokens but 1 block
        assert False, "should have raised ValueError"
    except ValueError:
        pass


def test_validation_bad_dk():
    try:
        PrefixCache(d_k=0)
        assert False, "should have raised ValueError"
    except ValueError:
        pass


def test_validation_bad_max_blocks():
    try:
        PrefixCache(d_k=16, max_cached_blocks=0)
        assert False, "should have raised ValueError"
    except ValueError:
        pass


def test_validation_wrong_vector_size():
    cache = PrefixCache(d_k=D_K)
    try:
        cache.insert([1], [([1.0] * 5, [1.0] * D_K)])  # k wrong size
        assert False, "should have raised ValueError"
    except ValueError:
        pass


# --- Properties ---

def test_d_k_property():
    cache = PrefixCache(d_k=32)
    assert cache.d_k == 32


def test_total_blocks_property():
    cache = PrefixCache(d_k=D_K)
    assert cache.total_blocks == 0
    cache.insert([1, 2], [make_kv(D_K) for _ in range(2)])
    assert cache.total_blocks == 2


if __name__ == "__main__":
    import pytest
    pytest.main([__file__, "-v"])
