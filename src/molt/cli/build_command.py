"""``molt build`` as a library call.

`build` checks the request, then runs the three build stages: input
preparation (`build_inputs`), the frontend (`frontend_pipeline`) and the
backend and link (`build_pipeline`). It imports them only when it builds: a
request whose recorded inputs are unchanged ends before any of them loads.
The entrypoint loads `build` only for commands that compile. The existing
build scope owns managed resource use while both validating a cached result
and performing an actual build; a cache hit still skips frontend/backend work.
"""

from __future__ import annotations

from molt.compilation_target import target_name

from molt.compilation_target import TargetKind, is_wasm_target

import functools
import os
import sys
from pathlib import Path
from typing import Callable

from molt.backend_environment import CodegenSelection
from molt.backend_executable_names import DEFAULT_CODEGEN_BACKEND
from molt import build_record as _build_record
from molt import input_report as _input_report
from molt.cli.build_request import BuildRequest, build_request_values
from molt.cli.output import fail as _fail
from molt.cli.output import success as _success
from molt.default_paths import _default_molt_cache


# How much a build prints does not change what it builds. The output path and
# the output directory do: the linker signs the binary under its file name and
# its debug map names the object files under the output directory.
_RECORD_PRESENTATION_ARGUMENTS = frozenset({"verbose"})


def _host_identity(portable: bool) -> dict[str, str]:
    """Machine facts a native artifact depends on.

    A non-portable build targets the host CPU's features, so the CPU model
    joins the key; a record never crosses hosts that would compile differently.
    """
    import platform

    identity = {
        "system": platform.system(),
        "release": platform.release(),
        "machine": platform.machine(),
    }
    if not portable:
        identity["cpu"] = _cpu_model()
    return identity


def _cpu_model() -> str:
    if sys.platform == "darwin":
        import ctypes

        libc = ctypes.CDLL(None)
        size = ctypes.c_size_t(256)
        buffer = ctypes.create_string_buffer(size.value)
        name = b"machdep.cpu.brand_string"
        if libc.sysctlbyname(name, buffer, ctypes.byref(size), None, 0) == 0:
            return buffer.value.decode("utf-8", "replace")
        return "unknown-darwin-cpu"
    try:
        with open("/proc/cpuinfo", encoding="utf-8", errors="replace") as handle:
            lines = [
                line
                for line in handle
                if line.startswith(("model name", "flags", "Features"))
            ]
        return "".join(sorted(set(lines)))
    except OSError:
        import platform

        return platform.processor() or "unknown-cpu"


def _early_cutoff_request(request: BuildRequest) -> tuple[str, Path] | None:
    """(key, record path) when this request may reuse a recorded build.

    Only a native binary build with an explicit absolute output file and the
    cache enabled qualifies. A request that publishes anything beyond the
    binary, or reads an input no authority reports to the build record, never
    cuts off.
    """
    output = request.output
    capabilities = request.capabilities
    if (
        request.target is not TargetKind.NATIVE
        or not request.cache
        or request.json_output
        or output is None
        or not Path(output).is_absolute()
        # A directory output names the binary after the entry module.
        or output.endswith(("/", os.sep))
        or Path(output).is_dir()
        or request.emit not in (None, "bin")
        or request.emit_ir is not None
        or request.linked
        or request.linked_output is not None
        or request.split_runtime
        or request.snapshot
        or request.bolt
        or request.fact_graph_request is not None
        or request.diagnostics
        or request.diagnostics_file is not None
        or request.cache_report
        or request.capture_link_inputs
        or request.audit_log is not None
        or request.codegen_backend != DEFAULT_CODEGEN_BACKEND
        # Inputs named by path whose loaders do not yet report to the record.
        or request.type_facts_path is not None
        or request.pgo_profile is not None
        or request.runtime_feedback is not None
        or request.capability_manifest is not None
        or request.sysroot is not None
        or (isinstance(capabilities, str) and Path(capabilities).exists())
        # `ty` under the trust policy is never cached as a compiler proof.
        or request.type_hint_policy == "trust"
        or os.environ.get("MOLT_BUILD_DIAGNOSTICS")
        or os.environ.get("MOLT_BUILD_DIAGNOSTICS_FILE")
        or os.environ.get("MOLT_EXTERNAL_STATIC_PACKAGES")
    ):
        return None
    key = _build_record.request_key(
        {
            name: value
            for name, value in build_request_values(request).items()
            if name not in _RECORD_PRESENTATION_ARGUMENTS
        },
        environment=os.environ,
        cwd=os.getcwd(),
        interpreter=sys.executable,
        interpreter_version=sys.version,
        host=_host_identity(request.portable),
    )
    if key is None:
        return None
    return key, _build_record.record_path(_default_molt_cache(), key)


