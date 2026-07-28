from __future__ import annotations

import contextlib
import hashlib
import json
import os
import subprocess
import sys
import uuid
from pathlib import Path
from typing import (
    Any,
    NamedTuple,
    Sequence,
)

from molt._wasm_runtime_exports import (
    wasm_cpython_abi_requested_export_names,
)
from molt.cli import wasm_toolchain
from molt.cli.artifact_state import (
    _build_state_root,
    _runtime_fingerprint_path,
    _runtime_target_fingerprint_path,
)
from molt.cli.build_locks import _build_lock
from molt.cli.cargo_execution import (
    _build_slot,
    _cargo_build_env,
    _maybe_enable_sccache,
    _run_cargo_with_sccache_retry,
)
from molt.cli.command_runtime import (
    _run_completed_command,
    _run_subprocess_captured_to_tempfiles,
)
from molt.cli.compiler_metadata import _compiler_root
from molt.cli.runtime_artifact_selection import (
    RUNTIME_STATICLIB_ARTIFACTS,
    RuntimeCrateType,
)
from molt.cli.runtime_fingerprints import (
    _read_runtime_fingerprint,
    _refresh_runtime_fingerprint_metadata,
    _runtime_artifact_fingerprint_matches,
    _runtime_fingerprint,
    _runtime_fingerprint_metadata_needs_refresh,
    _write_runtime_fingerprint,
)
from molt.cli.runtime_paths import (
    _cargo_profile_dir,
    _cargo_target_root,
)
from molt.cli.runtime_wasm_build_policy import _resolve_wasm_cargo_profile
from molt.cli.runtime_wasm_build_timings import (
    _record_runtime_wasm_longdouble_archives,
)
from molt.cli.runtime_wasm_validation import (
    _is_valid_runtime_wasm_artifact,
    _runtime_wasm_exports_satisfy,
    _runtime_wasm_missing_exports,
    _split_runtime_wasm_exports_satisfy,
    _split_runtime_wasm_missing_exports,
)
from molt.cli.wasm_link_args import (
    wasm_link_args_from_rustflags as _wasm_link_args_from_rustflags,
)
from molt.cli.wasm_link_args import (
    write_wasm_link_args_response_file as _write_wasm_link_args_response_file,
)


def _configure_wasm_cc_env(env: dict[str, str]) -> None:
    if env.get("CC_wasm32-wasip1") or env.get("CC_wasm32_wasip1"):
        return
    for candidate in (
        "/opt/homebrew/opt/llvm/bin/clang",
        "/usr/local/opt/llvm/bin/clang",
    ):
        cc_path = Path(candidate)
        if cc_path.exists() and os.access(cc_path, os.X_OK):
            env["CC_wasm32-wasip1"] = str(cc_path)
            env["CC_wasm32_wasip1"] = str(cc_path)
            return


def _configure_wasi_sysroot_env(env: dict[str, str]) -> None:
    explicit_sysroot = env.get("WASI_SYSROOT") or env.get("MOLT_WASI_SYSROOT")
    if explicit_sysroot:
        normalized = wasm_toolchain.normalize_wasi_sysroot(explicit_sysroot)
        sysroot = str(normalized if normalized is not None else Path(explicit_sysroot))
        env.setdefault("WASI_SYSROOT", sysroot)
        env.setdefault("MOLT_WASI_SYSROOT", sysroot)
        return
    wasi_sysroot = wasm_toolchain.resolve_wasi_sysroot()
    if wasi_sysroot is not None:
        sysroot = str(wasi_sysroot)
        env["WASI_SYSROOT"] = sysroot
        env["MOLT_WASI_SYSROOT"] = sysroot


