"""An installed module's namespace never determines whether it is imported."""

# Filesystem finder capability must be available before optional util startup.
# This virtual path is admitted by a public hook, with no source-file fallback.
import sys


class _EarlyLoader:
    def create_module(self, spec):
        return None

    def exec_module(self, module):
        module.marker = "early path hook"


class _EarlyFinder:
    def find_spec(self, name, target=None):
        if name == "_molt_early_path_hook":
            return _EarlySpec(name, _EarlyLoader())
        return None


_early_events = []


def _early_hook(path):
    if path != "_molt_virtual_path":
        raise ImportError("other path")
    _early_events.append(path)
    return _EarlyFinder()


class _StandinMachinery:
    @property
    def PathFinder(self):
        raise AssertionError("cached stand-in acquired bootstrap provenance")

    @property
    def _default_path_hooks(self):
        raise AssertionError("cached stand-in acquired bootstrap hooks")


_early_machinery = sys.modules.get("importlib.machinery")
_early_standin = _StandinMachinery()
sys.modules["importlib.machinery"] = _early_standin
_early_path = sys.path
_early_hooks = sys.path_hooks
_early_util = sys.modules.pop("importlib.util", None)
sys.path = ["_molt_virtual_path"]
sys.path_hooks = [_early_hook]
sys.path_importer_cache.pop("_molt_virtual_path", None)
try:
    from _frozen_importlib import ModuleSpec as _EarlySpec

    _early = __import__("_molt_early_path_hook")
    assert _early.marker == "early path hook"
    assert _early_events == ["_molt_virtual_path"]
    assert "importlib.util" not in sys.modules
    assert sys.modules["importlib.machinery"] is _early_standin
    print("early-path-hook", _early.marker, _early_events)
finally:
    sys.path = _early_path
    sys.path_hooks = _early_hooks
    sys.path_importer_cache.pop("_molt_virtual_path", None)
    sys.modules.pop("_molt_early_path_hook", None)
    if _early_util is not None:
        sys.modules["importlib.util"] = _early_util
    if _early_machinery is None:
        del sys.modules["importlib.machinery"]
    else:
        sys.modules["importlib.machinery"] = _early_machinery


import _warnings
import importlib
import importlib.machinery
import importlib.util
import sys
import types


def _importer_factory():
    return importlib.import_module


_returned_importers = [_importer_factory()]
assert "fromlist_protocol_pkg" not in sys.modules
assert "fromlist_protocol_pkg.child" not in sys.modules
_returned_child = _returned_importers[0]("fromlist_protocol_pkg.child")
print(
    "returned-importer",
    _returned_child.value,
    sys.modules["fromlist_protocol_pkg.child"] is _returned_child,
)


_return_root = types.ModuleType("_molt_return_pkg")
_return_root.__path__ = []
_return_branch = types.ModuleType("_molt_return_pkg.branch")
_return_branch.__path__ = []
_return_leaf = types.ModuleType("_molt_return_pkg.branch.leaf")
_return_globals = {"__package__": "_molt_return_pkg"}
for _return_module in (_return_root, _return_branch, _return_leaf):
    sys.modules[_return_module.__name__] = _return_module


class _FalseReturnFromlist:
    def __init__(self, mode):
        self.mode = mode
        self.calls = 0

    def __bool__(self):
        self.calls += 1
        if self.mode == "none":
            sys.modules["_molt_return_pkg.branch"] = None
        elif self.mode == "missing":
            del sys.modules["_molt_return_pkg.branch"]
        elif self.mode == "alias":
            sys.modules["_molt_return_pkg.branch"] = _return_leaf
        elif self.mode == "absolute-none":
            sys.modules["_molt_return_pkg"] = None
        elif self.mode == "absolute-missing":
            del sys.modules["_molt_return_pkg"]
        return False


