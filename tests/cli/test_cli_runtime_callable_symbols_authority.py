from __future__ import annotations

import hashlib
import inspect
from pathlib import Path
import os

import pytest

import molt.cli as cli
from molt.cli import runtime_callable_symbols, native_symbol_inspection

_RUNTIME_CALLABLE_SYMBOL_NAMES = (
    "_runtime_callable_symbols_digest",
    "_runtime_callable_symbols_file",
    "_stage_runtime_callable_symbols_for_native_codegen",
)


def test_cli_runtime_callable_symbols_authority_is_single_home() -> None:
    for name in _RUNTIME_CALLABLE_SYMBOL_NAMES:
        assert getattr(cli, name) is getattr(runtime_callable_symbols, name)

    cli_source = inspect.getsource(cli)
    for name in _RUNTIME_CALLABLE_SYMBOL_NAMES:
        assert f"def {name}(" not in cli_source


def test_native_callable_symbol_stage_excludes_raw_borrowed_intrinsics(
    monkeypatch, tmp_path: Path
) -> None:
    runtime_lib = tmp_path / "molt_runtime.lib"
    runtime_lib.write_bytes(b"runtime")

    def inspect_archive(path, *, target_triple, identity, requirement):
        assert path == runtime_lib
        assert target_triple == "x86_64-pc-windows-msvc"
        assert identity.sha256
        assert requirement.function_prefix == "molt_"
        assert "molt_dict_getitem_borrowed" in requirement.excluded_functions
        symbols = frozenset(
            {
                "molt_len",
                "molt_dict_getitem_borrowed",
                "molt_list_getitem_borrowed",
                "molt_tuple_getitem_borrowed",
            }
        )
        return native_symbol_inspection._NativeGlobalSymbolFacts(
            symbols, frozenset(), symbols, artifact_digest=identity.sha256
        )

    monkeypatch.setattr(
        native_symbol_inspection, "_native_archive_global_symbol_facts", inspect_archive
    )

    symbols_file, failure = runtime_callable_symbols._runtime_callable_symbols_file(
        runtime_lib,
        identity=native_symbol_inspection._native_symbol_artifact_identity(runtime_lib),
        target_triple="x86_64-pc-windows-msvc",
    )

    assert failure is None
    assert symbols_file is not None
    assert ".callable_symbols.v3." in symbols_file.identity.path.name
    assert symbols_file.identity.path.read_bytes() == b"molt_len\n"
    assert symbols_file.identity.sha256 == hashlib.sha256(b"molt_len\n").hexdigest()