def _configure_wasm_long_double_env(env: dict[str, str]) -> None:
    """Thread the resolved long-double link archives to molt-runtime's build.rs.

    The deploy ``molt_runtime.wasm`` cdylib link is rustc-driven (so molt cannot
    order a trailing ``-lc-printscan-long-double`` ahead of the self-contained
    ``-lc``); build.rs instead links these archives as build-script
    ``rustc-link-lib`` entries, which rustc emits in its LOCAL-native-libraries
    group AHEAD of ``-lc`` â€” the real ``vfprintf``/``__floatscan`` override
    wasi-libc's ``long_double_not_supported`` stub. This is the deploy-cdylib arm
    of the SAME single authority the reloc / split-app ``wasm-ld`` paths apply;
    env-threaded so build.rs consumes the Python resolver's path (incl. the
    durable ``vendor/wasm-builtins`` fallback), not merely a session sysroot. The
    ``artifact_poison_gate`` attests the effect on the built cdylib. (Harmless on
    the sibling staticlib crate-type: ``rustc-link-lib`` is metadata there, and
    the reloc link whole-archives its own printscan copy.)
    """
    policy = wasm_toolchain.resolve_long_double_link_policy(required=False)
    if policy.printscan is not None:
        env["MOLT_WASM_LONGDOUBLE_ARCHIVE"] = str(
            policy.printscan.resolve(strict=False)
        )
    if policy.builtins is not None:
        env["MOLT_WASM_BUILTINS_ARCHIVE"] = str(policy.builtins.resolve(strict=False))


def _wasm_runtime_artifact_path(target_root: Path, profile_dir: str) -> Path:
    return target_root / "wasm32-wasip1" / profile_dir / "molt_runtime.wasm"


def _wasm_runtime_staticlib_path(target_root: Path, profile_dir: str) -> Path:
    return target_root / "wasm32-wasip1" / profile_dir / "libmolt_runtime.a"


def _wasm_cpython_abi_staticlib_path(target_root: Path, profile_dir: str) -> Path:
    return target_root / "wasm32-wasip1" / profile_dir / "libmolt_cpython_abi.a"


def _wasm_cpython_abi_staticlib_candidates(
    target_root: Path,
    profile_dir: str,
) -> list[Path]:
    primary = _wasm_cpython_abi_staticlib_path(target_root, profile_dir)
    candidates: list[Path] = []
    if primary.exists():
        candidates.append(primary)
    deps_dir = _wasm_runtime_deps_dir(target_root, profile_dir)
    deps_primary = deps_dir / "libmolt_cpython_abi.a"
    if deps_primary.exists():
        candidates.append(deps_primary)
    deps_candidates: list[tuple[int, str, Path]] = []
    for path in deps_dir.glob("libmolt_cpython_abi-*.a"):
        try:
            stat = path.stat()
        except OSError:
            continue
        deps_candidates.append((stat.st_mtime_ns, path.name, path))
    candidates.extend(
        path for _mtime_ns, _name, path in sorted(deps_candidates, reverse=True)
    )
    return candidates


def _wasm_runtime_staticlib_candidates(
    target_root: Path,
    profile_dir: str,
) -> list[Path]:
    primary = _wasm_runtime_staticlib_path(target_root, profile_dir)
    candidates: list[Path] = []
    if primary.exists():
        candidates.append(primary)
    deps_dir = _wasm_runtime_deps_dir(target_root, profile_dir)
    deps_candidates: list[tuple[int, str, Path]] = []
    for path in deps_dir.glob("libmolt_runtime-*.a"):
        try:
            stat = path.stat()
        except OSError:
            continue
        deps_candidates.append((stat.st_mtime_ns, path.name, path))
    candidates.extend(
        path for _mtime_ns, _name, path in sorted(deps_candidates, reverse=True)
    )
    return candidates


def _wasm_runtime_deps_dir(target_root: Path, profile_dir: str) -> Path:
    return target_root / "wasm32-wasip1" / profile_dir / "deps"


