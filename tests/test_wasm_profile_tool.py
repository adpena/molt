from __future__ import annotations

import json
import sys
from pathlib import Path
from types import SimpleNamespace

import tools.wasm_profile as wasm_profile


def test_wasm_profile_main_uses_current_bench_wasm_api(
    monkeypatch,
    tmp_path: Path,
) -> None:
    build_calls: list[tuple[bool, Path]] = []

    def _fake_build_runtime_wasm(
        *,
        reloc: bool,
        output: Path,
    ) -> bool:
        build_calls.append((reloc, output))
        return True

    cleanup_state = {"called": False}

    class _TempDir:
        def cleanup(self) -> None:
            cleanup_state["called"] = True

    prepared_kwargs: dict[str, object] = {}

    def _fake_prepare_wasm_binary(
        script: str,
        *,
        require_linked: bool = False,
        tty: bool = False,
        log: object | None = None,
        keep_temp: bool = False,
    ):
        prepared_kwargs["script"] = script
        prepared_kwargs["require_linked"] = require_linked
        prepared_kwargs["tty"] = tty
        prepared_kwargs["log"] = log
        prepared_kwargs["keep_temp"] = keep_temp
        return SimpleNamespace(
            run_env={"MOLT_WASM_PATH": "/tmp/output.wasm"},
            linked_used=True,
            size_kb=42.0,
            build_s=0.25,
            temp_dir=_TempDir(),
        )

    profile_calls: list[tuple[dict[str, str], Path, str, int | None]] = []

    def _fake_run_node_profile(
        *, env: dict[str, str], out_dir: Path, name: str, interval_us: int | None
    ) -> bool:
        profile_calls.append((dict(env), out_dir, name, interval_us))
        return True

    monkeypatch.setattr(
        wasm_profile.bench_wasm, "build_runtime_wasm", _fake_build_runtime_wasm
    )
    monkeypatch.setattr(
        wasm_profile.bench_wasm, "prepare_wasm_binary", _fake_prepare_wasm_binary
    )
    monkeypatch.setattr(wasm_profile.bench_wasm, "_git_rev", lambda: "deadbeef")
    monkeypatch.setattr(wasm_profile, "_run_node_profile", _fake_run_node_profile)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "wasm_profile.py",
            "--bench",
            "bench_sum",
            "--linked",
            "--runs",
            "1",
            "--out-dir",
            str(tmp_path),
        ],
    )

    wasm_profile.main()

    assert build_calls == [
        (False, wasm_profile.bench_wasm.RUNTIME_WASM),
        (True, wasm_profile.bench_wasm.RUNTIME_WASM_RELOC),
    ]
    assert prepared_kwargs["script"] == "tests/benchmarks/bench_sum.py"
    assert prepared_kwargs["require_linked"] is False
    assert prepared_kwargs["tty"] is False
    assert prepared_kwargs["log"] is None
    assert prepared_kwargs["keep_temp"] is False
    assert cleanup_state["called"] is True
    assert len(profile_calls) == 1
    assert profile_calls[0][1] == tmp_path

    manifest = json.loads((tmp_path / "profile_manifest.json").read_text())
    assert manifest["bench"] == "tests/benchmarks/bench_sum.py"
    assert manifest["linked_requested"] is True
    assert manifest["linked_used"] is True
