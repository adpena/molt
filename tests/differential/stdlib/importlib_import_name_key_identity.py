# MOLT_META: expect_fail=molt expect_fail_reason=open_finding finding=HF-276 expect_molt_returncode=1 expect_molt_stdout_sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
"""Purpose: `__import__` and `import_module` keep the caller's module-name key.

`__import__` looks the name up in `sys.modules` as the object it was given, so
a `str` subclass with its own equality finds the entry it equals.
`import_module` slices the name to an exact `str` first and does not. A lone
surrogate in a missing name survives into `ModuleNotFoundError.name` and its
message (HF-276: lossy host-string conversion of import names).
"""

import importlib
import sys
import types


class Alias(str):
    """A name that equals the published name and hashes like it."""

    def __hash__(self):
        return hash("hf276_target")

    def __eq__(self, other):
        return other == "hf276_target" or str.__eq__(self, other)


sys.modules["hf276_target"] = types.ModuleType("hf276_target")
print("dunder", __import__(Alias("hf276_alias")).__name__)
try:
    importlib.import_module(Alias("hf276_alias"))
except ModuleNotFoundError as exc:
    print("import_module", type(exc).__name__, ascii(exc.name))

name = "hf276_missing_\udcff"
try:
    __import__(name)
except ModuleNotFoundError as exc:
    print("surrogate dunder", ascii(exc.name), ascii(str(exc)))
try:
    importlib.import_module(name)
except ModuleNotFoundError as exc:
    print("surrogate import_module", ascii(exc.name), ascii(str(exc)))