def _ensure_wasm_cpython_abi_staticlib(
    *,
    project_root: Path,
    json_output: bool,
    cargo_profile: str,
    cargo_timeout: float | None,
) -> Path | None:
    root = project_root or _compiler_root()
    cargo_profile = _resolve_wasm_cargo_profile(cargo_profile)
    profile_dir = _cargo_profile_dir(cargo_profile)
    target_root = _cargo_target_root(root)
    staticlib_path = _wasm_cpython_abi_staticlib_path(target_root, profile_dir)
    target_label = "wasm32-wasip1.cpython-abi"
    fingerprint_path = _runtime_fingerprint_path(
        root,
        staticlib_path,
        cargo_profile,
        target_label,
    )
    base_rustflags = os.environ.get("RUSTFLAGS", "").strip()
    rustflags = _wasm_runtime_codegen_rustflags(
        base_rustflags,
        simd_enabled=True,
        freestanding=False,
    )
    stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
    fingerprint = _runtime_fingerprint(
        root,
        cargo_profile=cargo_profile,
        target_triple="wasm32-wasip1",
        rustflags=rustflags,
        runtime_features=("molt-cpython-abi-static-link",),
        artifact_selection=RUNTIME_STATICLIB_ARTIFACTS,
        stored_fingerprint=stored_fingerprint,
    )
    candidates = _wasm_cpython_abi_staticlib_candidates(target_root, profile_dir)
    if os.environ.get("MOLT_SKIP_RUNTIME_REBUILD") == "1":
        for candidate in candidates:
            if candidate.exists():
                return candidate
    if fingerprint is None:
        if not json_output:
            print("Failed to compute CPython ABI wasm fingerprint.", file=sys.stderr)
        return None

    lock_name = f"runtime.{cargo_profile}.wasm32-wasip1.cpython-abi"
    build_state_root = _build_state_root(root)
    with _build_lock(root, lock_name):
        current = _current_runtime_target_artifact(
            _wasm_cpython_abi_staticlib_candidates(target_root, profile_dir),
            build_state_root=build_state_root,
            cargo_profile=cargo_profile,
            target_label=target_label,
            fingerprint=fingerprint,
        )
        if current is not None:
            return current[0]
        if _runtime_artifact_fingerprint_matches(
            staticlib_path,
            fingerprint,
            fingerprint_path,
            require_artifact_digest=True,
        ):
            if _runtime_fingerprint_metadata_needs_refresh(
                stored_fingerprint,
                fingerprint,
            ):
                with contextlib.suppress(OSError):
                    _refresh_runtime_fingerprint_metadata(
                        fingerprint_path,
                        fingerprint,
                    )
            return staticlib_path

        if not json_output:
            print("Building wasm CPython ABI link provider...", file=sys.stderr)
        env = _cargo_build_env()
        env["CARGO_TARGET_DIR"] = str(target_root)
        if rustflags:
            env["RUSTFLAGS"] = rustflags
        _configure_wasm_cc_env(env)
        _configure_wasi_sysroot_env(env)
        if os.environ.get("MOLT_WASM_DISABLE_SCCACHE") != "1":
            _maybe_enable_sccache(env)
        else:
            env.pop("RUSTC_WRAPPER", None)
        cmd = [
            "cargo",
            "rustc",
            "--package",
            "molt-lang-cpython-abi",
            "--profile",
            cargo_profile,
            "--target",
            "wasm32-wasip1",
            "--lib",
        ]
        RUNTIME_STATICLIB_ARTIFACTS.select_in(cmd)
        cargo_cmd = _cargo_cmd_with_json_artifact_messages(cmd)
        with _build_slot() as _slot:
            build = _run_cargo_with_sccache_retry(
                cargo_cmd,
                cwd=root,
                env=env,
                timeout=cargo_timeout,
                json_output=json_output,
                label="CPython ABI wasm build",
                tempfile_runner=_run_subprocess_captured_to_tempfiles,
                progress_label=None if json_output else "CPython ABI wasm build",
            )
        if build.returncode != 0:
            detail = (build.stderr or build.stdout or "").strip()
            msg = "CPython ABI wasm build failed"
            if detail:
                msg = f"{msg}: {detail}"
            print(msg, file=sys.stderr)
            return None
        provider = _reported_cpython_abi_staticlib_from_cargo_stdout(
            build.stdout,
            target_root=target_root,
        )
        if provider is None or not provider.exists():
            if not json_output:
                print(
                    "CPython ABI wasm build succeeded but Cargo did not report "
                    "the staticlib artifact.",
                    file=sys.stderr,
                )
            return None
        try:
            fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
            _write_runtime_fingerprint(
                fingerprint_path,
                fingerprint,
                artifact=provider,
            )
            provider_fingerprint_path = _runtime_target_fingerprint_path(
                build_state_root,
                provider,
                cargo_profile=cargo_profile,
                target_label=target_label,
            )
            provider_fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
            _write_runtime_fingerprint(
                provider_fingerprint_path,
                fingerprint,
                artifact=provider,
            )
        except OSError:
            if not json_output:
                print(
                    "Failed to publish CPython ABI wasm staticlib metadata.",
                    file=sys.stderr,
                )
            return None
        return provider


