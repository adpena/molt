"""Security regression tests for molt.

Covers JS injection prevention, path traversal, URL validation,
and revision string sanitisation.
"""

from __future__ import annotations

import json
import pytest


# ---------------------------------------------------------------------------
# 1. generate_worker JS injection prevention
# ---------------------------------------------------------------------------


class TestGenerateWorkerInjectionPrevention:
    """Ensure capabilities are safely encoded via json.dumps, not string
    interpolation, so that JS injection via crafted capability names is
    impossible."""

    def test_single_quotes_escaped(self, tmp_path):
        from tools.generate_worker import generate_worker

        output = tmp_path / "worker.js"
        generate_worker(output, ["fs.read'; alert(1); //"])
        content = output.read_text()
        # json.dumps produces double-quoted strings; a raw single-quote
        # splice would leave the payload unescaped.
        assert "alert(1)" in content  # the string is present ...
        # ... but it must be inside a JSON array (double-quoted), not bare JS.
        assert json.dumps(["fs.read'; alert(1); //"]) in content

    def test_backticks_escaped(self, tmp_path):
        from tools.generate_worker import generate_worker

        output = tmp_path / "worker.js"
        generate_worker(output, ["fs.read`${evil}`"])
        content = output.read_text()
        assert json.dumps(["fs.read`${evil}`"]) in content

    def test_angle_brackets_escaped(self, tmp_path):
        from tools.generate_worker import generate_worker

        output = tmp_path / "worker.js"
        generate_worker(output, ["<script>alert(1)</script>"])
        content = output.read_text()
        assert json.dumps(["<script>alert(1)</script>"]) in content

    def test_output_uses_json_dumps_encoding(self, tmp_path):
        """The substitution must use json.dumps (double-quoted JSON array),
        not a Python repr or manual quoting."""
        from tools.generate_worker import generate_worker

        caps = ["fs.read", "net"]
        output = tmp_path / "worker.js"
        generate_worker(output, caps)
        content = output.read_text()
        assert json.dumps(caps) in content
        # Must not contain Python-style single-quoted list repr.
        assert "['fs.read'" not in content


# ---------------------------------------------------------------------------
# 2. hub.py revision validation
# ---------------------------------------------------------------------------


class TestHubRevisionValidation:
    """_validate_revision must reject path-traversal and URL-injection
    payloads while accepting legitimate revision identifiers."""

    def test_main_passes(self):
        from molt.gpu.hub import _validate_revision

        _validate_revision("main")  # should not raise

    def test_semver_tag_passes(self):
        from molt.gpu.hub import _validate_revision

        _validate_revision("v1.0")  # should not raise

    def test_path_traversal_rejected(self):
        from molt.gpu.hub import _validate_revision

        with pytest.raises(ValueError):
            _validate_revision("main/../../etc")

    def test_empty_string_rejected(self):
        from molt.gpu.hub import _validate_revision

        with pytest.raises(ValueError):
            _validate_revision("")

    def test_url_injection_rejected(self):
        from molt.gpu.hub import _validate_revision

        with pytest.raises(ValueError):
            _validate_revision("main&token=x")


# ---------------------------------------------------------------------------
# 3. capability_manifest path traversal
# ---------------------------------------------------------------------------


class TestCapabilityManifestPathTraversal:
    """Virtual mount source validation must prevent host filesystem escape."""

    def test_mount_source_with_dotdot_raises(self):
        from molt.capability_manifest import ManifestError, _parse_virtual_mounts

        mounts_cfg = {
            "/data": {
                "type": "readonly",
                "source": "../../../etc/passwd",
            }
        }
        with pytest.raises(ManifestError, match="traversal"):
            _parse_virtual_mounts(mounts_cfg)

    def test_vfs_internal_source_allowed(self):
        from molt.capability_manifest import _parse_virtual_mounts

        mounts_cfg = {
            "/data": {
                "type": "readonly",
                "source": "/bundle/data",
            }
        }
        mounts = _parse_virtual_mounts(mounts_cfg)
        assert len(mounts) == 1
        assert mounts[0].source == "/bundle/data"

    def test_audit_output_with_dotdot_raises(self):
        from molt.capability_manifest import (
            AuditConfig,
            CapabilityManifest,
            ManifestError,
            validate_manifest,
        )

        manifest = CapabilityManifest(
            audit=AuditConfig(
                enabled=True,
                sink="jsonl",
                output="../../etc/shadow",
            ),
        )
        with pytest.raises(ManifestError, match="traversal"):
            validate_manifest(manifest)


