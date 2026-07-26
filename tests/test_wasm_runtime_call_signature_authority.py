from __future__ import annotations

import json
import os
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]


def _source(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def _stripped_source(path: str) -> str:
    return "\n".join(line.strip() for line in _source(path).splitlines())


def test_browser_embed_requires_binary_callable_table_signatures() -> None:
    source = _stripped_source("wasm/browser_embed.js")

    assert (
        "const directSignature =\n"
        "callableTableSignature(appCallableTable, dispatchIdx) ||\n"
        "callableTableSignature(runtimeCallableTable, dispatchIdx);"
    ) in source
    assert (
        "if (typeof tableFn === 'function' && directSignature) {\n"
        "try {\n"
        "return callWithSignature(tableFn, directSignature, args);"
    ) in source
    assert "requireWasmCallableTable(appBytes, 'app wasm')" in source
    assert "verifyCallableTableEntries(appCallableTable, table, 'app wasm')" in source
    assert "appTableRefSignatures" not in source
    assert "appDirectFn" not in source


def test_node_runner_requires_binary_callable_table_signatures() -> None:
    source = _stripped_source("wasm/run_wasm.js")

    assert (
        "const directSignature =\n"
        "callableTableSignature(outputCallableTable, dispatchIdx) ||\n"
        "callableTableSignature(runtimeCallableTable, dispatchIdx);"
    ) in source
    assert (
        "if (typeof fn === 'function' && directSignature) {\n"
        "return callWithWasmSignature(fn, directSignature, args.slice(1));"
    ) in source
    assert "requireWasmCallableTable(wasmBuffer, 'output wasm')" in source
    assert (
        "verifyCallableTableEntries(outputCallableTable, table, 'output wasm')"
        in source
    )
    assert "appDirectFn" not in source
    assert "runtimeDirectFn" not in source
    assert "outputExportSignatures" not in source


def _parse_wasm_export_signatures(
    path: Path,
) -> dict[str, tuple[tuple[str, ...], tuple[str, ...]]]:
    """Parse function export signatures from a non-relocatable wasm binary."""
    data = path.read_bytes()
    assert data[:4] == b"\0asm", f"not a wasm binary: {path}"

    def leb(pos: int) -> tuple[int, int]:
        result = 0
        shift = 0
        while True:
            byte = data[pos]
            pos += 1
            result |= (byte & 0x7F) << shift
            if not byte & 0x80:
                return result, pos
            shift += 7

    def limits(pos: int) -> int:
        flags = data[pos]
        pos += 1
        _, pos = leb(pos)
        if flags & 1:
            _, pos = leb(pos)
        return pos

    types: list[tuple[tuple[int, ...], tuple[int, ...]]] = []
    func_types: list[int] = []
    exports: dict[str, int] = {}
    imported_funcs = 0
    pos = 8
    while pos < len(data):
        section = data[pos]
        pos += 1
        size, pos = leb(pos)
        end = pos + size
        cursor = pos
        if section == 1:
            count, cursor = leb(cursor)
            for _ in range(count):
                form = data[cursor]
                cursor += 1
                assert form == 0x60, f"non-MVP type form {form:#x} in {path}"
                n_params, cursor = leb(cursor)
                params = tuple(data[cursor : cursor + n_params])
                cursor += n_params
                n_results, cursor = leb(cursor)
                results = tuple(data[cursor : cursor + n_results])
                cursor += n_results
                types.append((params, results))
        elif section == 2:
            count, cursor = leb(cursor)
            for _ in range(count):
                module_len, cursor = leb(cursor)
                cursor += module_len
                name_len, cursor = leb(cursor)
                cursor += name_len
                kind = data[cursor]
                cursor += 1
                if kind == 0:
                    _, cursor = leb(cursor)
                    imported_funcs += 1
                elif kind == 1:
                    cursor += 1
                    cursor = limits(cursor)
                elif kind == 2:
                    cursor = limits(cursor)
                elif kind == 3:
                    cursor += 2
                else:
                    raise AssertionError(f"unknown import kind {kind} in {path}")
        elif section == 3:
            count, cursor = leb(cursor)
            for _ in range(count):
                type_index, cursor = leb(cursor)
                func_types.append(type_index)
        elif section == 7:
            count, cursor = leb(cursor)
            for _ in range(count):
                name_len, cursor = leb(cursor)
                name = data[cursor : cursor + name_len].decode()
                cursor += name_len
                kind = data[cursor]
                cursor += 1
                index, cursor = leb(cursor)
                if kind == 0:
                    exports[name] = index
        pos = end

    valtype = {0x7F: "i32", 0x7E: "i64", 0x7D: "f32", 0x7C: "f64"}
    signatures: dict[str, tuple[tuple[str, ...], tuple[str, ...]]] = {}
    for name, index in exports.items():
        local_index = index - imported_funcs
        if 0 <= local_index < len(func_types):
            params, results = types[func_types[local_index]]
            signatures[name] = (
                tuple(valtype[v] for v in params),
                tuple(valtype[v] for v in results),
            )
    return signatures


def test_runtime_wasm_export_signatures_match_import_registry() -> None:
    """Every app-declared runtime import type must equal the runtime export type.

    wasm-ld fails closed on function signature mismatches when the app links
    against the relocatable runtime, so drift between the manifest static type
    table and the runtime's compiled export signatures is a guaranteed
    linked-lane build failure. The canonical runtime artifact root is resolved
    through Molt's DX path authority so stale repo-local ignored artifacts do
    not masquerade as compiled truth.
    """
    import sys

    sys.path.insert(0, str(ROOT / "src"))
    sys.path.insert(0, str(ROOT / "tools"))
    from molt._wasm_abi_generated import (
        WASM_IMPORT_SIGNATURES,
        WASM_RUNTIME_IMPORT_EXPORT_NAMES,
    )
    from molt.cli.runtime_build import _build_state_root
    from molt.cli.runtime_build_identity import RuntimeBuildIdentity
    from molt.cli.runtime_paths import _runtime_wasm_artifact_path_from_env
    from molt.cli.runtime_wasm_generation import (
        read_runtime_wasm_generation,
        runtime_wasm_generation_path,
    )
    from molt.dx import development_artifact_env
    from wasm_abi_gen.manifest import load_manifest

    runtime_env = development_artifact_env(
        ROOT,
        os.environ,
        session_prefix="wasm-signature",
        create_dirs=False,
    )
    runtime_artifact = _runtime_wasm_artifact_path_from_env(
        ROOT,
        "molt_runtime.wasm",
        runtime_env,
    )
    generation_manifest = runtime_wasm_generation_path(runtime_artifact)
    if not generation_manifest.exists():
        pytest.skip(
            f"{generation_manifest} is a generated build artifact; build the "
            "WASM runtime pair before running the compiled export-signature gate"
        )
    expected_dir = _build_state_root(ROOT) / "runtime_wasm_generations"
    expected_paths = sorted(expected_dir.glob("*.expected.json"))
    if not expected_paths:
        pytest.skip(
            f"{runtime_artifact} has no trusted expected pair identity; build the "
            "WASM runtime pair before running the compiled export-signature gate"
        )
    matching_expected: list[Path] = []
    selected_runtime = None
    for expected_path in expected_paths:
        try:
            expected = json.loads(expected_path.read_text(encoding="utf-8"))
            if (
                not isinstance(expected, dict)
                or expected.get("schema") != "molt.runtime-wasm-expected-pair.v1"
            ):
                continue
            shared_identity = RuntimeBuildIdentity.from_dict(expected.get("shared"))
            reloc_identity = RuntimeBuildIdentity.from_dict(expected.get("reloc"))
        except (OSError, json.JSONDecodeError, ValueError):
            continue
        generation = read_runtime_wasm_generation(
            generation_manifest,
            expected_shared_identity=shared_identity,
            expected_reloc_identity=reloc_identity,
        )
        if generation is not None:
            matching_expected.append(expected_path)
            selected_runtime = generation.shared
    assert matching_expected, (
        f"{runtime_artifact} matches no trusted shared+reloc generation identity; "
        "rebuild runtime artifacts before trusting compiled export signatures"
    )
    assert selected_runtime is not None
    runtime_signatures = _parse_wasm_export_signatures(selected_runtime)
    assert len(runtime_signatures) > 1000, (
        "runtime export signature parse collapsed; the gate would pass vacuously"
    )

    manifest = load_manifest()
    manifest_imports = {entry["name"]: entry for entry in manifest["import"]}
    declared = {name: (tuple(p), tuple(r)) for name, p, r in WASM_IMPORT_SIGNATURES}
    drifted: list[str] = []
    missing: list[str] = []
    matched = 0
    for import_name, export_name in WASM_RUNTIME_IMPORT_EXPORT_NAMES:
        runtime_signature = runtime_signatures.get(export_name)
        declared_signature = declared.get(import_name)
        if declared_signature is None:
            missing.append(f"{import_name}: missing generated import signature")
            continue
        if runtime_signature is None:
            manifest_entry = manifest_imports.get(import_name, {})
            if isinstance(manifest_entry.get("runtime_feature"), str):
                continue
            missing.append(f"{import_name}: missing runtime export {export_name}")
            continue
        matched += 1
        if runtime_signature != declared_signature:
            drifted.append(
                f"{import_name} ({export_name}): "
                f"declared={declared_signature} runtime={runtime_signature}"
            )
    assert not missing, (
        "runtime import/export signature obligation missing:\n" + "\n".join(missing)
    )
    assert matched > 1000, (
        "import/export signature comparison collapsed; the gate would pass vacuously"
    )
    assert not drifted, (
        "runtime import signature drift (rebuild wasm runtime artifacts or fix "
        "wasm_abi_manifest.toml static types):\n" + "\n".join(drifted)
    )