def _wasm_runtime_wasm_candidates(
    target_root: Path,
    profile_dir: str,
) -> list[Path]:
    primary = _wasm_runtime_artifact_path(target_root, profile_dir)
    candidates: list[Path] = []
    if primary.exists():
        candidates.append(primary)
    deps_primary = (
        _wasm_runtime_deps_dir(target_root, profile_dir) / "molt_runtime.wasm"
    )
    if deps_primary.exists():
        candidates.append(deps_primary)
    deps_dir = _wasm_runtime_deps_dir(target_root, profile_dir)
    deps_candidates: list[tuple[int, str, Path]] = []
    for path in deps_dir.glob("molt_runtime-*.wasm"):
        try:
            stat = path.stat()
        except OSError:
            continue
        deps_candidates.append((stat.st_mtime_ns, path.name, path))
    candidates.extend(
        path for _mtime_ns, _name, path in sorted(deps_candidates, reverse=True)
    )
    return candidates


def _current_runtime_target_artifact(
    candidates: Sequence[Path],
    *,
    build_state_root: Path,
    cargo_profile: str,
    target_label: str,
    fingerprint: dict[str, Any],
) -> tuple[Path, Path] | None:
    for candidate in candidates:
        fingerprint_path = _runtime_target_fingerprint_path(
            build_state_root,
            candidate,
            cargo_profile=cargo_profile,
            target_label=target_label,
        )
        stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
        if _runtime_artifact_fingerprint_matches(
            candidate,
            fingerprint,
            fingerprint_path,
            require_artifact_digest=True,
        ):
            if _runtime_fingerprint_metadata_needs_refresh(
                stored_fingerprint,
                fingerprint,
            ):
                with contextlib.suppress(OSError):
                    _refresh_runtime_fingerprint_metadata(
                        fingerprint_path,
                        fingerprint,
                    )
            return candidate, fingerprint_path
    return None


def _runtime_cargo_report_missing_artifact_path(
    target_root: Path,
    profile_dir: str,
    artifact_kind: RuntimeCrateType,
) -> Path:
    suffix = "a" if artifact_kind is RuntimeCrateType.STATICLIB else "wasm"
    return (
        _wasm_runtime_deps_dir(target_root, profile_dir)
        / f".molt_runtime.cargo-report-missing.{suffix}"
    )


def _cargo_cmd_with_json_artifact_messages(cmd: Sequence[str]) -> list[str]:
    if any(arg.startswith("--message-format") for arg in cmd):
        return list(cmd)
    try:
        rustc_arg_index = list(cmd).index("--")
    except ValueError:
        return [*cmd, "--message-format=json-render-diagnostics"]
    return [
        *cmd[:rustc_arg_index],
        "--message-format=json-render-diagnostics",
        *cmd[rustc_arg_index:],
    ]


