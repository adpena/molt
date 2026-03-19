"""Capability-gated runpy support for Molt."""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _intrinsics_require

_molt_runpy_run_module = _intrinsics_require("molt_runpy_run_module")
_molt_runpy_resolve_path = _intrinsics_require("molt_runpy_resolve_path")
_molt_runpy_run_path = _intrinsics_require("molt_runpy_run_path")
_molt_capabilities_trusted = _intrinsics_require("molt_capabilities_trusted")
_molt_capabilities_require = _intrinsics_require("molt_capabilities_require")

__all__ = ["run_module", "run_path"]


def _require_intrinsic(fn: Any, name: str) -> Any:
    if not callable(fn):
        raise RuntimeError(f"missing intrinsic: {name}")
    return fn


def _require_fs_read(
    _capabilities_trusted_intrinsic=_molt_capabilities_trusted,
    _capabilities_require_intrinsic=_molt_capabilities_require,
) -> None:
    trusted = _require_intrinsic(
        _capabilities_trusted_intrinsic, "molt_capabilities_trusted"
    )
    if trusted():
        return
    require = _require_intrinsic(
        _capabilities_require_intrinsic, "molt_capabilities_require"
    )
    require("fs.read")


def _fspath(path_name: Any) -> Any:
    if isinstance(path_name, (str, bytes)):
        return path_name
    fspath = getattr(path_name, "__fspath__", None)
    if fspath is None:
        raise TypeError("path_name must be a path-like object")
    return fspath()


def _runpy_module_file() -> str | None:
    module_file = globals().get("__file__")
    if isinstance(module_file, str):
        return module_file
    return None


def _resolve_run_path(
    path: str, _resolve_path_intrinsic=_molt_runpy_resolve_path
) -> str:
    resolver = _require_intrinsic(_resolve_path_intrinsic, "molt_runpy_resolve_path")
    payload = resolver(path, _runpy_module_file())
    if not isinstance(payload, dict):
        raise RuntimeError("invalid runpy path payload: dict expected")
    abs_path = payload.get("abspath")
    is_file = payload.get("is_file")
    if not isinstance(abs_path, str):
        raise RuntimeError("invalid runpy path payload: abspath")
    if not isinstance(is_file, bool):
        raise RuntimeError("invalid runpy path payload: is_file")
    if not is_file:
        raise FileNotFoundError(abs_path)
    return abs_path


def run_module(
    mod_name: str,
    init_globals: dict[str, Any] | None = None,
    run_name: str | None = None,
    alter_sys: bool = False,
    _run_module_intrinsic=_molt_runpy_run_module,
) -> dict[str, Any]:
    if not isinstance(mod_name, str):
        raise TypeError("mod_name must be a string")
    if init_globals is not None and not isinstance(init_globals, dict):
        raise TypeError("init_globals must be a dict or None")
    if run_name is not None and not isinstance(run_name, str):
        raise TypeError("run_name must be a string or None")
    runner = _require_intrinsic(_run_module_intrinsic, "molt_runpy_run_module")
    return runner(mod_name, run_name, init_globals, alter_sys)


def run_path(
    path_name: Any,
    init_globals: dict[str, Any] | None = None,
    run_name: str | None = None,
    _run_path_intrinsic=_molt_runpy_run_path,
) -> dict[str, Any]:
    if init_globals is not None and not isinstance(init_globals, dict):
        raise TypeError("init_globals must be a dict or None")
    try:
        path = _fspath(path_name)
    except TypeError as exc:
        raise TypeError("path_name must be a path-like object") from exc
    if not isinstance(path, str):
        raise TypeError("path_name must resolve to str")
    if run_name is not None and not isinstance(run_name, str):
        raise TypeError("run_name must be a string or None")
    _require_fs_read()
    abs_path = _resolve_run_path(path)
    runner = _require_intrinsic(_run_path_intrinsic, "molt_runpy_run_path")
    return runner(abs_path, run_name, init_globals)


for _name in (
    "_molt_runpy_run_module",
    "_molt_runpy_resolve_path",
    "_molt_runpy_run_path",
    "_molt_capabilities_trusted",
    "_molt_capabilities_require",
):
    globals().pop(_name, None)
