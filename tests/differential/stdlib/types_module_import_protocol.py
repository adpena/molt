# MOLT_ENV: PYTHONPATH=src:tests/differential/stdlib
"""Observe ModuleType hooks through imports, publication, and star exports."""

import builtins
import importlib
import sys
import warnings
import module_protocol_pkg as package


def observe(label, operation, warning_action="always"):
    package.events.clear()
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter(warning_action, ImportWarning)
        try:
            result = operation()
            status = "ok"
        except Exception as error:
            result = None
            status = type(error).__name__ + ":" + str(error)
    print(label, status, package.events, [type(w.message).__name__ for w in caught])
    return result


normal = observe("first", lambda: importlib.import_module("module_protocol_pkg.child_normal"))
cached = observe("cached", lambda: importlib.import_module("module_protocol_pkg.child_normal"))
print("identity", normal is cached, normal is package.child_normal)
del package.child_normal
cached = observe("missing-parent-attr", lambda: importlib.import_module("module_protocol_pkg.child_normal"))
print("cache-does-not-publish", cached is normal, "child_normal" in package.__dict__)
observe("reload", lambda: importlib.reload(normal))
observe("loader-exec", lambda: normal.__spec__.loader.exec_module(normal))
print("execution-does-not-publish", "child_normal" in package.__dict__)

package.events.clear()
from module_protocol_pkg import child_from, virtual
print("from-first", child_from.value, virtual, package.events)
package.events.clear()
from module_protocol_pkg import child_from
print("from-cached", child_from.value, package.events)

package.mode = "attribute"
attribute = observe("attribute", lambda: importlib.import_module("module_protocol_pkg.child_attribute"))
print("attribute-state", attribute is sys.modules["module_protocol_pkg.child_attribute"],
      "child_attribute" in package.__dict__)
observe("attribute-cached", lambda: importlib.import_module("module_protocol_pkg.child_attribute"))
observe("warning-error", lambda: importlib.import_module("module_protocol_pkg.child_warning_error"), "error")
print("warning-error-state", "module_protocol_pkg.child_warning_error" in sys.modules,
      "child_warning_error" in package.__dict__)
observe("warning-error-cached", lambda: importlib.import_module("module_protocol_pkg.child_warning_error"))

package.mode = "value"
observe("value", lambda: importlib.import_module("module_protocol_pkg.child_value"))
print("value-state", "module_protocol_pkg.child_value" in sys.modules,
      "child_value" in package.__dict__)
observe("value-cached", lambda: importlib.import_module("module_protocol_pkg.child_value"))

package.mode = "delete"
deleted = observe("delete", lambda: importlib.import_module("module_protocol_pkg.child_delete"))
print("delete-state", deleted.value, deleted is package.child_delete,
      "module_protocol_pkg.child_delete" in sys.modules)
reimported = observe("delete-next", lambda: importlib.import_module("module_protocol_pkg.child_delete"))
print("delete-next-identity", reimported is deleted, reimported is package.child_delete)

package.mode = "replace"
replaced = observe("replace", lambda: importlib.import_module("module_protocol_pkg.child_replace"))
print("replace-state", replaced.value, replaced is package.child_replace,
      sys.modules["module_protocol_pkg.child_replace"] is replaced)
replacement = observe("replace-next", lambda: importlib.import_module("module_protocol_pkg.child_replace"))
print("replace-next-identity", replacement.value, replacement is replaced,
      replacement is package.child_replace)

package.mode = "normal"
descriptor = observe("descriptor", lambda: importlib.import_module("module_protocol_pkg.child_descriptor"))
print("descriptor-identity", descriptor is package.child_descriptor)

from module_protocol_pkg.exports import *
print("star-all", exported)
from module_protocol_pkg.fallback import *
print("star-fallback", first, second, "added_later" in globals())
normal.__all__ = ["absent"]
try:
    from module_protocol_pkg.child_normal import *
except AttributeError as error:
    print("star-missing", type(error).__name__, str(error))


class ModuleNotFoundError(ValueError):
    pass


def source_from_import():
    from module_protocol_pkg import import_raise
    return import_raise


class UnformattableError(ValueError):
    def __str__(self):
        raise AssertionError("import failure classification called __str__")


for entry, operation in (
    ("importlib", lambda: importlib.import_module("module_protocol_pkg.import_raise")),
    ("builtin", lambda: __import__("module_protocol_pkg.import_raise", fromlist=["*"])),
    ("from", source_from_import),
):
    for failure in (
        ValueError("No module named 'misleading'"),
        ModuleNotFoundError("custom name"),
        builtins.ImportError("No module named 'module_protocol_pkg.import_raise'"),
        builtins.ModuleNotFoundError("No module named 'module_protocol_pkg.import_raise'"),
        TypeError("import returned non-module payload"),
        UnformattableError("keep the exception object"),
    ):
        failure.payload = object()
        failure.add_note("preserved note")
        failure.__cause__ = LookupError("original cause")
        expected_args = failure.args
        package.import_failure = failure
        package.events.clear()
        try:
            operation()
        except Exception as caught:
            print("import-error-identity", entry, caught is failure,
                  caught.args is expected_args, caught.payload is failure.payload,
                  caught.__notes__ == ["preserved note"],
                  caught.__cause__ is failure.__cause__)
        else:
            raise AssertionError("import body must propagate its exception")
        print("failed-body-once", package.events == [("import-body",)])
        print("failed-body-removed", "module_protocol_pkg.import_raise" not in sys.modules)

for entry, operation in (
    ("importlib", lambda: importlib.import_module("module_protocol_pkg.no_such_child")),
    ("builtin", lambda: __import__("module_protocol_pkg.no_such_child", fromlist=["*"])),
):
    try:
        operation()
    except builtins.ModuleNotFoundError:
        print("genuine-missing", entry)
    else:
        raise AssertionError("missing module must raise ModuleNotFoundError")