def _report_executed_code(ledger: _build_record.InputLedger) -> None:
    """Every module this process executed is compiler code the build ran."""
    for module in list(sys.modules.values()):
        filename = getattr(module, "__file__", None)
        if isinstance(filename, str):
            ledger.file(filename, _input_report.stat_token(filename), None)


def _excluded_read_roots(request: BuildRequest) -> tuple[Path, ...]:
    """Directories that hold only content-verified caches, scratch files and
    build products, never inputs. Each is a named product root. A source
    checkout, a user directory, the temporary directory and the shared scratch
    root are never excluded: projects live there too. The declared published
    binary is projected separately as one product file."""
    from molt import dx, memory_guard_paths, temporary_artifacts
    from molt.cli import runtime_paths
    from molt.cli.build_output_layout import _default_build_base
    from molt.cli.project_roots import _find_project_root
    from molt.source_root import compiler_source_root

    entry = request.file_path
    project_root = _find_project_root(Path(entry) if entry else Path.cwd())
    # The compiler checkout owns product roots of its own: the runtime builds
    # into its Cargo target, and module caches live in its build state. A
    # guarded child (Cargo, the backend, a linker) leaves custody markers in
    # the guard's state root and a scratch generation that the guard removes
    # when the child ends; the guard follows the same compiler sources
    # (`process_guard`). Each of these once refused every record.
    compiler_root = compiler_source_root()
    roots = [
        _default_molt_cache(),
        dx.backend_daemon_socket_dir(project_root, os.environ),
        runtime_paths._build_state_root(project_root),
        runtime_paths._cargo_target_root(project_root),
        runtime_paths._build_state_root(compiler_root),
        runtime_paths._cargo_target_root(compiler_root),
        memory_guard_paths.memory_guard_state_root(compiler_root, os.environ),
        temporary_artifacts.scratch_root(compiler_root, os.environ),
    ]
    if os.environ.get("MOLT_CACHE"):
        roots.append(Path(os.environ["MOLT_CACHE"]))
    if request.cache_dir:
        roots.append(Path(request.cache_dir).expanduser())
    out_dir = request.out_dir
    if out_dir:
        roots.append(Path(out_dir).expanduser() / ".molt_build")
    else:
        # Every build's default artifacts root lies under this base.
        roots.append(_default_build_base())
    return tuple(roots)


def _record_build(
    record: Path,
    key: str,
    ledger: _build_record.InputLedger,
    request: BuildRequest,
    output: Path,
) -> str | None:
    """Write the record of a successful build; returns why none was written.
    A record never fails the build it describes."""
    try:
        _report_executed_code(ledger)
        capabilities = request.capabilities
        if isinstance(capabilities, str):
            # Inline capabilities name a file only when one exists there.
            ledger.probe(capabilities, _input_report.resolved_probe(capabilities))
        if sys.platform == "darwin":
            # Apple's compiler shims resolve the developer directory in a
            # child process the audit hook cannot see.
            selection = "/var/db/xcode_select_link"
            ledger.probe(selection, _input_report.resolved_probe(selection))
        _build_record.resolve_observed(
            ledger, _excluded_read_roots(request), published_output=output
        )
        return _build_record.write_record(record, key, ledger, output=output)
    except Exception as error:  # noqa: BLE001 - the build already succeeded
        return f"record failed: {type(error).__name__}: {error}"


