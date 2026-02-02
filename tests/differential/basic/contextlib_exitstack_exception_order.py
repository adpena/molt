"""Purpose: differential coverage for contextlib exitstack exception order."""

import contextlib


log = []


@contextlib.contextmanager
def ctx(name: str):
    log.append(f"enter:{name}")
    try:
        yield name
    finally:
        log.append(f"exit:{name}")


try:
    with contextlib.ExitStack() as stack:
        stack.enter_context(ctx("a"))
        stack.enter_context(ctx("b"))
        raise ValueError("boom")
except Exception as exc:
    print(type(exc).__name__)

print(log)