def test_callable_projection_cannot_reuse_same_size_restored_mtime(
    monkeypatch, tmp_path: Path
) -> None:
    runtime_lib = tmp_path / "runtime.a"
    runtime_lib.write_bytes(b"first")
    stamp = runtime_lib.stat()

    def inspect_archive(path, *, target_triple, identity, requirement):
        symbol = "molt_" + path.read_text()
        symbols = frozenset({symbol})
        return native_symbol_inspection._NativeGlobalSymbolFacts(
            symbols, frozenset(), symbols, artifact_digest=identity.sha256
        )

    monkeypatch.setattr(
        native_symbol_inspection, "_native_archive_global_symbol_facts", inspect_archive
    )
    first, failure = runtime_callable_symbols._runtime_callable_symbols_file(
        runtime_lib,
        identity=native_symbol_inspection._native_symbol_artifact_identity(runtime_lib),
    )
    assert failure is None and first is not None
    runtime_lib.write_bytes(b"later")
    os.utime(runtime_lib, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
    second, failure = runtime_callable_symbols._runtime_callable_symbols_file(
        runtime_lib,
        identity=native_symbol_inspection._native_symbol_artifact_identity(runtime_lib),
    )
    assert failure is None and second is not None
    assert second.identity.path != first.identity.path
    assert second.identity.path.read_text() == "molt_later\n"
    second.identity.path.write_text("corrupt\n")
    rejected, failure = runtime_callable_symbols._runtime_callable_symbols_file(
        runtime_lib,
        identity=native_symbol_inspection._native_symbol_artifact_identity(runtime_lib),
    )
    assert rejected is None
    assert failure is not None and "archive-derived admission" in failure
    assert second.identity.path.read_text() == "corrupt\n"


def test_callable_projection_concurrent_creators_preserve_winner_generation(
    monkeypatch, tmp_path: Path
) -> None:
    from molt import file_publication
    from molt.toolchain_identity import verify_stable_regular_file_identity

    runtime_lib = tmp_path / "runtime.a"
    runtime_lib.write_bytes(b"runtime")
    identity = native_symbol_inspection._native_symbol_artifact_identity(runtime_lib)
    symbols = frozenset({"molt_len"})
    monkeypatch.setattr(
        native_symbol_inspection,
        "_native_archive_global_symbol_facts",
        lambda *_args, **_kwargs: native_symbol_inspection._NativeGlobalSymbolFacts(
            symbols, frozenset(), symbols, artifact_digest=identity.sha256
        ),
    )
    publish = file_publication.durable_publish_exclusive
    winners = []

    def interleave(staged, destination):
        # Both operations reach publication with an absent destination. B
        # completes and binds its generation before A attempts its publication.
        monkeypatch.setattr(file_publication, "durable_publish_exclusive", publish)
        winner, failure = runtime_callable_symbols._runtime_callable_symbols_file(
            runtime_lib, identity=identity
        )
        assert failure is None and winner is not None
        winners.append(winner)
        return publish(staged, destination)

    monkeypatch.setattr(file_publication, "durable_publish_exclusive", interleave)
    loser, failure = runtime_callable_symbols._runtime_callable_symbols_file(
        runtime_lib, identity=identity
    )
    assert failure is None and loser is not None
    assert len(winners) == 1
    assert loser == winners[0]
    verify_stable_regular_file_identity(winners[0].identity, label="first binding")
    assert not list(tmp_path.glob(".molt-*"))


@pytest.mark.parametrize("failure", ["unreadable", "changed"])
def test_callable_projection_preserves_shared_reader_failures(
    monkeypatch, tmp_path: Path, failure: str
) -> None:
    runtime_lib = tmp_path / "runtime.a"
    runtime_lib.write_bytes(b"runtime")

    def inspect_archive(path, **kwargs):
        raise native_symbol_inspection.NativeSymbolInspectionError(path, [failure])

    monkeypatch.setattr(
        native_symbol_inspection, "_native_archive_global_symbol_facts", inspect_archive
    )
    path, diagnostic = runtime_callable_symbols._runtime_callable_symbols_file(
        runtime_lib,
        identity=native_symbol_inspection._native_symbol_artifact_identity(runtime_lib),
    )
    assert path is None
    assert diagnostic is not None and failure in diagnostic
    assert not list(tmp_path.glob("*.callable_symbols.*"))


@pytest.fixture
def admitted_runtime(tmp_path, monkeypatch):
    from molt.cli import runtime_native_build
    from molt.cli.models import _RuntimeArtifactState
    from tests.cli.native_link_test_support import (
        native_codegen_binding,
        write_test_native_link_manifest,
        write_test_static_archive,
    )
    from tests.runtime_build_identity_helper import native_runtime_staticlib_identity

    root = tmp_path / "dev-fast"
    root.mkdir()
    archive = root / "molt_runtime.lib"
    write_test_static_archive(archive)
    identity = native_runtime_staticlib_identity(cargo_profile="dev-fast")
    write_test_native_link_manifest(archive, build_identity=identity)
    state = _RuntimeArtifactState(
        runtime_lib=archive,
        native_runtime_build_identity=identity,
        native_runtime_codegen_binding=native_codegen_binding(archive, identity),
    )
    monkeypatch.setattr(
        runtime_native_build, "_build_state_root", lambda _root: tmp_path / "state"
    )
    # The final consumer is never allowed to rebuild or hydrate another runtime.
    monkeypatch.setattr(
        runtime_native_build,
        "_ensure_runtime_lib",
        lambda *_args, **_kwargs: pytest.fail(
            "final admission attempted runtime reselection"
        ),
    )
    return state


def _admit_final_runtime(state, tmp_path):
    from molt.cli import runtime_native_build

    return runtime_native_build._ensure_native_runtime_lib_ready_before_link(
        state,
        target_triple=None,
        json_output=True,
        runtime_cargo_profile="dev-fast",
        molt_root=tmp_path,
        cargo_timeout=None,
        diagnostics_enabled=False,
        phase_starts={},
        stdlib_profile="micro",
        resolved_modules={"json"},
    )


def test_final_codegen_admission_recaptures_once_without_reselecting(
    admitted_runtime, tmp_path, monkeypatch
):
    from molt.cli import runtime_native_build

    from molt.cli.runtime_build_python import BuildPythonAdmission

    state = admitted_runtime
    state.build_python_admission = BuildPythonAdmission()
    original = state.native_runtime_codegen_binding
    captured = []

    def current(root, archive, **kwargs):
        captured.append((root, archive, kwargs))
        return original.build_identity

    monkeypatch.setattr(
        runtime_native_build, "current_native_runtime_build_identity", current
    )
    assert _admit_final_runtime(state, tmp_path)
    assert state.native_runtime_codegen_binding is original
    assert captured == [
        (
            tmp_path,
            original.runtime_lib,
            {
                "target_triple": None,
                "cargo_profile": "dev-fast",
                "stdlib_profile": "micro",
                "extra_runtime_features": (),
                "build_python_admission": state.build_python_admission,
            },
        )
    ]

    state.build_python_admission.close()


@pytest.mark.parametrize("changed_input", ["source", "configuration", "python"])
def test_final_codegen_admission_rejects_current_input_drift(
    admitted_runtime, tmp_path, monkeypatch, changed_input
):
    import hashlib
    from molt.cli import runtime_native_build
    from tests.cli.native_link_test_support import (
        native_codegen_binding,
        write_test_native_link_manifest,
    )
    from tests.runtime_build_identity_helper import native_runtime_staticlib_identity

    # Independently derive simulated resolver receipts from real mutable inputs.
    # These tests own the codegen/final-consumer boundary; resolver closure and
    # isolated-Python capture behavior retain their existing dedicated tests.
    inputs = {name: tmp_path / name for name in ("source", "configuration", "python")}
    for path in inputs.values():
        path.write_bytes(b"before")

    def current(*_args, **_kwargs):
        digest = hashlib.sha256(
            b"\0".join(path.read_bytes() for path in inputs.values())
        ).hexdigest()
        return native_runtime_staticlib_identity(
            cargo_profile="dev-fast", family_seed=digest
        )

    state = admitted_runtime
    original = current()
    write_test_native_link_manifest(state.runtime_lib, build_identity=original)
    state.native_runtime_build_identity = original
    state.native_runtime_codegen_binding = native_codegen_binding(
        state.runtime_lib, original
    )
    monkeypatch.setattr(
        runtime_native_build, "current_native_runtime_build_identity", current
    )
    inputs[changed_input].write_bytes(b"after!")
    assert not _admit_final_runtime(state, tmp_path)
    assert state.native_runtime_codegen_binding is None
    assert state.native_runtime_build_identity is None
    assert state.native_runtime_build_failure.stage == "codegen-link-admission"
    assert (
        "inputs changed after code generation"
        in state.native_runtime_build_failure.summary
    )


@pytest.mark.parametrize("which", ["archive", "callable_symbols"])
@pytest.mark.parametrize("when", ["before", "during"])
@pytest.mark.parametrize("mutation", ["rewrite", "replace"])
def test_final_codegen_admission_fences_exact_file_generations(
    admitted_runtime, tmp_path, monkeypatch, which, when, mutation
):
    from molt.cli import runtime_native_build

    state = admitted_runtime
    binding = state.native_runtime_codegen_binding
    path = getattr(binding, which).path
    original = path.read_bytes()
    metadata = path.stat()
    captures = []

    def mutate():
        if mutation == "replace":
            # Even identical content in a new file is not the admitted generation.
            replacement = path.with_name(path.name + ".replacement")
            replacement.write_bytes(original)
            replacement.replace(path)
        else:
            path.write_bytes(bytes([original[0] ^ 1]) + original[1:])
        os.utime(path, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))

    def current(*_args, **_kwargs):
        captures.append(True)
        if when == "during":
            mutate()
        return binding.build_identity

    monkeypatch.setattr(
        runtime_native_build, "current_native_runtime_build_identity", current
    )
    if when == "before":
        mutate()
    assert not _admit_final_runtime(state, tmp_path)
    assert len(captures) == (1 if when == "during" else 0)
    assert state.native_runtime_codegen_binding is None
    assert state.native_runtime_build_identity is None
    assert "codegen" in state.native_runtime_build_failure.summary


