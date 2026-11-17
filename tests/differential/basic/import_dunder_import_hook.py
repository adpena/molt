"""Purpose: differential coverage for a rebound builtins.__import__ (bedrock §13 D3).

CPython's IMPORT_NAME reads the frame's __import__ and calls it unless it is
the original: statement imports pass the module's globals, the frame's
locals (None in a function body) and the fromlist and level. Restoring the
original sends statements back to the import protocol.
"""

import builtins

_original = builtins.__import__
calls = []


def recording_import(name, globals=None, locals=None, fromlist=(), level=0):
    # Only this program's own statements: stdlib bodies differ by build.
    if globals is not None and globals.get("__name__") == __name__:
        frame_locals = (
            "none" if locals is None else ("globals" if locals is globals else "dict")
        )
        calls.append((name, frame_locals, tuple(fromlist or ()), level))
    return _original(name, globals, locals, fromlist, level)


def function_imports():
    import json
    from os import sep
    import xml.dom

    return json.__name__, sep == "/" or sep == "\\", xml.__name__


builtins.__import__ = recording_import
import collections
from functools import reduce

print(function_imports())
print(collections.__name__, reduce.__name__)
builtins.__import__ = _original
import string

for call in calls:
    print(call)
print(len(calls))


def replacing_import(name, *args):
    if name == "virtual_module":
        return "virtual"
    return _original(name, *args)


builtins.__import__ = replacing_import
import virtual_module

print(virtual_module)
builtins.__import__ = _original
del builtins.__import__
try:
    import os
except ImportError as exc:
    print(type(exc).__name__, exc)
builtins.__import__ = _original
import os

print(os.__name__)
