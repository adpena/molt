from __future__ import annotations

import argparse
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
import os
from pathlib import Path
import subprocess
import sys
from typing import Any


@dataclass(frozen=True)
class FactGraphRequest:
    output_path: Path
    function_name: str


def add_factgraph_parser(
    subparsers: Any,
    *,
    formatter_class: type[argparse.HelpFormatter],
    build_profile_choices: Sequence[str],
) -> argparse.ArgumentParser:
    parser = subparsers.add_parser(
        "factgraph",
        help="Emit a TIR fact graph for one compiled function.",
        description="Compile a Python entry and write the selected function's TIR fact graph JSON.",
        formatter_class=formatter_class,
    )
    parser.add_argument("file", nargs="?", help="Path to Python source")
    parser.add_argument(
        "function",
        help="Function name to extract after the TIR module pipeline.",
    )
    parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    parser.add_argument(
        "--output",
        required=True,
        help="Output path for the fact graph JSON artifact.",
    )
    parser.add_argument(
        "--target",
        choices=["native", "wasm", "luau", "llvm"],
        default="native",
        help="Target semantics used by the TIR pipeline (default: native).",
    )
    parser.add_argument(
        "--backend",
        choices=["cranelift", "llvm", "auto"],
        default="auto",
        help="Native backend facts to select when --target is native.",
    )
    parser.add_argument(
        "--profile",
        choices=build_profile_choices,
        default="release",
        help="Build profile for frontend/midend optimization (default: release).",
    )
    parser.add_argument(
        "--type-hints",
        choices=["ignore", "trust", "check"],
        default="check",
        help="Apply type annotations to guide lowering and specialization.",
    )
    parser.add_argument(
        "--fallback",
        choices=["error", "bridge"],
        default="error",
        help="Fallback policy for unsupported constructs.",
    )
    parser.add_argument(
        "--python-version",
        default=None,
        help="Target Python semantics (3.12, 3.13, or 3.14).",
    )
    parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest.",
    )
    parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted native analysis.",
    )
    parser.add_argument(
        "--json", action="store_true", help="Emit JSON status for tooling."
    )
    parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    return parser


def backend_command_prefix(
    *,
    backend_bin: Path,
    is_luau_transpile: bool,
    is_rust_transpile: bool,
    is_wasm: bool,
    target_triple: str | None,
    wasm_link: bool = False,
    wasm_data_base: int | None = None,
    wasm_table_base: int | None = None,
    wasm_split_runtime_runtime_table_min: int | None = None,
) -> list[str]:
    cmd = [str(backend_bin)]
    if is_luau_transpile:
        cmd.extend(["--target", "luau"])
    elif is_rust_transpile:
        cmd.extend(["--target", "rust"])
    elif is_wasm:
        cmd.extend(["--target", "wasm"])
        if wasm_link:
            cmd.append("--wasm-link")
        if wasm_data_base is not None:
            cmd.extend(["--wasm-data-base", str(wasm_data_base)])
        if wasm_table_base is not None:
            cmd.extend(["--wasm-table-base", str(wasm_table_base)])
        if wasm_split_runtime_runtime_table_min is not None:
            cmd.extend(
                [
                    "--wasm-split-runtime-runtime-table-min",
                    str(wasm_split_runtime_runtime_table_min),
                ]
            )
    elif target_triple:
        cmd.extend(["--target-triple", target_triple])
    return cmd


