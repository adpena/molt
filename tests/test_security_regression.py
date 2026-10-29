"""Security regression tests for molt.

Covers JS injection prevention, path traversal, URL validation,
and revision string sanitisation.
"""

from __future__ import annotations

from tests.wasm_execution_manifest import (
    EMPTY_WASM_MODULE,
    write_wasm_execution_manifest,
)

import json
import pytest


# ---------------------------------------------------------------------------
# 1. generate_worker JS injection prevention
# ---------------------------------------------------------------------------


class TestGenerateWorkerInjectionPrevention:
    """Admitted manifest strings remain JSON data in generated JavaScript."""

    @staticmethod
    def _render(tmp_path, payload):
        from tools.generate_worker import generate_worker

        output = tmp_path / "worker.js"
        module = tmp_path / "worker_linked.wasm"
        module.write_bytes(EMPTY_WASM_MODULE)
        manifest = write_wasm_execution_manifest(tmp_path, linked=module)
        deployment = json.loads(manifest.read_text(encoding="utf-8"))
        deployment["fixture_metadata"] = payload
        deployment["launch"]["environment"]["FIXTURE_VALUE"] = (
            payload if isinstance(payload, str) else json.dumps(payload)
        )
        manifest.write_text(json.dumps(deployment), encoding="utf-8")
        generate_worker(output)
        return output.read_text(encoding="utf-8")

    @staticmethod
    def _assert_transported(content, payload):
        prefix = "const deploymentManifest = globalThis.MoltWasmLoaderBridge.requireMoltDeploymentManifest(JSON.parse("
        line = next(line for line in content.splitlines() if line.startswith(prefix))
        assert line.endswith("));")
        # Independently decode the outer JS string literal and the inner JSON
        # document; hostile text must be data at both serialization boundaries.
        deployment = json.loads(json.loads(line[len(prefix) : -3]))
        assert deployment["fixture_metadata"] == payload
        expected_env = payload if isinstance(payload, str) else json.dumps(payload)
        assert deployment["launch"]["environment"]["FIXTURE_VALUE"] == expected_env
        assert deployment["launch"]["environment"]["MOLT_CAPABILITIES"] == ""
        assert deployment["launch"]["environment"]["MOLT_CAPABILITY_TIER"] == "none"

    def test_single_quotes_escaped(self, tmp_path):
        payload = "fs.read'; alert(1); //"
        content = self._render(tmp_path, payload)
        assert "alert(1)" in content
        self._assert_transported(content, payload)

    def test_backticks_escaped(self, tmp_path):
        payload = "fs.read`${evil}`"
        self._assert_transported(self._render(tmp_path, payload), payload)

    def test_angle_brackets_escaped(self, tmp_path):
        payload = "<script>alert(1)</script>"
        self._assert_transported(self._render(tmp_path, payload), payload)

    def test_output_uses_json_dumps_encoding(self, tmp_path):
        payload = ["fs.read", "net"]
        content = self._render(tmp_path, payload)
        self._assert_transported(content, payload)
        assert "['fs.read'" not in content

    def test_manifest_template_tokens_remain_literal_data(self, tmp_path):
        payload = "{{TMP_QUOTA_MB}} {{WASM_FILENAME}} {{DEBUG_STDERR}}"
        self._assert_transported(self._render(tmp_path, payload), payload)


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
        from molt.cli.deps import _is_private_ip

        assert _is_private_ip("169.254.169.254") is True

    def test_loopback_is_private(self):
        from molt.cli.deps import _is_private_ip

        assert _is_private_ip("127.0.0.1") is True

    def test_rfc1918_is_private(self):
        from molt.cli.deps import _is_private_ip

        assert _is_private_ip("10.0.0.1") is True

    def test_public_host_is_not_private(self):
        from molt.cli.deps import _is_private_ip

        assert _is_private_ip("pypi.org") is False

    def test_file_scheme_rejected(self):
        from molt.cli.deps import _download_artifact

        with pytest.raises(ValueError, match="https"):
            _download_artifact("file:///etc/passwd", "sha256:abc123")

    def test_http_scheme_rejected(self):
        from molt.cli.deps import _download_artifact

        with pytest.raises(ValueError, match="https"):
            _download_artifact("http://example.com/pkg.whl", "sha256:abc123")

    def test_ipv4_mapped_ipv6_detected_as_private(self):
        """::ffff:169.254.169.254 must be detected as private."""
        from molt.cli.deps import _is_private_ip

        # IPv4-mapped IPv6 representation of the AWS metadata endpoint
        assert _is_private_ip("::ffff:169.254.169.254") is True

    def test_malformed_hash_rejected(self):
        """Hash without algorithm prefix must be rejected."""
        from molt.cli.deps import _download_artifact

        with pytest.raises(ValueError, match="algorithm:hex"):
            _download_artifact("https://example.com/file.whl", "deadbeef")

    def test_non_sha256_hash_rejected(self):
        """Only sha256 is accepted."""
        from molt.cli.deps import _download_artifact

        with pytest.raises(ValueError, match="unsupported"):
            _download_artifact("https://example.com/file.whl", "md5:" + "a" * 32)

    def test_unresolvable_host_is_blocked(self):
        """DNS failure must be treated as private (fail-closed)."""
        from molt.cli.deps import _is_private_ip

        # This host should not resolve
        assert (
            _is_private_ip("this-host-definitely-does-not-exist-xyzzy.invalid") is True
        )

    def test_ipv6_scope_id_handled(self):
        """IPv6 with scope ID must not bypass the check."""
        from molt.cli.deps import _is_private_ip

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
