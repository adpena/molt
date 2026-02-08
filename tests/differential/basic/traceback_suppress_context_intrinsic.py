"""Purpose: validate traceback suppress-context handling is intrinsic-backed."""

import traceback


# Explicitly suppress context with "from None" and ensure traceback wrapper sees it.
try:
    try:
        raise ValueError("inner")
    except ValueError:
        raise RuntimeError("outer") from None
except RuntimeError as exc:
    wrapped = traceback.TracebackException.from_exception(exc)
    print(bool(getattr(wrapped, "_TracebackException__suppress_context__", False)))


# Normal chained exception should not suppress context by default.
try:
    try:
        raise ValueError("inner")
    except ValueError as inner:
        raise RuntimeError("outer") from inner
except RuntimeError as exc:
    wrapped = traceback.TracebackException.from_exception(exc)
    print(bool(getattr(wrapped, "_TracebackException__suppress_context__", False)))
