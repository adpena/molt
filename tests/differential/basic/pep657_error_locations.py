"""Purpose: differential coverage for PEP 657 error location metadata."""

import traceback


def boom(x: int) -> float:
    return (1 + x) / (x - x)


try:
    boom(1)
except Exception as exc:
    tb = traceback.TracebackException.from_exception(exc)
    frame = tb.stack[-1]
    print(frame.lineno, frame.end_lineno, frame.colno, frame.end_colno)
