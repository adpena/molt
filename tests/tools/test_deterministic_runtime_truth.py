from __future__ import annotations

from pathlib import Path

from tools import check_deterministic_runtime as determinism


def test_determinism_requires_two_observations(tmp_path: Path) -> None:
    source = tmp_path / "source.py"
    source.write_text("print('ok')\n", encoding="utf-8")

    result = determinism.check_determinism(str(source), 1, "dev")

    assert result["status"] == "error"
    assert result["error"] == "runs must be at least 2"


def test_stderr_is_part_of_deterministic_observable(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "source.py"
    binary = tmp_path / "program"
    source.write_text("print('ok')\n", encoding="utf-8")
    binary.write_bytes(b"binary")
    observations = iter([(b"same", b"first", 0), (b"same", b"second", 0)])
    monkeypatch.setattr(
        determinism,
        "build_program",
        lambda *_args, **_kwargs: (str(binary), "", {"status": "ok"}),
    )
    monkeypatch.setattr(
        determinism, "run_binary", lambda *_args, **_kwargs: next(observations)
    )

    result = determinism.check_determinism(str(source), 2, "dev")

    assert result["status"] == "fail"
    assert result["deterministic"] is False
    assert result["diffs"][0]["stderr_changed"] is True
