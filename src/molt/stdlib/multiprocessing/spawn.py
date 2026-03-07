"""Spawn entrypoint for Molt multiprocessing."""

from __future__ import annotations

import os as _os
import runpy as _runpy
import sys as _sys
import types as _types

import multiprocessing as _multiprocessing

from _intrinsics import require_intrinsic as _require_intrinsic
from multiprocessing._api_surface import apply_module_api_surface as _apply_api_surface


def _debug_spawn(message: str) -> None:
    trace = getattr(_multiprocessing, "_spawn_trace", None)
    if callable(trace):
        try:
            trace(message)
        except Exception:
            pass


_MOLT_ENV_GET = _require_intrinsic("molt_env_get", globals())


def _get_env(key: str, default: str = "") -> str:
    return str(_MOLT_ENV_GET(key, default))


_SPAWN_EXECUTABLE = _get_env("MOLT_SYS_EXECUTABLE", _sys.executable)

WINEXE = _os.name == "nt" and bool(getattr(_sys, "frozen", False))
WINSERVICE = _os.name == "nt" and bool(getattr(_sys, "frozen", False))
old_main_modules: list[object] = []
os = _os
runpy = _runpy
sys = _sys
types = _types
process = _types
reduction = _types
util = _types


def is_forking(argv: list[str] | None = None) -> bool:
    if argv is None:
        argv = list(_sys.argv)
    return "--multiprocessing-fork" in argv


def freeze_support() -> None:
    if is_forking():
        spawn_main()


def get_command_line(**_kwargs):
    return [get_executable(), "-m", "multiprocessing.spawn"]


def get_executable() -> str:
    return _SPAWN_EXECUTABLE


def set_executable(path: str) -> None:
    global _SPAWN_EXECUTABLE
    _SPAWN_EXECUTABLE = str(path)


def get_preparation_data(name: str):
    return {
        "name": name,
        "sys_path": list(_sys.path),
        "sys_argv": list(_sys.argv),
    }


def import_main_path(main_path: str) -> None:
    _runpy.run_path(main_path, run_name="__mp_main__")


def prepare(data) -> dict[str, object]:
    if isinstance(data, dict):
        return dict(data)
    return {}


def spawn_main(*_args, **_kwargs):
    target = getattr(_multiprocessing, "_spawn_main", None)
    if not callable(target):
        raise RuntimeError("multiprocessing._spawn_main is unavailable")
    return target()


get_start_method = _multiprocessing.get_start_method
set_start_method = _multiprocessing.set_start_method

_spawn_flag = _get_env("MOLT_MP_SPAWN")
_entry_override = _get_env("MOLT_ENTRY_MODULE")

if _spawn_flag == "1" or _entry_override == "multiprocessing.spawn":
    _debug_spawn(
        f"spawn.py _spawn_main type={type(getattr(_multiprocessing, '_spawn_main', None)).__name__}"
    )
    spawn_main()

import sys as _mp_spawn_sys
_apply_api_surface(
    "multiprocessing.spawn",
    _mp_spawn_sys.modules[__name__].__dict__,
    providers={
        "WINEXE": WINEXE,
        "WINSERVICE": WINSERVICE,
        "freeze_support": freeze_support,
        "get_command_line": get_command_line,
        "get_executable": get_executable,
        "get_preparation_data": get_preparation_data,
        "get_start_method": get_start_method,
        "import_main_path": import_main_path,
        "is_forking": is_forking,
        "old_main_modules": old_main_modules,
        "os": os,
        "prepare": prepare,
        "process": process,
        "reduction": reduction,
        "runpy": runpy,
        "set_executable": set_executable,
        "set_start_method": set_start_method,
        "spawn_main": spawn_main,
        "sys": sys,
        "types": types,
        "util": util,
    },
    prune=True,
)
