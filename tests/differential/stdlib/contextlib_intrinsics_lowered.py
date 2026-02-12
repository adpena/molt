"""Purpose: differential coverage for intrinsic-backed contextlib paths."""

import contextlib
import io


events = []


@contextlib.contextmanager
def marker(name: str):
    events.append(f"enter:{name}")
    try:
        yield name
    finally:
        events.append(f"exit:{name}")


with marker("cm") as value:
    print("value", value)


stack = contextlib.ExitStack()
stack.callback(lambda tag: events.append(f"cb:{tag}"), "a")
stack.callback(lambda tag: events.append(f"cb:{tag}"), "b")
moved = stack.pop_all()
stack.close()
print("after-close", events)
moved.close()
print("after-moved", events)


buf = io.StringIO()
with contextlib.redirect_stdout(buf):
    print("redirected")
print("redirect", buf.getvalue().strip())


with contextlib.suppress(KeyError, ValueError):
    raise KeyError("k")
print("suppress", "ok")

try:
    with contextlib.suppress(KeyError):
        raise RuntimeError("boom")
except Exception as exc:
    print("suppress-miss", type(exc).__name__)


class Closable:
    def __init__(self) -> None:
        self.closed = 0

    def close(self) -> None:
        self.closed += 1


item = Closable()
with contextlib.closing(item) as got:
    print("closing-same", got is item)
print("closed", item.closed)

with contextlib.nullcontext(42) as val:
    print("null", val)
