"""Capability-gated runpy support for Molt."""

from __future__ import annotations

from typing import Any
from os import fsdecode as _os_fsdecode
from os import fspath as _os_fspath

from _intrinsics import require_intrinsic as _intrinsics_require

_molt_runpy_run_module = _intrinsics_require("molt_runpy_run_module")
_molt_runpy_run_path = _intrinsics_require("molt_runpy_run_path")

__all__ = ["run_module", "run_path"]


def _require_intrinsic(fn: Any, name: str) -> Any:
    if not callable(fn):
        raise RuntimeError(f"missing intrinsic: {name}")
    return fn


def _fspath(
    path_name: Any,
    _fspath_fn=_os_fspath,
    _fsdecode_fn=_os_fsdecode,
) -> str:
    return _fsdecode_fn(_fspath_fn(path_name))


def _bind_run_module(runner_intrinsic):
    runner = _require_intrinsic(runner_intrinsic, "molt_runpy_run_module")

    def run_module(
        mod_name,
        init_globals=None,
        run_name=None,
        alter_sys=False,
    ):
        if not isinstance(mod_name, str):
            raise TypeError("mod_name must be a string")
        if init_globals is not None and not isinstance(init_globals, dict):
            raise TypeError("init_globals must be a dict or None")
        if run_name is not None and not isinstance(run_name, str):
            raise TypeError("run_name must be a string or None")
        return runner(mod_name, run_name, init_globals, alter_sys)

    run_module.__qualname__ = "run_module"
    return run_module


def _bind_run_path(runner_intrinsic):
    runner = _require_intrinsic(runner_intrinsic, "molt_runpy_run_path")

    def run_path(
        path_name,
        init_globals=None,
        run_name=None,
    ):
        if init_globals is not None and not isinstance(init_globals, dict):
            raise TypeError("init_globals must be a dict or None")
        path = _fspath(path_name)
        if run_name is not None and not isinstance(run_name, str):
            raise TypeError("run_name must be a string or None")
        return runner(path, run_name, init_globals)

    run_path.__qualname__ = "run_path"
    return run_path


run_module = _bind_run_module(_molt_runpy_run_module)
run_path = _bind_run_path(_molt_runpy_run_path)


for _name in (
    "_molt_runpy_run_module",
    "_molt_runpy_run_path",
    "_os_fspath",
    "_os_fsdecode",
    "_bind_run_module",
    "_bind_run_path",
):
    globals().pop(_name, None)