def test_final_codegen_admission_rejects_foreign_manifest_diagnostically(
    admitted_runtime, tmp_path, monkeypatch
):
    from molt.cli import runtime_native_build
    from tests.cli.native_link_test_support import write_test_native_link_manifest
    from tests.runtime_build_identity_helper import native_runtime_staticlib_identity

    state = admitted_runtime
    write_test_native_link_manifest(
        state.runtime_lib,
        build_identity=native_runtime_staticlib_identity(
            cargo_profile="dev-fast", family_seed="foreign"
        ),
    )
    monkeypatch.setattr(
        runtime_native_build,
        "current_native_runtime_build_identity",
        lambda *_a, **_k: pytest.fail(
            "foreign archive receipt must fail before capture"
        ),
    )
    assert not _admit_final_runtime(state, tmp_path)
    assert state.native_runtime_codegen_binding is None
    assert (
        "runtime build identity mismatch" in state.native_runtime_build_failure.summary
    )


def test_async_runtime_completion_is_bound_before_codegen(
    admitted_runtime, tmp_path, monkeypatch
):
    from concurrent.futures import Future
    from molt.cli import runtime_native_build

    monkeypatch.setenv("MOLT_RUNTIME_CALLABLE_SYMBOLS", "another-operation")
    monkeypatch.setenv("MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256", "another-operation-sha")
    state = admitted_runtime
    identity = state.native_runtime_build_identity
    state.native_runtime_codegen_binding = None
    future = Future()
    future.set_result(True)
    state.runtime_lib_ready_future = future
    symbols = frozenset({"molt_async_fixture"})
    monkeypatch.setattr(
        native_symbol_inspection,
        "_native_archive_global_symbol_facts",
        lambda _path, **kwargs: native_symbol_inspection._NativeGlobalSymbolFacts(
            symbols, frozenset(), symbols, artifact_digest=kwargs["identity"].sha256
        ),
    )
    digest, failure = (
        runtime_callable_symbols._stage_runtime_callable_symbols_for_native_codegen(
            state,
            target_triple=None,
            json_output=True,
            runtime_cargo_profile="dev-fast",
            molt_root=tmp_path,
            cargo_timeout=None,
            stdlib_profile="micro",
        )
    )
    assert failure is None and digest
    assert state.runtime_lib_ready_future is None
    binding = state.native_runtime_codegen_binding
    assert binding is not None and binding.build_identity == identity
    assert binding.callable_symbols.path.read_text() == "molt_async_fixture\n"
    assert (
        binding.callable_symbols.sha256
        == hashlib.sha256(b"molt_async_fixture\n").hexdigest()
    )
    assert os.environ["MOLT_RUNTIME_CALLABLE_SYMBOLS"] == "another-operation"
    assert os.environ["MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256"] == "another-operation-sha"
    captures = []
    monkeypatch.setattr(
        runtime_native_build,
        "current_native_runtime_build_identity",
        lambda *_a, **_k: captures.append(True) or identity,
    )
    assert _admit_final_runtime(state, tmp_path)
    assert captures == [True]


