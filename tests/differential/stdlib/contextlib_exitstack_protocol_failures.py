"""Purpose: verify ExitStack protocol + callback failure semantics."""

import contextlib


class MissingEnter:
    def __exit__(self, exc_type, exc, tb):
        return False


class MissingExit:
    def __enter__(self):
        return "ok"


class EnterBoom:
    def __enter__(self):
        raise RuntimeError("enter boom")

    def __exit__(self, exc_type, exc, tb):
        return False


def check_enter(label: str, cm) -> None:
    try:
        with contextlib.ExitStack() as stack:
            stack.enter_context(cm)
    except Exception as exc:
        print(label, type(exc).__name__)


check_enter("missing-enter", MissingEnter())
check_enter("missing-exit", MissingExit())
check_enter("enter-boom", EnterBoom())


events: list[str] = []


def callback(tag: str) -> None:
    events.append(f"cb:{tag}")
    raise ValueError(tag)


try:
    with contextlib.ExitStack() as stack:
        stack.callback(callback, "x")
        print("body")
except Exception as exc:
    print("callback", type(exc).__name__)

print("events", events)
