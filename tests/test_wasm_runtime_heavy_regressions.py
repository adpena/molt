from __future__ import annotations

from dataclasses import dataclass
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

from molt.node_runtime import resolve_node_runtime
from molt.wasm_artifact import parse_wasm_imports, wasm_runtime_manifest_path
from tests.wasm_runtime_heavy_witnesses import WasmWitnessLayout, heavy_wasm_witness
from tests.wasm_linked_runner import (
    _run_wasm_test_process,
    build_wasm_linked,
    run_wasm_linked,
)


def _assert_no_recursive_mutex_panic(stderr: str) -> None:
    assert "cannot recursively acquire mutex" not in stderr, stderr


def test_wasm_pure_profile_runs_runtime_imported_core_modules(tmp_path: Path) -> None:
    """The runtime imports ``copyreg`` for ``object.__reduce_ex__``. A Pure guest
    that never imports it must still run the call: no pass removes the import
    plan's core roots (doc 60 slice 1). Pure-profile dead-module elimination
    dropped ``copyreg``, and Cloudflare answered an empty HTTP 500. The import
    statement checks the other half: a Pure guest roots no Python import
    machinery, so ``os`` (which Pure cannot initialize) stays out of the image."""
    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "reduce_ex.py"
    src.write_text(
        "import json\n\n\nclass A:\n    pass\n\n\n"
        "print(A().__reduce_ex__(2)[0].__name__)\n"
        'print(json.dumps({"a": [1, 2]}))\n',
        encoding="utf-8",
    )
    expected = _run_wasm_test_process(
        [sys.executable, str(src)], role="tool", cwd=tmp_path, check=True
    ).stdout
    output_wasm = build_wasm_linked(
        root, src, tmp_path, extra_args=["--wasm-profile", "pure"]
    )
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout == expected


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


def _build_wasm_layout(
    root: Path,
    src: Path,
    tmp_path: Path,
    layout: WasmWitnessLayout,
    *,
    extra_args: tuple[str, ...],
) -> Path:
    """Build one layout and return its execution manifest."""
    if layout == "linked":
        output = build_wasm_linked(root, src, tmp_path, extra_args=list(extra_args))
        return wasm_runtime_manifest_path(output)
    if layout == "split":
        from tests.test_wasm_split_runtime import _build_split

        output_dir = tmp_path / "split"
        build = _build_split(
            src,
            output_dir,
            extra_args=list(extra_args),
        )
        assert build.returncode == 0, build.stdout + build.stderr
        return output_dir / "manifest.json"
    raise ValueError(f"Unsupported heavy WASM execution layout: {layout!r}")


def _run_wasm_layout(
    root: Path,
    src: Path,
    tmp_path: Path,
    layout: WasmWitnessLayout,
    *,
    extra_args: tuple[str, ...],
) -> subprocess.CompletedProcess[str]:
    manifest = _build_wasm_layout(root, src, tmp_path, layout, extra_args=extra_args)
    return _run_wasm_manifest(root, tmp_path, manifest)


def _run_wasm_manifest(
    root: Path, tmp_path: Path, manifest: Path
) -> subprocess.CompletedProcess[str]:
    node = resolve_node_runtime(source_root=root, guard_prefix="MOLT_WASM_TEST")
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
        role="guest-host",
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
    # The witness grants fs.read,fs.write,env.read at build time.
    run = run_wasm_linked(root, output_wasm)
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
    # The witness grants net,env.read at build time.
    run = run_wasm_linked(root, output_wasm)
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
    payload_dir = tmp_path / "payload"
    payload_dir.mkdir()
    source = payload_dir / "source.bin"
    destination = payload_dir / "destination.bin"
    payload = b"source payload\x00with binary bytes\xff"
    sentinel = b"destination must survive denied truncation"
    source.write_bytes(payload)
    for index, (grants, missing) in enumerate(
        [
            ("", "fs.read, fs.write"),
            ("fs.read", "fs.write"),
            ("fs.write", "fs.read"),
            ("fs.read,fs.write", None),
        ]
    ):
        # Deployment manifest v4: the build's launch policy owns the grants,
        # and every host overwrites run-time MOLT_CAPABILITIES with it.
        build_dir = tmp_path / f"build-{index}"
        build_dir.mkdir()
        capability_args = ["--capabilities", grants] if grants else []
        output_wasm = build_wasm_linked(
            root,
            src,
            build_dir,
            extra_args=[*witness.extra_args, *capability_args],
        )
        manifest = wasm_runtime_manifest_path(output_wasm)
        command = [str(node.path), str(root / "wasm" / "run_wasm.js"), str(manifest)]
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
        # A run-time grant of everything cannot widen the deployment policy.
        env["MOLT_CAPABILITY_TIER"] = "full"
        env["MOLT_CAPABILITIES"] = "fs.read,fs.write"
        run = _run_wasm_test_process(
            command, role="guest-host", cwd=payload_dir, env=env
        )
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


