from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

from molt.node_runtime import resolve_node_runtime
from molt.wasm_artifact import wasm_runtime_manifest_path
from tests.wasm_runtime_heavy_witnesses import WasmWitnessLayout, heavy_wasm_witness
from tests.wasm_linked_runner import (
    _run_wasm_test_process,
    build_wasm_linked,
    run_wasm_linked,
)


def _assert_no_recursive_mutex_panic(stderr: str) -> None:
    assert "cannot recursively acquire mutex" not in stderr, stderr


def test_wasm_imported_syntax_errors_keep_native_identity_and_payload(
    tmp_path: Path,
) -> None:
    from tests.cli.test_cli_smoke import _assert_imported_syntax_error_payload

    _assert_imported_syntax_error_payload(tmp_path, target="wasm")


def test_wasm_asyncio_task_basic_has_no_table_ref_trap(
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    witness = heavy_wasm_witness("asyncio", "linked")
    src = witness.materialize(root, tmp_path)

    output_wasm = build_wasm_linked(
        root, src, tmp_path, extra_args=list(witness.extra_args)
    )
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


def _run_wasm_layout(
    root: Path,
    src: Path,
    tmp_path: Path,
    layout: WasmWitnessLayout,
    *,
    extra_args: tuple[str, ...],
) -> subprocess.CompletedProcess[str]:
    node = resolve_node_runtime(source_root=root, guard_prefix="MOLT_WASM_TEST")
    if layout == "linked":
        output = build_wasm_linked(root, src, tmp_path, extra_args=list(extra_args))
        manifest = wasm_runtime_manifest_path(output)
    elif layout == "split":
        from tests.test_wasm_split_runtime import _build_split

        output_dir = tmp_path / "split"
        build = _build_split(
            src,
            output_dir,
            extra_args=list(extra_args),
        )
        assert build.returncode == 0, build.stdout + build.stderr
        manifest = output_dir / "manifest.json"
    else:
        raise ValueError(f"Unsupported heavy WASM execution layout: {layout!r}")
    env = os.environ.copy()
    for key in (
        "MOLT_WASM_TABLE_BASE",
        "MOLT_RUNTIME_WASM",
        "MOLT_WASM_MANIFEST_PATH",
        "MOLT_VFS_BUNDLE",
    ):
        env.pop(key, None)
    env.update(
        PATH="",
        PYTHONPATH="",
        PYTHONHOME="",
        MOLT_CAPABILITY_TIER="none",
        MOLT_CAPABILITIES="",
    )
    return _run_wasm_test_process(
        [str(node.path), str(root / "wasm" / "run_wasm.js"), str(manifest)],
        cwd=tmp_path,
        env=env,
    )


@pytest.mark.parametrize("layout", ["linked", "split"])
def test_wasm_compiled_finder_describes_module_before_initialization(
    tmp_path: Path,
    layout: WasmWitnessLayout,
) -> None:
    root = Path(__file__).resolve().parents[1]
    witness = heavy_wasm_witness("finder", layout)
    src = witness.materialize(root, tmp_path)

    run = _run_wasm_layout(root, src, tmp_path, layout, extra_args=witness.extra_args)
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.splitlines() == [
        "package-metadata True",
        "absent True",
        "spec True True",
        "unpublished True",
        "subject-exec",
        "ready True",
        "invalidate://before",
        "invalidate://before",
        "invalidate://after",
        "1",
        "2",
        "True",
        "pathfinder-cache-invalidation True True True True",
        "invalidation-errors True",
        "import-root-cases 4",
    ]


@pytest.mark.parametrize("layout", ["linked", "split"])
def test_wasm_final_tir_preserves_effects_scalar_proofs_and_escaped_calls(
    tmp_path: Path, layout: WasmWitnessLayout
) -> None:
    root = Path(__file__).resolve().parents[1]
    witness = heavy_wasm_witness("final-tir", layout)
    src = witness.materialize(root, tmp_path)

    run = _run_wasm_layout(root, src, tmp_path, layout, extra_args=witness.extra_args)
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout.splitlines() == [
        "unused-init",
        "from-init",
        "branch 27 12",
        "checked 70368744177664 9223372036854775808",
        "escaped 13 True",
    ]


def test_wasm_zipimport_package_lookup_parity(
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    witness = heavy_wasm_witness("zipimport", "linked")
    src = witness.materialize(root, tmp_path)

    output_wasm = build_wasm_linked(
        root, src, tmp_path, extra_args=list(witness.extra_args)
    )
    run = run_wasm_linked(
        root,
        output_wasm,
        env_overrides={
            "MOLT_CAPABILITIES": "fs.read,fs.write,env.read",
        },
    )
    assert run.returncode != 0
    assert run.stdout.splitlines() == [
        "package True value = 1",
        "child False value = 2",
        "nested False value = 3",
    ]
    assert "ZipImportError: can't find module 'pkg.mod'" in run.stderr, run.stderr
    _assert_no_recursive_mutex_panic(run.stderr)


def test_wasm_smtplib_thread_dependent_paths_fail_fast(
    tmp_path: Path,
) -> None:
    root = Path(__file__).resolve().parents[1]
    witness = heavy_wasm_witness("smtplib", "linked")
    src = witness.materialize(root, tmp_path)

    output_wasm = build_wasm_linked(
        root, src, tmp_path, extra_args=list(witness.extra_args)
    )
    run = run_wasm_linked(
        root,
        output_wasm,
        env_overrides={
            "MOLT_CAPABILITIES": "net,env.read",
        },
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip().splitlines() == [
        "NotImplementedError",
        "threads are unavailable in wasm",
        "caught",
    ]
    _assert_no_recursive_mutex_panic(run.stderr)


@heavy_wasm_witness("missing-grants").mark_test
def test_wasm_open_reports_all_missing_grants_before_mutation(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    # This is an actual Node/WASI proof. Required tools fail selection/build;
    # a missing host must not turn the security assertion into a skipped test.
    node = resolve_node_runtime(source_root=root, guard_prefix="MOLT_WASM_TEST")
    witness = heavy_wasm_witness("missing-grants", "linked")
    src = witness.materialize(root, tmp_path)

    output_wasm = build_wasm_linked(
        root, src, tmp_path, extra_args=list(witness.extra_args)
    )
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
