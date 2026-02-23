"""Purpose: differential coverage for atexit exception reporting/continuation."""

import atexit
import contextlib
import io


class Boom:
    def __call__(self) -> None:
        raise RuntimeError("boom")

    def __repr__(self) -> str:
        return "<boom-callback>"


def ok() -> None:
    print("ok-ran")


atexit._clear()
atexit.register(ok)
atexit.register(Boom())

stderr_buf = io.StringIO()
with contextlib.redirect_stderr(stderr_buf):
    atexit._run_exitfuncs()

stderr_text = stderr_buf.getvalue()
print("count-after-run", atexit._ncallbacks())
print(
    "stderr-prefix",
    "Exception ignored in atexit callback: <boom-callback>" in stderr_text,
)
print("stderr-traceback", "Traceback (most recent call last):" in stderr_text)
print("stderr-runtimeerror", "RuntimeError: boom" in stderr_text)
