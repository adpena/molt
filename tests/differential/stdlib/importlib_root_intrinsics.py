"""Purpose: validate importlib root intrinsic lowering for resolve/absence checks."""

import importlib
import sys


def _print_error(label: str, fn) -> None:
    try:
        fn()
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__, str(exc))
    else:
        print(label, "ok")


_print_error("relative_no_package", lambda: importlib.import_module(".demo"))
_print_error(
    "relative_beyond_top",
    lambda: importlib.import_module("..demo", "pkg"),
)

if sys.platform != "win32":
    _print_error(
        "spawn_win32_nonwin",
        lambda: importlib.import_module("multiprocessing.popen_spawn_win32"),
    )
