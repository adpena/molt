from __future__ import annotations

from contextlib import contextmanager
import json
import sys
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))

import translation_validate


def test_temp_root_defaults_to_repo_tmp(monkeypatch) -> None:
    for key in ("MOLT_DIFF_TMPDIR", "TMPDIR", "MOLT_EXT_ROOT"):
        monkeypatch.delenv(key, raising=False)

    assert (
        translation_validate._temp_root({}) == translation_validate._REPO_ROOT / "tmp"
    )


def test_temp_root_prefers_explicit_overrides(tmp_path: Path) -> None:
    diff_tmp = tmp_path / "diff-tmp"
    ambient_tmp = tmp_path / "ambient-tmp"
    ext_root = tmp_path / "ext-root"
    env = {
        "MOLT_DIFF_TMPDIR": str(diff_tmp),
        "TMPDIR": str(ambient_tmp),
        "MOLT_EXT_ROOT": str(ext_root),
    }

    assert translation_validate._temp_root(env) == diff_tmp

    env.pop("MOLT_DIFF_TMPDIR")
    assert translation_validate._temp_root(env) == ambient_tmp

    env.pop("TMPDIR")
    assert translation_validate._temp_root(env) == ext_root / "tmp"


def test_target_root_defaults_to_repo_target(monkeypatch) -> None:
    for key in ("CARGO_TARGET_DIR", "MOLT_EXT_ROOT"):
        monkeypatch.delenv(key, raising=False)

    assert translation_validate._cargo_target_root({}) == (
        translation_validate._REPO_ROOT / "target"
    )


def test_target_root_prefers_explicit_override(tmp_path: Path) -> None:
    ext_root = tmp_path / "ext-root"
    cargo_target_dir = tmp_path / "cargo-target"
    env = {
        "CARGO_TARGET_DIR": str(cargo_target_dir),
        "MOLT_EXT_ROOT": str(ext_root),
    }

    assert translation_validate._cargo_target_root(env) == cargo_target_dir

    env.pop("CARGO_TARGET_DIR")
    assert translation_validate._cargo_target_root(env) == ext_root / "target"


def test_main_wraps_run_in_conformance_memory_guard_scope(
    monkeypatch, tmp_path: Path, capsys
) -> None:
    src = tmp_path / "case.py"
    src.write_text("print('ok')\n", encoding="utf-8")
    calls: list[dict[str, object]] = []

    @contextmanager
    def fake_guarded_harness_scope(**kwargs):
        calls.append(kwargs)

        class Scope:
            limits = kwargs["limits"]
            memory_guard = {"enabled": True, "sentinel": "unit"}

        yield Scope()

    monkeypatch.setattr(
        translation_validate.harness_memory_guard,
        "guarded_harness_scope",
        fake_guarded_harness_scope,
    )
    monkeypatch.setattr(
        translation_validate,
        "validate_file",
        lambda *args, **kwargs: translation_validate.ValidationResult(
            source_path=str(src),
            cpython=translation_validate.RunResult("ok\n", "", 0, 1.0, "cpython"),
            molt_full=translation_validate.RunResult(
                "ok\n", "", 0, 1.0, "molt_full"
            ),
            molt_no_midend=translation_validate.RunResult(
                "ok\n", "", 0, 1.0, "molt_no_midend"
            ),
            match_full_vs_cpython=True,
            match_no_midend_vs_cpython=True,
            match_full_vs_no_midend=True,
        ),
    )

    rc = translation_validate.main(["--json", str(src)])

    assert rc == 0
    assert calls[0]["prefix"] == "MOLT_CONFORMANCE"
    assert calls[0]["repo_root"] == translation_validate._REPO_ROOT
    assert calls[0]["label"] == "translation_validate"
    payload = json.loads(capsys.readouterr().out)
    assert payload["memory_guard"] == {"enabled": True, "sentinel": "unit"}