@pytest.mark.parametrize("missing", ["binding", "identity", "path"])
def test_final_codegen_admission_never_uses_unbound_state(
    admitted_runtime, tmp_path, monkeypatch, missing
):
    from molt.cli import runtime_native_build

    state = admitted_runtime
    if missing == "binding":
        state.native_runtime_codegen_binding = None
    elif missing == "identity":
        state.native_runtime_build_identity = None
    else:
        state.runtime_lib = tmp_path / "another-runtime.lib"
    monkeypatch.setattr(
        runtime_native_build,
        "current_native_runtime_build_identity",
        lambda *_a, **_k: pytest.fail("unbound state must fail before capture"),
    )
    assert not _admit_final_runtime(state, tmp_path)
    assert state.native_runtime_codegen_binding is None
    assert state.native_runtime_build_identity is None


def test_callable_dispatch_and_all_cache_tiers_are_operation_owned(
    admitted_runtime, tmp_path, monkeypatch
):
    import json
    from molt.cli import backend_compile, backend_execution, backend_cache_setup
    from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
    from molt.cli.models import _ModuleGraphMetadata
    from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION
    from tests.cli.native_link_test_support import native_codegen_binding

    def binding_for(name, content):
        root = tmp_path / name
        root.mkdir()
        archive = root / "runtime.lib"
        archive.write_bytes(b"runtime fixture")
        archive.with_name("runtime.lib.test-callables").write_bytes(content)
        return native_codegen_binding(
            archive, admitted_runtime.native_runtime_build_identity
        )

    first = binding_for("first", b"molt_first\n")
    second = binding_for("second", b"molt_second\n")
    relocated_first = binding_for("relocated", b"molt_first\n")
    backend = tmp_path / "molt-backend"
    backend.write_bytes(b"compiler fixture")
    monkeypatch.setattr(
        backend_cache_setup,
        "_backend_binary_identity",
        lambda _path: "fixture-compiler",
    )
    monkeypatch.setattr(
        backend_cache_setup,
        "_native_stdlib_object_split_enabled",
        lambda **_kwargs: True,
    )
    metadata = _ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        module_execution_kind_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module={"sys": True},
    )

    def cache_for(binding):
        return backend_cache_setup._prepare_backend_cache_setup(
            backend_bin=backend,
            cache_enabled=True,
            ir={"functions": []},
            target="native",
            target_triple=None,
            profile="dev",
            runtime_cargo_profile="dev-fast",
            backend_cargo_profile="dev-fast",
            emit_mode="bin",
            is_wasm=False,
            linked=False,
            project_root=tmp_path,
            cache_dir=str(tmp_path / "cache"),
            output_artifact=tmp_path / "out.a",
            warnings=[],
            entry_module="__main__",
            module_graph_metadata=metadata,
            target_python=_DEFAULT_TARGET_PYTHON_VERSION,
            backend_compiler_fingerprint="fixture-compiler",
            native_runtime_codegen_binding=binding,
        )

    def keys(setup):
        return (
            setup.cache_key,
            setup.function_cache_key,
            setup.stdlib_object_cache_key,
        )

    def dispatch(binding):
        result, error = backend_compile._prepare_backend_dispatch(
            is_rust_transpile=False,
            is_luau_transpile=False,
            is_wasm=False,
            wasm_layout=None,
            deterministic=True,
            profile="dev",
            cargo_timeout=None,
            molt_root=tmp_path,
            target_triple=None,
            backend_cargo_profile="dev-fast",
            diagnostics_enabled=False,
            phase_starts={},
            json_output=True,
            backend_daemon_config_digest=None,
            warnings=[],
            backend_bin=backend,
            backend_compiler_fingerprint="fixture-compiler",
            start_daemon=False,
            native_runtime_codegen_binding=binding,
        )
        assert error is None and result is not None
        return result.backend_env

    first_cache = cache_for(first)
    # Deterministic reentrant interleaving: B stages/dispatches between A's cache
    # selection and actual transport. Ambient and already-prepared maps both lie.
    monkeypatch.setenv(
        "MOLT_RUNTIME_CALLABLE_SYMBOLS", str(second.callable_symbols.path)
    )
    monkeypatch.setenv(
        "MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256", second.callable_symbols.sha256
    )
    second_cache = cache_for(second)
    second_env = dispatch(second)
    first_env = dispatch(first)
    assert first_env["MOLT_RUNTIME_CALLABLE_SYMBOLS"] == str(
        first.callable_symbols.path
    )
    assert (
        first_env["MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256"]
        == first.callable_symbols.sha256
    )
    assert (
        second_env["MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256"]
        == second.callable_symbols.sha256
    )
    assert all(keys(first_cache)) and all(keys(second_cache))
    assert all(a != b for a, b in zip(keys(first_cache), keys(second_cache)))
    assert keys(cache_for(first)) == keys(first_cache)
    assert keys(cache_for(relocated_first)) == keys(first_cache), (
        "paths must not split content caches"
    )

    for probe in (True, False):
        payload, error = backend_execution._backend_daemon_compile_request_bytes(
            ir=None if probe else {"functions": []},
            backend_output=tmp_path / "out.a",
            artifact_contract=resolve_backend_artifact_contract(
                target="native", emit_mode="bin"
            ),
            wasm_link=False,
            wasm_data_base=None,
            wasm_table_base=None,
            cache_key=first_cache.cache_key,
            function_cache_key=first_cache.function_cache_key,
            config_digest=None,
            skip_module_output_if_synced=False,
            skip_function_output_if_synced=False,
            probe_cache_only=probe,
            native_runtime_codegen_binding=first,
        )
        assert error is None
        request = json.loads(payload)
        assert request["env"]["MOLT_RUNTIME_CALLABLE_SYMBOLS"] == str(
            first.callable_symbols.path
        )
        assert (
            request["env"]["MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256"]
            == first.callable_symbols.sha256
        )
        assert request["jobs"][0]["cache_key"].startswith(
            first_cache.cache_key + "|artifact:"
        )

    captured = []

    def external_boundary(_cmd, *, env, **_kwargs):
        captured.append(dict(env))
        raise OSError("fixture reached subprocess boundary")

    monkeypatch.setattr(
        backend_compile, "_run_subprocess_captured_to_tempfiles", external_boundary
    )
    # Execution must project A again even if given an earlier environment for B.
    result, error = backend_compile._execute_backend_compile(
        cache=False,
        cache_path=None,
        function_cache_path=None,
        artifacts_root=tmp_path,
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=False,
        diagnostics_enabled=False,
        phase_starts={},
        daemon_ready=False,
        daemon_socket=None,
        project_root=tmp_path,
        output_artifact=tmp_path / "out.a",
        cache_key=first_cache.cache_key,
        function_cache_key=first_cache.function_cache_key,
        cache_setup=first_cache,
        target_triple=None,
        backend_daemon_config_digest=None,
        entry_module="__main__",
        ir={"functions": []},
        json_output=True,
        warnings=[],
        verbose=False,
        backend_bin=backend,
        backend_env=second_env,
        backend_timeout=None,
        molt_root=tmp_path,
        backend_cargo_profile="dev-fast",
        _ensure_backend_ir_file_path=lambda: tmp_path / "ir.json",
        cache_hit=False,
        backend_daemon_cached=None,
        backend_daemon_cache_tier=None,
        backend_daemon_health=None,
        native_runtime_codegen_binding=first,
    )
    assert result is None and error is not None and len(captured) == 1
    assert captured[0]["MOLT_RUNTIME_CALLABLE_SYMBOLS"] == str(
        first.callable_symbols.path
    )
    assert (
        captured[0]["MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256"]
        == first.callable_symbols.sha256
    )
    # No stage or transport may overwrite B's unrelated process-global state.
    assert os.environ["MOLT_RUNTIME_CALLABLE_SYMBOLS"] == str(
        second.callable_symbols.path
    )
    assert (
        os.environ["MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256"]
        == second.callable_symbols.sha256
    )


