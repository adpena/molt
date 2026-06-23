"""Purpose: validate Rust-owned importlib loader exec_module transactions."""

import importlib.machinery
import importlib.util
import os
import sys
import tempfile
import zipfile


def run_source_loader(root: str) -> None:
    path = os.path.join(root, "txsource.py")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write("value = 101\n")

    spec = importlib.util.spec_from_file_location("txsource", path)
    module = importlib.util.module_from_spec(spec) if spec is not None else None
    if spec is not None and spec.loader is not None and module is not None:
        sys.modules.pop("txsource", None)
        spec.loader.exec_module(module)

    print(module is not None and getattr(module, "value", None) == 101)
    print(module is not None and module.__loader__ is spec.loader)
    print(module is not None and module.__file__ == path)
    print(module is not None and module.__package__ == "")


def run_source_loader_mutation(root: str) -> None:
    path = os.path.join(root, "txsource_mutates.py")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(
            "value = 404\n"
            "__loader__ = 'mutated-loader'\n"
            "__file__ = 'mutated-file'\n"
            "__package__ = 'mutated-package'\n"
        )

    spec = importlib.util.spec_from_file_location("txsource_mutates", path)
    module = importlib.util.module_from_spec(spec) if spec is not None else None
    if spec is not None and spec.loader is not None and module is not None:
        sys.modules.pop("txsource_mutates", None)
        spec.loader.exec_module(module)

    print(module is not None and getattr(module, "value", None) == 404)
    print(module is not None and module.__loader__ == "mutated-loader")
    print(module is not None and module.__file__ == "mutated-file")
    print(module is not None and module.__package__ == "mutated-package")


def run_zip_source_loader(root: str) -> None:
    archive = os.path.join(root, "txmods.zip")
    with zipfile.ZipFile(archive, "w") as zf:
        zf.writestr("ziptx.py", "value = 202\n")

    orig_path = list(sys.path)
    try:
        sys.path[:] = [archive]
        spec = importlib.util.find_spec("ziptx")
        module = importlib.util.module_from_spec(spec) if spec is not None else None
        if spec is not None and spec.loader is not None and module is not None:
            sys.modules.pop("ziptx", None)
            spec.loader.exec_module(module)
    finally:
        sys.path[:] = orig_path
        sys.modules.pop("ziptx", None)

    print(spec is not None and spec.loader is not None)
    print(module is not None and getattr(module, "value", None) == 202)
    print(
        module is not None
        and isinstance(module.__file__, str)
        and module.__file__.endswith("txmods.zip/ziptx.py")
    )


def run_extension_loader(root: str) -> None:
    ext_path = os.path.join(root, "txext.so")
    with open(ext_path, "wb") as handle:
        handle.write(b"")
    with open(f"{ext_path}.py", "w", encoding="utf-8") as handle:
        handle.write("value = 303\n")

    loader = importlib.machinery.ExtensionFileLoader("txext", ext_path)
    spec = importlib.util.spec_from_file_location("txext", ext_path, loader=loader)

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
    run_source_loader(tmp)
    run_source_loader_mutation(tmp)
    run_zip_source_loader(tmp)
    run_extension_loader(tmp)
    run_sourceless_loader(tmp)
