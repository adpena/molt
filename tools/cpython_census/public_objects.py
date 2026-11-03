"""Observe public object identities in isolated, versioned CPython processes.

Run this file with the selected oracle interpreter. The module inventory comes
from the shipped source paths, never from a scraper of intrinsic assignments.
Import failures are evidence, not empty successful module surfaces. Each module
gets a fresh process so imports cannot add attributes to earlier observations.
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
import inspect
import json
from pathlib import Path
import sys
import tempfile

_REPO_ROOT = Path(__file__).resolve().parents[2]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.command_execution import CommandExecutor  # noqa: E402


def identities(module):
    names = set(name for name in dir(module) if not name.startswith("_"))
    names.update(getattr(module, "__all__", ()))
    if module.__name__ == "builtins":
        names.update(("__build_class__", "__import__", "__debug__"))
    if module.__name__ == "copyreg":
        names.update(("__newobj__", "__newobj_ex__", "_reconstructor", "_reduce_ex"))
    rows = {}
    for name in sorted(names):
        try:
            value = getattr(module, name)
            row = {"kind": type(value).__name__}
            for attribute in ("__name__", "__qualname__", "__module__"):
                item = getattr(value, attribute, None)
                row[attribute] = item if isinstance(item, str) else None
            rows[name] = row
        except Exception as error:
            rows[name] = {"error": type(error).__name__ + ": " + str(error)}
    return rows


def shipped_modules(root):
    modules = set()
    for path in root.rglob("*.py"):
        relative = path.relative_to(root).with_suffix("")
        parts = relative.parts
        if parts[-1] == "__init__":
            parts = parts[:-1]
        if parts:
            modules.add(".".join(parts))
    return sorted(modules)


def exclusion(module):
    # These modules execute commands or launch an external browser on import.
    if module in (
        "antigravity",
        "this",
        "__hello__",
        "__phello__",
        "idlelib.idle",
    ) or module.endswith(".__main__"):
        return "import executes an application or has external effects"
    if module.split(".")[0] not in sys.stdlib_module_names:
        return "not a CPython stdlib module in this interpreter"
    return None


def probe_source(modules, *, host=False):
    """One observation algorithm with each runtime's normal builtin startup."""
    import textwrap

    source = "import json\n" + inspect.getsource(identities) + "\n_census = {}\n"
    if host:
        # -S excludes editable installs and user/site packages. Restore only
        # CPython's normal builtin objects through their canonical publisher;
        # compiled Molt already publishes these six objects at runtime init.
        source += (
            "import site as _site\n"
            "_site.setquit()\n_site.setcopyright()\n_site.sethelper()\n"
        )
    selected = len(modules) > 1
    if selected:
        source += (
            "import sys\n"
            "_selection = sys.argv[1] if len(sys.argv) > 1 else None\n"
            f"if _selection is not None and _selection not in {tuple(modules)!r}:\n"
            "    raise ValueError('module is outside the compiled census inventory')\n"
        )
    for module in modules:
        if not all(part.isidentifier() for part in module.split(".")):
            raise ValueError("probe module must be a dotted identifier")
        observation = (
            f"try:\n    import {module} as _census_module\n"
            f"    _census[{module!r}] = {{'objects': identities(_census_module)}}\n"
            "except BaseException as _error:\n"
            f"    _census[{module!r}] = {{'error': type(_error).__name__ + ': ' + str(_error)}}\n"
        )
        source += (
            f"if _selection is None or _selection == {module!r}:\n"
            + textwrap.indent(observation, "    ")
            if selected
            else observation
        )
    return source + 'print(json.dumps({"modules": _census}, sort_keys=True))\n'


def observe(module, commands):
    import subprocess

    reason = exclusion(module)
    if reason is not None:
        return module, {"excluded": reason}
    # A real file gives both runtimes the same bootstrap, including json's
    # imports. Importing this driver in the oracle would preload argparse,
    # inspect and contextlib and fabricate extra namespace attributes.
    with tempfile.TemporaryDirectory(prefix="molt-public-census-") as temporary:
        source = Path(temporary) / "probe.py"
        source.write_text(probe_source([module], host=True), encoding="utf-8")
        try:
            child = commands.run(
                [sys.executable, "-I", "-S", str(source)],
                capture_output=True,
                text=True,
                encoding="utf-8",
                timeout=30,
                check=False,
            )
        except subprocess.TimeoutExpired:
            return module, {"unverified": "oracle command timed out after 30 seconds"}
    if child.returncode:
        return module, {
            "error": "oracle child exit " + str(child.returncode),
            "stderr": child.stderr,
        }
    try:
        # Keep import diagnostics separate from the final observation line.
        return module, json.loads(child.stdout.splitlines()[-1])["modules"][module]
    except (IndexError, KeyError, TypeError, json.JSONDecodeError):
        return module, {"error": "oracle emitted no census", "stdout": child.stdout}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--module", action="append")
    parser.add_argument(
        "--stdlib-root",
        type=Path,
        default=Path(__file__).resolve().parents[2] / "src/molt/stdlib",
    )
    parser.add_argument("--output", type=Path)
    parser.add_argument(
        "--progress", type=Path, help="private per-module JSONL receipt"
    )
    parser.add_argument(
        "--fixture",
        action="store_true",
        help="omit machine-local executable and build paths",
    )
    parser.add_argument(
        "--emit-probe",
        action="append",
        help="write a standalone statically imported Molt census consumer (repeat for a batch)",
    )
    args = parser.parse_args()
    if args.emit_probe:
        if not all(
            part.isidentifier()
            for module in args.emit_probe
            for part in module.split(".")
        ):
            parser.error("probe module must be a dotted identifier")
        source = probe_source(args.emit_probe)
        if not args.output:
            parser.error("--emit-probe requires --output")
        args.output.write_text(source, encoding="utf-8")
        return
    modules = args.module or shipped_modules(args.stdlib_root)
    # Oracle children execute standalone probes and never import this driver.
    commands = CommandExecutor.for_file(__file__, stem="MOLT_PROOF")
    from concurrent.futures import as_completed

    observations = {}
    progress = args.progress.open("w", encoding="utf-8") if args.progress else None
    try:
        with ThreadPoolExecutor(max_workers=4) as executor:
            futures = [executor.submit(observe, module, commands) for module in modules]
            try:
                for future in as_completed(futures):
                    module, observation = future.result()
                    observations[module] = observation
                    if progress is not None:
                        progress.write(json.dumps({module: observation}) + "\n")
                        progress.flush()
            except BaseException:
                for future in futures:
                    future.cancel()
                raise
    finally:
        if progress is not None:
            progress.close()
    result = {
        "implementation": sys.implementation.name,
        "version": list(sys.version_info[:3]) if args.fixture else sys.version,
        "platform": sys.platform,
        "modules": observations,
    }
    if not args.fixture:
        result["executable"] = sys.executable
    encoded = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded, encoding="utf-8")
    else:
        print(encoded, end="")


if __name__ == "__main__":
    main()
