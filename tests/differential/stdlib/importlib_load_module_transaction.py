# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for SourceFileLoader.load_module transaction."""

import importlib.machinery
import os
import sys
import tempfile
import types


def show(label, func):
    try:
        value = func()
    except BaseException as exc:  # noqa: BLE001
        print(label, type(exc).__name__, str(exc))
    else:
        print(label, "OK", repr(value))


class BoomLoader(importlib.machinery.SourceFileLoader):
    def exec_module(self, module):
        print("boom-preseed", sys.modules.get(module.__name__) is module)
        sys.modules[module.__name__] = "partial"
        raise RuntimeError("boom")


class SubstituteLoader(importlib.machinery.SourceFileLoader):
    def exec_module(self, module):
        print("sub-preseed", sys.modules.get(module.__name__) is module)
        sys.modules[module.__name__] = "substitute"


class ExistingBoomLoader(importlib.machinery.SourceFileLoader):
    def __init__(self, fullname, path, previous):
        super().__init__(fullname, path)
        self.previous = previous

    def exec_module(self, module):
        print("existing-is-previous", module is self.previous)
        sys.modules[module.__name__] = "partial-existing"
        raise RuntimeError("boom-existing")


with tempfile.TemporaryDirectory() as tmp:
    path = os.path.join(tmp, "loader_target.py")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write("value = 41\n")

    sys.modules.pop("demo_lm", None)
    show("boom", lambda: BoomLoader("demo_lm", path).load_module("demo_lm"))
    print("boom-after", "demo_lm" in sys.modules, sys.modules.get("demo_lm"))

    sys.modules.pop("demo_lm", None)
    show("sub", lambda: SubstituteLoader("demo_lm", path).load_module("demo_lm"))
    print("sub-after", sys.modules.get("demo_lm"))

    previous = types.ModuleType("demo_lm")
    sys.modules["demo_lm"] = previous
    show(
        "existing-boom",
        lambda: ExistingBoomLoader("demo_lm", path, previous).load_module("demo_lm"),
    )
    print("existing-after", sys.modules.get("demo_lm"))

    sys.modules.pop("demo_real", None)
    module = importlib.machinery.SourceFileLoader("demo_real", path).load_module(
        "demo_real"
    )
    print("real", module.__name__, module.value, sys.modules.get("demo_real") is module)