try:
    assert __import__("_molt_return_pkg.branch.leaf") is _return_root
    assert __import__("branch.leaf", _return_globals, None, (), 1) is _return_branch
    assert (
        __import__("leaf", {"__package__": "_molt_return_pkg.branch"}, None, (), 1)
        is _return_leaf
    )
    assert __import__("", _return_globals, None, (), 1) is _return_root
    print("relative-return", True)
    for _mode in ("none", "missing", "alias"):
        sys.modules["_molt_return_pkg.branch"] = _return_branch
        _fromlist = _FalseReturnFromlist(_mode)
        try:
            _selected = __import__("branch.leaf", _return_globals, None, _fromlist, 1)
        except KeyError as _error:
            assert _mode == "missing"
            assert _error.args == (
                "'_molt_return_pkg.branch' not in sys.modules as expected",
            )
        else:
            assert _mode != "missing"
            assert _selected is (None if _mode == "none" else _return_leaf)
        assert _fromlist.calls == 1
        print("relative-return-cache", _mode, _fromlist.calls)
    sys.modules["_molt_return_pkg.branch"] = _return_branch
    for _mode in ("absolute-none", "absolute-missing"):
        sys.modules["_molt_return_pkg"] = _return_root
        _fromlist = _FalseReturnFromlist(_mode)
        try:
            __import__("_molt_return_pkg.branch.leaf", None, None, _fromlist)
        except ModuleNotFoundError as _error:
            assert _error.name == "_molt_return_pkg"
        else:
            raise AssertionError("absolute return selection must import the root again")
        assert _fromlist.calls == 1
        print("absolute-return-cache", _mode, _fromlist.calls)
finally:
    for _return_name in (
        "_molt_return_pkg.branch.leaf",
        "_molt_return_pkg.branch",
        "_molt_return_pkg",
    ):
        sys.modules.pop(_return_name, None)


class OpaqueModule(types.ModuleType):
    def __getattribute__(self, name):
        if name in ("__name__", "__file__"):
            raise AssertionError("a cached import must not inspect module metadata")
        return super().__getattribute__(name)


for name, module in (
    ("cached_empty", types.ModuleType("cached_empty")),
    ("cached_private", types.ModuleType("cached_private")),
    ("multiprocessing_cached", types.ModuleType("multiprocessing_cached")),
    ("cached_opaque", OpaqueModule("cached_opaque")),
    ("cached_object", object()),
    ("cached_int", 42),
    ("cached_dict", {}),
):
    if name == "cached_private":
        module._molt_runtime = object()
    sys.modules[name] = module
    try:
        print(name, importlib.import_module(name) is module, __import__(name) is module)
    finally:
        del sys.modules[name]
    try:
        importlib.import_module(name)
    except ModuleNotFoundError as error:
        print("deleted", name, error.name == name)
    else:
        raise AssertionError("deleted public cache entry was revived")

sys.modules["cached_none"] = None
try:
    importlib.import_module("cached_none")
except ModuleNotFoundError:
    print("none blocks import")
finally:
    del sys.modules["cached_none"]


class SpecFailure(types.ModuleType):
    def __getattribute__(self, name):
        if name == "__spec__":
            raise ValueError("spec probe")
        return super().__getattribute__(name)


module = SpecFailure("cached_probe")
sys.modules["cached_probe"] = module
try:
    try:
        importlib.import_module("cached_probe")
    except ValueError as error:
        print(type(error).__name__, str(error))
    try:
        print("builtin probe", __import__("cached_probe") is module)
    except ValueError as error:
        print("builtin probe", type(error).__name__, str(error))
finally:
    del sys.modules["cached_probe"]