def test_absent_binding_cannot_inherit_ambient_callable_inputs(tmp_path, monkeypatch):
    from molt.cli import backend_execution
    from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
    from molt.cli.runtime_native_codegen import native_runtime_codegen_environment

    monkeypatch.setenv("MOLT_RUNTIME_CALLABLE_SYMBOLS", "foreign-callables")
    monkeypatch.setenv("MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256", "a" * 64)
    assert "MOLT_RUNTIME_CALLABLE_SYMBOLS" not in native_runtime_codegen_environment(
        os.environ, None
    )
    baseline = backend_execution._backend_codegen_env_digest(is_wasm=False)
    monkeypatch.setenv("MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256", "b" * 64)
    assert backend_execution._backend_codegen_env_digest(is_wasm=False) == baseline
    payload, error = backend_execution._backend_daemon_compile_request_bytes(
        ir={"functions": []},
        backend_output=tmp_path / "probe.o",
        artifact_contract=resolve_backend_artifact_contract(
            target="native", emit_mode="obj"
        ),
        wasm_link=False,
        wasm_data_base=None,
        wasm_table_base=None,
        cache_key=None,
        function_cache_key=None,
        config_digest=None,
        skip_module_output_if_synced=False,
        skip_function_output_if_synced=False,
    )
    assert payload is None
    assert error == "native backend request requires a runtime codegen binding"


