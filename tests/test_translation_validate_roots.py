from __future__ import annotations

import sys
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))

import translation_validate


def test_temp_root_defaults_to_repo_tmp(monkeypatch) -> None:
    for key in ("MOLT_DIFF_TMPDIR", "TMPDIR", "MOLT_EXT_ROOT"):
        monkeypatch.delenv(key, raising=False)

    assert translation_validate._temp_root({}) == translation_validate._REPO_ROOT / "tmp"


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