def observe_initializing_probe(mode, entry):
    name = "_molt_spec_probe_" + mode.replace("-", "_") + "_" + entry
    events = []
    marker = ValueError("probe marker")
    replacement = 42

    class Truth:
        def __bool__(self):
            events.append("bool")
            if mode == "bool-error":
                raise marker
            if mode in ("delete", "delete-true"):
                sys.modules.pop(name, None)
            if mode == "replace":
                sys.modules[name] = replacement
            return mode in ("true", "delete-true")

    class Spec:
        @property
        def _initializing(self):
            events.append("get")
            if mode == "get-error":
                raise marker
            return Truth()

    original = types.ModuleType(name)
    original.__spec__ = Spec()
    sys.modules[name] = original
    missing_name_matches = False
    result_is_replacement = False
    try:
        try:
            result = (
                __import__(name)
                if entry == "builtin"
                else importlib.import_module(name)
            )
            status = "returned"
            same_value_or_error = result is original
            result_is_replacement = result is replacement
            assert same_value_or_error or (mode == "replace" and result_is_replacement)
            if mode in ("false", "true"):
                assert same_value_or_error
        except ValueError as error:
            status = type(error).__name__
            same_value_or_error = error is marker
            assert mode in ("get-error", "bool-error") and same_value_or_error
        except ModuleNotFoundError as error:
            status = type(error).__name__
            same_value_or_error = error is marker
            missing_name_matches = error.name == name
            assert mode in ("delete", "delete-true") and missing_name_matches
        assert events == (["get"] if mode == "get-error" else ["get", "bool"])
        assert (name in sys.modules) == (mode not in ("delete", "delete-true"))
        if mode == "replace":
            assert sys.modules[name] is replacement
        # Print the actual observation: patch releases differ in whether a false
        # flag retains the observed module or rechecks a callback-mutated cache.
        # The differential oracle, not a minor-version rule, selects that policy.
        print(
            "initializing-probe",
            mode,
            entry,
            events,
            status,
            same_value_or_error,
            result_is_replacement,
            missing_name_matches,
            name in sys.modules,
        )
    finally:
        sys.modules.pop(name, None)


for probe_mode in (
    "false",
    "true",
    "get-error",
    "bool-error",
    "delete",
    "replace",
    "delete-true",
):
    for probe_entry in ("builtin", "importlib"):
        observe_initializing_probe(probe_mode, probe_entry)


# Removed stdlib providers remain replaceable by an admitted Python loader.
class BackportLoader:
    def create_module(self, spec):
        return None

    def exec_module(self, module):
        module.marker = "backport:" + module.__name__


class BackportFinder:
    def __init__(self):
        self.events = []
        self.failure = None

    def find_spec(self, fullname, path=None, target=None):
        if fullname in ("msilib", "msilib.schema"):
            self.events.append((fullname, path is None, target is None))
            if self.failure is not None:
                raise self.failure
            return importlib.machinery.ModuleSpec(
                fullname, BackportLoader(), is_package=fullname == "msilib"
            )
        return None


def load_literal_child():
    import msilib.schema as child

    return child


def load_from_child():
    from msilib import schema

    return schema


saved = {
    name: sys.modules.pop(name)
    for name in ("msilib.schema", "msilib")
    if name in sys.modules
}
finder = BackportFinder()
sys.meta_path.insert(0, finder)
try:
    backport = importlib.import_module("msilib")
    child = importlib.import_module("msilib.schema")
    print("backport-child", child.marker, backport.schema is child)
    del sys.modules["msilib.schema"]
    second = importlib.import_module("msilib.schema")
    print("backport-reimport", second is not child, backport.schema is second)

    # A compiler-admitted initializer is an ordinary finder candidate. Literal
    # imports and fromlist child imports must honor the same ordered finders as
    # import_module, including the finder's exception and the public cache hit.
    for entry, load in (("literal", load_literal_child), ("from", load_from_child)):
        sys.modules.pop("msilib.schema", None)
        sys.modules.pop("msilib", None)
        finder.events.clear()
        loaded = load()
        parent = sys.modules["msilib"]
        assert loaded.marker == "backport:msilib.schema"
        assert parent.schema is loaded
        assert finder.events == [
            ("msilib", True, True),
            ("msilib.schema", False, True),
        ]
        assert load() is loaded
        assert len(finder.events) == 2
        print("backport-source-import", entry, finder.events, parent.schema is loaded)

        del parent.schema
        del sys.modules["msilib.schema"]
        finder.events.clear()
        marker = ValueError("finder failure")
        finder.failure = marker
        try:
            load()
        except ValueError as error:
            assert error is marker
        else:
            raise AssertionError("a compiled initializer bypassed a finder failure")
        finally:
            finder.failure = None
        assert finder.events == [("msilib.schema", False, True)]
        assert "msilib.schema" not in sys.modules
        assert not hasattr(parent, "schema")
        print("backport-finder-failure", entry, finder.events)