@pytest.mark.parametrize("when", ["final_capture", "object_projection"])
def test_native_object_output_rejects_runtime_generation_drift(
    admitted_runtime, tmp_path, monkeypatch, when
):
    from types import SimpleNamespace as NS
    from molt import file_publication
    from molt.cli import backend_output_pipeline, runtime_native_build

    state = admitted_runtime
    binding = state.native_runtime_codegen_binding
    calls = []
    staged_object = file_publication.staged_file_path(
        tmp_path / "output.o", purpose="native-object", suffix=".o"
    )

    def mutate():
        path = binding.callable_symbols.path
        original = path.stat()
        path.write_bytes(b"molt_changed\n")
        os.utime(path, ns=(original.st_atime_ns, original.st_mtime_ns))

    def current(*_args, **_kwargs):
        calls.append("capture")
        if when == "final_capture":
            mutate()
        return binding.build_identity

    def project(**_kwargs):
        calls.append("project")
        assert when == "object_projection"
        mutate()
        return staged_object, None

    monkeypatch.setattr(
        runtime_native_build, "current_native_runtime_build_identity", current
    )
    monkeypatch.setattr(
        backend_output_pipeline._link_pipeline,
        "_prepare_native_object_artifact",
        project,
    )
    monkeypatch.setattr(
        backend_output_pipeline,
        "_emit_non_native_build_result",
        lambda **_kwargs: pytest.fail("mutated object reported success"),
    )
    result = backend_output_pipeline._emit_backend_pipeline_outputs(
        native_object_destination=tmp_path / "output.o",
        prepared_build_preamble=NS(
            diagnostics_enabled=False,
            phase_starts={},
            resolved_diagnostics_verbosity="quiet",
        ),
        prepared_build_roots=NS(molt_root=tmp_path),
        prepared_build_config=NS(runtime_cargo_profile="dev-fast", cargo_timeout=None),
        resolved_build_entry=NS(),
        output_layout=NS(
            is_rust_transpile=False,
            is_luau_transpile=False,
            is_wasm=False,
            emit_mode="obj",
            target_triple=None,
            output_artifact=staged_object,
        ),
        prepared_backend_setup=NS(cache_setup=NS(stdlib_object_path=None)),
        prepared_backend_runtime_context=NS(
            runtime_state=state,
            ensure_runtime_wasm_both=None,
            cache_key=None,
            function_cache_key=None,
            cache_path=None,
            function_cache_path=None,
        ),
        prepared_backend_compile=NS(
            cache_enabled=False,
            cache_hit=False,
            cache_hit_tier=None,
            backend_daemon_cached=False,
            backend_daemon_cache_tier=None,
            backend_daemon_config_digest=None,
            wasm_table_base=None,
        ),
        app_export_contract_path=None,
        native_artifact_plan=None,
        artifacts_root=tmp_path,
        resolved_modules=frozenset(),
        build_diagnostics_payload=lambda: (None, None),
        pipeline_stage_ms=None,
        target="native",
        deterministic=True,
        trusted=False,
        verbose=False,
        require_linked=False,
        json_output=True,
        stdlib_profile="micro",
    )
    assert result != 0
    assert calls == (["capture"] if when == "final_capture" else ["capture", "project"])
    assert state.native_runtime_codegen_binding is None
    assert state.native_runtime_build_identity is None


