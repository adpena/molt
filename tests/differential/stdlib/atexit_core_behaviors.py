"""Purpose: differential coverage for intrinsic-backed atexit core semantics."""

import atexit
import sys


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

atexit._clear()
same_callback = EqCallable("identity")
atexit.register(same_callback)
atexit.register(same_callback)
atexit.unregister(same_callback)
print("count-after-identity-unregister", atexit._ncallbacks())
atexit._clear()

unraisables: list[object] = []
old_unraisablehook = sys.unraisablehook
sys.unraisablehook = unraisables.append  # type: ignore[assignment]


def returns_caught_exception() -> Exception:
    try:
        raise ValueError("caught")
    except ValueError as exc:
        return exc


atexit.register(returns_caught_exception)
atexit._run_exitfuncs()
sys.unraisablehook = old_unraisablehook
print("caught-exception-return-unraisables", len(unraisables))

events.clear()


class ReentrantEqCallable:
    armed = True

    def __init__(self, name: str) -> None:
        self.name = name

    def __call__(self) -> None:
        events.append((self.name, (), ()))

    def __eq__(self, other: object) -> bool:
        if ReentrantEqCallable.armed:
            ReentrantEqCallable.armed = False
            atexit.register(callback, "reentrant-register")
        return isinstance(other, ReentrantEqCallable) and self.name == other.name


atexit.register(ReentrantEqCallable("same"))
atexit.unregister(ReentrantEqCallable("same"))
print("count-after-reentrant-unregister", atexit._ncallbacks())
atexit._run_exitfuncs()
print("events-after-reentrant-unregister", events)
print("count-after-reentrant-run", atexit._ncallbacks())

events.clear()
atexit.register(callback, "clear-1")
atexit.register(callback, "clear-2")
print("count-before-clear", atexit._ncallbacks())
atexit._clear()
print("count-after-clear", atexit._ncallbacks())
atexit._run_exitfuncs()
print("events-after-clear-run", events)
print("count-final", atexit._ncallbacks())