finally:
    sys.meta_path.remove(finder)
    sys.modules.pop("msilib.schema", None)
    sys.modules.pop("msilib", None)
    sys.modules.update(saved)


# Public helper rebinding controls direct calls, never ordinary import/reload.
_util_public = importlib.util.find_spec
_util_private = importlib.util._find_spec
_rebind_error = ValueError("public util helper was called")


def _forbidden_util(*args, **kwargs):
    raise _rebind_error


saved = {
    name: sys.modules.pop(name)
    for name in ("msilib.schema", "msilib")
    if name in sys.modules
}
finder = BackportFinder()
sys.meta_path.insert(0, finder)
importlib.util.find_spec = _forbidden_util
importlib.util._find_spec = _forbidden_util
try:
    try:
        importlib.util.find_spec("msilib")
    except ValueError as error:
        assert error is _rebind_error
    else:
        raise AssertionError("the direct public helper ignored rebinding")
    parent = importlib.import_module("msilib")
    child = load_literal_child()
    assert parent.schema is child
    assert importlib.reload(parent) is parent
    assert parent.marker == "backport:msilib"
    print("util-helper-rebind", child.marker, parent.marker)
finally:
    importlib.util.find_spec = _util_public
    importlib.util._find_spec = _util_private
    sys.meta_path.remove(finder)
    sys.modules.pop("msilib.schema", None)
    sys.modules.pop("msilib", None)
    sys.modules.update(saved)


# Latest 3.12 walks the live list; latest 3.13+ copies before callbacks.
from importlib import _bootstrap
import _imp

for _mutation in ("append", "remove"):
    _events = []
    _name = "_molt_finder_mutation_" + _mutation
    _spec = importlib.machinery.ModuleSpec(_name, None)

    class _TailFinder:
        @property
        def find_spec(self):
            assert _imp.lock_held()
            _events.append("get-tail")
            return self.call

        def call(self, name, path, target):
            assert _imp.lock_held()
            _events.append("tail")
            return _spec

    _tail = _TailFinder()

    class _MutatingFinder:
        def find_spec(self, name, path, target):
            assert _imp.lock_held()
            _events.append("head")
            if _mutation == "append":
                sys.meta_path.append(_tail)
            else:
                sys.meta_path.remove(_tail)
            return None

    _previous = sys.meta_path
    sys.meta_path = [_MutatingFinder()] + ([_tail] if _mutation == "remove" else [])
    try:
        _selected = _bootstrap._find_spec(_name, None)
        _snapshot = sys.version_info >= (3, 13)
        _visited = _snapshot if _mutation == "remove" else not _snapshot
        assert (_selected is _spec) == _visited
        assert _events == (["head", "get-tail", "tail"] if _visited else ["head"])
        assert not _imp.lock_held()
        print("finder-mutation", _mutation, _events, _selected is _spec)
    finally:
        sys.meta_path = _previous


class _FailingFinder:
    @property
    def find_spec(self):
        assert _imp.lock_held()
        raise _rebind_error


_previous = sys.meta_path
sys.meta_path = [_FailingFinder()]
try:
    try:
        _bootstrap._find_spec("_molt_finder_failure", None)
    except ValueError as error:
        assert error is _rebind_error
    else:
        raise AssertionError("finder descriptor failure was hidden")
    assert not _imp.lock_held()
    _imp.acquire_lock()
    _imp.acquire_lock()
    assert _imp.lock_held()
    _imp.release_lock()
    assert _imp.lock_held()
    _imp.release_lock()
    assert not _imp.lock_held()
    try:
        _imp.release_lock()
    except RuntimeError as error:
        assert str(error) == "not holding the import lock"
    else:
        raise AssertionError("unowned import lock release succeeded")
    print("finder-lock-cleanup", True)
finally:
    sys.meta_path = _previous


# Empty-path warning dispatch observes the actual loaded warning callable.

