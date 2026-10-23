"""Purpose: weakref.finalize exit protocol: the registry owns the atexit flag,
selection runs newest first, and no finalizer runs once exit processing ends."""

import atexit
import sys
import weakref


class Box:
    pass


class Truthy:
    def __bool__(self):
        raise AssertionError("bool() evaluated on a dead finalizer")


# Registered before any finalizer, so it runs after the exit pass.
late_live = []


def after_exit_pass():
    late = Box()
    late_live.append(late)
    fin = weakref.finalize(late, print, "late finalizer ran")
    print("after exit pass: late alive", fin.alive, "atexit", fin.atexit)


atexit.register(after_exit_pass)

# A dead finalizer reports atexit False and ignores the setter.
dead_target = Box()
dead = weakref.finalize(dead_target, print, "dead callback")
dead()
print("dead atexit", dead.atexit, "alive", dead.alive)
dead.atexit = Truthy()
print("dead atexit after set", dead.atexit)

# Disabled at exit, its target outlives the exit pass.
kept = Box()
disabled = weakref.finalize(kept, print, "disabled finalizer ran")
disabled.atexit = False
print("disabled atexit", disabled.atexit)

# Enabled ones run newest first; one creates another during the pass.
first = Box()
second = Box()


def spawn():
    print("second ran")
    child = Box()
    late_live.append(child)
    weakref.finalize(child, print, "spawned during exit pass")


weakref.finalize(first, print, "first ran")
weakref.finalize(second, spawn)

# An Exception from one finalizer reaches sys.excepthook; the pass goes on.
failing = Box()


def fail():
    raise ValueError("finalizer failed")


def hook(exc_type, exc, tb):
    print("excepthook", exc_type.__name__, exc)


sys.excepthook = hook
weakref.finalize(failing, fail)


def before_exit_pass():
    print("before exit pass: disabled alive", disabled.alive)


atexit.register(before_exit_pass)
print("main done")
