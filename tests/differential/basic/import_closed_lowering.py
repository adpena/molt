"""Purpose: differential coverage for closed imports (bedrock §13 D2, S3).

Every row here is Fixed: no code writes sys.modules, a module spec or an
import hook, so each import lowers to the ready-word read. The program checks
that the closed path returns the same objects CPython's protocol does, binds
the root of a dotted import, prepares a package fromlist, re-runs a failed
body, and stays exact when called many times in a function.
"""

import os
import os.path
import json as j
import importlib
import xml.dom.minidom as md
from collections import OrderedDict

import import_closed_support.pkg
from import_closed_support.pkg import child, lazy


def function_imports():
    import os
    import os.path as p
    from os import sep

    return os.sep == sep and p is os.path


def warm_loop(count):
    hits = 0
    for _ in range(count):
        import os

        hits += len(os.sep)
    return hits


def runtime_rows():
    # sys, builtins and _io reflect their own namespaces only through literal
    # keys, so their rows are Fixed too (import closed world, step 1).
    import sys
    import builtins
    import _io
    from sys import modules

    return (
        sys.modules["sys"] is sys,
        modules is sys.modules,
        builtins.len is len,
        _io.StringIO.__name__,
        "_NS" in vars(builtins),
    )


print(function_imports(), function_imports())
print(runtime_rows(), runtime_rows() == runtime_rows())
print(warm_loop(1000))
print(os.path.join("a", "b"))
print(j.dumps({"a": 1}))
print(list(OrderedDict(a=1).items()))
print(importlib.import_module("os.path") is os.path)
print(importlib.import_module("json") is j)
print(md.__name__)
print(import_closed_support.pkg.__name__, import_closed_support.__name__)
print(child.VALUE, import_closed_support.pkg.LOADED)
print(lazy.__name__.rsplit(".", 1)[-1])

for attempt in range(2):
    try:
        import import_closed_support.flaky as flaky

        print("flaky ready", flaky.READY)
    except RuntimeError as exc:
        print("flaky failed", exc)

try:
    import import_closed_support.missing_child
except ImportError as exc:
    print(type(exc).__name__, exc.name)
