"""Content-addressed reuse of final WASM link results."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

from molt.cli import wasm_final_link_cache as cache


def _link_cmd(
    tmp_path: Path, *, out_dir: Path, app: Path, timings: str, split_dir: str
) -> list[str]:
    return [
        sys.executable,
        str(tmp_path / "wasm_link.py"),
        "--input",
        str(app),
        "--output",
        str(out_dir / "hello_linked.wasm"),
        "--split-output-dir",
        split_dir,
        "--phase-timings-file",
        timings,
        "--optimize-level",
        "O1",
    ]


def _inputs(tmp_path: Path) -> Path:
    app = tmp_path / "app.wasm"
    app.write_bytes(b"\0asm\x01\0\0\0app-v1")
    return app


def test_key_ignores_private_locations_and_follows_input_content(
    tmp_path: Path,
) -> None:
    app = _inputs(tmp_path)
    facts = ({"role": "wasm-link-source-closure", "content_digest": "a" * 64},)

    def key(out: str, timings: str, split: str) -> str:
        return cache.final_link_cache_key(
            _link_cmd(
                tmp_path,
                out_dir=tmp_path / out,
                app=app,
                timings=timings,
                split_dir=split,
            ),
            cwd=tmp_path,
            tool_facts=facts,
        )

    first = key("ci-row-a", "/tmp/.molt-link-timings-1.tmp", "/tmp/gen-1")
    # A fresh output directory, generation directory and timings file: one result.
    assert key("ci-row-b", "/tmp/.molt-link-timings-2.tmp", "/tmp/gen-2") == first
    # Changed input bytes at the same path: a different result.
    app.write_bytes(b"\0asm\x01\0\0\0app-v2")
    assert key("ci-row-a", "/tmp/.molt-link-timings-1.tmp", "/tmp/gen-1") != first


def test_key_keeps_the_embedded_module_name_and_hashes_relative_inputs(
    tmp_path: Path,
) -> None:
    app = _inputs(tmp_path)
    facts = ({"role": "wasm-link-source-closure", "content_digest": "a" * 64},)
    base = _link_cmd(tmp_path, out_dir=tmp_path, app=app, timings="t", split_dir="s")
    renamed = list(base)
    renamed[renamed.index("--output") + 1] = str(tmp_path / "other_linked.wasm")
    relative = list(base)
    relative[relative.index("--input") + 1] = app.name
    key = cache.final_link_cache_key(base, cwd=tmp_path, tool_facts=facts)
    # wasm-ld embeds the output file name in the module.
    assert cache.final_link_cache_key(renamed, cwd=tmp_path, tool_facts=facts) != key
    # The tool resolves a relative input against its cwd; it is keyed by content.
    assert cache.final_link_cache_key(relative, cwd=tmp_path, tool_facts=facts) == key
    other_facts = ({"role": "wasm-link-source-closure", "content_digest": "b" * 64},)
    assert cache.final_link_cache_key(base, cwd=tmp_path, tool_facts=other_facts) != key


def test_result_roundtrips_and_a_tampered_entry_is_a_miss(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    entry = cache.final_link_cache_entry("c" * 64)
    produced = {
        "linked": tmp_path / "gen-a" / "hello_linked.wasm",
        "optimizer": tmp_path / "gen-a" / "hello_linked.wasm.wasm-opt.json",
    }
    for role, path in produced.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(f"{role} bytes".encode())
    fresh = {role: tmp_path / "gen-b" / path.name for role, path in produced.items()}

    assert not cache.restore_final_link_result(entry, fresh)
    cache.publish_final_link_result(entry, produced)
    assert cache.restore_final_link_result(entry, fresh)
    assert {role: path.read_bytes() for role, path in fresh.items()} == {
        role: path.read_bytes() for role, path in produced.items()
    }

    # A request for a different output family never matches this entry.
    assert not cache.restore_final_link_result(entry, {"linked": fresh["linked"]})

    cache.publish_final_link_result(entry, produced)
    (entry.root / "roles" / "linked").write_bytes(b"tampered")
    again = {role: tmp_path / "gen-c" / path.name for role, path in produced.items()}
    assert not cache.restore_final_link_result(entry, again)
    assert not any(path.exists() for path in again.values())
    assert not entry.metadata.exists()
