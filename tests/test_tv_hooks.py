from __future__ import annotations

from pathlib import Path

from molt.frontend import tv_hooks


def test_temp_root_defaults_to_scratch_outside_the_checkout(monkeypatch) -> None:
    for key in ("MOLT_DIFF_TMPDIR", "TMPDIR", "MOLT_EXT_ROOT", "MOLT_TV_DIR"):
        monkeypatch.delenv(key, raising=False)
    tv_hooks.reset()

    checkout = Path(__file__).resolve().parents[1]
    assert not tv_hooks._temp_root({}).is_relative_to(checkout)


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
    assert tv_hooks._temp_root(env) == ext_root.resolve() / "tmp"


def test_tv_dump_dir_explicit_override_wins(tmp_path: Path, monkeypatch) -> None:
    explicit_dump = tmp_path / "dump"
    monkeypatch.setenv("MOLT_TV_DIR", str(explicit_dump))
    tv_hooks.reset()

    assert tv_hooks.tv_dump_dir() == explicit_dump
