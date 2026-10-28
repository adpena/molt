"""Purpose: validate Rust-owned importlib loader exec_module transactions."""

import _frozen_importlib
import builtins
import importlib._bootstrap
import importlib.machinery
import importlib.util
import os
import sys
import tempfile
import types
import zipfile


def _admit_compiled_loader_module() -> None:
    import runpy_compiled_fixture  # noqa: F401 - static admission root


module_name = "runpy_compiled_fixture"
module_path = os.path.join(os.path.dirname(__file__), f"{module_name}.py")


def run_source_loader() -> None:
    spec = importlib.util.spec_from_file_location(module_name, module_path)
    module = importlib.util.module_from_spec(spec) if spec is not None else None
    if spec is not None and spec.loader is not None and module is not None:
        sys.modules.pop(module_name, None)
        spec.loader.exec_module(module)

    print(module is not None and getattr(module, "value", None) == 11)
    print(module is not None and module.__loader__ is spec.loader)
    print(module is not None and module.__file__ == module_path)
    print(module is not None and module.__package__ == "")


def run_create_module_identity_and_error() -> None:
    created = types.ModuleType("txcreated")

    class ReturningLoader:
        def create_module(self, _spec):
            return created

        def exec_module(self, _module):
            raise AssertionError("module_from_spec must not execute the module")

    class FailingLoader:
        def create_module(self, _spec):
            raise ValueError("create-phase-marker")

        def exec_module(self, _module):
            raise AssertionError("failed creation must not execute the module")

    created_spec = importlib.machinery.ModuleSpec("txcreated", ReturningLoader())
    failed_spec = importlib.machinery.ModuleSpec("txfailed", FailingLoader())
    print(importlib.util.module_from_spec(created_spec) is created)
    try:
        importlib.util.module_from_spec(failed_spec)
    except ValueError as exc:
        print(str(exc) == "create-phase-marker")
    else:
        print(False)


def run_factory_metadata_contract() -> None:
    created = types.ModuleType("factory-owned-name")
    created.__loader__ = "factory-owned-loader"
    created.__package__ = "factory-owned-package"
    created.__path__ = ["factory-owned-path"]
    created.__file__ = "factory-owned-file"
    created.__cached__ = "factory-owned-cache"
    created.__spec__ = "superseded-spec"
    original_path = created.__path__

    class FactoryLoader:
        def create_module(self, _spec):
            return created

        def exec_module(self, _module):
            raise AssertionError("metadata initialization must not execute the body")

    spec = importlib.machinery.ModuleSpec(
        "factory_candidate", FactoryLoader(), origin="new-location", is_package=True
    )
    spec.has_location = True
    spec.cached = "new-cache"
    result = importlib.util.module_from_spec(spec)
    print(result is created)
    print(result.__name__ == "factory-owned-name")
    print(result.__loader__ == "factory-owned-loader")
    print(result.__package__ == "factory-owned-package")
    print(result.__path__ is original_path)
    print(result.__file__ == "factory-owned-file")
    print(result.__cached__ == "factory-owned-cache")
    print(result.__spec__ is spec)


def run_spec_location_and_path_identity() -> None:
    class DefaultLoader:
        def create_module(self, _spec):
            return None

        def exec_module(self, _module):
            raise AssertionError("module_from_spec must not execute the body")

    spec = importlib.machinery.ModuleSpec(
        "descriptive_origin",
        DefaultLoader(),
        origin="descriptive-only",
        is_package=True,
    )
    spec.submodule_search_locations = ("package-root",)
    module = importlib.util.module_from_spec(spec)
    print(module.__path__ is spec.submodule_search_locations)
    print(module.__package__ == "descriptive_origin")
    print(not hasattr(module, "__file__"))
    print(not hasattr(module, "__cached__"))

    located = importlib.machinery.ModuleSpec(
        "located_without_cache", DefaultLoader(), origin="descriptive-only"
    )
    located.has_location = True
    located.cached = None
    module = importlib.util.module_from_spec(located)
    print(module.__file__ == located.origin)
    print(not hasattr(module, "__cached__"))