@pytest.mark.parametrize(
    "when", ["before_projection_capture", "after_projection_return"]
)
@pytest.mark.parametrize("mutation", ["rewrite", "replace"])
def test_callable_projection_cannot_admit_bytes_disconnected_from_archive(
    admitted_runtime, tmp_path, monkeypatch, when, mutation
):
    from concurrent.futures import Future

    ambient_path = os.environ.get("MOLT_RUNTIME_CALLABLE_SYMBOLS")
    ambient_sha = os.environ.get("MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256")
    state = admitted_runtime
    ready = Future()
    ready.set_result(True)
    state.runtime_lib_ready_future = ready
    state.native_runtime_codegen_binding = None
    symbols = frozenset({"molt_allowed"})
    monkeypatch.setattr(
        native_symbol_inspection,
        "_native_archive_global_symbol_facts",
        lambda _path, **kwargs: native_symbol_inspection._NativeGlobalSymbolFacts(
            symbols, frozenset(), symbols, artifact_digest=kwargs["identity"].sha256
        ),
    )
    mutations = []

    def mutate(path):
        # Wrong bytes have the same size and restored mtime; the path's
        # archive-derived filename still claims the allowed symbol set.
        assert path.read_bytes() == b"molt_allowed\n"
        stamp = path.stat()
        if mutation == "replace":
            replacement = path.with_name(path.name + ".replacement")
            replacement.write_bytes(b"molt_hostile\n")
            replacement.replace(path)
        else:
            path.write_bytes(b"molt_hostile\n")
        os.utime(path, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
        mutations.append(path)

    if when == "before_projection_capture":
        capture = runtime_callable_symbols.capture_stable_regular_file

        def capture_changed(path, **kwargs):
            mutate(path)
            return capture(path, **kwargs)

        monkeypatch.setattr(
            runtime_callable_symbols, "capture_stable_regular_file", capture_changed
        )
    else:
        project = runtime_callable_symbols._runtime_callable_symbols_file

        def project_then_change(*args, **kwargs):
            projection, failure = project(*args, **kwargs)
            assert projection is not None and failure is None
            assert (
                projection.identity.sha256
                == hashlib.sha256(b"molt_allowed\n").hexdigest()
            )
            mutate(projection.identity.path)
            return projection, failure

        monkeypatch.setattr(
            runtime_callable_symbols,
            "_runtime_callable_symbols_file",
            project_then_change,
        )

    digest, failure = (
        runtime_callable_symbols._stage_runtime_callable_symbols_for_native_codegen(
            state,
            target_triple=None,
            json_output=True,
            runtime_cargo_profile="dev-fast",
            molt_root=tmp_path,
            cargo_timeout=None,
            stdlib_profile="micro",
        )
    )
    assert mutations and failure is not None and not digest
    assert state.native_runtime_codegen_binding is None
    assert state.native_runtime_build_identity is None
    assert os.environ.get("MOLT_RUNTIME_CALLABLE_SYMBOLS") == ambient_path
    assert os.environ.get("MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256") == ambient_sha


def _projection_at(
    tmp_path: Path,
    content: bytes,
    *,
    archive_sha256: str | None = None,
    directory: Path | None = None,
):
    runtime_lib = tmp_path / "runtime.a"
    runtime_lib.write_bytes(b"runtime")
    archive = native_symbol_inspection._native_symbol_artifact_identity(runtime_lib)
    digest = hashlib.sha256(content).hexdigest()
    folder = directory or tmp_path
    folder.mkdir(parents=True, exist_ok=True)
    projection = folder / runtime_callable_symbols._runtime_callable_projection_name(
        runtime_lib.name,
        archive_sha256=archive_sha256 or archive.sha256,
        projection_sha256=digest,
    )
    projection.write_bytes(content)
    return runtime_lib, archive, projection, digest


def test_callable_projection_admission_accepts_the_producer_encoding(tmp_path):
    symbols = ("molt_a", "molt_b")
    content = runtime_callable_symbols._runtime_callable_projection_content(symbols)
    runtime_lib, archive, projection, digest = _projection_at(tmp_path, content)
    admitted = runtime_callable_symbols._admit_runtime_callable_projection(
        projection,
        runtime_lib=runtime_lib,
        archive_identity=archive,
        expected_sha256=digest,
    )
    assert admitted.identity.sha256 == digest
    assert admitted.semantic_digest == (
        runtime_callable_symbols._runtime_callable_symbols_digest(symbols)
    )


@pytest.mark.parametrize(
    "content",
    [
        b"molt_b\nmolt_a\n",
        b"molt_a",
        b"molt_a\nmolt_a\n",
        b"molt_a\n\n",
        b"\n",
        b"other\n",
        b"molt_dict_getitem_borrowed\n",
        b"\xff\n",
    ],
)
def test_callable_projection_admission_requires_canonical_bytes(tmp_path, content):
    runtime_lib, archive, projection, digest = _projection_at(tmp_path, content)
    with pytest.raises(ValueError, match="canonical"):
        runtime_callable_symbols._admit_runtime_callable_projection(
            projection,
            runtime_lib=runtime_lib,
            archive_identity=archive,
            expected_sha256=digest,
        )


@pytest.mark.parametrize(
    ("claim", "match"),
    [
        ("foreign-archive", "named"),
        ("elsewhere", "adjacent"),
        ("wrong-digest", "archive-derived"),
    ],
)
def test_callable_projection_admission_binds_bytes_name_and_archive(
    tmp_path, claim, match
):
    runtime_lib, archive, projection, digest = _projection_at(
        tmp_path,
        b"molt_len\n",
        archive_sha256="0" * 64 if claim == "foreign-archive" else None,
        directory=tmp_path / "elsewhere" if claim == "elsewhere" else None,
    )
    with pytest.raises(ValueError, match=match):
        runtime_callable_symbols._admit_runtime_callable_projection(
            projection,
            runtime_lib=runtime_lib,
            archive_identity=archive,
            expected_sha256="f" * 64 if claim == "wrong-digest" else digest,
        )