def _reported_runtime_artifact_matches(
    path: Path,
    *,
    target_root: Path,
    artifact_kind: RuntimeCrateType,
) -> bool:
    try:
        resolved_path = path.resolve(strict=False)
        resolved_root = target_root.resolve(strict=False)
    except OSError:
        return False
    if not (
        resolved_path == resolved_root or resolved_path.is_relative_to(resolved_root)
    ):
        return False
    name = resolved_path.name
    if artifact_kind is RuntimeCrateType.STATICLIB:
        return name == "libmolt_runtime.a" or (
            name.startswith("libmolt_runtime-") and name.endswith(".a")
        )
    return name == "molt_runtime.wasm" or (
        name.startswith("molt_runtime-") and name.endswith(".wasm")
    )


def _reported_runtime_artifact_from_cargo_stdout(
    stdout: str,
    *,
    target_root: Path,
    artifact_kind: RuntimeCrateType,
) -> Path | None:
    return _reported_runtime_artifacts_from_cargo_stdout(
        stdout,
        target_root=target_root,
    ).get(artifact_kind)


def _reported_cargo_artifact_paths_from_stdout(
    stdout: str,
    *,
    target_root: Path,
    package_marker: str,
    target_names: frozenset[str],
) -> tuple[Path, ...]:
    """Return in-target artifact paths from the matching Cargo package report."""
    reported: list[Path] = []
    try:
        resolved_root = target_root.resolve(strict=False)
    except OSError:
        return ()
    for line in stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if (
            not isinstance(message, dict)
            or message.get("reason") != "compiler-artifact"
        ):
            continue
        target = message.get("target")
        target_name = target.get("name") if isinstance(target, dict) else None
        package_id = message.get("package_id")
        package_text = package_id if isinstance(package_id, str) else ""
        if target_name not in target_names or package_marker not in package_text:
            continue
        filenames = message.get("filenames")
        if not isinstance(filenames, list):
            continue
        for filename in filenames:
            if not isinstance(filename, str) or not filename:
                continue
            path = Path(filename)
            if not path.is_absolute():
                path = target_root / path
            try:
                resolved_path = path.resolve(strict=False)
            except OSError:
                continue
            if resolved_path == resolved_root or resolved_path.is_relative_to(
                resolved_root
            ):
                reported.append(path)
    return tuple(reported)


def _reported_runtime_artifacts_from_cargo_stdout(
    stdout: str,
    *,
    target_root: Path,
) -> dict[RuntimeCrateType, Path]:
    """Return the exact runtime crate-type artifacts reported by this Cargo run."""
    reported: dict[RuntimeCrateType, Path] = {}
    paths = _reported_cargo_artifact_paths_from_stdout(
        stdout,
        target_root=target_root,
        package_marker="molt-runtime",
        target_names=frozenset({"molt_runtime", "molt-runtime"}),
    )
    for path in paths:
        for kind in (RuntimeCrateType.CDYLIB, RuntimeCrateType.STATICLIB):
            if _reported_runtime_artifact_matches(
                path,
                target_root=target_root,
                artifact_kind=kind,
            ):
                reported[kind] = path
    return reported


def _reported_cpython_abi_staticlib_from_cargo_stdout(
    stdout: str,
    *,
    target_root: Path,
) -> Path | None:
    paths = _reported_cargo_artifact_paths_from_stdout(
        stdout,
        target_root=target_root,
        package_marker="molt-lang-cpython-abi",
        target_names=frozenset({"molt_cpython_abi", "molt-lang-cpython-abi"}),
    )
    reported: Path | None = None
    for path in paths:
        name = path.name
        if name == "libmolt_cpython_abi.a" or (
            name.startswith("libmolt_cpython_abi-") and name.endswith(".a")
        ):
            reported = path
    return reported


def _append_rustflags_text(base: str, flags: str) -> str:
    return f"{base.strip()} {flags.strip()}".strip()


