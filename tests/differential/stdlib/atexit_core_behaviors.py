"""Purpose: differential coverage for intrinsic-backed atexit core semantics."""

import atexit


events: list[tuple[str, tuple[object, ...], tuple[tuple[str, object], ...]]] = []


def callback(tag: str, *args: object, **kwargs: object) -> None:
    events.append((tag, args, tuple(sorted(kwargs.items()))))


atexit._clear()
print("start", atexit._ncallbacks())

try:
    atexit.register(1)  # type: ignore[arg-type]
except Exception as exc:  # noqa: BLE001
    print("register-noncallable", type(exc).__name__)

returned = atexit.register(callback, "first", 1, kind="alpha")
atexit.register(callback, "second", 2, kind="beta")
atexit.register(callback, "third", 3, kind="gamma")
print("register-return", returned is callback)
print("count-after-register", atexit._ncallbacks())

atexit._run_exitfuncs()
print("events-after-run", events)
print("count-after-run", atexit._ncallbacks())

events.clear()


class EqCallable:
    __slots__ = ("name",)

    def __init__(self, name: str) -> None:
        self.name = name

    def __call__(self) -> None:
        events.append((self.name, (), ()))

    def __eq__(self, other: object) -> bool:
        return isinstance(other, EqCallable) and self.name == other.name

    def __hash__(self) -> int:
        return hash(self.name)


atexit.register(EqCallable("same"))
atexit.register(EqCallable("other"))
atexit.register(EqCallable("same"))
print("count-before-unregister", atexit._ncallbacks())
print("unregister-return", atexit.unregister(EqCallable("same")) is None)
print("count-after-unregister", atexit._ncallbacks())

atexit._run_exitfuncs()
print("events-after-unregister-run", events)
print("count-after-unregister-run", atexit._ncallbacks())

events.clear()
atexit.register(callback, "clear-1")
atexit.register(callback, "clear-2")
print("count-before-clear", atexit._ncallbacks())
atexit._clear()
print("count-after-clear", atexit._ncallbacks())
atexit._run_exitfuncs()
print("events-after-clear-run", events)
print("count-final", atexit._ncallbacks())
