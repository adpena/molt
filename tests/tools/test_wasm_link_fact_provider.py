"""Rejected WASM evidence survives scratch and private deployment cleanup."""

from __future__ import annotations

import hashlib
import json
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt.cli.wasm_deployment import WasmDeploymentGeneration, WasmDeploymentPlan
from tools import wasm_link_fact_provider as provider


def _rejected_run(argv, **_kwargs):
    payload = {
        "schema_version": provider.WASM_LINK_FACTS_SCHEMA_VERSION,
        "ok": False,
        "error": "section out of order (at offset 0x8459)",
    }
    return subprocess.CompletedProcess(argv, 1, json.dumps(payload), "")


def test_rejected_scan_input_outlives_scratch_and_private_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    scanner = tmp_path / "molt-backend.exe"
    scanner.write_bytes(b"scanner")
    final_root = tmp_path / "build"
    evidence_root = final_root / "wasm-link-evidence"
    module = b"\0asm\x01\0\0\0"
    plan = WasmDeploymentPlan(
        {"linked": final_root / "app.wasm"}, final_root, False, ()
    )
    monkeypatch.setattr(provider, "_COMMANDS", SimpleNamespace(run=_rejected_run))
    with WasmDeploymentGeneration.prepare(plan) as generation:
        scratch = generation.root / "scan"
        provide = provider.make_rust_wasm_facts_provider(
            scanner, scratch, evidence_root=evidence_root
        )
        with pytest.raises(ValueError, match="rejected input kept at") as excinfo:
            provide(module)
        kept = Path(str(excinfo.value).rsplit("rejected input kept at ", 1)[1])
        assert kept.parent == evidence_root
        assert (
            kept.name
            == f"facts-scan-{hashlib.sha256(module).hexdigest()}.wasm.rejected"
        )
        assert kept.read_bytes() == module
        assert not list(scratch.glob("wasm-facts-*.wasm"))
        # The captured executable stays in scratch until its invocation ends.
        assert provide.scanner_identity.path.read_bytes() == b"scanner"
    assert not generation.root.exists()
    assert kept.read_bytes() == module
    assert not plan.outputs["linked"].exists()


@pytest.mark.parametrize(
    "stage", ["facts-scan", "linked-validation", "split-native-link"]
)
def test_rejected_publication_is_idempotent_and_refuses_collision(
    tmp_path: Path, stage: str
) -> None:
    data = b"\0asm\x01\0\0\0rejected"
    destination = provider.preserve_rejected_wasm(data, tmp_path, stage=stage)
    assert provider.preserve_rejected_wasm(data, tmp_path, stage=stage) == destination
    destination.write_bytes(b"different evidence")
    with pytest.raises(ValueError, match="evidence content changed"):
        provider.preserve_rejected_wasm(data, tmp_path, stage=stage)
    assert destination.read_bytes() == b"different evidence"


def test_rejected_scan_reports_original_rejection_and_evidence_io_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    scanner = tmp_path / "scanner"
    scanner.write_bytes(b"scanner")
    evidence_root = tmp_path / "not-a-directory"
    evidence_root.write_bytes(b"preserve me")
    monkeypatch.setattr(provider, "_COMMANDS", SimpleNamespace(run=_rejected_run))
    provide = provider.make_rust_wasm_facts_provider(
        scanner, tmp_path / "scratch", evidence_root=evidence_root
    )
    with pytest.raises(
        ValueError, match="section out of order.*failed to preserve rejected input"
    ):
        provide(b"\0asm\x01\0\0\0")
    assert evidence_root.read_bytes() == b"preserve me"
