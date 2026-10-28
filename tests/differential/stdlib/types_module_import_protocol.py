# MOLT_ENV: PYTHONPATH=src:tests/differential/stdlib
"""Observe ModuleType hooks through imports, publication, and star exports."""

import builtins
import importlib
import importlib.util
import sys
import warnings
import module_protocol_pkg as package


def observe_ordered_fromlist():
    # Isolate this witness from the publication-hook package used below.
    names = ("fromlist_protocol_pkg", "fromlist_protocol_pkg.child",
             "fromlist_protocol_pkg.broken")
    absent = object()
    previous = {name: sys.modules.get(name, absent) for name in names}
    for name in names:
        sys.modules.pop(name, None)
    try:
        import fromlist_protocol_pkg as fromlist_package

        fromlist_package.child = 0
        from fromlist_protocol_pkg import child as falsey_child
        print("fromlist-falsey", falsey_child,
              "fromlist_protocol_pkg.child" in sys.modules, fromlist_package.events)
        del fromlist_package.child
        fromlist_package.events.clear()
        from fromlist_protocol_pkg import child as fresh_child
        print("fromlist-unloaded", fresh_child is fromlist_package.child,
              fresh_child is sys.modules["fromlist_protocol_pkg.child"],
              fresh_child.value, fromlist_package.events)

        del fromlist_package.child
        fromlist_package.events.clear()
        from fromlist_protocol_pkg import child as cached_child
        print("fromlist-cached", cached_child is fresh_child,
              "child" in fromlist_package.__dict__, fromlist_package.events)

        sys.modules.pop("fromlist_protocol_pkg.child")
        fromlist_package.events.clear()
        first = second = third = absent
        try:
            from fromlist_protocol_pkg import child as first, broken as second, later as third
        except ValueError as error:
            print("fromlist-late-failure", error is fromlist_package.failure,
                  first is absent, second is absent, third is absent)
        else:
            raise AssertionError("later child failure must abort all bindings")
        print("fromlist-surviving-effects", fromlist_package.events,
              fromlist_package.child is sys.modules["fromlist_protocol_pkg.child"],
              "fromlist_protocol_pkg.broken" in sys.modules,
              "broken" in fromlist_package.__dict__,
              ("get", "later") in fromlist_package.events)
    finally:
        for name in names:
            if previous[name] is absent:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = previous[name]


observe_ordered_fromlist()


def observe(label, operation, warning_action="always", expected_error=None):
    package.events.clear()
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter(warning_action, ImportWarning)
        try:
            result = operation()
            assert expected_error is None, "import body must propagate its exact exception"
            status = "ok"
        except Exception as error:
            if expected_error is not None:
                assert error is expected_error, (
                    label, type(error).__name__, error.args,
                    error is package.recursive_error,
                    error is package.recursive_failure,
                )
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

recursive_name = "module_protocol_pkg.child_recursive"
for recursive_mode in ("present", "deleted", "inner-caught", "inner-uncaught", "outer-failure",
                       "outer-replace", "outer-none"):
    sys.modules.pop(recursive_name, None)
    package.__dict__.pop("child_recursive", None)
    package.recursive_mode = recursive_mode
    package.recursive_runs = 0
    package.recursive_outer = None
    package.recursive_nested = None
    package.recursive_error = None
    package.recursive_failure = ValueError("recursive initializer failure")
    expected = (package.recursive_failure
                if recursive_mode in ("inner-uncaught", "outer-failure", "outer-replace", "outer-none")
                else None)
    recursive = observe("recursive-" + recursive_mode,
                        lambda: importlib.import_module("module_protocol_pkg.child_recursive"),
                        expected_error=expected)
    print("recursive-state", recursive_mode, package.recursive_runs,
          recursive_name in sys.modules,
          recursive is not None and recursive is package.recursive_nested,
          recursive is not None and recursive is sys.modules.get(recursive_name),
          recursive is not None and recursive is package.child_recursive,
          package.recursive_error is package.recursive_failure)
    if package.recursive_nested is not None:
        print("recursive-generations", package.recursive_outer.generation,
              package.recursive_nested.generation,
              package.recursive_outer.nested is package.recursive_nested)

private_spec = package.recursive_outer.__spec__
for private_publication in ("missing", "present", "none"):
    sys.modules.pop(recursive_name, None)
    package.recursive_mode = "private"
    package.recursive_runs = 0
    package.private_nested = None
    private_target = importlib.util.module_from_spec(private_spec)
    public_target = importlib.util.module_from_spec(private_spec)
    if private_publication != "missing":
        sys.modules[recursive_name] = (None if private_publication == "none" else public_target)
    try:
        private_spec.loader.exec_module(private_target)
    except builtins.ModuleNotFoundError:
        private_status = "ModuleNotFoundError"
    else:
        private_status = "ok"
    print("private-self-import", private_publication, private_status, package.recursive_runs,
          package.private_nested is private_target,
          package.private_nested is public_target,
          package.private_nested is sys.modules.get(recursive_name))
sys.modules.pop(recursive_name, None)


class CleanupKey:
    def __hash__(self):
        return hash("module_protocol_pkg.child_recursive")

    def __eq__(self, other):
        if package.cleanup_armed:
            package.cleanup_calls += 1
            raise package.cleanup_failure
        return False


for cleanup_kind in (KeyError, RuntimeError):
    sys.modules.pop(recursive_name, None)
    package.recursive_runs = 0
    package.recursive_mode = ("cleanup-key-error" if cleanup_kind is KeyError
                              else "cleanup-runtime-error")
    package.recursive_failure = ValueError("source failure identity")
    package.cleanup_failure = cleanup_kind("cleanup failure identity")
    package.cleanup_armed = False
    package.cleanup_calls = 0
    package.cleanup_key = CleanupKey()
    try:
        importlib.import_module("module_protocol_pkg.child_recursive")
    except Exception as error:
        print("cleanup-error", cleanup_kind.__name__,
              error is (package.recursive_failure if cleanup_kind is KeyError
                        else package.cleanup_failure),
              package.cleanup_failure.__context__ is package.recursive_failure,
              package.cleanup_calls)
    else:
        raise AssertionError("source failure must propagate")
    finally:
        package.cleanup_armed = False
        del sys.modules[package.cleanup_key]


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
