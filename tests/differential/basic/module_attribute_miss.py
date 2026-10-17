"""Purpose: differential coverage for the AttributeError a module raises."""

import sys
import types


def report(label, subject, action):
    try:
        action()
    except AttributeError as exc:
        print(label, "|", str(exc), "|", exc.name, "|", exc.obj is subject)
    except Exception as exc:
        # 3.13 propagates a spec lookup error that 3.12 cleared.
        print(label, "|", type(exc).__name__, "|", str(exc))
    else:
        print(label, "| no error")


def delete(module, name):
    def action():
        delattr(module, name)

    return action


class InitializingSpec:
    _initializing = True


class PendingSpec:
    _initializing = False
    _uninitialized_submodules = ["child"]


class BrokenSpec:
    @property
    def _initializing(self):
        raise RuntimeError("a spec error means 'not initializing'")


class Sub(types.ModuleType):
    pass


plain = types.ModuleType("plain")
report("get", plain, lambda: plain.missing)
report("getattr", plain, lambda: getattr(plain, "missing"))
print("hasattr", hasattr(plain, "missing"))

renamed = types.ModuleType("before")
renamed.__name__ = "after"
report("renamed", renamed, lambda: renamed.missing)

nameless = types.ModuleType("nameless")
del nameless.__dict__["__name__"]
report("nameless", nameless, lambda: nameless.missing)

numbered = types.ModuleType("numbered")
numbered.__name__ = 5
report("non-str name", numbered, lambda: numbered.missing)

initializing = types.ModuleType("initializing")
initializing.__spec__ = InitializingSpec()
report("initializing", initializing, lambda: initializing.missing)

pending = types.ModuleType("pending")
pending.__spec__ = PendingSpec()
report("uninitialized submodule", pending, lambda: pending.child)
report("other name", pending, lambda: pending.other)

broken = types.ModuleType("broken")
broken.__spec__ = BrokenSpec()
report("broken spec", broken, lambda: broken.missing)
# A defaulted lookup suppresses the miss before any spec attribute runs.
print("hasattr broken", hasattr(broken, "missing"))
print("getattr default broken", getattr(broken, "missing", "default"))

sub = Sub("sub")
report("subclass get", sub, lambda: sub.missing)

report("delete", plain, delete(plain, "missing"))
report("delete subclass", sub, delete(sub, "missing"))
report("delete annotations", plain, delete(plain, "__annotations__"))

try:
    from module_attribute_miss_pkg import *
except AttributeError as exc:
    package = sys.modules["module_attribute_miss_pkg"]
    print("star |", str(exc), "|", exc.name, "|", exc.obj is package)
else:
    print("star | no error")
