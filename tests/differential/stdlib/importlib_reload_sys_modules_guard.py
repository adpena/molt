"""Purpose: differential coverage for importlib.reload sys.modules guard semantics."""

import importlib
import importlib.machinery
import sys
import types


def _expect_error(label, fn, kind, message):
    try:
        fn()
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))
        assert type(exc).__name__ == kind
        assert str(exc) == message
    else:
        raise AssertionError(f"{label} unexpectedly succeeded")


missing = types.ModuleType("molt_reload_guard_missing")
_expect_error(
    "missing-module",
    lambda: importlib.reload(missing),
    "ImportError",
    "module molt_reload_guard_missing not in sys.modules",
)


class NamedObject:
    __name__ = "molt_reload_named_not_module"


_expect_error(
    "named-object",
    lambda: importlib.reload(NamedObject()),
    "ImportError",
    "module molt_reload_named_not_module not in sys.modules",
)


class NonStringName:
    __name__ = 1


_expect_error(
    "non-string-name",
    lambda: importlib.reload(NonStringName()),
    "ImportError",
    "module 1 not in sys.modules",
)

_expect_error(
    "bad-arg",
    lambda: importlib.reload(1),
    "TypeError",
    "reload() argument must be a module",
)

math_mod = importlib.import_module("math")
removed = sys.modules.pop("math", None)
try:
    _expect_error(
        "popped-real-module",
        lambda: importlib.reload(math_mod),
        "ImportError",
        "module math not in sys.modules",
    )
finally:
    if removed is not None:
        sys.modules["math"] = removed


class ReloadLoader:
    def __init__(self, name, mode):
        self.name = name
        self.mode = mode
        self.calls = 0
        self.seen = None
        self.failure = ValueError("reload body failed")
        self.replacement = object()

    def create_module(self, spec):
        raise AssertionError("reload must execute the original namespace")

    def exec_module(self, module):
        self.calls += 1
        self.seen = module
        module.body_ran = True
        sys.modules[self.name + ":after"] = object()
        if self.mode in ("replace", "replace-fail"):
            sys.modules[self.name] = self.replacement
        elif self.mode == "none":
            sys.modules[self.name] = None
        elif self.mode in ("delete", "delete-fail"):
            del sys.modules[self.name]
        if self.mode in ("fail", "replace-fail", "delete-fail"):
            raise self.failure


class ReloadFinder:
    def __init__(self, name, spec, mode):
        self.name = name
        self.spec = spec
        self.mode = mode
        self.calls = 0
        self.target = None
        self.replacement = object()

    def find_spec(self, fullname, path=None, target=None):
        if fullname != self.name:
            return None
        self.calls += 1
        self.target = target
        if self.mode == "replace":
            sys.modules[fullname] = self.replacement
        elif self.mode == "none":
            sys.modules[fullname] = None
        elif self.mode == "delete":
            del sys.modules[fullname]
        return self.spec


def run_reload_transaction(mode):
    name = "molt_reload_transaction_" + mode.replace("-", "_")
    module = types.ModuleType(name)
    old_loader = object()
    module.__spec__ = importlib.machinery.ModuleSpec(name, old_loader)
    module.__loader__ = old_loader
    module.__package__ = "stale-package"
    loader = ReloadLoader(name, mode)
    spec = importlib.machinery.ModuleSpec(name, loader, origin="reload-source")
    spec.has_location = True
    spec.cached = "reload-cache"
    finder = ReloadFinder(name, spec, "same")
    sys.modules[name] = module
    sys.meta_path.insert(0, finder)
    try:
        result = None
        error = None
        try:
            result = importlib.reload(module)
        except BaseException as exc:
            error = exc
        assert finder.calls == 1 and finder.target is module
        assert loader.calls == 1 and loader.seen is module and module.body_ran
        assert module.__spec__ is spec and module.__loader__ is loader
        assert module.__package__ == ""
        assert module.__file__ == "reload-source"
        if sys.version_info >= (3, 15):
            assert not hasattr(module, "__cached__")
        else:
            assert module.__cached__ == "reload-cache"
        assert not getattr(spec, "_initializing", False)
        if mode in ("delete", "delete-fail"):
            assert type(error) is KeyError and error.args == (name,)
            assert name not in sys.modules
            if mode == "delete-fail":
                assert error.__context__ is loader.failure
        else:
            expected = (
                loader.replacement
                if mode in ("replace", "replace-fail")
                else None
                if mode == "none"
                else module
            )
            assert sys.modules[name] is expected
            keys = list(sys.modules)
            assert keys.index(name) > keys.index(name + ":after")
            if mode in ("fail", "replace-fail"):
                assert error is loader.failure
            else:
                assert error is None and result is expected
        print("reload-transaction", mode, True)
    finally:
        sys.meta_path.remove(finder)
        sys.modules.pop(name, None)
        sys.modules.pop(name + ":after", None)


def run_reload_discovery(mode):
    name = "molt_reload_discovery_" + mode
    module = types.ModuleType(name)
    old_loader = object()
    module.__spec__ = importlib.machinery.ModuleSpec(name, old_loader)
    module.__loader__ = old_loader
    loader = ReloadLoader(name, "same")
    spec = None if mode == "missing" else importlib.machinery.ModuleSpec(name, loader)
    finder = ReloadFinder(name, spec, mode)
    sys.modules[name] = module
    sys.meta_path.insert(0, finder)
    try:
        error = None
        try:
            importlib.reload(module)
        except BaseException as exc:
            error = exc
        assert finder.calls == 1 and finder.target is module
        assert loader.calls == 0 and module.__loader__ is old_loader
        assert module.__spec__ is spec
        if mode == "missing":
            assert type(error) is ModuleNotFoundError
            assert str(error) == "spec not found for the module " + repr(name)
            assert error.name == name and sys.modules[name] is module
        else:
            assert type(error) is ImportError
            assert str(error) == "module " + repr(name) + " not in sys.modules"
            assert error.name == name
            if mode == "delete":
                assert name not in sys.modules
            else:
                assert sys.modules[name] is (
                    None if mode == "none" else finder.replacement
                )
        print("reload-discovery", mode, True)
    finally:
        sys.meta_path.remove(finder)
        sys.modules.pop(name, None)


