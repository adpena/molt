# MOLT_META: area=property-testing
"""Property-based tests for hashlib and base64 intrinsics.

Tests determinism, fixed output lengths, collision resistance (probabilistic),
and base64 encode/decode roundtrip.
"""

from __future__ import annotations

import base64
import hashlib

from hypothesis import assume, given, settings
from hypothesis import strategies as st

# ---------------------------------------------------------------------------
# Strategies
# ---------------------------------------------------------------------------

binary_data = st.binary(min_size=0, max_size=500)
short_binary = st.binary(min_size=0, max_size=100)

SETTINGS = dict(max_examples=200, deadline=None, database=None)


# ---------------------------------------------------------------------------
# hashlib.md5
# ---------------------------------------------------------------------------


class TestMd5:
    """hashlib.md5() properties."""

    @given(data=binary_data)
    @settings(**SETTINGS)
    def test_md5_hexdigest_length(self, data: bytes) -> None:
        """hashlib.md5(data).hexdigest() always has length 32."""
        assert len(hashlib.md5(data).hexdigest()) == 32

    @given(data=binary_data)
    @settings(**SETTINGS)
    def test_md5_digest_length(self, data: bytes) -> None:
        """hashlib.md5(data).digest() always has length 16."""
        assert len(hashlib.md5(data).digest()) == 16

    @given(data=binary_data)
    @settings(**SETTINGS)
    def test_md5_deterministic(self, data: bytes) -> None:
        """Same input always produces the same MD5 hash."""
        assert hashlib.md5(data).hexdigest() == hashlib.md5(data).hexdigest()

    @given(data=binary_data)
    @settings(**SETTINGS)
    def test_md5_hexdigest_is_hex(self, data: bytes) -> None:
        """MD5 hexdigest contains only hex characters."""
        digest = hashlib.md5(data).hexdigest()
        assert all(c in "0123456789abcdef" for c in digest)

    @given(data=binary_data)
    @settings(**SETTINGS)
    def test_md5_digest_hexdigest_consistent(self, data: bytes) -> None:
        """digest().hex() == hexdigest()."""
        h = hashlib.md5(data)
        assert h.digest().hex() == h.hexdigest()


# ---------------------------------------------------------------------------
# hashlib.sha256
# ---------------------------------------------------------------------------


class TestSha256:
    """hashlib.sha256() properties."""

    @given(data=binary_data)
    @settings(**SETTINGS)
    def test_sha256_hexdigest_length(self, data: bytes) -> None:
        """hashlib.sha256(data).hexdigest() always has length 64."""
        assert len(hashlib.sha256(data).hexdigest()) == 64

    @given(data=binary_data)
    @settings(**SETTINGS)
    def test_sha256_digest_length(self, data: bytes) -> None:
        """hashlib.sha256(data).digest() always has length 32."""
        assert len(hashlib.sha256(data).digest()) == 32

    @given(data=binary_data)
    @settings(**SETTINGS)
    def test_sha256_deterministic(self, data: bytes) -> None:
        """Same input always produces the same SHA-256 hash."""
        assert hashlib.sha256(data).hexdigest() == hashlib.sha256(data).hexdigest()

    @given(data=binary_data)
    @settings(**SETTINGS)
    def test_sha256_hexdigest_is_hex(self, data: bytes) -> None:
        """SHA-256 hexdigest contains only hex characters."""
        digest = hashlib.sha256(data).hexdigest()
        assert all(c in "0123456789abcdef" for c in digest)

    @given(data=binary_data)
    @settings(**SETTINGS)
    def test_sha256_digest_hexdigest_consistent(self, data: bytes) -> None:
        """digest().hex() == hexdigest()."""
        h = hashlib.sha256(data)
        assert h.digest().hex() == h.hexdigest()


# ---------------------------------------------------------------------------
# Hash determinism (cross-algorithm)
# ---------------------------------------------------------------------------


