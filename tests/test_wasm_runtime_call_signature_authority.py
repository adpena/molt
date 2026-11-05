from __future__ import annotations

import os
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]


@pytest.mark.parametrize("family", ["dispatch", "isolate"])
def test_app_resolved_runtime_declarations_have_physical_linker_identities(
    tmp_path: Path, family: str
) -> None:
    import json
    import shutil

    from molt._wasm_abi_generated import (
        WASM_CALL_INDIRECT_IMPORTS,
        runtime_execution_abi,
    )
    from molt.llvm_toolchain import resolve_wasi_sdk_tool
    from molt.cli.runtime_wasm_build_spec import _runtime_wasm_app_import_link_argument
    from molt.wasm_artifact import read_wasm_imports
    from molt.wasm_linking_symbols import (
        FLAG_UNDEFINED,
        SYMBOL_BINDING_MASK,
        read_wasm_linking_symbols,
    )
    from tests.process_guard_common import run_guarded_test_process

    rustc = shutil.which("rustc")
    assert rustc is not None, "WASM declaration proof requires admitted rustc"
    declarations = (
        [(name, arity + 2) for arity, name in enumerate(WASM_CALL_INDIRECT_IMPORTS)]
        if family == "dispatch"
        else [("molt_isolate_import", 2)]
    )
    owner = (
        ROOT
        / "runtime/molt-runtime/src"
        / ("lib.rs" if family == "dispatch" else "builtins/module_table.rs")
    )
    text = owner.read_text(encoding="utf-8")
    first = text.index("fn " + declarations[0][0] + "(")
    start = text.rindex('#[cfg(target_arch = "wasm32")]', 0, first)
    end = text.index("\n}", first) + 2
    block = text[start:end]
    abi = tmp_path / "libmolt_codegen_abi.rlib"
    common = [rustc, "--edition=2024", "--target", "wasm32-wasip1"]
    run_guarded_test_process(
        [
            *common,
            "--crate-name",
            "molt_codegen_abi",
            "--crate-type",
            "rlib",
            str(ROOT / "runtime/molt-codegen-abi/src/lib.rs"),
            "-o",
            str(abi),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
        timeout=60,
    )
    calls = "\n".join(
        f"out ^= {name}({', '.join(['0_u64'] * arity)}) as u64;"
        for name, arity in declarations
    )
    source = tmp_path / "runtime_declarations.rs"
    source.write_text(
        "#![no_std]\nextern crate molt_codegen_abi;\n"
        + block
        + '\n#[unsafe(no_mangle)]\npub unsafe extern "C" fn witness() -> u64 {\n'
        + "unsafe { let mut out = 0_u64;\n"
        + calls
        + "\nout } }\n",
        encoding="utf-8",
    )
    runtime = tmp_path / "runtime.o"
    compile_object = [
        *common,
        "--crate-type",
        "lib",
        "--emit",
        "obj",
        "--extern",
        f"molt_codegen_abi={abi}",
    ]
    run_guarded_test_process(
        [*compile_object, str(source), "-o", str(runtime)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
        timeout=60,
    )
    imports = read_wasm_imports(runtime)
    symbols = read_wasm_linking_symbols(runtime, wasm_imports=imports).function_symbols
    expected = set()
    definitions = []
    for ordinal, (name, arity) in enumerate(declarations):
        execution = runtime_execution_abi(name)
        assert execution is not None and execution.context == "Attached"
        physical = execution.physical_symbol
        expected.add(physical)
        parameters = ", ".join(f"_arg{i}: u64" for i in range(arity))
        definitions.append(
            f"#[unsafe(export_name = {json.dumps(physical)})]\n"
            f'pub extern "C" fn app_{ordinal}({parameters}) -> u64 {{ 0 }}\n'
        )
    selected = [
        (index, item)
        for index, item in enumerate(item for item in imports if item.kind == 0)
        if item.name in expected
    ]
    assert {item.name for _, item in selected} == expected
    for index, item in selected:
        assert item.module == "env"
        bindings = [symbol for symbol in symbols if symbol.index == index]
        assert len(bindings) == 1
        assert bindings[0].name == item.name
        assert bindings[0].flags & FLAG_UNDEFINED
        assert bindings[0].flags & SYMBOL_BINDING_MASK == 0
    linker = str(resolve_wasi_sdk_tool(ROOT, "wasm-ld"))
    allowlist_argument = _runtime_wasm_app_import_link_argument(ROOT)
    shared = tmp_path / "shared.wasm"
    run_guarded_test_process(
        [
            linker,
            "--no-entry",
            "--export=witness",
            allowlist_argument,
            str(runtime),
            "-o",
            str(shared),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
        timeout=60,
    )
    assert expected <= {item.name for item in read_wasm_imports(shared)}
    restricted = tmp_path / "restricted.txt"
    allowed_path = Path(allowlist_argument.partition("=")[2])
    omitted = next(iter(sorted(expected)))
    restricted.write_text(
        "\n".join(
            name
            for name in allowed_path.read_text(encoding="utf-8").splitlines()
            if name != omitted
        )
        + "\n",
        encoding="ascii",
    )
    rejected = run_guarded_test_process(
        [
            linker,
            "--no-entry",
            "--export=witness",
            f"--allow-undefined-file={restricted}",
            str(runtime),
            "-o",
            str(tmp_path / "rejected.wasm"),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert rejected.returncode != 0 and omitted in rejected.stderr
    app_source = tmp_path / "app.rs"
    app_source.write_text("#![no_std]\n" + "\n".join(definitions), encoding="utf-8")
    app = tmp_path / "app.o"
    run_guarded_test_process(
        [*compile_object, str(app_source), "-o", str(app)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
        timeout=60,
    )
    linked = tmp_path / "linked.wasm"
    result = run_guarded_test_process(
        [
            linker,
            "--no-entry",
            "--export=witness",
            str(runtime),
            str(app),
            "-o",
            str(linked),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
        timeout=60,
    )
    assert "function signature mismatch:" not in result.stderr
    assert linked.read_bytes().startswith(b"\0asm\x01\0\0\0")


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
        "return callWithSignature(tableFn, directSignature, [context, ...args]);"
    ) in source
    # The app's callable-table attestation is required through the shared
    # admission authority; test_wasm_runner_table_base.py
    # test_hosts_share_callable_table_admission proves it behaviorally for
    # this loader (missing and malformed attestations are named errors).
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
        "return callWithWasmSignature(fn, directSignature, [args[0], ...args.slice(2)]);"
    ) in source
    # Callable-table attestation is required through the shared admission
    # authority; test_wasm_runner_table_base.py
    # test_hosts_share_callable_table_admission proves it for the Node loaders.
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


def _checkout_expected_runtime_pair(project_root: Path, runtime_artifact: Path):
    from molt.cli.cargo_profiles import _resolve_cargo_profile_name
    from molt.cli.runtime_wasm_build_spec import (
        _compute_runtime_wasm_build_spec,
        _resolve_runtime_wasm_cargo_specs,
    )
    from molt.cli.runtime_wasm_generation import (
        RuntimeWasmExpectedPair,
        RuntimeWasmReceiptRole,
    )
    from molt.cli.runtime_wasm_pair_build import resolve_runtime_wasm_pair_identity

    # The proof plan's wasm.build.shared-runtime and local prebuild use this
    # same explicit dev/full coordinate, including resolved Cargo overrides.
    cargo_profile, error = _resolve_cargo_profile_name("dev", wasm=True)
    if error is not None:
        pytest.fail(error)

    def spec(*, reloc: bool):
        return _compute_runtime_wasm_build_spec(
            project_root,
            runtime_artifact.with_name("molt_runtime_reloc.wasm")
            if reloc
            else runtime_artifact,
            reloc=reloc,
            build_profile="dev",
            cargo_profile=cargo_profile,
            simd_enabled=True,
            freestanding=False,
            stdlib_profile="full",
            resolved_modules=None,
            required_link_features=frozenset(),
            required_exports=None,
        )

    shared_spec, reloc_spec = _resolve_runtime_wasm_cargo_specs(
        project_root,
        spec(reloc=False),
        spec(reloc=True),
        simd_enabled=True,
        freestanding=False,
    )
    identity = resolve_runtime_wasm_pair_identity(project_root, shared_spec, reloc_spec)
    if shared_spec.cargo_plan is None:
        pytest.fail("the dev/full runtime WASM spec resolved no Cargo plan")
    # Admission verifies the build's own resolved Cargo environment, as the
    # pair builder does, not the developer environment of this process.
    return (
        RuntimeWasmExpectedPair(
            identity.shared, identity.reloc, RuntimeWasmReceiptRole.SOURCE_BUILD
        ),
        shared_spec.cargo_plan.environment,
    )


def _runtime_signature_generation(project_root: Path):
    from molt.cli.artifact_state import _build_state_root
    from molt.cli.runtime_paths import _runtime_wasm_artifact_path_from_env
    from molt.cli.runtime_wasm_cache import admit_runtime_wasm_pair_from_shared_cache
    from molt.cli.runtime_wasm_generation import RuntimeWasmExpectedPair
    from molt.dx import development_artifact_env

    runtime_env = development_artifact_env(
        project_root,
        os.environ,
        session_prefix="wasm-signature",
        create_dirs=False,
    )
    runtime_artifact = _runtime_wasm_artifact_path_from_env(
        project_root,
        "molt_runtime.wasm",
        runtime_env,
    )
    expected, cargo_environment = _checkout_expected_runtime_pair(
        project_root, runtime_artifact
    )
    expected_path = expected.path(_build_state_root(project_root))
    reason = (
        "candidate-runtime-wasm-pair-unavailable: build this checkout's dev/full "
        "WASM runtime pair before the compiled export-signature gate "
        f"(expected {expected_path})"
    )
    try:
        recorded = RuntimeWasmExpectedPair.read(expected_path)
    except ValueError as exc:
        pytest.fail(f"{reason}: {exc}")
    if recorded != expected:
        pytest.fail(f"{reason}: expected-pair receipt differs from checkout inputs")
    # Admit this checkout's input-keyed family, as codegen does. The host
    # selection beside the runtime artifact is shared by every checkout of the
    # artifact root, so another checkout's build can replace it (HF-658).
    generation = admit_runtime_wasm_pair_from_shared_cache(
        shared_identity=expected.shared,
        reloc_identity=expected.reloc,
        environment=cargo_environment,
    )
    if generation is None:
        pytest.fail(
            f"{reason}: no admitted generation for family {expected.shared.family_digest}"
        )
    return generation


def test_runtime_wasm_export_signatures_match_import_registry() -> None:
    """Compare compiled exports only with this checkout's input-keyed pair.

    The proof plan builds the dev/full pair first. Missing or damaged candidate
    evidence fails by name; another checkout's receipt can never satisfy it.
    """
    from molt._wasm_abi_generated import (
        WASM_IMPORT_SIGNATURES,
        WASM_RUNTIME_IMPORT_EXPORT_NAMES,
        wasm_runtime_export_name,
        runtime_execution_abi,
    )
    from tools.wasm_abi_gen.manifest import load_manifest

    selected_runtime = _runtime_signature_generation(ROOT).shared
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
        logical = wasm_runtime_export_name(import_name)
        execution = runtime_execution_abi(logical) if logical is not None else None
        assert execution is not None, f"missing execution ABI for {import_name}"
        export_name = execution.physical_symbol
        runtime_signature = runtime_signatures.get(export_name)
        declared_signature = declared.get(import_name)
        if declared_signature is None:
            missing.append(f"{import_name}: missing generated import signature")
            continue
        if execution.context == "Attached":
            params, results = declared_signature
            declared_signature = (("i64", *params), results)
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
