from __future__ import annotations

import importlib.util
import os
import sys
import sysconfig
from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class RuntimeFeatures:
    cpu_count: int
    gil_enabled: bool | None
    free_threaded_build: bool
    subinterpreters_available: bool
    interpreter_pool_available: bool

    def to_log_fields(self) -> dict[str, object]:
        return {
            "cpu_count": self.cpu_count,
            "gil_enabled": self.gil_enabled,
            "free_threaded_build": self.free_threaded_build,
            "subinterpreters_available": self.subinterpreters_available,
            "interpreter_pool_available": self.interpreter_pool_available,
        }


def detect_runtime_features() -> RuntimeFeatures:
    cpu_count = max(os.cpu_count() or 1, 1)
    free_threaded_build = bool(sysconfig.get_config_var("Py_GIL_DISABLED"))

    gil_enabled: bool | None = None
    is_gil_enabled = getattr(sys, "_is_gil_enabled", None)
    if callable(is_gil_enabled):
        try:
            gil_enabled = bool(is_gil_enabled())
        except Exception:
            gil_enabled = None

    subinterpreters_available = (
        importlib.util.find_spec("concurrent.interpreters") is not None
    )

    interpreter_pool_available = False
    try:
        from concurrent import futures

        interpreter_pool_available = hasattr(futures, "InterpreterPoolExecutor")
    except Exception:
        interpreter_pool_available = False

    return RuntimeFeatures(
        cpu_count=cpu_count,
        gil_enabled=gil_enabled,
        free_threaded_build=free_threaded_build,
        subinterpreters_available=subinterpreters_available,
        interpreter_pool_available=interpreter_pool_available,
    )
