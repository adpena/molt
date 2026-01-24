"""Purpose: differential coverage for contextlib.contextmanager."""

import contextlib


@contextlib.contextmanager
def managed(value: int):
    yield value


@contextlib.contextmanager
def managed_error(log: list[str]):
    try:
        yield "ok"
    except ValueError:
        log.append("handled")


with managed(3) as result:
    print(result)

events = []
with managed_error(events) as out:
    events.append(out)
    raise ValueError("boom")
print(events)
