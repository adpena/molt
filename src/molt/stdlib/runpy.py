"""Capability-gated runpy support for Molt."""

from __future__ import annotations

from typing import Any
import os as _os

from _intrinsics import require_intrinsic as _intrinsics_require

from molt import capabilities as _capabilities

_molt_runpy_run_module = _intrinsics_require("molt_runpy_run_module", globals())

__all__ = ["run_module", "run_path"]


def _require_intrinsic(fn: Any, name: str) -> Any:
    if not callable(fn):
        raise RuntimeError(f"missing intrinsic: {name}")
    return fn


def _require_fs_read() -> None:
    if not _capabilities.trusted():
        _capabilities.require("fs.read")


def run_module(
    mod_name: str,
    init_globals: dict[str, Any] | None = None,
    run_name: str | None = None,
    alter_sys: bool = False,
) -> dict[str, Any]:
    if not isinstance(mod_name, str):
        raise TypeError("mod_name must be a string")
    if init_globals is not None and not isinstance(init_globals, dict):
        raise TypeError("init_globals must be a dict or None")
    if run_name is not None and not isinstance(run_name, str):
        raise TypeError("run_name must be a string or None")
    if alter_sys:
        # TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial): implement full runpy alter_sys semantics (argv/sys.modules updates + package execution parity).
        raise NotImplementedError("run_module(alter_sys=True) is not supported")
    runner = _require_intrinsic(_molt_runpy_run_module, "molt_runpy_run_module")
    return runner(mod_name, run_name, init_globals)


def run_path(
    path_name: Any,
    init_globals: dict[str, Any] | None = None,
    run_name: str | None = None,
) -> dict[str, Any]:
    if init_globals is not None and not isinstance(init_globals, dict):
        raise TypeError("init_globals must be a dict or None")
    try:
        path = _os.fspath(path_name)
    except TypeError as exc:
        raise TypeError("path_name must be a path-like object") from exc
    if not isinstance(path, str):
        raise TypeError("path_name must resolve to str")
    if run_name is not None and not isinstance(run_name, str):
        raise TypeError("run_name must be a string or None")
    _require_fs_read()
    abs_path = _os.path.abspath(path)
    if not _os.path.isfile(abs_path):
        raise FileNotFoundError(abs_path)
    # TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P1, status:missing): add Rust intrinsic-backed run_path execution once runtime code-object execution is available (no Python-source fallback).
    raise NotImplementedError("run_path() is not supported yet")
