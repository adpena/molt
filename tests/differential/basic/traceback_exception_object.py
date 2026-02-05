"""Purpose: differential coverage for traceback exception object."""

import traceback


def boom():
    raise RuntimeError("bad")


try:
    boom()
except Exception as exc:
    tbe = traceback.TracebackException.from_exception(exc)
    print(tbe.exc_type.__name__)
    print(any("boom" in line for line in tbe.stack.format()))