def execute_backend_fact_graph(
    *,
    request: FactGraphRequest,
    is_luau_transpile: bool,
    is_rust_transpile: bool,
    is_wasm: bool,
    target_triple: str | None,
    json_output: bool,
    verbose: bool,
    backend_bin: Path,
    backend_env: dict[str, str] | None,
    backend_timeout: float | None,
    entry_module: str,
    ensure_backend_ir_file_path: Callable[[], Path],
    run_subprocess_captured_to_tempfiles: Callable[
        ..., subprocess.CompletedProcess[bytes]
    ],
    subprocess_output_text: Callable[[str | bytes | None], str],
    fail: Callable[..., int],
    entry_override_env: str,
) -> int | None:
    if is_rust_transpile:
        return fail(
            "factgraph does not support the rust transpile backend",
            json_output,
            command="factgraph",
        )
    if backend_env is None:
        backend_env = os.environ.copy()
    backend_env[entry_override_env] = entry_module
    request.output_path.parent.mkdir(parents=True, exist_ok=True)
    cmd = backend_command_prefix(
        backend_bin=backend_bin,
        is_luau_transpile=is_luau_transpile,
        is_rust_transpile=is_rust_transpile,
        is_wasm=is_wasm,
        target_triple=target_triple,
    )
    try:
        ir_file_path = ensure_backend_ir_file_path()
    except OSError as exc:
        return fail(
            f"Backend IR lease write failed: {exc}",
            json_output,
            command="factgraph",
        )
    cmd.extend(
        [
            "--ir-file",
            str(ir_file_path),
            "--fact-graph-output",
            str(request.output_path),
            "--fact-graph-function",
            request.function_name,
        ]
    )
    try:
        backend_process = run_subprocess_captured_to_tempfiles(
            cmd,
            env=backend_env,
            timeout=backend_timeout,
            progress_label=None if json_output else "TIR fact graph emission",
        )
    except subprocess.TimeoutExpired:
        return fail(
            "TIR fact graph emission timed out",
            json_output,
            command="factgraph",
        )
    backend_stderr = subprocess_output_text(backend_process.stderr)
    backend_stdout = subprocess_output_text(backend_process.stdout)
    if verbose and not json_output:
        if backend_stdout:
            print(backend_stdout, end="")
        if backend_stderr:
            print(backend_stderr, end="", file=sys.stderr)
    if backend_process.returncode != 0:
        detail = backend_stderr.strip() or backend_stdout.strip()
        message = (
            f"TIR fact graph emission failed (exit code {backend_process.returncode})"
        )
        if detail:
            message = f"{message}:\n{detail}"
        return fail(
            message,
            json_output,
            backend_process.returncode or 1,
            command="factgraph",
        )
    if not request.output_path.is_file():
        return fail(
            f"TIR fact graph output missing: {request.output_path}",
            json_output,
            command="factgraph",
        )
    if not json_output:
        print(f"Wrote TIR fact graph: {request.output_path}", file=sys.stderr)
    return None


def resolve_request_output_path(
    request: FactGraphRequest, project_root: Path
) -> FactGraphRequest:
    if request.output_path.is_absolute():
        return request
    return FactGraphRequest(
        output_path=project_root / request.output_path,
        function_name=request.function_name,
    )


