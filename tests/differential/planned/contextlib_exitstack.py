"""Purpose: differential coverage for contextlib exitstack."""

import contextlib


log = []


def make(name: str):
    @contextlib.contextmanager
    def _ctx():
        log.append(f"enter:{name}")
        try:
            yield name
        finally:
            log.append(f"exit:{name}")

    return _ctx()


with contextlib.ExitStack() as stack:
    a = stack.enter_context(make("a"))
    b = stack.enter_context(make("b"))
    print(a, b)

print(log)
