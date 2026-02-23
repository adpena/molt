"""Purpose: differential coverage for custom sys.unraisablehook parity in atexit."""

import atexit
import contextlib
import io
import sys


class Boom:
    def __call__(self) -> None:
        raise RuntimeError("boom")

    def __repr__(self) -> str:
        return "<boom-callback>"


capture: dict[str, object] = {}


def hook(args: object) -> None:
    capture["type"] = type(args).__name__
    capture["msg"] = getattr(args, "err_msg", None)
    capture["obj"] = getattr(args, "object", None)
    exc_type = getattr(args, "exc_type", None)
    capture["exc_type"] = getattr(exc_type, "__name__", None)
    exc_value = getattr(args, "exc_value", None)
    capture["exc_is_runtime"] = isinstance(exc_value, RuntimeError)
    capture["has_tb"] = getattr(args, "exc_traceback", None) is not None


had_hook = hasattr(sys, "unraisablehook")
old_hook = getattr(sys, "unraisablehook", None)

atexit._clear()
atexit.register(Boom())

stderr_buf = io.StringIO()
with contextlib.redirect_stderr(stderr_buf):
    sys.unraisablehook = hook
    atexit._run_exitfuncs()

if had_hook:
    sys.unraisablehook = old_hook
else:
    del sys.unraisablehook

print("hook-called", bool(capture))
print("hook-type", capture.get("type"))
if sys.version_info >= (3, 13):
    print(
        "hook-msg-ok",
        capture.get("msg") == "Exception ignored in atexit callback <boom-callback>",
    )
    print("hook-obj-none", capture.get("obj") is None)
else:
    print("hook-msg-ok", capture.get("msg") == "Exception ignored in atexit callback")
    print("hook-obj-repr", repr(capture.get("obj")) == "<boom-callback>")
print("hook-exc-type", capture.get("exc_type"))
print("hook-exc-runtime", capture.get("exc_is_runtime"))
print("hook-has-tb", capture.get("has_tb"))
print("stderr-empty", stderr_buf.getvalue() == "")
