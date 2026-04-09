from __future__ import annotations

from pathlib import Path

from molt.frontend import tv_hooks


def test_temp_root_defaults_to_repo_tmp(monkeypatch) -> None:
    for key in ("MOLT_DIFF_TMPDIR", "TMPDIR", "MOLT_EXT_ROOT", "MOLT_TV_DIR"):
        monkeypatch.delenv(key, raising=False)
    tv_hooks.reset()

    assert tv_hooks._temp_root({}) == Path(__file__).resolve().parents[1] / "tmp"


def test_temp_root_prefers_explicit_overrides(tmp_path: Path) -> None:
    diff_tmp = tmp_path / "diff-tmp"
    ambient_tmp = tmp_path / "ambient-tmp"
    ext_root = tmp_path / "ext-root"
    env = {
        "MOLT_DIFF_TMPDIR": str(diff_tmp),
        "TMPDIR": str(ambient_tmp),
        "MOLT_EXT_ROOT": str(ext_root),
    }

    assert tv_hooks._temp_root(env) == diff_tmp

    env.pop("MOLT_DIFF_TMPDIR")
    assert tv_hooks._temp_root(env) == ambient_tmp

    env.pop("TMPDIR")
    assert tv_hooks._temp_root(env) == ext_root / "tmp"


def test_tv_dump_dir_explicit_override_wins(tmp_path: Path, monkeypatch) -> None:
    explicit_dump = tmp_path / "dump"
    monkeypatch.setenv("MOLT_TV_DIR", str(explicit_dump))
    tv_hooks.reset()

    assert tv_hooks.tv_dump_dir() == explicit_dump
