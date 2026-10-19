#!/usr/bin/env python3
"""Build the standalone Rust browser SIMD artifact reported by Cargo."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import subprocess
import sys

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

ROOT = bind_repository_imports(__file__)

from molt import wasm_artifact  # noqa: E402
from molt.artifact_publication import atomic_write_bytes  # noqa: E402
from molt.cargo_execution_policy import (  # noqa: E402
    normalize_cargo_environment,
    require_cargo_build_capacity,
)
from molt.cli.cargo_build_reads import (  # noqa: E402
    CargoBuildReadsError,
    _local_package_root,
    _messages,
)
from molt.dx import RunContext  # noqa: E402
from molt.toolchain_identity import capture_stable_regular_file  # noqa: E402
from molt.wasm_artifact import WASM_HEADER  # noqa: E402
from tools.command_execution import CommandExecutor  # noqa: E402

_COMMANDS = CommandExecutor.for_file(__file__)
_MAX_ARTIFACT_BYTES = 16 * 1024 * 1024
_BUILD_TIMEOUT_SECONDS = 120
_BROWSER_FUNCTIONS = {
    "matmul_f32_tiled": ("i32",) * 6,
    "softmax_f32_fused": ("i32",) * 3,
    "rms_norm_f32": ("i32",) * 4 + ("f32",),
    "add_f32": ("i32",) * 4,
    "mul_f32": ("i32",) * 4,
}


class BrowserSimdBuildError(ValueError):
    """Cargo did not establish one complete browser SIMD artifact."""


def _validate_browser_abi(content: bytes) -> None:
    """Admit the consumer's callable, memory and allocation capabilities."""
    sections = wasm_artifact.parse_wasm_sections(content)
    imports = wasm_artifact._iter_wasm_imports(sections)
    exports = wasm_artifact._iter_wasm_exports(sections)
    issues: list[str] = []
    if imports:
        issues.append(
            "imported ABI is unsupported: "
            + ", ".join(f"{item.module}.{item.name}" for item in imports)
        )
    by_name = {}
    for item in exports:
        if item.name in by_name:
            issues.append(f"duplicate export {item.name}")
        by_name[item.name] = item
    for name, parameters in _BROWSER_FUNCTIONS.items():
        export = by_name.get(name)
        if export is None or export.kind != wasm_artifact.WASM_EXTERN_KIND_FUNCTION:
            issues.append(f"{name} must be a function export")
            continue
        try:
            signature = wasm_artifact._wasm_export_function_signatures_from_sections(
                sections, export_names=(name,)
            )[name]
        except (KeyError, ValueError) as exc:
            issues.append(f"{name} has no valid function signature: {exc}")
            continue
        if tuple(signature["params"]) != parameters or signature["result"] != "nil":
            issues.append(
                f"{name} must have signature ({', '.join(parameters)}) -> void"
            )
    memory = by_name.get("memory")
    memory_bytes = 0
    if (
        memory is None
        or memory.kind != wasm_artifact.WASM_EXTERN_KIND_MEMORY
        or memory.index != 0
    ):
        issues.append("memory must export the owned memory at index 0")
    try:
        payload = next(
            (payload for section, payload in sections if section == 5), b"\0"
        )
        count, cursor = wasm_artifact._read_wasm_varuint(payload, 0)
        if count != 1:
            issues.append("memory must have exactly one owned definition")
        else:
            flags, minimum, maximum, cursor = wasm_artifact.read_wasm_limits(
                payload, cursor
            )
            if (
                cursor != len(payload)
                or flags not in (0, 1)
                or not 0 < minimum <= 65536
                or (maximum is not None and not minimum <= maximum <= 65536)
            ):
                issues.append("memory must have valid unshared wasm32 limits")
            else:
                memory_bytes = minimum * 65536
    except ValueError as exc:
        issues.append(f"memory has invalid limits: {exc}")
    heap = by_name.get("__heap_base")
    globals_by_index = {
        item.index: item for item in wasm_artifact._iter_wasm_defined_globals(sections)
    }
    global_ = None if heap is None else globals_by_index.get(heap.index)
    if (
        heap is None
        or heap.kind != wasm_artifact.WASM_EXTERN_KIND_GLOBAL
        or global_ is None
    ):
        issues.append("__heap_base must export an owned global")
    elif (
        global_.value_type != wasm_artifact.WASM_VALUE_TYPE_I32
        or global_.mutable
        or not global_.i32_const_canonical
        or global_.i32_const is None
    ):
        issues.append("__heap_base must be an immutable i32 constant")
    elif (
        global_.i32_const <= 0
        or global_.i32_const % 16
        or global_.i32_const > memory_bytes
    ):
        issues.append(
            "__heap_base must be positive, 16-byte aligned and within initial memory"
        )
    if issues:
        raise BrowserSimdBuildError("browser SIMD ABI mismatch: " + "; ".join(issues))