def _with_early_cutoff(
    build_function: Callable[[BuildRequest], int],
) -> Callable[[BuildRequest], int]:
    """Finish at once when the recorded build of this request is current;
    otherwise build, recording what the build read."""

    @functools.wraps(build_function)
    def build(request: BuildRequest) -> int:
        first = _build_record.enter_build()
        cutoff = _early_cutoff_request(request)
        if cutoff is None:
            return build_function(request)
        key, record = cutoff
        assert request.output is not None
        output = Path(request.output)
        change = _build_record.check(record, key, output)
        if change is None:
            _success(f"Successfully built {output} (inputs unchanged)", file=sys.stderr)
            return 0
        verbose = request.verbose
        if verbose:
            print(f"Build record: {change}", file=sys.stderr)
        if not first:
            return build_function(request)
        with _build_record.recording(
            _build_record.transient_root(_default_molt_cache())
        ) as ledger:
            result = build_function(request)
        if result == 0:
            reason = _record_build(record, key, ledger, request, output)
            if reason is not None and verbose:
                print(f"Build record not written: {reason}", file=sys.stderr)
        return result

    return build


def _in_build_scope(
    build_function: Callable[[BuildRequest], int],
) -> Callable[[BuildRequest], int]:
    """Hold the build's claim on its artifacts root for exactly this build."""

    @functools.wraps(build_function)
    def build(request: BuildRequest) -> int:
        from molt.cli.build_output_layout import (
            build_scope,
            resolve_build_output_options,
        )

        try:
            resolve_build_output_options(
                target=request.target,
                trusted=request.trusted,
                split_runtime=request.split_runtime,
                require_linked=request.require_linked,
                linked=request.linked,
                linked_output=request.linked_output,
                emit=request.emit,
            )
        except ValueError as exc:
            return _fail(str(exc), request.json_output, command="build")
        with build_scope() as scope:
            result = build_function(request)
        if scope.maintenance_environment is not None and not (
            scope.maintenance_environment.get("MOLT_PROOF_QUEUE")
        ):
            from molt.dx import collect_checkout_state_after_result

            collect_checkout_state_after_result(
                scope.maintenance_environment, cwd=scope.cwd
            )
        return result

    return build