@dataclass(frozen=True)
class _AsyncPollsBuild:
    layout: WasmWitnessLayout
    source: Path
    work: Path
    manifest: Path


@pytest.fixture(scope="module", params=["linked", "split"])
def async_polls_build(
    request: pytest.FixtureRequest, tmp_path_factory: pytest.TempPathFactory
) -> _AsyncPollsBuild:
    """One build per layout of the HF-699 witness, shared by its tests."""
    layout: WasmWitnessLayout = request.param
    root = Path(__file__).resolve().parents[1]
    work = tmp_path_factory.mktemp(f"async-polls-{layout}")
    witness = heavy_wasm_witness("async-polls", layout)
    source = witness.materialize(root, work)
    manifest = _build_wasm_layout(
        root, source, work, layout, extra_args=witness.extra_args
    )
    return _AsyncPollsBuild(layout, source, work, manifest)


def test_wasm_runtime_polls_match_cpython(async_polls_build: _AsyncPollsBuild) -> None:
    """HF-699: a runtime poll's identity is its key on every target, and the
    runtime calls the resolved poll itself. A split build trapped on the first
    ``await asyncio.sleep`` with ``molt_call_indirect1 missing table entry at
    2``: slot 2 was ``async_sleep_poll`` in a poll region nothing filled."""
    root = Path(__file__).resolve().parents[1]
    build = async_polls_build
    expected = _run_wasm_test_process(
        [sys.executable, str(build.source)], role="tool", cwd=build.work, check=True
    ).stdout
    run = _run_wasm_manifest(root, build.work, build.manifest)
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout == expected
    assert "missing table entry" not in run.stderr, run.stderr
    _assert_no_recursive_mutex_panic(run.stderr)


@pytest.fixture(scope="module", params=["linked", "split"])
def reserved_callables_build(
    request: pytest.FixtureRequest, tmp_path_factory: pytest.TempPathFactory
) -> _AsyncPollsBuild:
    """One build per layout of the HF-699 E2 witness, shared by its tests."""
    layout: WasmWitnessLayout = request.param
    root = Path(__file__).resolve().parents[1]
    work = tmp_path_factory.mktemp(f"reserved-callables-{layout}")
    witness = heavy_wasm_witness("reserved-callables", layout)
    source = witness.materialize(root, work)
    manifest = _build_wasm_layout(
        root, source, work, layout, extra_args=witness.extra_args
    )
    return _AsyncPollsBuild(layout, source, work, manifest)


def test_wasm_reserved_runtime_callables_match_cpython(
    reserved_callables_build: _AsyncPollsBuild,
) -> None:
    """HF-699 E2: a reserved runtime callable is its key on every target, and
    a function object resolves the key when it is built. Before E2 a split
    build dispatched these slots through a host table of names, and a split
    app that binds the runtime's ``molt_call_indirectN`` to its own export
    trapped on the empty reserved region (``uninitialized element``)."""
    root = Path(__file__).resolve().parents[1]
    build = reserved_callables_build
    expected = _run_wasm_test_process(
        [sys.executable, str(build.source)], role="tool", cwd=build.work, check=True
    ).stdout
    run = _run_wasm_manifest(root, build.work, build.manifest)
    assert run.returncode == 0, run.stdout + run.stderr
    assert run.stdout == expected
    assert "uninitialized element" not in run.stderr, run.stderr
    _assert_no_recursive_mutex_panic(run.stderr)


def _element_function_indices(module: bytes) -> list[int]:
    """Function indices of every table-0 element row (active or passive)."""
    from molt.wasm_artifact import (
        _read_wasm_varuint,
        _skip_wasm_init_expr,
        parse_wasm_sections,
    )

    indices: list[int] = []
    for section_id, payload in parse_wasm_sections(module):
        if section_id != 9:
            continue
        count, offset = _read_wasm_varuint(payload, 0)
        for _ in range(count):
            flags, offset = _read_wasm_varuint(payload, offset)
            if flags == 0:
                offset, _base = _skip_wasm_init_expr(payload, offset)
            elif flags == 1:
                assert payload[offset] == 0x00, "funcref element kind"
                offset += 1
            else:
                raise AssertionError(f"unexpected element segment flags {flags}")
            elem_count, offset = _read_wasm_varuint(payload, offset)
            for _ in range(elem_count):
                func_index, offset = _read_wasm_varuint(payload, offset)
                indices.append(func_index)
    return indices