def _wasm_runtime_codegen_rustflags(
    rustflags: str,
    *,
    simd_enabled: bool,
    freestanding: bool,
) -> str:
    # Disable reference-types so that LLVM (Rust 1.94+ / LLVM 21+) does not
    # emit GC-proposal rec groups or `exact` heap types.  These are rejected
    # by Cloudflare Workers' V8 and by wasm-opt without --all-features.
    # Enable WASM SIMD (128-bit) for vectorized string/bytes operations.
    # Freestanding builds use the conservative baseline because the WASI stub
    # rewriter currently cannot remap SIMD-prefixed instruction streams.
    if "-C target-feature" not in rustflags:
        tf_parts = ["-reference-types"]
        if simd_enabled:
            tf_parts.append("+simd128")
        rustflags = _append_rustflags_text(
            rustflags, f"-C target-feature={','.join(tf_parts)}"
        )
    elif "-reference-types" not in rustflags:
        # Caller already set -C target-feature; append the ref-types disable.
        rustflags = rustflags.replace(
            "-C target-feature=", "-C target-feature=-reference-types,", 1
        )
    if freestanding and 'getrandom_backend="' not in rustflags:
        rustflags = _append_rustflags_text(
            rustflags, '--cfg getrandom_backend="unsupported"'
        )
    return rustflags


def _run_runtime_wasm_cargo_build(
    *,
    cmd: list[str],
    root: Path,
    env: dict[str, str],
    cargo_timeout: float | None,
    profile_dir: str,
    target_root_override: Path | None = None,
    json_output: bool,
    artifact_kind: RuntimeCrateType = RuntimeCrateType.CDYLIB,
) -> tuple[subprocess.CompletedProcess[str], Path]:
    build_env = env.copy()
    if target_root_override is not None:
        target_root = target_root_override
    else:
        target_root = _cargo_target_root(root)
    # Always propagate target_root to CARGO_TARGET_DIR so cargo builds
    # into the same directory the artifact lookup will check. Without
    # this, explicit and session-aware target resolution can drift apart.
    build_env["CARGO_TARGET_DIR"] = str(target_root)
    cargo_cmd = _cargo_cmd_with_json_artifact_messages(cmd)
    with _build_slot() as _slot:
        build = _run_cargo_with_sccache_retry(
            cargo_cmd,
            cwd=root,
            env=build_env,
            timeout=cargo_timeout,
            json_output=json_output,
            label="Runtime wasm build",
            tempfile_runner=_run_subprocess_captured_to_tempfiles,
            progress_label=None if json_output else "Runtime wasm build",
        )
    reported_artifact = _reported_runtime_artifact_from_cargo_stdout(
        build.stdout,
        target_root=target_root,
        artifact_kind=artifact_kind,
    )
    if reported_artifact is None:
        reported_artifact = _runtime_cargo_report_missing_artifact_path(
            target_root,
            profile_dir,
            artifact_kind,
        )
    return build, reported_artifact


def _reloc_link_archive_fingerprint_token() -> str:
    """Content token for the reloc link's long-double + builtins archives.

    Folded into the reloc-runtime-wasm fingerprint so a change to those archives
    (first provisioning, a version bump, or removal) invalidates the cached
    reloc runtime. Uses (name, size, mtime) â€” cheap and sufficient to detect a
    swapped/updated archive without hashing hundreds of KB every build.
    """
    parts: list[str] = []
    for label, archive in (
        ("longdouble", wasm_toolchain.wasm_wasi_printscan_long_double_archive()),
        ("builtins", wasm_toolchain.wasm_clang_rt_builtins_archive()),
    ):
        if archive is None:
            parts.append(f"{label}=none")
            continue
        try:
            st = archive.stat()
            parts.append(f"{label}={archive.name}:{st.st_size}:{int(st.st_mtime)}")
        except OSError:
            parts.append(f"{label}={archive.name}:unstat")
    return hashlib.sha256(";".join(parts).encode("utf-8")).hexdigest()[:16]


# Top-level packages whose native extensions format/parse `long double` (%L)
# during import, so the reloc runtime they link against MUST carry wasi-libc's
# long-double formatters (else the stub abort()s -> unreachable at import).
_LONG_DOUBLE_MODULE_PREFIXES = frozenset({"numpy", "scipy"})


