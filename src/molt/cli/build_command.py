"""``molt build`` as a library call.

`build` checks the request, then runs the three build stages: input
preparation (`build_inputs`), the frontend (`frontend_pipeline`) and the
backend and link (`build_pipeline`). It imports them only when it builds: a
request whose recorded inputs are unchanged ends before any of them loads.
`molt.cli` re-exports `build` lazily.
"""

from __future__ import annotations

import functools
import inspect
import os
import sys
from pathlib import Path
from typing import TYPE_CHECKING, Any, Callable, Mapping

from molt.backend_environment import CodegenSelection
from molt.backend_executable_names import DEFAULT_CODEGEN_BACKEND
from molt import build_record as _build_record
from molt import input_report as _input_report
from molt.cli.output import fail as _fail
from molt.cli.output import success as _success
from molt.default_paths import _default_molt_cache

if TYPE_CHECKING:
    from molt.backend_executable_names import CodegenBackend
    from molt.capability_policy import CapabilityInput
    from molt.cli import factgraph as _factgraph
    from molt.cli.models import (
        BuildProfile,
        EmitMode,
        FallbackPolicy,
        ParseCodec,
        Target,
        TypeHintPolicy,
    )


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


def _early_cutoff_request(arguments: Mapping[str, Any]) -> tuple[str, Path] | None:
    """(key, record path) when this request may reuse a recorded build.

    Only a native binary build with an explicit absolute output file and the
    cache enabled qualifies. A request that publishes anything beyond the
    binary, or reads an input no authority reports to the build record, never
    cuts off.
    """
    output = arguments["output"]
    capabilities = arguments["capabilities"]
    if (
        arguments["target"] != "native"
        or not arguments["cache"]
        or arguments["json_output"]
        or output is None
        or not Path(output).is_absolute()
        # A directory output names the binary after the entry module.
        or output.endswith(("/", os.sep))
        or Path(output).is_dir()
        or arguments["emit"] not in (None, "bin")
        or arguments["emit_ir"] is not None
        or arguments["linked"]
        or arguments["linked_output"] is not None
        or arguments["split_runtime"]
        or arguments["snapshot"]
        or arguments["bolt"]
        or arguments["fact_graph_request"] is not None
        or arguments["diagnostics"]
        or arguments["diagnostics_file"] is not None
        or arguments["cache_report"]
        or arguments["audit_log"] is not None
        or arguments["codegen_backend"] != DEFAULT_CODEGEN_BACKEND
        # Inputs named by path whose loaders do not yet report to the record.
        or arguments["type_facts_path"] is not None
        or arguments["pgo_profile"] is not None
        or arguments["runtime_feedback"] is not None
        or arguments["capability_manifest"] is not None
        or arguments["sysroot"] is not None
        or (isinstance(capabilities, str) and Path(capabilities).exists())
        # `ty` under the trust policy is never cached as a compiler proof.
        or arguments["type_hint_policy"] == "trust"
        or os.environ.get("MOLT_BUILD_DIAGNOSTICS")
        or os.environ.get("MOLT_BUILD_DIAGNOSTICS_FILE")
        or os.environ.get("MOLT_EXTERNAL_STATIC_PACKAGES")
    ):
        return None
    key = _build_record.request_key(
        {
            name: value
            for name, value in arguments.items()
            if name not in _RECORD_PRESENTATION_ARGUMENTS
        },
        environment=os.environ,
        cwd=os.getcwd(),
        interpreter=sys.executable,
        interpreter_version=sys.version,
        host=_host_identity(bool(arguments["portable"])),
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


def _excluded_read_roots(arguments: Mapping[str, Any]) -> tuple[Path, ...]:
    """Directories that hold only content-verified caches, scratch files and
    build products, never inputs. Each is a named product root. A source
    checkout, a user directory, the temporary directory and the shared scratch
    root are never excluded: projects live there too. The declared published
    binary is projected separately as one product file."""
    from molt import dx, memory_guard_paths, temporary_artifacts
    from molt.cli import runtime_paths
    from molt.cli.build_output_layout import _default_build_root
    from molt.cli.project_roots import _find_project_root
    from molt.source_root import compiler_source_root

    entry = arguments["file_path"]
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
    if arguments["cache_dir"]:
        roots.append(Path(arguments["cache_dir"]).expanduser())
    out_dir = arguments["out_dir"]
    if out_dir:
        roots.append(Path(out_dir).expanduser() / ".molt_build")
    else:
        # Every output's default artifact root shares this parent.
        roots.append(_default_build_root("entry").parent)
    return tuple(roots)


def _record_build(
    record: Path,
    key: str,
    ledger: _build_record.InputLedger,
    arguments: Mapping[str, Any],
    output: Path,
) -> str | None:
    """Write the record of a successful build; returns why none was written.
    A record never fails the build it describes."""
    try:
        _report_executed_code(ledger)
        capabilities = arguments["capabilities"]
        if isinstance(capabilities, str):
            # Inline capabilities name a file only when one exists there.
            ledger.probe(capabilities, _input_report.resolved_probe(capabilities))
        if sys.platform == "darwin":
            # Apple's compiler shims resolve the developer directory in a
            # child process the audit hook cannot see.
            selection = "/var/db/xcode_select_link"
            ledger.probe(selection, _input_report.resolved_probe(selection))
        _build_record.resolve_observed(
            ledger, _excluded_read_roots(arguments), published_output=output
        )
        return _build_record.write_record(record, key, ledger, output=output)
    except Exception as error:  # noqa: BLE001 - the build already succeeded
        return f"record failed: {type(error).__name__}: {error}"


def _with_early_cutoff(build_function: Callable[..., int]) -> Callable[..., int]:
    """Finish at once when the recorded build of this request is current;
    otherwise build, recording what the build read."""
    signature = inspect.signature(build_function)

    @functools.wraps(build_function)
    def build(*args: Any, **kwargs: Any) -> int:
        first = _build_record.enter_build()
        bound = signature.bind(*args, **kwargs)
        bound.apply_defaults()
        cutoff = _early_cutoff_request(bound.arguments)
        if cutoff is None:
            return build_function(*args, **kwargs)
        key, record = cutoff
        output = Path(bound.arguments["output"])
        change = _build_record.check(record, key, output)
        if change is None:
            _success(f"Successfully built {output} (inputs unchanged)", file=sys.stderr)
            return 0
        verbose = bool(bound.arguments["verbose"])
        if verbose:
            print(f"Build record: {change}", file=sys.stderr)
        if not first:
            return build_function(*args, **kwargs)
        with _build_record.recording(
            _build_record.transient_root(_default_molt_cache())
        ) as ledger:
            result = build_function(*args, **kwargs)
        if result == 0:
            reason = _record_build(record, key, ledger, bound.arguments, output)
            if reason is not None and verbose:
                print(f"Build record not written: {reason}", file=sys.stderr)
        return result

    return build


@_with_early_cutoff
def build(
    file_path: str | None,
    target: Target = "native",
    parse_codec: ParseCodec = "msgpack",
    type_hint_policy: TypeHintPolicy = "check",
    fallback_policy: FallbackPolicy = "error",
    type_facts_path: str | None = None,
    pgo_profile: str | None = None,
    runtime_feedback: str | None = None,
    output: str | None = None,
    json_output: bool = False,
    verbose: bool = False,
    deterministic: bool = True,
    deterministic_warn: bool = False,
    trusted: bool = False,
    capabilities: CapabilityInput | None = None,
    cache: bool = True,
    cache_dir: str | None = None,
    cache_report: bool = False,
    sysroot: str | None = None,
    emit_ir: str | None = None,
    emit: EmitMode | None = None,
    out_dir: str | None = None,
    profile: BuildProfile = "release",
    linked: bool = False,
    linked_output: str | None = None,
    require_linked: bool = False,
    respect_pythonpath: bool = False,
    module: str | None = None,
    diagnostics: bool | None = None,
    diagnostics_file: str | None = None,
    diagnostics_verbosity: str | None = None,
    portable: bool = False,
    wasm_opt_level: str = "Oz",
    precompile: bool = False,
    wasm_profile: str = "auto",
    snapshot: bool = False,
    stdlib_profile: str | None = None,
    tree_shake: bool = True,
    lib_paths: list[str] | None = None,
    split_runtime: bool = False,
    capability_manifest: str | None = None,
    require_signed_manifest: bool = False,
    audit_log: str | None = None,
    io_mode: str | None = None,
    python_version: str | None = None,
    build_config: Mapping[str, Any] | None = None,
    bolt: bool = False,
    bolt_training_cmd: str | None = None,
    fact_graph_request: _factgraph.FactGraphRequest | None = None,
    codegen_backend: CodegenBackend = DEFAULT_CODEGEN_BACKEND,
) -> int:
    from molt.cli import build_inputs as _build_inputs
    from molt.cli import build_pipeline as _build_pipeline
    from molt.cli import frontend_pipeline as _frontend_pipeline
    from molt.cli.cache_fingerprints import _source_tree_fingerprint_transaction
    from molt.cli.config_resolution import (
        _resolve_build_config,
        resolve_stdlib_profile,
    )

    if isinstance(profile, bool):
        profile = "release"
    if profile not in {"dev", "release"}:
        return _fail(f"Invalid build profile: {profile}", json_output, command="build")
    if bolt_training_cmd is not None and not bolt:
        return _fail(
            "BOLT training command requires BOLT optimization.",
            json_output,
            command="build",
        )
    if bolt and profile != "release":
        return _fail(
            "BOLT requires the release build profile.",
            json_output,
            command="build",
        )
    if bolt and target in {"wasm", "wasm-freestanding", "rust", "luau", "mlir"}:
        return _fail(
            f"BOLT requires a native Linux ELF target, not {target!r}.",
            json_output,
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
        flag=stdlib_profile,
        build_cfg=(
            _resolve_build_config(dict(build_config))
            if build_config is not None
            else None
        ),
    )
    # Every value this call resolves travels as a parameter. The backend
    # process receives its share in its own environment mapping
    # (CodegenSelection.environment); build() never writes os.environ, so an
    # in-process caller (library use, the batch build server, a test) cannot
    # leak one build's choices into later work.
    codegen = CodegenSelection(
        backend=codegen_backend,
        portable=portable,
        wasm_profile=(
            wasm_profile
            if target in {"wasm", "wasm-freestanding"} and wasm_profile
            else None
        ),
    )
    with _source_tree_fingerprint_transaction():
        if file_path and module:
            return _fail(
                "Use a file path or --module, not both.", json_output, command="build"
            )
        prepared_build_inputs, prepared_build_inputs_error = (
            _build_inputs._prepare_build_inputs(
                file_path=file_path,
                module=module,
                diagnostics=diagnostics,
                diagnostics_file=diagnostics_file,
                diagnostics_verbosity=diagnostics_verbosity,
                json_output=json_output,
                target=target,
                deterministic=deterministic,
                deterministic_warn=deterministic_warn,
                sysroot=sysroot,
                profile=profile,
                pgo_profile=pgo_profile,
                runtime_feedback=runtime_feedback,
                capabilities=capabilities,
                capability_manifest=capability_manifest,
                require_signed_manifest=require_signed_manifest,
                trusted=trusted,
                audit_log=audit_log,
                io_mode=io_mode,
                respect_pythonpath=respect_pythonpath,
                lib_paths=lib_paths or [],
                python_version=python_version,
                build_config=build_config,
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
                parse_codec=parse_codec,
                type_hint_policy=type_hint_policy,
                fallback_policy=fallback_policy,
                profile=profile,
                json_output=json_output,
                target=target,
                verbose=verbose,
                out_dir=out_dir,
                trusted=trusted,
                split_runtime=split_runtime,
                require_linked=require_linked,
                linked=linked,
                linked_output=linked_output,
                emit=emit,
                output=output,
                emit_ir=emit_ir,
                type_facts_path=type_facts_path,
                tree_shake=tree_shake,
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
            profile=profile,
            json_output=json_output,
            target=target,
            cache_dir=cache_dir,
            cache=cache,
            cache_report=cache_report,
            deterministic=deterministic,
            trusted=trusted,
            verbose=verbose,
            require_linked=require_linked,
            wasm_opt_level=wasm_opt_level,
            precompile=precompile,
            snapshot=snapshot,
            stdlib_profile=stdlib_profile,
            bolt_requested=bolt,
            bolt_training_cmd=bolt_training_cmd,
            fact_graph_request=fact_graph_request,
        )
