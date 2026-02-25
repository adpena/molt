from __future__ import annotations

import builtins
import importlib.util
import os
import sys
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]
TEMPFILE_MODULE = REPO_ROOT / "src" / "molt" / "stdlib" / "tempfile.py"


def _load_tempfile_module(name: str):
    for key in list(sys.modules):
        if key == name or key.startswith(f"{name}."):
            sys.modules.pop(key, None)

    registry = getattr(builtins, "_molt_intrinsics", None)
    if not isinstance(registry, dict):
        registry = {}
        setattr(builtins, "_molt_intrinsics", registry)
    registry["molt_path_join"] = os.path.join

    spec = importlib.util.spec_from_file_location(name, TEMPFILE_MODULE)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def test_candidate_tempdir_list_falls_back_when_env_read_denied(
    monkeypatch: pytest.MonkeyPatch, tmp_path
) -> None:
    molt_tempfile = _load_tempfile_module("molt_tempfile_env_fallback")

    def _deny_env(_key: str):
        raise PermissionError("env.read denied")

    monkeypatch.setattr(molt_tempfile._os, "getenv", _deny_env)
    monkeypatch.setattr(molt_tempfile._os, "getcwd", lambda: str(tmp_path))

    candidates = molt_tempfile._candidate_tempdir_list()
    assert str(tmp_path) in candidates
    if molt_tempfile._os.name == "nt":
        assert r"c:\temp" in [path.lower() for path in candidates]
    else:
        assert "/tmp" in candidates


def test_pick_tempdir_prefers_first_usable_candidate(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    molt_tempfile = _load_tempfile_module("molt_tempfile_pick_dir")

    monkeypatch.setattr(
        molt_tempfile, "_candidate_tempdir_list", lambda: ["/missing", "/usable"]
    )
    monkeypatch.setattr(molt_tempfile, "_dir_is_usable", lambda path: path == "/usable")

    assert molt_tempfile._pick_tempdir() == "/usable"


def test_pick_tempdir_raises_when_no_candidate_works(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    molt_tempfile = _load_tempfile_module("molt_tempfile_pick_dir_error")

    monkeypatch.setattr(molt_tempfile, "_candidate_tempdir_list", lambda: ["/missing"])
    monkeypatch.setattr(molt_tempfile, "_dir_is_usable", lambda _path: False)

    with pytest.raises(FileNotFoundError, match="No usable temporary directory found"):
        molt_tempfile._pick_tempdir()


def test_dir_is_usable_accepts_existing_writable_directory(tmp_path) -> None:
    molt_tempfile = _load_tempfile_module("molt_tempfile_dir_usable")
    assert molt_tempfile._dir_is_usable(str(tmp_path))