def _reloc_runtime_requires_long_double(
    *,
    resolved_modules: set[str] | frozenset[str] | None,
    required_exports: set[str] | frozenset[str] | None,
) -> bool:
    """Whether this reloc runtime links code that hits wasi-libc's ``%L`` path.

    True for the CPython-ABI tier (numpy/scipy C extensions format/parse
    ``long double`` during import) â€” identified by a non-empty CPython-ABI
    requested-export set â€” or when a resolved module is (a submodule of) numpy or
    scipy. For these builds a missing long-double formatter archive is a HARD
    ERROR (the runtime would relink wasi-libc's ``long_double_not_supported``
    abort() stub -> raw ``unreachable`` trap at ``_multiarray_umath`` import), not
    a silent graceful degrade. Non-numpy / micro builds stay degradable.
    """
    if wasm_cpython_abi_requested_export_names(required_exports):
        return True
    if resolved_modules:
        for module in resolved_modules:
            if module.split(".", 1)[0] in _LONG_DOUBLE_MODULE_PREFIXES:
                return True
    return False


class _RelocLongDoubleArchives(NamedTuple):
    """Resolved reloc long-double link archives + fail-loud / degrade decision."""

    longdouble: Path | None
    builtins: Path | None
    error: str | None
    warnings: tuple[str, ...]


def _resolve_reloc_long_double_archives(
    *, long_double_required: bool
) -> _RelocLongDoubleArchives:
    """Resolve the reloc long-double archives and decide fail-loud vs degrade.

    Thin wrapper over the single authority
    :func:`wasm_toolchain.resolve_long_double_link_policy` that additionally
    records the ``longdouble_archives`` build attestation (present/MISSING). When
    ``long_double_required`` and either archive is unresolved, propagates the
    authority's ``error`` (the caller MUST abort the build â€” a numpy runtime that
    traps is never acceptable). Otherwise returns the archives plus any degrade
    ``warnings`` for a build that provably does not need long double.
    """
    policy = wasm_toolchain.resolve_long_double_link_policy(
        required=long_double_required
    )
    missing = policy.printscan is None or policy.builtins is None
    if not long_double_required:
        _record_runtime_wasm_longdouble_archives(
            "not_required" if missing else "present"
        )
    else:
        _record_runtime_wasm_longdouble_archives("MISSING" if missing else "present")
    return _RelocLongDoubleArchives(
        policy.printscan, policy.builtins, policy.error, policy.warnings
    )


