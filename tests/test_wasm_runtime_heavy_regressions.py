from __future__ import annotations

import os
import textwrap
from pathlib import Path

import pytest

from molt.node_runtime import resolve_node_runtime
from molt.wasm_artifact import wasm_runtime_manifest_path
from tests.wasm_linked_runner import (
    _run_wasm_test_process,
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


def _assert_no_recursive_mutex_panic(stderr: str) -> None:
    assert "cannot recursively acquire mutex" not in stderr, stderr


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
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip().splitlines() == [
        "child main",
        "True",
        "child2 child",
        "res 5",
        "main main2",
    ]
    assert "null function or function signature mismatch" not in run.stderr, run.stderr
    assert "__molt_table_ref_" not in run.stderr, run.stderr
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
            "MOLT_CAPABILITIES": "fs.read,fs.write,env.read",
        },
    )
    assert run.returncode != 0
    assert "ZipImportError: can't find module 'pkg.mod'" in run.stderr, run.stderr
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


@pytest.mark.slow
def test_wasm_open_reports_all_missing_grants_before_mutation(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    # This is an actual Node/WASI proof. Required tools fail selection/build;
    # a missing host must not turn the security assertion into a skipped test.
    node = resolve_node_runtime(source_root=root, guard_prefix="MOLT_WASM_TEST")
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
    output_wasm = build_wasm_linked(root, src, tmp_path)
    manifest = wasm_runtime_manifest_path(output_wasm)
    payload_dir = tmp_path / "payload"
    payload_dir.mkdir()
    source = payload_dir / "source.bin"
    destination = payload_dir / "destination.bin"
    payload = b"source payload\x00with binary bytes\xff"
    sentinel = b"destination must survive denied truncation"
    source.write_bytes(payload)
    command = [str(node.path), str(root / "wasm" / "run_wasm.js"), str(manifest)]
    for grants, missing in [
        ("", "fs.read, fs.write"),
        ("fs.read", "fs.write"),
        ("fs.write", "fs.read"),
        ("fs.read,fs.write", None),
    ]:
        destination.write_bytes(sentinel)
        env = os.environ.copy()
        # The explicit linked manifest owns runtime/table/bundle selection.
        for key in (
            "MOLT_WASM_TABLE_BASE",
            "MOLT_RUNTIME_WASM",
            "MOLT_WASM_MANIFEST_PATH",
            "MOLT_VFS_BUNDLE",
        ):
            env.pop(key, None)
        env["MOLT_CAPABILITY_TIER"] = "none"
        env["MOLT_CAPABILITIES"] = grants
        run = _run_wasm_test_process(command, cwd=payload_dir, env=env)
        diagnostic = f"grants={grants!r}\nstdout:\n{run.stdout}\nstderr:\n{run.stderr}"
        assert run.returncode == 0, diagnostic
        expected = (
            f"PermissionError missing [{missing}] capabilities for 'io.open.readwrite'"
            if missing is not None
            else "copied"
        )
        assert run.stdout.strip().splitlines() == [
            expected,
            "ValueError unknown host operation 'unknown.operation'",
            "TypeError host operation name must be str",
        ], diagnostic
        # Host reads are an independent oracle: denied guest code cannot read
        # its own sentinel without granting the very permission being tested.
        assert source.read_bytes() == payload, diagnostic
        assert destination.read_bytes() == (
            sentinel if missing is not None else payload
        ), diagnostic
        _assert_no_recursive_mutex_panic(run.stderr)
