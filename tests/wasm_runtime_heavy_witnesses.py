"""Actual heavy-WASM inputs shared by preparation and semantic execution.

Only source/build coordinates live here. Runtime features and identities remain
derived by the normal compiler from final IR; expected results stay in tests.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
import sys
import textwrap
from typing import Literal

WasmWitnessLayout = Literal["run", "linked", "split"]
WasmWitnessName = Literal[
    "syntax", "asyncio", "finder", "final-tir", "zipimport", "smtplib", "missing-grants"
]


def syntax_error_target_arguments(target: str) -> list[str]:
    return [
        "--target",
        target,
        "--python-version",
        f"{sys.version_info.major}.{sys.version_info.minor}",
    ]


def syntax_error_run_arguments(source: Path, *, target: str) -> list[str]:
    return ["run", "--json", *syntax_error_target_arguments(target), str(source)]


def materialize_syntax_error_payload(tmp_path: Path) -> tuple[Path, int]:
    sources = {
        "bad_syntax": b"def broken(:\n    pass\n",
        "bad_indent": b"if True:\npass\n",
        "bad_tabs": b"if True:\n\tpass\n        pass\n",
        "bad_cookie": b"# coding: not-a-real-encoding\nvalue = 1\n",
        "bad_bytes": b"# coding: utf-8\nvalue = '\xff'\n",
        "bad_bom_bytes": b"\xef\xbb\xbfvalue = '\xff'\n",
        "bad_bytes_crlf": b"# coding: utf-8\r\nvalue = '\xff'\r\n",
        "bad_unicode": "value = ('α',\n         ) +\n".encode("utf-8"),
        "good_cookie": b"# coding: latin-1\nvalue = 'caf\xe9'\n",
    }
    for name, content in sources.items():
        (tmp_path / f"{name}.py").write_bytes(content)
    script = tmp_path / "entry.py"
    imports = "\n".join(
        f"try:\n    import {name}\nexcept saved_syntax as error:\n    report(error)"
        for name in sources
        if name != "good_cookie"
    )
    script.write_text(
        "import builtins\n"
        "saved_syntax = builtins.SyntaxError\n"
        "saved_indent = builtins.IndentationError\n"
        "saved_tabs = builtins.TabError\n"
        "def report(error):\n"
        "    print(type(error).__name__, error.args, error.msg, error.filename, "
        "error.lineno, error.offset, repr(error.text), error.end_lineno, "
        "error.end_offset, error.print_file_and_line, str(error))\n"
        "builtins.SyntaxError = ValueError\n"
        "builtins.IndentationError = ValueError\n"
        "builtins.TabError = ValueError\n" + imports + "\n"
        "builtins.SyntaxError = saved_syntax\n"
        "builtins.IndentationError = saved_indent\n"
        "builtins.TabError = saved_tabs\n"
        "import good_cookie\nprint('valid', good_cookie.value)\n",
        encoding="utf-8",
    )
    return script, len(sources)


def asyncio_source(root: Path, tmp_path: Path) -> Path:
    src = tmp_path / "wasm_asyncio_task_basic.py"
    src.write_text(
        textwrap.dedent(
            """\
            import asyncio
            import contextvars

            var = contextvars.ContextVar("var", default="none")


            async def child() -> int:
                print("child", var.get())
                print(asyncio.current_task() is not None)
                var.set("child")
                await asyncio.sleep(0)
                print("child2", var.get())
                return 5


            async def main() -> None:
                var.set("main")
                task = asyncio.create_task(child())
                var.set("main2")
                res = await task
                print("res", res)
                print("main", var.get())


            asyncio.run(main())
            """
        ),
        encoding="utf-8",
    )
    return src


def finder_source(root: Path, tmp_path: Path) -> Path:
    (tmp_path / "_molt_wasm_catalog_subject.py").write_text(
        "print('subject-exec')\nvalue = 297\n", encoding="utf-8"
    )
    src = tmp_path / "catalog_probe.py"
    src.write_text(
        "import importlib.machinery\n"
        "import importlib.util\n"
        "import sys\n"
        "print('package-metadata', importlib.__path__ is\n"
        "      importlib.__spec__.submodule_search_locations)\n"
        "name = '_molt_wasm_catalog_subject'\n"
        "print('absent', name not in sys.modules)\n"
        "spec = importlib.util.find_spec(name)\n"
        "print('spec', spec.name == name,\n"
        "      spec.loader is importlib.machinery._MOLT_LOADER)\n"
        "print('unpublished', name not in sys.modules)\n"
        "import _molt_wasm_catalog_subject\n"
        "print('ready', _molt_wasm_catalog_subject.value == 297)\n"
        + (
            root
            / "tests/differential/stdlib/importlib_find_spec_invalidate_caches_intrinsic.py"
        ).read_text(encoding="utf-8"),
        encoding="utf-8",
    )
    return src


def final_tir_source(root: Path, tmp_path: Path) -> Path:
    (tmp_path / "_molt_facts_unused.py").write_text(
        "print('unused-init')\n", encoding="utf-8"
    )
    (tmp_path / "_molt_facts_from.py").write_text(
        "print('from-init')\ntoken = 47\n", encoding="utf-8"
    )
    (tmp_path / "_molt_facts_subject.py").write_text(
        "def scalar(flag, value):\n"
        "    if flag:\n"
        "        result = value + 2\n"
        "    else:\n"
        "        result = value - 3\n"
        "    return result * 3\n"
        "def checked(value):\n"
        "    return value + 1\n"
        "def make_escaped(base):\n"
        "    def escaped(value):\n"
        "        return base + value\n"
        "    return escaped\n",
        encoding="utf-8",
    )
    src = tmp_path / "facts_probe.py"
    src.write_text(
        "from _molt_facts_subject import scalar, checked, make_escaped\n"
        "def load_unused():\n"
        "    import _molt_facts_unused as unused\n"
        "    from _molt_facts_from import token as unused_token\n"
        "load_unused()\n"
        "print('branch', scalar(True, 7), scalar(False, 7))\n"
        "print('checked', checked((1 << 46) - 1), checked((1 << 63) - 1))\n"
        "escaped = make_escaped(11)\n"
        "print('escaped', escaped(2), callable(escaped))\n",
        encoding="utf-8",
    )
    return src


def zipimport_source(root: Path, tmp_path: Path) -> Path:
    src = tmp_path / "wasm_zipimport_basic.py"
    src.write_text(
        textwrap.dedent(
            """\
            import os
            import tempfile
            import zipfile
            import zipimport

            root = tempfile.mkdtemp()
            zip_path = os.path.join(root, "pkg.zip")
            with zipfile.ZipFile(zip_path, "w", compression=zipfile.ZIP_DEFLATED) as zf:
                zf.writestr("pkg/__init__.py", "value = 1")
                zf.writestr("pkg/mod.py", "value = 2")
                zf.writestr("nested/m.py", "value = 3")

            importer = zipimport.zipimporter(zip_path)
            # Discovery does not execute arbitrary runtime-created source.
            # CPython resolves each fullname relative to this importer's prefix.
            print("package", importer.is_package("pkg"), importer.get_source("pkg"))
            package_importer = zipimport.zipimporter(f"{zip_path}/pkg")
            print(
                "child",
                package_importer.is_package("pkg.mod"),
                package_importer.get_source("pkg.mod"),
            )
            subimporter = zipimport.zipimporter(f"{zip_path}/nested")
            print("nested", subimporter.is_package("m"), subimporter.get_source("m"))

            # The archive-root importer searches mod.py, not pkg/mod.py.
            importer.load_module("pkg.mod")
            """
        ),
        encoding="utf-8",
    )
    return src


def smtplib_source(root: Path, tmp_path: Path) -> Path:
    src = tmp_path / "wasm_smtplib_thread_gate.py"
    src.write_text(
        textwrap.dedent(
            """\
            import smtplib
            import socketserver
            import threading


            class Handler(socketserver.StreamRequestHandler):
                def handle(self):
                    self.wfile.write(b"220 ready\\r\\n")


            server = socketserver.TCPServer(("127.0.0.1", 0), Handler)
            thread = threading.Thread(target=server.serve_forever, daemon=True)

            try:
                thread.start()
                print("unexpected-thread-success")
            except Exception as exc:
                print(type(exc).__name__)
                print(str(exc))
                print("caught")
            finally:
                server.server_close()

            _ = smtplib.SMTP
            """
        ),
        encoding="utf-8",
    )
    return src


def missing_grants_source(root: Path, tmp_path: Path) -> Path:
    src = tmp_path / "wasm_operation_permissions.py"
    src.write_text(
        textwrap.dedent(
            """\
            from _intrinsics import require_intrinsic

            require_operation = require_intrinsic("molt_capabilities_require_operation")
            try:
                with open("destination.bin", "w+b") as destination:
                    with open("source.bin", "rb") as source:
                        destination.write(source.read())
            except PermissionError as exc:
                print(type(exc).__name__, str(exc))
            else:
                print("copied")

            for name in ("unknown.operation", b"io.open.readwrite"):
                try:
                    require_operation(name)
                except (ValueError, TypeError) as exc:
                    print(type(exc).__name__, str(exc))
                else:
                    print("unexpected operation admission")
            """
        ),
        encoding="utf-8",
    )
    return src


def syntax_source(root: Path, tmp_path: Path) -> Path:
    return materialize_syntax_error_payload(tmp_path)[0]


@dataclass(frozen=True)
class HeavyWasmWitness:
    name: WasmWitnessName
    materialize: Callable[[Path, Path], Path]
    layout: WasmWitnessLayout = "linked"
    extra_args: tuple[str, ...] = ()
    slow: bool = False

    def mark_test(self, test: Callable[..., None]) -> Callable[..., None]:
        """Project the same selection fact used by compiled preparation."""
        if self.slow:
            import pytest

            return pytest.mark.slow(test)
        return test


HEAVY_WASM_WITNESSES = (
    HeavyWasmWitness("syntax", syntax_source, "run"),
    HeavyWasmWitness("asyncio", asyncio_source),
    HeavyWasmWitness("finder", finder_source, extra_args=("--no-trusted",)),
    HeavyWasmWitness(
        "finder", finder_source, "split", ("--build-profile", "dev", "--no-trusted")
    ),
    HeavyWasmWitness("final-tir", final_tir_source, extra_args=("--no-trusted",)),
    HeavyWasmWitness(
        "final-tir",
        final_tir_source,
        "split",
        ("--build-profile", "dev", "--no-trusted"),
    ),
    # Deployment manifest v4 makes the build's launch policy authoritative:
    # a host overwrites run-time MOLT_CAPABILITIES with it, so grants are
    # build arguments, shared with compiled preparation.
    HeavyWasmWitness(
        "zipimport",
        zipimport_source,
        extra_args=("--capabilities", "fs.read,fs.write,env.read"),
    ),
    HeavyWasmWitness(
        "smtplib", smtplib_source, extra_args=("--capabilities", "net,env.read")
    ),
    HeavyWasmWitness("missing-grants", missing_grants_source, slow=True),
)


def heavy_wasm_witness(
    name: WasmWitnessName, layout: WasmWitnessLayout = "linked"
) -> HeavyWasmWitness:
    for witness in HEAVY_WASM_WITNESSES:
        if witness.name == name and witness.layout == layout:
            return witness
    raise ValueError(f"Unknown heavy WASM witness coordinate: {name!r}/{layout!r}")
