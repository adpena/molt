"""Purpose: differential coverage for contextlib contextmanager error."""

import contextlib


@contextlib.contextmanager
def ctx():
    yield
    raise ValueError("boom")


try:
    with ctx():
        pass
except Exception as exc:
    print(type(exc).__name__)