def run_metadata_assignment_errors() -> None:
    marker = ValueError("metadata-assignment-marker")

    class RejectingModule(types.ModuleType):
        def __setattr__(self, name, value):
            if name == "__spec__":
                if self.reject_with_attribute_error:
                    raise AttributeError("metadata is read-only")
                raise marker
            super().__setattr__(name, value)

    for ignore_attribute_error in (True, False):
        created = RejectingModule("assignment-target")
        created.reject_with_attribute_error = ignore_attribute_error

        class FactoryLoader:
            def create_module(self, _spec):
                return created

            def exec_module(self, _module):
                raise AssertionError(
                    "metadata initialization must not execute the body"
                )

        spec = importlib.machinery.ModuleSpec("assignment_candidate", FactoryLoader())
        try:
            result = importlib.util.module_from_spec(spec)
        except ValueError as error:
            print(not ignore_attribute_error and error is marker)
        else:
            print(ignore_attribute_error and result is created)


def run_namespace_and_factory_phase_contract() -> None:
    spec = importlib.machinery.ModuleSpec("namespace_candidate", None, is_package=True)
    spec.submodule_search_locations = ["namespace-root"]
    module = importlib.util.module_from_spec(spec)
    print(isinstance(spec.loader, importlib.machinery.NamespaceLoader))
    print(module.__loader__ is spec.loader)
    print(module.__path__ is spec.submodule_search_locations)
    print(module.__file__ is None)

    created = object()

    class ObjectFactory:
        def create_module(self, _spec):
            return created

        def exec_module(self, _module):
            raise AssertionError("module_from_spec must not execute the body")

    spec = importlib.machinery.ModuleSpec("object_candidate", ObjectFactory())
    print(importlib.util.module_from_spec(spec) is created)

    class MissingCreate:
        def exec_module(self, _module):
            raise AssertionError("failed creation must not execute the body")

    spec = importlib.machinery.ModuleSpec("invalid_loader_candidate", MissingCreate())
    try:
        importlib.util.module_from_spec(spec)
    except ImportError:
        print(True)
    else:
        print(False)


def run_factory_callback_observations() -> None:
    for callback in (None, 42):

        class NonCallableFactory:
            create_module = callback

            def exec_module(self, _module):
                raise AssertionError("creation must fail before execution")

        spec = importlib.machinery.ModuleSpec(
            "noncallable_factory", NonCallableFactory()
        )
        try:
            importlib.util.module_from_spec(spec)
        except TypeError as error:
            print(str(error) == f"'{type(callback).__name__}' object is not callable")
        else:
            print(False)

        class MissingFactory:
            exec_module = callback

        spec = importlib.machinery.ModuleSpec("missing_factory", MissingFactory())
        try:
            importlib.util.module_from_spec(spec)
        except ImportError as error:
            print(
                str(error)
                == "loaders that define exec_module() must also define create_module()"
            )
        else:
            print(False)

    reads = []
    calls = []
    created = types.ModuleType("observed_factory")

    class ObservedFactory:
        @property
        def create_module(self):
            reads.append("create")
            return self.first if len(reads) == 1 else self.second

        def first(self, _spec):
            calls.append("first")
            return None

        def second(self, _spec):
            calls.append("second")
            return created

    spec = importlib.machinery.ModuleSpec("observed_factory", ObservedFactory())
    result = importlib.util.module_from_spec(spec)
    print(result is created and reads == ["create", "create"] and calls == ["second"])

    loader_reads = []
    created.__loader__ = "factory-owned-loader"
    created.__package__ = ""
    created.__path__ = []

    class ProbeLoader:
        create_module = None

    class ActualLoader:
        def create_module(self, _spec):
            calls.append("actual-loader")
            return created

    class ChangingSpec:
        has_location = False

        @property
        def loader(self):
            loader_reads.append("loader")
            return ProbeLoader() if len(loader_reads) == 1 else ActualLoader()

    result = importlib.util.module_from_spec(ChangingSpec())
    print(
        result is created
        and loader_reads == ["loader", "loader"]
        and calls == ["second", "actual-loader"]
    )

    lifetime = []

    class TemporaryProbeLoader:
        create_module = None

        def __del__(self):
            lifetime.append("probe-released")

    class ReleasedProbeFactory:
        def create_module(self, _spec):
            lifetime.append("create")
            return created

    class TemporaryReceiverSpec:
        has_location = False

        @property
        def loader(self):
            lifetime.append("loader")
            if len(lifetime) == 1:
                return TemporaryProbeLoader()
            if lifetime != ["loader", "probe-released", "loader"]:
                raise AssertionError(
                    "presence receiver must retire before the next read"
                )
            return ReleasedProbeFactory()

    result = importlib.util.module_from_spec(TemporaryReceiverSpec())
    print(
        result is created
        and lifetime == ["loader", "probe-released", "loader", "create"]
    )

    marker = AttributeError("required-loader-marker")
    observations = []

    class MissingLoaderSpec:
        @property
        def loader(self):
            observations.append("loader")
            raise marker

        @property
        def name(self):
            raise AssertionError("loader receiver failure must precede module creation")

    try:
        importlib.util.module_from_spec(MissingLoaderSpec())
    except AttributeError as error:
        print(error is marker and observations == ["loader"])
    else:
        print(False)


