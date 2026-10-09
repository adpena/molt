"""tools/wasm_link_fact_provider.py: a rejected scan keeps its input as evidence."""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

import pytest

from tools import wasm_link_fact_provider as provider


class _Commands:
    """The provider's scanner runner, answering with one fixed reply."""

    def __init__(self, fake_run):
        self.run = fake_run


def test_rejected_scan_input_is_kept_under_the_evidence_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    scanner = tmp_path / "molt-backend.exe"
    scanner.write_bytes(b"scanner")
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    evidence_root = tmp_path / "build" / "wasm-link-evidence"
    module = b"\0asm\x01\0\0\0"

    def fake_run(argv, **kwargs):
        payload = {
            "schema_version": provider.WASM_LINK_FACTS_SCHEMA_VERSION,
            "ok": False,
            "error": "section out of order (at offset 0x8459)",
        }
        return subprocess.CompletedProcess(argv, 0, json.dumps(payload), "")

    monkeypatch.setattr(provider, "_COMMANDS", _Commands(fake_run))
    provide = provider.make_rust_wasm_facts_provider(
        scanner, scratch, evidence_root=evidence_root
    )
    # The provider keeps its private scanner snapshot in scratch for its life.
    before_scan = set(scratch.rglob("*"))
    with pytest.raises(ValueError, match="rejected input kept at") as excinfo:
        provide(module)
    kept = Path(str(excinfo.value).rsplit("rejected input kept at ", 1)[1])
    assert kept.parent == evidence_root
    assert kept.name.endswith(".wasm.rejected")
    assert kept.read_bytes() == module
    # The scan's scratch copy is gone: only the evidence survives the scan.
    assert set(scratch.rglob("*")) == before_scan
    assert os.path.isdir(evidence_root)