def emit_pipeline_fact_graph(
    *,
    request: FactGraphRequest,
    output_layout: Any,
    deterministic: bool,
    profile: str,
    runtime_context: Any,
    build_config: Any,
    build_roots: Any,
    build_preamble: Any,
    resolved_modules: set[str] | frozenset[str],
    json_output: bool,
    verbose: bool,
    target: str,
    entry_module: str,
    prepare_backend_dispatch: Callable[..., tuple[Any | None, int | None]],
    ensure_backend_ir_file_path: Callable[[], Path],
    cleanup_backend_ir_file_path: Callable[[], None],
    run_subprocess_captured_to_tempfiles: Callable[
        ..., subprocess.CompletedProcess[bytes]
    ],
    subprocess_output_text: Callable[[str | bytes | None], str],
    fail: Callable[..., int],
    emit_json: Callable[[dict[str, Any], bool], None],
    json_payload: Callable[..., dict[str, Any]],
    entry_override_env: str,
) -> int:
    request = resolve_request_output_path(request, build_roots.project_root)
    try:
        prepared_backend_dispatch, dispatch_error = prepare_backend_dispatch(
            is_rust_transpile=output_layout.is_rust_transpile,
            is_luau_transpile=output_layout.is_luau_transpile,
            is_wasm=output_layout.is_wasm,
            split_runtime=output_layout.split_runtime,
            linked=output_layout.linked,
            deterministic=deterministic,
            profile=profile,
            runtime_state=runtime_context.runtime_state,
            runtime_cargo_profile=build_config.runtime_cargo_profile,
            cargo_timeout=build_config.cargo_timeout,
            molt_root=build_roots.molt_root,
            target_triple=output_layout.target_triple,
            backend_cargo_profile=build_config.backend_cargo_profile,
            diagnostics_enabled=build_preamble.diagnostics_enabled,
            phase_starts=build_preamble.phase_starts,
            json_output=json_output,
            backend_daemon_config_digest=build_preamble.backend_daemon_config_digest,
            ensure_runtime_wasm_shared=runtime_context.ensure_runtime_wasm_shared,
            ensure_runtime_wasm_reloc=runtime_context.ensure_runtime_wasm_reloc,
            resolved_modules=resolved_modules,
            warnings=build_preamble.warnings,
            start_daemon=False,
        )
        if dispatch_error is not None:
            return dispatch_error
        assert prepared_backend_dispatch is not None
        fact_graph_error = execute_backend_fact_graph(
            request=request,
            is_luau_transpile=output_layout.is_luau_transpile,
            is_rust_transpile=output_layout.is_rust_transpile,
            is_wasm=output_layout.is_wasm,
            target_triple=output_layout.target_triple,
            json_output=json_output,
            verbose=verbose,
            backend_bin=prepared_backend_dispatch.backend_bin,
            backend_env=prepared_backend_dispatch.backend_env,
            backend_timeout=build_config.backend_timeout,
            entry_module=entry_module,
            ensure_backend_ir_file_path=ensure_backend_ir_file_path,
            run_subprocess_captured_to_tempfiles=run_subprocess_captured_to_tempfiles,
            subprocess_output_text=subprocess_output_text,
            fail=fail,
            entry_override_env=entry_override_env,
        )
        if fact_graph_error is not None:
            return fact_graph_error
        if json_output:
            emit_json(
                json_payload(
                    "factgraph",
                    "ok",
                    data={
                        "output": str(request.output_path),
                        "function": request.function_name,
                        "target": target,
                        "profile": profile,
                    },
                    warnings=build_preamble.warnings,
                ),
                json_output=True,
            )
        return 0
    finally:
        cleanup_backend_ir_file_path()


def run_factgraph_command(
    *,
    args: argparse.Namespace,
    build: Callable[..., int],
    build_config: Mapping[str, Any],
    config_capabilities: Any,
    coerce_bool: Callable[[Any, bool], bool],
    fail: Callable[..., int],
) -> int:
    if args.file and args.module:
        return fail(
            "Use a file path or --module, not both.",
            args.json,
            command="factgraph",
        )
    if not args.file and not args.module:
        return fail(
            "Missing entry file or module.",
            args.json,
            command="factgraph",
        )
    target = args.target
    backend_choice = args.backend or "auto"
    if target == "llvm":
        if backend_choice not in {"auto", "llvm"}:
            return fail(
                "`--target llvm` selects the LLVM backend; it conflicts "
                f"with `--backend {backend_choice}`.",
                args.json,
                command="factgraph",
            )
        backend_choice = "llvm"
        target = "native"
    effective_backend = "cranelift" if backend_choice == "auto" else backend_choice
    os.environ["MOLT_BACKEND"] = effective_backend
    return build(
        file_path=args.file,
        target=target,
        type_hint_policy=args.type_hints,
        fallback_policy=args.fallback,
        output=None,
        json_output=args.json,
        verbose=args.verbose,
        trusted=(
            coerce_bool(build_config.get("trusted"), False)
            if args.trusted is None
            else args.trusted
        ),
        capabilities=(
            args.capabilities or build_config.get("capabilities") or config_capabilities
        ),
        cache=False,
        module=args.module,
        profile=args.profile,
        python_version=args.python_version,
        fact_graph_request=FactGraphRequest(
            output_path=Path(args.output),
            function_name=args.function,
        ),
        build_config=build_config,
    )