_previous = sys.meta_path
_original_warn = _warnings.warn
_warning_events = []


def _record_empty_path(message, category):
    assert not _imp.lock_held()
    _warning_events.append((message, category is ImportWarning))


def _raise_empty_path(message, category):
    _record_empty_path(message, category)
    raise _rebind_error


sys.meta_path = []
try:
    for _warning_mode, _warn in (
        ("record", _record_empty_path),
        ("raise", _raise_empty_path),
        ("none", None),
        ("noncallable", 42),
    ):
        _warnings.warn = _warn
        _warning_events.clear()
        try:
            _selected = _bootstrap._find_spec("_molt_empty_path_warning", None)
        except ValueError as error:
            assert _warning_mode == "raise" and error is _rebind_error
        except TypeError:
            assert _warning_mode in ("none", "noncallable")
        else:
            assert _warning_mode == "record" and _selected is None
        assert _warning_events == (
            [("sys.meta_path is empty", True)]
            if _warning_mode in ("record", "raise")
            else []
        )
        assert not _imp.lock_held()
        print("finder-empty-warning", _warning_mode, _warning_events)
finally:
    _warnings.warn = _original_warn
    sys.meta_path = _previous


# Copying the 3.13+ iterable releases its source before finder callbacks.
_owner_events = []
_owner_spec = importlib.machinery.ModuleSpec("_molt_finder_owners", None)


class _OwnerFinder:
    def find_spec(self, name, path, target):
        _owner_events.append("finder")
        return _owner_spec


class _TemporaryMetaPath:
    def __iter__(self):
        sys.meta_path = []
        yield _OwnerFinder()

    def __del__(self):
        _owner_events.append("iterable-released")


_previous = sys.meta_path
sys.meta_path = _TemporaryMetaPath()
try:
    assert _bootstrap._find_spec("_molt_finder_owners", None) is _owner_spec
    _expected = (
        ["iterable-released", "finder"]
        if sys.version_info >= (3, 13)
        else ["finder", "iterable-released"]
    )
    assert _owner_events == _expected
    print("finder-iterable-owner", _owner_events)
finally:
    sys.meta_path = _previous


# A subscription receiver ends before the selected module's spec lookup;
# the selected module itself remains owned until the finder function returns.
_owner_events.clear()
_real_modules = sys.modules
_membership_count = 0


class _TemporarySelectedModule:
    @property
    def __spec__(self):
        _owner_events.append("spec")
        return _owner_spec

    def __del__(self):
        _owner_events.append("module-released")


class _TemporaryModules:
    def __contains__(self, name):
        global _membership_count
        _membership_count += 1
        return _membership_count > 1

    def __getitem__(self, name):
        sys.modules = _real_modules
        return _TemporarySelectedModule()

    def __del__(self):
        _owner_events.append("mapping-released")


_previous = sys.meta_path
sys.meta_path = [_OwnerFinder()]
sys.modules = _TemporaryModules()
try:
    assert _bootstrap._find_spec("_molt_finder_owners", None) is _owner_spec
    assert _owner_events == ["finder", "mapping-released", "spec", "module-released"]
    print("finder-selected-owners", _owner_events)
finally:
    sys.modules = _real_modules
    sys.meta_path = _previous


import weakref as _cold_weakref
from _frozen_importlib import ModuleSpec as _ColdModuleSpec


_cache_root = "_molt_cached_leaf_parent"
_cache_name = _cache_root + ".child"
_cache_leaf = types.ModuleType(_cache_name)
sys.modules[_cache_name] = _cache_leaf
try:
    for _cache_state in ("absent", "none"):
        sys.modules.pop(_cache_root, None)
        if _cache_state == "none":
            sys.modules[_cache_root] = None
        assert importlib.import_module(_cache_name) is _cache_leaf
        assert __import__(_cache_name, fromlist=("value",)) is _cache_leaf
        print("cached-leaf-parent", _cache_state, True)
    sys.modules[_cache_name] = None
    try:
        importlib.import_module(_cache_name)
    except ModuleNotFoundError as _cache_error:
        assert _cache_error.name == _cache_name
        print("cached-leaf-none", True)
    else:
        raise AssertionError("a preexisting None leaf must halt import")
