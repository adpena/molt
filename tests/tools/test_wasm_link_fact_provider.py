"""tools/wasm_link_fact_provider.py: a rejected scan keeps its input as evidence."""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

import pytest

from tools import wasm_link_fact_provider as provider


def _api(fake_run):
    class Commands:
        @staticmethod
        def run(argv, **kwargs):
            return fake_run(argv, **kwargs)

    return {
        "_COMMANDS": Commands,
        "_file_stat_identity": lambda stat: (
            stat.st_size,
            int(stat.st_mtime_ns),
            stat.st_ino,
            stat.st_dev,
        ),
        "_stable_file_sha256_cached": lambda path, identity: "a" * 64,
    }


def test_rejected_scan_input_is_kept_under_the_evidence_root(tmp_path: Path) -> None:
    scanner = tmp_path / "molt-backend.exe"
    scanner.write_bytes(b"scanner")
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    evidence_root = tmp_path / "build" / "wasm-link-evidence"
    module = b"\0asm\x01\0\0\0"

    def fake_run(argv, **kwargs):
        payload = {
            "schema_version": provider._WASM_LINK_FACTS_SCHEMA_VERSION,
            "ok": False,
            "error": "section out of order (at offset 0x8459)",
        }
        return subprocess.CompletedProcess(argv, 0, json.dumps(payload), "")

    provide = provider.make_rust_wasm_facts_provider(
        _api(fake_run), scanner, scratch, evidence_root=evidence_root
    )
    with pytest.raises(ValueError, match="rejected input kept at") as excinfo:
        provide(module)
    kept = Path(str(excinfo.value).rsplit("rejected input kept at ", 1)[1])
    assert kept.parent == evidence_root
    assert kept.name.endswith(".wasm.rejected")
    assert kept.read_bytes() == module
    # The scratch copy is gone: nothing but the evidence survives the scan.
    assert not any(scratch.iterdir())
    assert os.path.isdir(evidence_root)
