from __future__ import annotations

import textwrap
from pathlib import Path

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def _assert_no_recursive_mutex_panic(stderr: str) -> None:
    assert "cannot recursively acquire mutex" not in stderr


def test_wasm_asyncio_task_basic_has_no_table_ref_trap(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
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

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm, env_overrides={"MOLT_RUNTIME_WASM": ""})
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip().splitlines() == [
        "child main",
        "True",
        "child2 child",
        "res 5",
        "main main2",
    ]
    assert "null function or function signature mismatch" not in run.stderr
    assert "__molt_table_ref_" not in run.stderr
    _assert_no_recursive_mutex_panic(run.stderr)


def test_wasm_zipimport_package_lookup_parity(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
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
            pkg = importer.load_module("pkg")
            mod = importer.load_module("pkg.mod")
            print(pkg.value)
            print(mod.value)

            subimporter = zipimport.zipimporter(f"{zip_path}/nested")
            submod = subimporter.load_module("m")
            print(submod.value)
            """
        ),
        encoding="utf-8",
    )

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(
        root,
        output_wasm,
        env_overrides={
            "MOLT_RUNTIME_WASM": "",
            "MOLT_CAPABILITIES": "fs.read,fs.write,env.read",
        },
    )
    assert run.returncode != 0
    assert "ZipImportError: can't find module 'pkg.mod'" in run.stderr
    _assert_no_recursive_mutex_panic(run.stderr)


def test_wasm_smtplib_thread_dependent_paths_fail_fast(
    tmp_path: Path,
) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
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

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(
        root,
        output_wasm,
        env_overrides={
            "MOLT_RUNTIME_WASM": "",
            "MOLT_CAPABILITIES": "net.bind,net.listen,net.outbound,env.read",
        },
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip().splitlines() == [
        "NotImplementedError",
        "threads are unavailable in wasm",
        "caught",
    ]
    _assert_no_recursive_mutex_panic(run.stderr)
