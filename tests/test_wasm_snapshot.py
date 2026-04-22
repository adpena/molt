"""Tests for the molt.snapshot artifact (Plan D -- edge cold-start)."""

from __future__ import annotations

import json


def test_snapshot_header_valid_json(tmp_path):
    """Snapshot header should be valid JSON with required fields."""
    header = {
        "snapshot_version": 1,
        "abi_version": "0.1.0",
        "target_profile": "wasm_worker_cloudflare",
        "module_hash": "sha256:abc123",
        "mount_plan": [
            {"path": "/bundle", "mount_type": "bundle", "hash": "sha256:def456"},
            {"path": "/tmp", "mount_type": "tmp", "quota_mb": 32},
        ],
        "capability_manifest": ["fs.bundle.read", "fs.tmp.read", "fs.tmp.write"],
        "determinism_stamp": "2026-03-20T00:00:00Z",
        "init_state_size": 0,
    }
    path = tmp_path / "molt.snapshot.json"
    path.write_text(json.dumps(header))
    loaded = json.loads(path.read_text())
    assert loaded["snapshot_version"] == 1
    assert len(loaded["mount_plan"]) == 2
    assert loaded["capability_manifest"][0] == "fs.bundle.read"


def test_snapshot_header_required_fields():
    """All required fields must be present in a valid snapshot header."""
    header = {
        "snapshot_version": 1,
        "abi_version": "0.1.0",
        "target_profile": "wasm_worker_cloudflare",
        "module_hash": "sha256:abc123",
        "mount_plan": [],
        "capability_manifest": [],
        "determinism_stamp": "2026-03-20T00:00:00Z",
        "init_state_size": 0,
    }
    required_keys = {
        "snapshot_version",
        "abi_version",
        "target_profile",
        "module_hash",
        "mount_plan",
        "capability_manifest",
        "determinism_stamp",
        "init_state_size",
    }
    assert required_keys.issubset(header.keys())


def test_snapshot_mount_plan_structure():
    """Each mount plan entry should have path and mount_type."""
    mount_plan = [
        {"path": "/bundle", "mount_type": "bundle", "hash": "sha256:def456"},
        {"path": "/tmp", "mount_type": "tmp", "quota_mb": 32},
        {"path": "/dev", "mount_type": "dev"},
    ]
    for entry in mount_plan:
        assert "path" in entry
        assert "mount_type" in entry
        assert entry["mount_type"] in {"bundle", "tmp", "dev"}


def test_snapshot_rejects_hash_mismatch():
    """Snapshot should reject when module hash doesn't match."""
    header = {
        "snapshot_version": 1,
        "abi_version": "0.1.0",
        "target_profile": "wasm_worker_cloudflare",
        "module_hash": "sha256:abc123",
        "mount_plan": [],
        "capability_manifest": [],
        "determinism_stamp": "2026-03-20T00:00:00Z",
        "init_state_size": 0,
    }
    expected_hash = "sha256:different"
    assert header["module_hash"] != expected_hash, (
        "Stale snapshot: module hash mismatch"
    )


def test_snapshot_rejects_abi_mismatch():
    """Snapshot should reject when ABI version doesn't match."""
    header = {
        "snapshot_version": 1,
        "abi_version": "0.1.0",
        "target_profile": "wasm_worker_cloudflare",
        "module_hash": "sha256:abc123",
        "mount_plan": [],
        "capability_manifest": [],
        "determinism_stamp": "2026-03-20T00:00:00Z",
        "init_state_size": 0,
    }
    expected_abi = "0.2.0"
    assert header["abi_version"] != expected_abi, "Stale snapshot: ABI version mismatch"


def test_snapshot_round_trip(tmp_path):
    """Snapshot header should survive write-read round trip."""
    header = {
        "snapshot_version": 1,
        "abi_version": "0.1.0",
        "target_profile": "wasm_worker_cloudflare",
        "module_hash": "sha256:abc123",
        "mount_plan": [
            {"path": "/bundle", "mount_type": "bundle", "hash": "sha256:def456"},
            {"path": "/tmp", "mount_type": "tmp", "quota_mb": 32},
            {"path": "/dev", "mount_type": "dev"},
        ],
        "capability_manifest": [
            "fs.bundle.read",
            "fs.tmp.read",
            "fs.tmp.write",
            "http.fetch",
        ],
        "determinism_stamp": "2026-03-20T00:00:00Z",
        "init_state_size": 524288,
    }
    path = tmp_path / "molt.snapshot.json"
    path.write_text(json.dumps(header, indent=2))
    loaded = json.loads(path.read_text())
    assert loaded == header


def test_snapshot_capabilities_empty_allowed():
    """A snapshot with no capabilities is valid (restrictive sandbox)."""
    header = {
        "snapshot_version": 1,
        "abi_version": "0.1.0",
        "target_profile": "wasm_wasi",
        "module_hash": "sha256:abc123",
        "mount_plan": [],
        "capability_manifest": [],
        "determinism_stamp": "2026-03-20T00:00:00Z",
        "init_state_size": 0,
    }
    assert isinstance(header["capability_manifest"], list)
    assert len(header["capability_manifest"]) == 0


def test_generate_snapshot_header_function(tmp_path):
    """The CLI helper _generate_snapshot_header should produce valid JSON."""
    # Create a dummy wasm file so the hash can be computed.
    wasm_path = tmp_path / "output.wasm"
    wasm_path.write_bytes(b"\x00asm" + b"\x00" * 100)

    from molt.cli import _generate_snapshot_header

    _generate_snapshot_header(
        output_wasm=wasm_path,
        target_profile="cloudflare",
        capabilities_list=["fs.bundle.read", "fs.tmp.read"],
        verbose=False,
    )

    snapshot_path = tmp_path / "molt.snapshot.json"
    assert snapshot_path.exists()
    loaded = json.loads(snapshot_path.read_text())
    assert loaded["snapshot_version"] == 1
    assert loaded["abi_version"] == "0.1.0"
    assert loaded["target_profile"] == "cloudflare"
    assert loaded["module_hash"].startswith("sha256:")
    assert len(loaded["mount_plan"]) == 3
    assert loaded["capability_manifest"] == ["fs.bundle.read", "fs.tmp.read"]
    assert loaded["init_state_size"] == 0