def run_package_callback_observations() -> None:
    for callback in (None, 42):

        class NonCallablePackage:
            is_package = callback

        try:
            importlib.util.spec_from_loader(
                "noncallable_package", NonCallablePackage(), origin="descriptive"
            )
        except TypeError as error:
            print(str(error) == f"'{type(callback).__name__}' object is not callable")
        else:
            print(False)

    reads = []
    calls = []

    class ObservedPackage:
        @property
        def is_package(self):
            reads.append("is_package")
            return self.first if len(reads) == 1 else self.second

        def first(self, _name):
            calls.append("first")
            return False

        def second(self, name):
            calls.append(name)
            return True

    spec = importlib.util.spec_from_loader(
        "observed_package", ObservedPackage(), origin="descriptive"
    )
    print(
        spec.submodule_search_locations == []
        and reads == ["is_package", "is_package"]
        and calls == ["observed_package"]
    )

    for exception in (ImportError("undefined-package"), ValueError("package-marker")):

        class FailingPackage:
            def is_package(self, _name):
                raise exception

        try:
            spec = importlib.util.spec_from_loader(
                "failing_package", FailingPackage(), origin="descriptive"
            )
        except ValueError as error:
            print(error is exception)
        else:
            print(
                type(exception) is ImportError
                and spec.submodule_search_locations is None
            )

    truth_marker = ImportError("package-truth-marker")

    class PackageTruth:
        def __bool__(self):
            raise truth_marker

    class PackageWithTruth:
        def is_package(self, _name):
            return PackageTruth()

    try:
        importlib.util.spec_from_loader(
            "package_truth", PackageWithTruth(), origin="descriptive"
        )
    except ImportError as error:
        print(error is truth_marker)
    else:
        print(False)


def run_deadlock_exception_identity() -> None:
    error_type = importlib._bootstrap._DeadlockError
    error = error_type("module-cycle")
    print(
        error_type is _frozen_importlib._DeadlockError
        and error_type.__bases__ == (RuntimeError,)
        and error_type.__module__ == "_frozen_importlib"
        and not hasattr(builtins, "_DeadlockError")
        and not hasattr(builtins, "CancelledError")
        and not hasattr(builtins, "UnsupportedOperation")
        and isinstance(error, RuntimeError)
        and error.args == ("module-cycle",)
    )


def run_source_loader_mutation() -> None:
    spec = importlib.util.spec_from_file_location(module_name, module_path)
    module = importlib.util.module_from_spec(spec) if spec is not None else None
    if spec is not None and spec.loader is not None and module is not None:
        module.mutate_loader_metadata = True
        sys.modules.pop(module_name, None)
        spec.loader.exec_module(module)

    print(module is not None and getattr(module, "value", None) == 404)
    print(module is not None and module.__loader__ == "mutated-loader")
    print(module is not None and module.__file__ == "mutated-file")
    print(module is not None and module.__package__ == "mutated-package")