def _build_artifact_bytes(project_root: Path) -> bytes:
    root = Path(project_root).resolve(strict=True)
    crate = root / "deploy" / "browser" / "simd-ops-rs"
    manifest = crate / "Cargo.toml"
    environment = RunContext(root, session_prefix="browser-simd").canonical_env(
        os.environ, create_dirs=False, admit_toolchain_root=False
    )
    environment, _policies = normalize_cargo_environment(environment)
    cargo = environment.get("CARGO", "cargo")
    if not cargo:
        raise BrowserSimdBuildError("CARGO must name an executable")
    command = [
        cargo,
        "build",
        "--locked",
        "--release",
        "--target",
        "wasm32-unknown-unknown",
        "--message-format=json-render-diagnostics",
    ]
    require_cargo_build_capacity(command, cwd=crate, env=environment)
    result = _COMMANDS.run(
        command,
        cwd=crate,
        env=environment,
        timeout=_BUILD_TIMEOUT_SECONDS,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()[-4000:]
        raise BrowserSimdBuildError(
            f"browser SIMD Cargo build failed ({result.returncode}): {detail}"
        )
    candidates: list[Path] = []
    try:
        for message in _messages(result.stdout):
            if message.get("reason") != "compiler-artifact":
                continue
            target = message.get("target")
            if not isinstance(target, dict) or target.get("name") != "simd_ops":
                continue
            crate_types = target.get("crate_types")
            if not isinstance(crate_types, list) or "cdylib" not in crate_types:
                continue
            if _local_package_root(message.get("package_id")) != crate:
                continue
            raw_manifest = message.get("manifest_path")
            if not isinstance(raw_manifest, str):
                raise BrowserSimdBuildError("simd_ops artifact has no package manifest")
            reported_manifest = Path(raw_manifest)
            if not reported_manifest.is_absolute():
                reported_manifest = crate / reported_manifest
            if reported_manifest.resolve(strict=False) != manifest.resolve(
                strict=False
            ):
                continue
            filenames = message.get("filenames")
            if not isinstance(filenames, list) or any(
                not isinstance(name, str) or not name for name in filenames
            ):
                raise BrowserSimdBuildError("simd_ops artifact has invalid filenames")
            wasm = [Path(name) for name in filenames if Path(name).suffix == ".wasm"]
            if len(wasm) != 1:
                raise BrowserSimdBuildError(
                    "simd_ops artifact must report exactly one WASM file"
                )
            candidates.append(wasm[0] if wasm[0].is_absolute() else crate / wasm[0])
    except CargoBuildReadsError as exc:
        raise BrowserSimdBuildError(str(exc)) from exc
    if len(candidates) != 1:
        raise BrowserSimdBuildError(
            "Cargo must report exactly one simd_ops cdylib artifact"
        )
    _identity, content = capture_stable_regular_file(
        candidates[0],
        label="Cargo browser SIMD artifact",
        max_bytes=_MAX_ARTIFACT_BYTES,
    )
    if not content.startswith(WASM_HEADER):
        raise BrowserSimdBuildError(
            "Cargo browser SIMD artifact has an invalid WASM header"
        )
    _validate_browser_abi(content)
    return content


def build_browser_simd(output: Path, *, project_root: Path = ROOT) -> Path:
    """Build once, validate Cargo's actual output, and atomically publish it."""
    content = _build_artifact_bytes(project_root)
    destination = Path(output).expanduser().absolute()
    atomic_write_bytes(destination, content)
    return destination


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output", type=Path, default=ROOT / "deploy/browser/simd-ops.wasm"
    )
    parser.add_argument(
        "--check", action="store_true", help="build and compare without publishing"
    )
    args = parser.parse_args(argv)
    try:
        if args.check:
            fresh = _build_artifact_bytes(ROOT)
            _identity, current = capture_stable_regular_file(
                args.output,
                label="published browser SIMD artifact",
                max_bytes=_MAX_ARTIFACT_BYTES,
            )
            if fresh != current:
                raise BrowserSimdBuildError(
                    "browser SIMD artifact differs from the fresh build"
                )
        else:
            build_browser_simd(args.output)
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as exc:
        print(f"browser SIMD: {exc}", file=sys.stderr)
        return 1
    print(f"browser SIMD: {'current' if args.check else 'built'} {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