def _link_runtime_staticlib_to_reloc_wasm(
    *,
    staticlib_path: Path,
    output_path: Path,
    json_output: bool,
    link_timeout: float | None,
    export_link_args: str = "",
    long_double_required: bool = False,
) -> bool:
    try:
        wasm_linker = wasm_toolchain.resolve_wasm_linker()
    except wasm_toolchain.WasmLinkerContractError as exc:
        if not json_output:
            print(
                f"Runtime relocatable wasm linker contract failed: {exc}",
                file=sys.stderr,
            )
        return False
    if wasm_linker is None:
        if not json_output:
            print(
                "Runtime relocatable wasm link failed: wasm-ld not found.",
                file=sys.stderr,
            )
        return False
    wasm_ld = str(wasm_linker.path)
    libc_archive = wasm_toolchain.wasm_wasi_libc_archive()
    if libc_archive is None:
        if not json_output:
            print(
                "Runtime relocatable wasm link failed: Rust wasm32-wasip1 libc.a not found.",
                file=sys.stderr,
            )
        return False
    staticlib_path = staticlib_path.resolve(strict=False)
    libc_archive = libc_archive.resolve(strict=False)
    output_path = output_path.resolve(strict=False)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    tmp_output_path = output_path.with_name(
        f".{output_path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp"
    )
    # E1 witness fix (long-double %L trap): wasi-libc's default libc.a stubs the
    # `%L` (long double) printf/scanf conversions with a `long_double_not_supported`
    # abort() that lowers to a raw `unreachable` trap â€” reached by numpy's
    # longdouble repr/parse (NumPyOS_ascii_formatl/strtold) during
    # `_multiarray_umath` import. Whole-archive wasi-libc's companion long-double
    # formatter archive so its real vfprintf/vfscanf/strtod/floatscan override the
    # stub objects (libc.a's stay lazy and are skipped once defined), and add
    # wasi-sdk's compiler-rt builtins so the binary128 soft-float the formatters
    # call (__addtf3/__multf3/â€¦) â€” and numpy's own longdouble arithmetic â€” resolve
    # here instead of degrading to unresolved imports at the final app link.
    #
    # When this runtime links numpy/scipy (``long_double_required``), a missing
    # archive is a HARD ERROR: building a runtime that would relink the abort()
    # stub and trap at import is never acceptable, and the old warn-but-proceed
    # graceful degrade silently masked exactly that regression. Non-numpy / micro
    # builds keep the degrade path (they never hit %L).
    archives = _resolve_reloc_long_double_archives(
        long_double_required=long_double_required
    )
    if archives.error is not None:
        if not json_output:
            print(archives.error, file=sys.stderr)
        return False
    if not json_output:
        for warning in archives.warnings:
            print(warning, file=sys.stderr)
    # Single authority (reloc arm): whole-archive the staticlib + printscan's
    # real long-double formatters ahead of libc.a; libc.a + builtins stay lazy.
    long_double_argv = wasm_toolchain.long_double_whole_archive_link_argv(
        wasm_toolchain.LongDoubleLinkPolicy(
            archives.longdouble, archives.builtins, archives.error, archives.warnings
        ),
        whole_archive=[str(staticlib_path)],
        trailing=[str(libc_archive)],
    )
    export_args = _wasm_link_args_from_rustflags(export_link_args)
    if export_args:
        export_response_path = _write_wasm_link_args_response_file(
            output_path.parent / ".molt_link_args",
            label=f"{output_path.stem}.reloc",
            link_args=export_args,
        )
        export_args = [f"@{export_response_path}"]
    try:
        process = _run_completed_command(
            [
                wasm_ld,
                "-r",
                *export_args,
                *long_double_argv,
                "-o",
                str(tmp_output_path),
            ],
            cwd=output_path.parent,
            env=None,
            capture_output=True,
            memory_guard_prefix="MOLT_WASM_LINK",
            timeout=link_timeout,
        )
        if process.returncode != 0:
            if not json_output:
                err = (process.stderr or "").strip() or (process.stdout or "").strip()
                msg = "Runtime relocatable wasm link failed"
                if err:
                    msg = f"{msg}: {err}"
                print(msg, file=sys.stderr)
            return False
        if not _is_valid_runtime_wasm_artifact(tmp_output_path):
            if not json_output:
                print(
                    f"Runtime relocatable wasm artifact is invalid/incomplete: {tmp_output_path}",
                    file=sys.stderr,
                )
            return False
        os.replace(tmp_output_path, output_path)
        if os.name == "posix":
            with contextlib.suppress(OSError):
                dir_fd = os.open(output_path.parent, os.O_RDONLY)
                try:
                    os.fsync(dir_fd)
                finally:
                    os.close(dir_fd)
    finally:
        with contextlib.suppress(OSError):
            if tmp_output_path.exists():
                tmp_output_path.unlink()
    return True


def _runtime_exports_satisfy_for_mode(
    path: Path,
    required_exports: set[str] | frozenset[str] | None,
    *,
    reloc: bool,
) -> bool:
    if reloc:
        return _runtime_wasm_exports_satisfy(path, required_exports)
    return _split_runtime_wasm_exports_satisfy(path, required_exports)


def _runtime_missing_exports_for_mode(
    path: Path,
    required_exports: set[str] | frozenset[str] | None,
    *,
    reloc: bool,
) -> set[str]:
    if reloc:
        return _runtime_wasm_missing_exports(path, required_exports)
    return _split_runtime_wasm_missing_exports(path, required_exports)