def run_zip_source_loader(root: str) -> None:
    archive = os.path.join(root, "txmods.zip")
    with zipfile.ZipFile(archive, "w") as zf:
        # The archive carries the same compiler-admitted body. Creating source
        # for a new module only at runtime is outside the compiled-body contract.
        with open(module_path, "rb") as source:
            zf.writestr(f"{module_name}.py", source.read())

    orig_path = list(sys.path)
    try:
        sys.path[:] = [archive]
        # Select the ZIP path protocol explicitly. Molt's default catalog
        # finder precedes PathFinder for standalone compiled admission.
        spec = importlib.machinery.PathFinder.find_spec(module_name, [archive])
        assert spec is not None and spec.origin.endswith(f"txmods.zip/{module_name}.py")
        module = importlib.util.module_from_spec(spec) if spec is not None else None
        if spec is not None and spec.loader is not None and module is not None:
            sys.modules.pop(module_name, None)
            spec.loader.exec_module(module)
    finally:
        sys.path[:] = orig_path
        sys.modules.pop(module_name, None)

    print(
        spec is not None
        and spec.loader is not None
        and module.__loader__ is spec.loader
        and module.__package__ == ""
    )
    print(module is not None and getattr(module, "value", None) == 11)
    print(
        module is not None
        and isinstance(module.__file__, str)
        and module.__file__.endswith(f"txmods.zip/{module_name}.py")
    )
    assert module.__file__.endswith(f"txmods.zip/{module_name}.py"), (
        module.__file__, spec.origin, type(spec.loader).__name__, module.file_seen,
    )


def run_extension_loader(root: str) -> None:
    ext_path = os.path.join(root, "txext.so")
    with open(ext_path, "wb") as handle:
        handle.write(b"")
    with open(f"{ext_path}.py", "w", encoding="utf-8") as handle:
        handle.write("value = 303\n")

    loader = importlib.machinery.ExtensionFileLoader("txext", ext_path)
    spec = importlib.util.spec_from_file_location("txext", ext_path, loader=loader)

    module = None
    loaded = False
    state_good = False
    error_name = "none"
    try:
        module = importlib.util.module_from_spec(spec) if spec is not None else None
        if spec is not None and spec.loader is not None and module is not None:
            spec.loader.exec_module(module)
            loaded = getattr(module, "value", None) == 303
            state_good = (
                module.__loader__ is loader
                and module.__file__ == ext_path
                and module.__package__ == ""
            )
    except BaseException as exc:
        error_name = exc.__class__.__name__

    print(loaded or error_name in {"ImportError", "PermissionError", "OSError"})
    print((not loaded) or state_good)


def run_sourceless_loader(root: str) -> None:
    pyc_path = os.path.join(root, "txbc.pyc")
    with open(pyc_path, "wb") as handle:
        handle.write(b"")
    with open(os.path.join(root, "txbc.molt.py"), "w", encoding="utf-8") as handle:
        handle.write("value = 404\n")

    loader = importlib.machinery.SourcelessFileLoader("txbc", pyc_path)
    spec = importlib.util.spec_from_file_location("txbc", pyc_path, loader=loader)

    loaded = False
    state_good = False
    error_name = "none"
    try:
        module = importlib.util.module_from_spec(spec) if spec is not None else None
        if spec is not None and spec.loader is not None and module is not None:
            spec.loader.exec_module(module)
            loaded = getattr(module, "value", None) == 404
            state_good = (
                module.__loader__ is loader
                and module.__file__ == pyc_path
                and module.__package__ == ""
            )
    except BaseException as exc:
        error_name = exc.__class__.__name__

    print(
        loaded
        or error_name
        in {"ImportError", "PermissionError", "OSError", "EOFError", "RuntimeError"}
    )
    print((not loaded) or state_good)


with tempfile.TemporaryDirectory(prefix="molt_loader_exec_tx_") as tmp:
    run_create_module_identity_and_error()
    run_factory_metadata_contract()
    run_spec_location_and_path_identity()
    run_metadata_assignment_errors()
    run_namespace_and_factory_phase_contract()
    run_factory_callback_observations()
    run_package_callback_observations()
    run_deadlock_exception_identity()
    run_source_loader()
    run_source_loader_mutation()
    run_zip_source_loader(tmp)
    run_extension_loader(tmp)
    run_sourceless_loader(tmp)