for reload_mode in (
    "same",
    "replace",
    "none",
    "delete",
    "fail",
    "replace-fail",
    "delete-fail",
):
    run_reload_transaction(reload_mode)
for reload_mode in ("replace", "none", "delete", "missing"):
    run_reload_discovery(reload_mode)


class ReentrantReload:
    def __init__(self, name, phase, replacement, fail):
        self.name = name
        self.phase = phase
        self.replace = replacement
        self.fail = fail
        self.find_calls = 0
        self.exec_calls = 0
        self.failure = ValueError("outer reload failed")
        self.fail_in_finder = False
        self.module = types.ModuleType(name)
        self.module.__spec__ = importlib.machinery.ModuleSpec(name, self)
        self.replacement = types.ModuleType(name)
        self.replacement.__spec__ = importlib.machinery.ModuleSpec(name, self)
        self.spec = importlib.machinery.ModuleSpec(name, self)

    def reenter(self, phase):
        if phase != self.phase:
            return
        if self.replace:
            sys.modules[self.name] = self.replacement
            try:
                importlib.reload(self.module)
            except ImportError as exc:
                assert str(exc) == "module " + self.name + " not in sys.modules"
                assert exc.name == self.name
            else:
                raise AssertionError("membership precedes active reload lookup")
        argument = self.replacement if self.replace else self.module
        try:
            assert importlib.reload(argument) is self.module
        finally:
            sys.modules[self.name] = self.module

    def find_spec(self, fullname, path=None, target=None):
        if fullname != self.name:
            return None
        self.find_calls += 1
        assert target is self.module
        if self.find_calls == 1:
            self.reenter("finder")
        if self.fail and self.fail_in_finder:
            raise self.failure
        return self.spec

    def create_module(self, spec):
        raise AssertionError("reload must preserve its target")

    def exec_module(self, module):
        self.exec_calls += 1
        assert module is self.module
        if self.exec_calls == 1:
            self.reenter("body")
        if self.fail:
            raise self.failure


def run_reentrant_reload(phase, replacement, fail):
    name = "molt_reload_reentrant_" + phase + str(replacement) + str(fail)
    owner = ReentrantReload(
        name, "finder" if phase == "finder-error" else phase, replacement, fail
    )
    owner.fail_in_finder = phase == "finder-error"
    sys.modules[name] = owner.module
    sys.meta_path.insert(0, owner)
    try:
        error = None
        try:
            assert importlib.reload(owner.module) is owner.module
        except BaseException as exc:
            error = exc
        assert error is (owner.failure if fail else None)
        first_exec_calls = 0 if owner.fail_in_finder and fail else 1
        assert owner.find_calls == 1 and owner.exec_calls == first_exec_calls
        # Either exit must retire the active marker. A later reload executes.
        owner.fail = False
        assert importlib.reload(owner.module) is owner.module
        assert owner.find_calls == 2 and owner.exec_calls == first_exec_calls + 1
        print("reload-reentry", phase, replacement, fail, True)
    finally:
        sys.meta_path.remove(owner)
        sys.modules.pop(name, None)


for reentry_phase in ("finder", "body"):
    for reentry_replacement in (False, True):
        for reentry_failure in (False, True):
            run_reentrant_reload(reentry_phase, reentry_replacement, reentry_failure)
run_reentrant_reload("finder-error", True, True)


class PublicationNameLoader:
    def __init__(self, final_name):
        self.final_name = final_name
        self.replacement = object()
        self.seen = None

    def create_module(self, spec):
        return None

    def exec_module(self, module):
        self.seen = module
        module.__spec__.name = self.final_name
        sys.modules[self.final_name] = self.replacement
        sys.modules[self.final_name + ":after"] = object()


def run_publication_name(mode):
    name = "molt_publication_name_" + mode
    final_name = name + "_other"
    module = types.ModuleType(name)
    module.__spec__ = importlib.machinery.ModuleSpec(name, object())
    loader = PublicationNameLoader(final_name)
    spec = importlib.machinery.ModuleSpec(
        final_name if mode == "selected" else name, loader
    )
    finder = ReloadFinder(name, spec, "same")
    if mode != "fresh":
        sys.modules[name] = module
    if mode == "selected":
        sys.modules[final_name] = module
    sys.meta_path.insert(0, finder)
    try:
        if mode == "fresh":
            assert importlib.import_module(name) is loader.replacement
            module = loader.seen
        else:
            assert importlib.reload(module) is module
        assert loader.seen is module and module.__spec__ is spec
        assert spec.name == final_name
        assert sys.modules[name] is module
        assert sys.modules[final_name] is loader.replacement
        keys = list(sys.modules)
        assert keys.index(final_name) > keys.index(final_name + ":after")
        assert not getattr(spec, "_initializing", False)
        print("publication-name", mode, True)
    finally:
        sys.meta_path.remove(finder)
        sys.modules.pop(name, None)
        sys.modules.pop(final_name, None)
        sys.modules.pop(final_name + ":after", None)


for publication_mode in ("selected", "renamed", "fresh"):
    run_publication_name(publication_mode)