finally:
    sys.modules.pop(_cache_name, None)
    sys.modules.pop(_cache_root, None)


class _ColdParent(types.ModuleType):
    def __del__(self):
        _cold_events.append("old-parent-retired")
        if _cold_mode == "rebind-finalizer":
            sys.modules[_cold_root] = _cold_other


class _ColdReplacement(types.ModuleType):
    def __setattr__(self, name, value):
        if name == "child":
            _cold_events.append("fresh-parent-bind")
        super().__setattr__(name, value)


class _ColdLoader:
    def create_module(self, spec):
        global _cold_old_ref
        if spec.name == _cold_root and _cold_mode in ("replace", "rebind-finalizer"):
            parent = _ColdParent(spec.name)
            _cold_old_ref = _cold_weakref.ref(parent)
            return parent
        return None

    def exec_module(self, module):
        if module.__name__ == _cold_root:
            if _cold_mode in ("publish-value", "publish-none"):
                sys.modules[_cold_name] = 42 if _cold_mode == "publish-value" else None
                del module.__path__
            return
        if _cold_mode in ("replace", "rebind-finalizer"):
            sys.modules[_cold_root] = _cold_replacement
            assert _cold_old_ref() is not None
            _cold_events.append("child-executed")
        elif _cold_mode == "remove-parent":
            del sys.modules[_cold_root]
        elif _cold_mode == "child-none":
            sys.modules[_cold_name] = None


class _ColdFinder:
    def find_spec(self, name, path=None, target=None):
        if name == _cold_root:
            _cold_finds.append("parent")
            return _ColdModuleSpec(name, _ColdLoader(), is_package=True)
        if name == _cold_name:
            _cold_finds.append("child")
            return _ColdModuleSpec(name, _ColdLoader())
        return None


_cold_saved_meta_path = sys.meta_path
sys.meta_path = [_ColdFinder()]
try:
    for _cold_mode in (
        "publish-value",
        "publish-none",
        "replace",
        "rebind-finalizer",
        "remove-parent",
        "child-none",
        "empty-child",
    ):
        _cold_root = "_molt_cold_parent_" + _cold_mode.replace("-", "_")
        _cold_child = "" if _cold_mode == "empty-child" else "child"
        _cold_name = _cold_root + "." + _cold_child
        _cold_replacement = _ColdReplacement(_cold_root)
        _cold_replacement.__path__ = []
        _cold_other = types.ModuleType(_cold_root)
        _cold_other.__path__ = []
        _cold_events = []
        _cold_finds = []
        try:
            try:
                _cold_result = importlib.import_module(_cold_name)
            except KeyError as _cold_error:
                assert _cold_mode == "remove-parent"
                assert _cold_error.args == (_cold_root,)
                assert _cold_name in sys.modules
                assert importlib.import_module(_cold_name) is sys.modules[_cold_name]
            else:
                assert _cold_mode != "remove-parent"
                if _cold_mode in ("publish-value", "publish-none"):
                    assert _cold_result == (42 if _cold_mode == "publish-value" else None)
                    assert _cold_finds == ["parent"]
                elif _cold_mode in ("replace", "rebind-finalizer"):
                    assert _cold_events == [
                        "child-executed",
                        "old-parent-retired",
                        "fresh-parent-bind",
                    ]
                    assert _cold_replacement.child is _cold_result
                    if _cold_mode == "rebind-finalizer":
                        assert sys.modules[_cold_root] is _cold_other
                        assert not hasattr(_cold_other, "child")
                    else:
                        assert sys.modules[_cold_root] is _cold_replacement
                else:
                    assert getattr(sys.modules[_cold_root], _cold_child) is _cold_result
                    if _cold_mode == "child-none":
                        assert _cold_result is None
            print("cold-parent-protocol", _cold_mode, True)
        finally:
            sys.modules.pop(_cold_name, None)
            sys.modules.pop(_cold_root, None)
finally:
    sys.meta_path = _cold_saved_meta_path