class TestHashDeterminism:
    """Cross-algorithm determinism and consistency."""

    @given(data=binary_data)
    @settings(**SETTINGS)
    def test_incremental_equals_oneshot_md5(self, data: bytes) -> None:
        """Incremental update produces same result as one-shot for MD5."""
        oneshot = hashlib.md5(data).hexdigest()
        h = hashlib.md5()
        h.update(data)
        assert h.hexdigest() == oneshot

    @given(data=binary_data)
    @settings(**SETTINGS)
    def test_incremental_equals_oneshot_sha256(self, data: bytes) -> None:
        """Incremental update produces same result as one-shot for SHA-256."""
        oneshot = hashlib.sha256(data).hexdigest()
        h = hashlib.sha256()
        h.update(data)
        assert h.hexdigest() == oneshot

    @given(
        a=short_binary,
        b=short_binary,
    )
    @settings(**SETTINGS)
    def test_split_update_md5(self, a: bytes, b: bytes) -> None:
        """MD5 of concatenated data == MD5 via split updates."""
        oneshot = hashlib.md5(a + b).hexdigest()
        h = hashlib.md5()
        h.update(a)
        h.update(b)
        assert h.hexdigest() == oneshot

    @given(
        a=short_binary,
        b=short_binary,
    )
    @settings(**SETTINGS)
    def test_split_update_sha256(self, a: bytes, b: bytes) -> None:
        """SHA-256 of concatenated data == SHA-256 via split updates."""
        oneshot = hashlib.sha256(a + b).hexdigest()
        h = hashlib.sha256()
        h.update(a)
        h.update(b)
        assert h.hexdigest() == oneshot


# ---------------------------------------------------------------------------
# Probabilistic collision resistance
# ---------------------------------------------------------------------------


class TestCollisionResistance:
    """Different inputs probabilistically produce different hashes."""

    @given(a=short_binary, b=short_binary)
    @settings(**SETTINGS)
    def test_different_inputs_different_md5(self, a: bytes, b: bytes) -> None:
        """Different inputs produce different MD5 hashes (when inputs differ)."""
        assume(a != b)
        # This is probabilistic — collision is astronomically unlikely
        assert hashlib.md5(a).hexdigest() != hashlib.md5(b).hexdigest()

    @given(a=short_binary, b=short_binary)
    @settings(**SETTINGS)
    def test_different_inputs_different_sha256(self, a: bytes, b: bytes) -> None:
        """Different inputs produce different SHA-256 hashes (when inputs differ)."""
        assume(a != b)
        assert hashlib.sha256(a).hexdigest() != hashlib.sha256(b).hexdigest()


# ---------------------------------------------------------------------------
# base64 roundtrip
# ---------------------------------------------------------------------------


class TestBase64Roundtrip:
    """base64 encode/decode roundtrip properties."""

    @given(data=binary_data)
    @settings(**SETTINGS)
    def test_b64_encode_decode_roundtrip(self, data: bytes) -> None:
        """base64.b64decode(base64.b64encode(data)) == data."""
        assert base64.b64decode(base64.b64encode(data)) == data

    @given(data=binary_data)
    @settings(**SETTINGS)
    def test_b64_encode_is_ascii(self, data: bytes) -> None:
        """base64.b64encode always produces ASCII bytes."""
        encoded = base64.b64encode(data)
        # All bytes in base64 output are valid ASCII
        encoded.decode("ascii")  # Raises if not ASCII

    @given(data=binary_data)
    @settings(**SETTINGS)
    def test_b64_encode_length(self, data: bytes) -> None:
        """base64.b64encode output length is ceil(len(data)/3)*4."""
        encoded = base64.b64encode(data)
        expected_len = ((len(data) + 2) // 3) * 4
        assert len(encoded) == expected_len

    @given(data=binary_data)
    @settings(**SETTINGS)
    def test_urlsafe_b64_roundtrip(self, data: bytes) -> None:
        """base64.urlsafe_b64decode(base64.urlsafe_b64encode(data)) == data."""
        assert base64.urlsafe_b64decode(base64.urlsafe_b64encode(data)) == data

    @given(data=binary_data)
    @settings(**SETTINGS)
    def test_b64_deterministic(self, data: bytes) -> None:
        """Same input always produces the same base64 encoding."""
        assert base64.b64encode(data) == base64.b64encode(data)