@_in_build_scope
@_with_early_cutoff
def build(request: BuildRequest) -> int:
    from molt.cli import build_inputs as _build_inputs
    from molt.cli import build_pipeline as _build_pipeline
    from molt.cli import frontend_pipeline as _frontend_pipeline
    from molt.cli.cache_fingerprints import _source_tree_fingerprint_transaction
    from molt.cli.config_resolution import (
        _resolve_build_config,
        resolve_stdlib_profile,
    )

    if request.profile not in {"dev", "release"}:
        return _fail(
            f"Invalid build profile: {request.profile}",
            request.json_output,
            command="build",
        )
    if request.bolt_training_cmd is not None and not request.bolt:
        return _fail(
            "BOLT training command requires BOLT optimization.",
            request.json_output,
            command="build",
        )
    if request.bolt and request.profile != "release":
        return _fail(
            "BOLT requires the release build profile.",
            request.json_output,
            command="build",
        )
    if request.bolt and (
        is_wasm_target(request.target)
        or request.target in {TargetKind.RUST, TargetKind.LUAU, TargetKind.MLIR}
    ):
        return _fail(
            f"BOLT requires a native Linux ELF target, not {target_name(request.target)!r}.",
            request.json_output,
            command="build",
        )
    # Resolve `stdlib_profile` through the ONE config authority. The CLI
    # dispatcher already resolves it before calling `build()`, but direct
    # library callers may pass `None`; resolving here (honoring the flag value,
    # `MOLT_STDLIB_PROFILE`, the `[tool.molt.build]` config, and the single
    # default) guarantees one concrete value that is passed both to module-graph
    # construction and to the runtime build, so the closure reader and the
    # staticlib selector can never disagree.
    stdlib_profile, _ = resolve_stdlib_profile(
        flag=request.stdlib_profile,
        build_cfg=(
            _resolve_build_config(dict(request.build_config))
            if request.build_config is not None
            else None
        ),
    )
    # Every value this call resolves travels as a parameter. The backend
    # process receives its share in its own environment mapping
    # (CodegenSelection.environment); build() never writes os.environ, so an
    # in-process caller (library use, the batch build server, a test) cannot
    # leak one build's choices into later work.
    codegen = CodegenSelection(
        backend=request.codegen_backend,
        portable=request.portable,
        wasm_profile=(
            request.wasm_profile
            if is_wasm_target(request.target) and request.wasm_profile
            else None
        ),
    )
    with _source_tree_fingerprint_transaction():
        if request.file_path and request.module:
            return _fail(
                "Use a file path or --module, not both.",
                request.json_output,
                command="build",
            )
        prepared_build_inputs, prepared_build_inputs_error = (
            _build_inputs._prepare_build_inputs(
                file_path=request.file_path,
                module=request.module,
                diagnostics=request.diagnostics,
                diagnostics_file=request.diagnostics_file,
                diagnostics_verbosity=request.diagnostics_verbosity,
                json_output=request.json_output,
                target=request.target,
                deterministic=request.deterministic,
                deterministic_warn=request.deterministic_warn,
                sysroot=request.sysroot,
                profile=request.profile,
                pgo_profile=request.pgo_profile,
                runtime_feedback=request.runtime_feedback,
                capabilities=request.capabilities,
                capability_manifest=request.capability_manifest,
                require_signed_manifest=request.require_signed_manifest,
                trusted=request.trusted,
                audit_log=request.audit_log,
                io_mode=request.io_mode,
                respect_pythonpath=request.respect_pythonpath,
                lib_paths=request.lib_paths or [],
                python_version=request.python_version,
                build_config=request.build_config,
                codegen=codegen,
            )
        )
        if prepared_build_inputs_error is not None:
            return prepared_build_inputs_error
        assert prepared_build_inputs is not None
        (
            prepared_build_preamble,
            prepared_build_roots,
            prepared_build_config,
            resolved_build_entry,
        ) = prepared_build_inputs
        prepared_frontend_pipeline_bundle, prepared_frontend_pipeline_error = (
            _frontend_pipeline._prepare_frontend_pipeline(
                prepared_build_preamble=prepared_build_preamble,
                prepared_build_roots=prepared_build_roots,
                prepared_build_config=prepared_build_config,
                resolved_build_entry=resolved_build_entry,
                parse_codec=request.parse_codec,
                type_hint_policy=request.type_hint_policy,
                profile=request.profile,
                json_output=request.json_output,
                target=request.target,
                verbose=request.verbose,
                out_dir=request.out_dir,
                trusted=request.trusted,
                split_runtime=request.split_runtime,
                require_linked=request.require_linked,
                linked=request.linked,
                linked_output=request.linked_output,
                emit=request.emit,
                output=request.output,
                emit_ir=request.emit_ir,
                type_facts_path=request.type_facts_path,
                tree_shake=request.tree_shake,
                stdlib_profile=stdlib_profile,
            )
        )
        if prepared_frontend_pipeline_error is not None:
            return prepared_frontend_pipeline_error
        assert prepared_frontend_pipeline_bundle is not None
        return _build_pipeline._run_build_pipeline(
            prepared_build_preamble=prepared_build_preamble,
            prepared_build_roots=prepared_build_roots,
            prepared_build_config=prepared_build_config,
            resolved_build_entry=resolved_build_entry,
            prepared_frontend_pipeline_bundle=prepared_frontend_pipeline_bundle,
            profile=request.profile,
            json_output=request.json_output,
            target=request.target,
            cache_dir=request.cache_dir,
            cache=request.cache,
            cache_report=request.cache_report,
            deterministic=request.deterministic,
            trusted=request.trusted,
            verbose=request.verbose,
            require_linked=request.require_linked,
            capture_link_inputs=request.capture_link_inputs,
            wasm_opt_level=request.wasm_opt_level,
            precompile=request.precompile,
            snapshot=request.snapshot,
            stdlib_profile=stdlib_profile,
            bolt_requested=request.bolt,
            bolt_training_cmd=request.bolt_training_cmd,
            fact_graph_request=request.fact_graph_request,
        )
