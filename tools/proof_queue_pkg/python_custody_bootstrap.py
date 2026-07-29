#!/usr/bin/env python3
"""Install proof custody before faithfully dispatching a Python payload."""

from __future__ import annotations

import builtins
import importlib.machinery
import importlib.util
import os
from pathlib import Path
import runpy
import sys
import types
import zipfile


def _install_custody() -> None:
    authority = Path(__file__).with_name("execution_custody.py").resolve(strict=True)
    spec = importlib.util.spec_from_file_location(
        "_molt_proof_execution_custody", authority
    )
    if spec is None or spec.loader is None:
        raise RuntimeError("proof execution custody authority cannot be loaded")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    module.install_python_child_custody()


def _reset_import_path(mode: str, target: str | None) -> Path | None:
    bootstrap_dir = Path(__file__).resolve(strict=True).parent
    bootstrap_norm = os.path.normcase(str(bootstrap_dir))
    sys.path[:] = [
        value
        for value in sys.path
        if os.path.normcase(os.path.abspath(value)) != bootstrap_norm
    ]
    if sys.flags.safe_path:
        return (
            Path(os.path.abspath(target)).resolve(strict=True)
            if mode == "script" and target is not None
            else None
        )
    if mode in {"command", "stdin"}:
        sys.path.insert(0, "")
        return None
    if mode == "module":
        sys.path.insert(0, os.path.abspath(os.curdir))
        return None
    if mode == "script" and target is not None:
        resolved = Path(os.path.abspath(target)).resolve(strict=True)
        # runpy owns sys.path[0] for directory and zip applications.  Adding
        # the target here would duplicate it during __main__ execution.
        if not resolved.is_dir() and not zipfile.is_zipfile(resolved):
            sys.path.insert(0, str(resolved.parent))
        return resolved
    raise RuntimeError(f"unknown Python bootstrap mode {mode!r}")


def _fresh_main(*, filename: str | None, package: str | None) -> dict[str, object]:
    module = types.ModuleType("__main__")
    namespace = module.__dict__
    namespace.update(
        {
            "__annotations__": {},
            "__builtins__": builtins,
            "__cached__": None,
            "__doc__": None,
            "__loader__": (
                importlib.machinery.BuiltinImporter
                if filename is None
                else importlib.machinery.SourceFileLoader("__main__", filename)
            ),
            "__package__": package,
            "__spec__": None,
        }
    )
    if filename is not None:
        namespace["__file__"] = filename
    sys.modules["__main__"] = module
    return namespace


def _run_command(command: str, arguments: list[str]) -> None:
    sys.argv[:] = ["-c", *arguments]
    namespace = _fresh_main(filename=None, package=None)
    exec(compile(command, "<string>", "exec"), namespace, namespace)


def _run_stdin(arguments: list[str]) -> None:
    sys.argv[:] = ["-", *arguments]
    namespace = _fresh_main(filename=None, package=None)
    source = sys.stdin.buffer.read()
    exec(compile(source, "<stdin>", "exec"), namespace, namespace)


def _run_module(module: str, arguments: list[str]) -> None:
    sys.argv[:] = [module, *arguments]
    _fresh_main(filename=None, package=None)
    runpy._run_module_as_main(module, alter_argv=True)


def _run_script(target: str, arguments: list[str], *, skip_first_line: bool) -> None:
    resolved = Path(os.path.abspath(target)).resolve(strict=True)
    sys.argv[:] = [target, *arguments]
    if resolved.is_dir() or zipfile.is_zipfile(resolved):
        runpy.run_path(str(resolved), run_name="__main__")
        return
    source = resolved.read_bytes()
    if source.startswith(importlib.util.MAGIC_NUMBER):
        if skip_first_line:
            raise RuntimeError("Python -x cannot execute a bytecode payload")
        runpy.run_path(str(resolved), run_name="__main__")
        return
    if skip_first_line:
        _first, separator, source = source.partition(b"\n")
        if not separator:
            source = b""
    namespace = _fresh_main(filename=str(resolved), package=None)
    exec(compile(source, str(resolved), "exec"), namespace, namespace)


def main() -> None:
    if len(sys.argv) < 3:
        raise SystemExit(
            "usage: python_custody_bootstrap.py MODE SKIP_FIRST_LINE [TARGET] [ARG ...]"
        )
    mode = sys.argv[1]
    skip_first_line = sys.argv[2] == "1"
    if sys.argv[2] not in {"0", "1"}:
        raise RuntimeError("malformed Python bootstrap skip-first-line authority")
    target_required = mode in {"command", "module", "script"}
    if target_required and len(sys.argv) < 4:
        raise RuntimeError(f"Python bootstrap mode {mode!r} has no target")
    target = sys.argv[3] if target_required else None
    arguments = sys.argv[4:] if target_required else sys.argv[3:]

    _install_custody()
    _reset_import_path(mode, target)
    if mode == "command" and target is not None:
        _run_command(target, arguments)
    elif mode == "module" and target is not None:
        _run_module(target, arguments)
    elif mode == "script" and target is not None:
        _run_script(target, arguments, skip_first_line=skip_first_line)
    elif mode == "stdin":
        _run_stdin(arguments)
    else:
        raise RuntimeError(f"unknown Python bootstrap mode {mode!r}")


if __name__ == "__main__":
    main()
