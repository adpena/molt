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