def test_wasm_app_callable_table_reserves_no_runtime_region(
    reserved_callables_build: _AsyncPollsBuild,
) -> None:
    """HF-699 K3: the app's table reserves no runtime region. It holds no
    runtime poll and no sentinel, and a reserved runtime callable at most once,
    as the app-resolver entry of an intrinsic the program names (giving those
    entries keys is HF-699's follow-up). Before E2 every app table began with
    the 190-slot prefix: a linked build of this witness held 79 reserved
    callables there and 20 sentinels."""
    from collections import Counter

    from molt._wasm_abi_generated import (
        WASM_CALL_INDIRECT_IMPORTS,
        WASM_POLL_TABLE_IMPORTS,
        WASM_RESERVED_RUNTIME_CALLABLES,
    )
    from molt.wasm_artifact import parse_wasm_exports

    module = _call_indirect_module(reserved_callables_build.manifest).read_bytes()
    # Imported functions take the first function indices, in import order.
    function_imports = [imp for imp in parse_wasm_imports(module) if imp.kind == 0]
    runtime_imports = {
        wasm_import.name: index
        for index, wasm_import in enumerate(function_imports)
        if wasm_import.module == "molt_runtime"
    }
    polls = {
        runtime_imports[name]
        for _slot, name in WASM_POLL_TABLE_IMPORTS
        if name in runtime_imports
    }
    reserved = {
        runtime_imports[import_name]
        for _index, _name, import_name, _arity, _dispatch in (
            WASM_RESERVED_RUNTIME_CALLABLES
        )
        if import_name in runtime_imports
    }
    exports = {
        export.name: export.index for export in parse_wasm_exports(module, kind=0)
    }
    sentinel = exports[WASM_CALL_INDIRECT_IMPORTS[0]] + len(WASM_CALL_INDIRECT_IMPORTS)
    elements = Counter(_element_function_indices(module))
    assert elements, "the app publishes its own callable table"
    assert not polls.intersection(elements)
    assert sentinel not in elements
    assert all(elements[index] == 1 for index in reserved.intersection(elements))


# (module (import "host" "hit" (func (param i64) (result i64)))
#         (func (export "f") (param i64) (result i64) local.get 0 call 0))
_RECORDER_WASM = (
    b"\x00asm\x01\x00\x00\x00\x01\x06\x01\x60\x01\x7e\x01\x7e"
    b"\x02\x0c\x01\x04host\x03hit\x00\x00\x03\x02\x01\x00"
    b"\x07\x05\x01\x01f\x00\x01\x0a\x08\x01\x06\x00\x20\x00\x10\x00\x0b"
)

# Instantiates one built module with inert imports, installs a recorder at
# the slot a wrapped key would reach, then calls molt_call_indirect1 with the
# slot (control) and with the key. Every runtime import throws if called.
_CALL_INDIRECT_KEY_PROBE = r"""
const fs = require('fs');
const spec = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
const wasmModule = new WebAssembly.Module(fs.readFileSync(spec.module));
let hits = 0;
const recorder = new WebAssembly.Instance(
  new WebAssembly.Module(Uint8Array.from(spec.recorder)),
  {host: {hit: () => { hits += 1; return 7n; }}},
).exports.f;
const imports = {};
let table = null;
for (const entry of spec.imports) {
  const namespace = (imports[entry.module] ??= {});
  if (entry.kind === 'function') {
    namespace[entry.name] = () => {
      throw new Error(`unexpected import call ${entry.module}.${entry.name}`);
    };
  } else if (entry.kind === 'table') {
    table = new WebAssembly.Table({initial: entry.minimum, element: 'anyfunc'});
    namespace[entry.name] = table;
  } else if (entry.kind === 'memory') {
    const descriptor = {initial: entry.minimum};
    if (entry.maximum !== null) descriptor.maximum = entry.maximum;
    if (entry.shared) descriptor.shared = true;
    namespace[entry.name] = new WebAssembly.Memory(descriptor);
  } else if (entry.kind === 'global') {
    const zero = entry.value === 'i64' ? 0n : 0;
    namespace[entry.name] = new WebAssembly.Global(
      {value: entry.value, mutable: entry.mutable}, zero);
  } else {
    throw new Error(`unsupported import kind ${entry.kind}`);
  }
}
const instance = new WebAssembly.Instance(wasmModule, imports);
table ??= instance.exports.molt_table ?? instance.exports.__indirect_function_table;
const callIndirect = instance.exports.molt_call_indirect1;
if (typeof callIndirect !== 'function') throw new Error('molt_call_indirect1 export missing');
const key = BigInt(spec.key);
const slot = Number(key & 0xffffffffn);
if (table.length <= slot) table.grow(slot + 1 - table.length);
table.set(slot, recorder);
const control = callIndirect(BigInt(slot), 5n);
const controlHits = hits;
let error = null;
try {
  callIndirect(key, 5n);
} catch (caught) {
  error = caught;
}
console.log(JSON.stringify({
  control: String(control),
  controlHits,
  keyHits: hits - controlHits,
  trap: error instanceof WebAssembly.RuntimeError,
  message: error === null ? null : String(error.message),
  stack: error === null ? null : String(error.stack),
}));
"""

