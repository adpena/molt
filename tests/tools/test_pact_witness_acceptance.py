from __future__ import annotations

from pathlib import Path

import tools.pact_witness_acceptance as acceptance


def test_pact_witness_acceptance_uses_run_scoped_attempt_dirs(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(acceptance, "ROOT", tmp_path)
    monkeypatch.setenv("MOLT_PROOF_QUEUE_RUN_ID", "run:id/with spaces")
    out_dir = tmp_path / "tmp" / "pact_witness_acceptance_queue"
    stale_build = out_dir / "build"
    stale_build.mkdir(parents=True)
    stale_file = stale_build / "output_linked.wat"
    stale_file.write_text("still held by a previous Windows process\n", encoding="utf-8")

    build_dir, run_dir = acceptance._prepare_attempt_dirs(out_dir)
    second_build_dir, second_run_dir = acceptance._prepare_attempt_dirs(out_dir)

    assert build_dir == out_dir / "runs" / "run_id_with_spaces" / "build"
    assert run_dir == out_dir / "runs" / "run_id_with_spaces" / "run"
    assert second_build_dir == out_dir / "runs" / "run_id_with_spaces-2" / "build"
    assert second_run_dir == out_dir / "runs" / "run_id_with_spaces-2" / "run"
    assert stale_file.read_text(encoding="utf-8").startswith("still held")
    assert (out_dir / "latest_attempt.txt").read_text(encoding="utf-8").strip() == str(
        second_build_dir.parent
    )


def test_pact_witness_acceptance_prefers_split_runtime_app_entry(
    tmp_path: Path,
) -> None:
    build_dir = tmp_path / "build"
    build_dir.mkdir()
    output_wasm = build_dir / "output.wasm"
    app_wasm = build_dir / "app.wasm"
    runtime_wasm = build_dir / "molt_runtime.wasm"
    output_wasm.write_bytes(b"monolithic-prelink")
    app_wasm.write_bytes(b"split-app")
    runtime_wasm.write_bytes(b"split-runtime")

    selected = acceptance._select_wasm_entry(build_dir)
    env = acceptance._wasm_run_env(selected)

    assert selected == app_wasm
    assert env["MOLT_WASM_DIRECT_LINK"] == "1"
    assert env["MOLT_WASM_PREFER_LINKED"] == "0"
    assert env["MOLT_RUNTIME_WASM"] == str(runtime_wasm)


def test_pact_witness_acceptance_uses_output_wasm_without_split_runtime(
    tmp_path: Path,
) -> None:
    build_dir = tmp_path / "build"
    build_dir.mkdir()
    output_wasm = build_dir / "output.wasm"
    output_wasm.write_bytes(b"monolithic")

    selected = acceptance._select_wasm_entry(build_dir)
    env = acceptance._wasm_run_env(selected)

    assert selected == output_wasm
    assert "MOLT_WASM_DIRECT_LINK" not in env
    assert "MOLT_RUNTIME_WASM" not in env