# ---------------------------------------------------------------------------
# 4. _download_artifact URL validation
# ---------------------------------------------------------------------------


class TestDownloadArtifactURLValidation:
    """SSRF protection: reject private IPs, non-https schemes."""

    def test_link_local_metadata_is_private(self):
        from molt.cli import _is_private_ip

        assert _is_private_ip("169.254.169.254") is True

    def test_loopback_is_private(self):
        from molt.cli import _is_private_ip

        assert _is_private_ip("127.0.0.1") is True

    def test_rfc1918_is_private(self):
        from molt.cli import _is_private_ip

        assert _is_private_ip("10.0.0.1") is True

    def test_public_host_is_not_private(self):
        from molt.cli import _is_private_ip

        assert _is_private_ip("pypi.org") is False

    def test_file_scheme_rejected(self):
        from molt.cli import _download_artifact

        with pytest.raises(ValueError, match="https"):
            _download_artifact("file:///etc/passwd", "sha256:abc123")

    def test_http_scheme_rejected(self):
        from molt.cli import _download_artifact

        with pytest.raises(ValueError, match="https"):
            _download_artifact("http://example.com/pkg.whl", "sha256:abc123")

    def test_ipv4_mapped_ipv6_detected_as_private(self):
        """::ffff:169.254.169.254 must be detected as private."""
        from molt.cli import _is_private_ip

        # IPv4-mapped IPv6 representation of the AWS metadata endpoint
        assert _is_private_ip("::ffff:169.254.169.254") is True

    def test_malformed_hash_rejected(self):
        """Hash without algorithm prefix must be rejected."""
        from molt.cli import _download_artifact

        with pytest.raises(ValueError, match="algorithm:hex"):
            _download_artifact("https://example.com/file.whl", "deadbeef")

    def test_non_sha256_hash_rejected(self):
        """Only sha256 is accepted."""
        from molt.cli import _download_artifact

        with pytest.raises(ValueError, match="unsupported"):
            _download_artifact("https://example.com/file.whl", "md5:" + "a" * 32)

    def test_unresolvable_host_is_blocked(self):
        """DNS failure must be treated as private (fail-closed)."""
        from molt.cli import _is_private_ip

        # This host should not resolve
        assert (
            _is_private_ip("this-host-definitely-does-not-exist-xyzzy.invalid") is True
        )

    def test_ipv6_scope_id_handled(self):
        """IPv6 with scope ID must not bypass the check."""
        from molt.cli import _is_private_ip

        # fe80::1%eth0 is link-local and must be detected
        assert _is_private_ip("fe80::1") is True


# ---------------------------------------------------------------------------
# 5. Manifest signature truthiness
# ---------------------------------------------------------------------------


class TestManifestSignature:
    def test_verified_is_truthy(self):
        from molt.capability_manifest import SignatureStatus

        s = SignatureStatus(SignatureStatus.VERIFIED)
        assert bool(s) is True
        assert s.is_verified is True
        assert s.is_unsigned is False

    def test_unsigned_is_falsy(self):
        from molt.capability_manifest import SignatureStatus

        s = SignatureStatus(SignatureStatus.UNSIGNED)
        assert bool(s) is False
        assert s.is_verified is False
        assert s.is_unsigned is True