_WASM_VALUE_TYPES = {0x7F: "i32", 0x7E: "i64", 0x7D: "f32", 0x7C: "f64"}
_WASM_IMPORT_KINDS = {0: "function", 1: "table", 2: "memory", 3: "global"}


def _call_indirect_module(manifest: Path) -> Path:
    """The app object the backend emitted for this build (``output.wasm``).

    It defines the ``molt_call_indirectN`` exports. A linked build binds the
    runtime's imports to them, and wasm-ld copies their bodies unchanged and
    exports none, so the runtime is their only caller. A split build's final
    ``app.wasm`` drops them, because the host serves the runtime's
    ``molt_call_indirectN`` imports itself (that host dispatch goes in E2)."""
    app_object = manifest.parent / "output.wasm"
    assert app_object.is_file(), f"build left no app object beside {manifest}"
    return app_object


def test_wasm_call_indirect_rejects_runtime_keys(
    async_polls_build: _AsyncPollsBuild,
) -> None:
    """HF-699 K2: every exported ``molt_call_indirectN`` traps on an index
    above ``u32::MAX``. ``i32.wrap_i64`` kept a key's low 32 bits, so a key
    called whatever function sat at that slot."""
    from molt._wasm_abi_generated import WASM_CALL_INDIRECT_IMPORTS

    assert "molt_call_indirect1" in WASM_CALL_INDIRECT_IMPORTS
    root = Path(__file__).resolve().parents[1]
    node = resolve_node_runtime(source_root=root, guard_prefix="MOLT_WASM_TEST")
    module = _call_indirect_module(async_polls_build.manifest)
    imports = []
    for wasm_import in parse_wasm_imports(module.read_bytes()):
        kind = _WASM_IMPORT_KINDS[wasm_import.kind]
        imports.append(
            {
                "module": wasm_import.module,
                "name": wasm_import.name,
                "kind": kind,
                "minimum": wasm_import.minimum,
                "maximum": wasm_import.maximum,
                "shared": bool((wasm_import.limits_flags or 0) & 0x02),
                "value": (
                    _WASM_VALUE_TYPES[wasm_import.value_type[0]]
                    if kind == "global" and wasm_import.value_type
                    else None
                ),
                "mutable": bool(wasm_import.mutable),
            }
        )
    # The async_sleep_poll key: base 0xFFFF_FF00_0000_0100 plus slot 1.
    key = 0xFFFF_FF00_0000_0101
    spec = async_polls_build.work / f"call-indirect-key-{async_polls_build.layout}.json"
    spec.write_text(
        json.dumps(
            {
                "module": str(module),
                "recorder": list(_RECORDER_WASM),
                "imports": imports,
                "key": str(key),
            }
        ),
        encoding="utf-8",
    )
    script = async_polls_build.work / "call_indirect_key_probe.cjs"
    script.write_text(_CALL_INDIRECT_KEY_PROBE, encoding="utf-8")
    run = _run_wasm_test_process(
        [str(node.path), str(script), str(spec)],
        role="guest-host",
        cwd=async_polls_build.work,
    )
    assert run.returncode == 0, run.stdout + run.stderr
    result = json.loads(run.stdout.strip().splitlines()[-1])
    # Control: the slot a wrapped key would reach dispatches the recorder.
    assert result["control"] == "7" and result["controlHits"] == 1, result
    # The key traps before dispatch, so the recorder never runs.
    assert result["keyHits"] == 0, result
    assert result["trap"] is True, result
    assert result["message"] == "unreachable", result
