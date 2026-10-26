"""Snapshot metadata-template and execution-identity regressions."""

from __future__ import annotations

from molt._wasm_abi_generated import WASM_EXECUTION_MANIFEST_VERSION

import hashlib
import json
from pathlib import Path

import pytest

from molt.capability_manifest import CapabilityManifest
from molt.cli.non_native_output import (
    _generate_snapshot_header,
    _snapshot_execution_identity,
)


def _asset(path: Path) -> dict[str, object]:
    payload = path.read_bytes()
    return {
        "path": path.name,
        "size": len(payload),
        "sha256": hashlib.sha256(payload).hexdigest(),
    }


def _write_manifest(root: Path, *, mode: str) -> tuple[str, ...]:
    if mode == "linked":
        linked = root / "program.wasm"
        linked.write_bytes(b"linked module")
        assets = (_asset(linked),)
        modules = {"linked": assets[0]}
    else:
        app = root / "app.wasm"
        runtime = root / "molt_runtime.wasm"
        app.write_bytes(b"app module")
        runtime.write_bytes(b"runtime module")
        assets = (_asset(app), _asset(runtime))
        modules = {"app": assets[0], "runtime": assets[1]}
    (root / "manifest.json").write_text(
        json.dumps(
            {
                "version": WASM_EXECUTION_MANIFEST_VERSION,
                "mode": mode,
                "modules": modules,
            }
        ),
        encoding="utf-8",
    )
    return tuple(f"sha256:{asset['sha256']}" for asset in assets)


def _generate(
    root: Path, *, capabilities: list[str] | None = None
) -> dict[str, object]:
    output = root / "output.wasm"
    output.write_bytes(b"\0asm")
    _generate_snapshot_header(
        output_wasm=output,
        target_profile="cloudflare",
        resolved_capability_policy=CapabilityManifest(
            allow=capabilities if capabilities is not None else []
        ).resolve(),
        verbose=False,
    )
    return json.loads((root / "molt.snapshot.json").read_text(encoding="utf-8"))


def test_metadata_template_never_claims_restorable_state(tmp_path: Path) -> None:
    header = _generate(tmp_path, capabilities=["fs.bundle.read", "fs.tmp.read"])

    assert header["snapshot_version"] == 2
    assert header["artifact_kind"] == "metadata-template"
    assert header["restorable"] is False
    assert header["state_scope"] is None
    assert header["execution_identity"] is None
    assert header["init_state_size"] == 0
    assert header["payload_hash"] is None
    assert header["integrity_hash"] is None
    assert header["capability_manifest"] == ["fs.bundle.read", "fs.tmp.read"]
    assert header["capability_policy_digest"].startswith("sha256:")
    assert header["determinism_stamp"] == "1980-01-01T00:00:00Z"


def test_execution_identity_uses_shared_linked_golden_encoding(tmp_path: Path) -> None:
    (linked_digest,) = _write_manifest(tmp_path, mode="linked")
    assert _snapshot_execution_identity(tmp_path / "output.wasm") == (
        f"molt.snapshot.execution.v2|linked|linked={linked_digest}"
    )
    assert _generate(tmp_path)["execution_identity"] == (
        f"molt.snapshot.execution.v2|linked|linked={linked_digest}"
    )


def test_execution_identity_binds_both_split_runtime_modules(tmp_path: Path) -> None:
    app_digest, runtime_digest = _write_manifest(tmp_path, mode="split-runtime")
    first = _snapshot_execution_identity(tmp_path / "output.wasm")
    assert first == (
        "molt.snapshot.execution.v2|split-runtime|"
        f"app={app_digest}|runtime={runtime_digest}"
    )

    runtime = tmp_path / "molt_runtime.wasm"
    runtime.write_bytes(b"different runtime module")
    manifest = json.loads((tmp_path / "manifest.json").read_text(encoding="utf-8"))
    manifest["modules"]["runtime"] = _asset(runtime)
    (tmp_path / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
    second = _snapshot_execution_identity(tmp_path / "output.wasm")
    assert second != first
    assert second.startswith(
        f"molt.snapshot.execution.v2|split-runtime|app={app_digest}|runtime=sha256:"
    )


@pytest.mark.parametrize(
    "mutation", ["legacy", "malformed", "digest-drift", "path-escape"]
)
def test_existing_execution_manifest_fails_closed(
    tmp_path: Path, mutation: str
) -> None:
    _write_manifest(tmp_path, mode="linked")
    manifest_path = tmp_path / "manifest.json"
    if mutation == "legacy":
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["version"] = 1
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    elif mutation == "malformed":
        manifest_path.write_text("{", encoding="utf-8")
    elif mutation == "digest-drift":
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["modules"]["linked"]["sha256"] = "0" * 64
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    else:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["modules"]["linked"]["path"] = "../program.wasm"
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

    with pytest.raises(ValueError):
        _generate(tmp_path)


@pytest.mark.parametrize(
    "version",
    ["missing", None, 0, 1, 2, 4, 99, True, False, "3", 3.0, 3.5],
)
def test_snapshot_rejects_incompatible_execution_version_before_module_read(
    tmp_path: Path, version: object
) -> None:
    _write_manifest(tmp_path, mode="linked")
    path = tmp_path / "manifest.json"
    manifest = json.loads(path.read_text(encoding="utf-8"))
    if version == "missing":
        del manifest["version"]
    else:
        manifest["version"] = version
    path.write_text(json.dumps(manifest), encoding="utf-8")
    (tmp_path / "program.wasm").unlink()
    with pytest.raises(
        ValueError, match="Molt deployment manifest version must be 3"
    ):
        _snapshot_execution_identity(tmp_path / "output.wasm")


def test_explicit_empty_snapshot_capabilities_do_not_regain_defaults(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("SOURCE_DATE_EPOCH", "0")
    header = _generate(tmp_path, capabilities=[])
    assert header["capability_manifest"] == []
    assert header["determinism_stamp"] == "1970-01-01T00:00:00Z"


@pytest.mark.parametrize(
    "asset_path",
    [
        "/program.wasm",
        "dir/program.wasm",
        "dir\\program.wasm",
        "C:program.wasm",
        "program.wasm:stream",
    ],
)
def test_manifest_assets_are_portable_adjacent_names(
    tmp_path: Path, asset_path: str
) -> None:
    _write_manifest(tmp_path, mode="linked")
    path = tmp_path / "manifest.json"
    manifest = json.loads(path.read_text(encoding="utf-8"))
    manifest["modules"]["linked"]["path"] = asset_path
    path.write_text(json.dumps(manifest), encoding="utf-8")
    with pytest.raises(ValueError, match="adjacent file"):
        _generate(tmp_path)
