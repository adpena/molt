from __future__ import annotations

from dataclasses import dataclass
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, cast

from molt.cli.artifact_state import (
    _artifact_state_path,
    _artifact_state_path_for_build_state_root,
    _canonical_build_state_root,
    _canonical_target_root,
    _maybe_hydrate_artifact_from_canonical_target,
)
from molt.cli.atomic_io import _atomic_copy_file
from molt.cli.backend_execution import _DEFAULT_BACKEND_FEATURES
from molt.cli.build_locks import _build_lock
from molt.cli.cache_fingerprints import _backend_source_paths
from molt.cli.cargo_execution import (
    _cargo_build_env,
    _maybe_enable_native_cpu,
    _maybe_enable_sccache,
    _run_cargo_with_sccache_retry,
)
from molt.cli.command_runtime import _run_subprocess_captured_to_tempfiles
from molt.cli.compiler_metadata import _rustc_version
from molt.cli.native_toolchain import _codesign_binary
from molt.cli.runtime_fingerprints import (
    _artifact_needs_rebuild,
    _artifact_content_looks_valid,
    _hash_runtime_file,
    _hash_source_tree_metadata,
    _read_runtime_fingerprint,
    _stored_fingerprint_matches_source_metadata,
    _write_runtime_fingerprint,
)
from molt.cli.runtime_paths import _cargo_profile_dir, _cargo_target_root
from molt.cli.setup_readiness import (
    _llvm_backend_unavailable_message,
)
from molt.llvm_toolchain import LlvmToolchainConfigError, required_llvm_backend_pin


@dataclass(frozen=True)
class _BackendBinaryEnsureResult:
    ok: bool
    detail: str | None = None
    returncode: int | None = None
    phase: str | None = None
    command: tuple[str, ...] = ()

    def __bool__(self) -> bool:
        return self.ok

    @property
    def message(self) -> str:
        return self.detail or "Backend build failed"


def _backend_ensure_success() -> _BackendBinaryEnsureResult:
    return _BackendBinaryEnsureResult(ok=True)


def _backend_ensure_failure(
    phase: str,
    detail: str,
    *,
    returncode: int | None = None,
    command: list[str] | tuple[str, ...] = (),
) -> _BackendBinaryEnsureResult:
    return _BackendBinaryEnsureResult(
        ok=False,
        detail=detail,
        returncode=returncode,
        phase=phase,
        command=tuple(command),
    )


def _process_text_tail(value: str | bytes | None, *, limit: int = 4000) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        text = value.decode("utf-8", errors="replace")
    else:
        text = value
    text = text.strip()
    if len(text) <= limit:
        return text
    return f"... <truncated to last {limit} chars>\n{text[-limit:]}"


def _completed_process_failure_detail(
    label: str,
    process: subprocess.CompletedProcess[str] | subprocess.CompletedProcess[bytes],
) -> str:
    rc = process.returncode
    body = _process_text_tail(process.stderr) or _process_text_tail(process.stdout)
    detail = f"{label} failed (exit {rc})"
    if body:
        detail = f"{detail}:\n{body}"
    return detail


def _backend_fingerprint_path(
    project_root: Path,
    artifact: Path,
    cargo_profile: str,
) -> Path:
    return _artifact_state_path(
        project_root,
        artifact,
        subdir="backend_fingerprints",
        stem_suffix=f"{cargo_profile}",
        extension="fingerprint",
    )


def _backend_fingerprint(
    project_root: Path,
    *,
    cargo_profile: str,
    rustflags: str,
    backend_features: tuple[str, ...],
    stored_fingerprint: dict[str, Any] | None = None,
) -> dict[str, str | None] | None:
    meta = f"profile:{cargo_profile}\n"
    meta += f"rustflags:{rustflags}\n"
    meta += f"features:{','.join(backend_features)}\n"
    meta_digest = hashlib.sha256(meta.encode("utf-8")).hexdigest()
    source_paths = _backend_source_paths(project_root, backend_features)
    rustc_info = _rustc_version()
    inputs_meta = _hash_source_tree_metadata(source_paths, project_root)
    inputs_digest = inputs_meta[0] if inputs_meta is not None else None
    if _stored_fingerprint_matches_source_metadata(
        stored_fingerprint,
        inputs_digest=inputs_digest,
        rustc=rustc_info,
        meta_digest=meta_digest,
    ):
        assert stored_fingerprint is not None
        return {
            "hash": cast(str, stored_fingerprint.get("hash")),
            "rustc": rustc_info,
            "inputs_digest": inputs_digest,
            "meta_digest": meta_digest,
        }

    hasher = hashlib.sha256()
    hasher.update(meta.encode("utf-8"))
    try:
        for path in sorted(source_paths, key=lambda p: str(p)):
            if path.is_dir():
                for item in sorted(path.rglob("*"), key=lambda p: str(p)):
                    if item.is_file():
                        _hash_runtime_file(item, project_root, hasher)
            elif path.exists():
                _hash_runtime_file(path, project_root, hasher)
    except OSError:
        return None
    return {
        "hash": hasher.hexdigest(),
        "rustc": rustc_info,
        "inputs_digest": inputs_digest,
        "meta_digest": meta_digest,
    }


def _ensure_backend_binary(
    backend_bin: Path,
    *,
    cargo_timeout: float | None,
    json_output: bool,
    cargo_profile: str,
    project_root: Path,
    backend_features: tuple[str, ...],
) -> _BackendBinaryEnsureResult:
    # MOLT_SKIP_RUNTIME_REBUILD=1 also skips the backend fingerprint check.
    if os.environ.get("MOLT_SKIP_RUNTIME_REBUILD") == "1":
        if backend_bin.exists():
            return _backend_ensure_success()
    rustflags = os.environ.get("RUSTFLAGS", "")
    fingerprint_path = _backend_fingerprint_path(
        project_root, backend_bin, cargo_profile
    )
    stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
    fingerprint = _backend_fingerprint(
        project_root,
        cargo_profile=cargo_profile,
        rustflags=rustflags,
        backend_features=backend_features,
        stored_fingerprint=stored_fingerprint,
    )
    features_tag = "_".join(sorted(backend_features)) if backend_features else "default"
    lock_name = f"backend.{cargo_profile}.{features_tag}"
    with _build_lock(project_root, lock_name):

        def _canonical_cargo_backend_output() -> Path:
            exe_suffix = ".exe" if os.name == "nt" else ""
            return backend_bin.parent / f"molt-backend{exe_suffix}"

        def _materialize_backend_binary_from(source: Path) -> bool:
            if not source.exists():
                return False
            if source != backend_bin:
                _atomic_copy_file(source, backend_bin, codesign=True)
            else:
                _codesign_binary(backend_bin)
            return backend_bin.exists()

        def _materialize_rebuilt_backend_binary() -> bool:
            return _materialize_backend_binary_from(_canonical_cargo_backend_output())

        def _backend_probe_target() -> str:
            if "wasm-backend" in backend_features:
                return "wasm"
            if "luau-backend" in backend_features:
                return "luau"
            if "rust-backend" in backend_features:
                return "rust"
            return "native"

        def _probe_backend_binary_support(
            probe_target: str,
            *,
            binary_path: Path | None = None,
        ) -> tuple[bool, str]:
            probe_ir = json.dumps(
                {
                    "functions": [],
                    "module": "__probe__",
                    "entry": "main",
                    "metadata": {"target": probe_target, "deterministic": True},
                }
            ).encode()
            probe_suffix = ".o"
            if probe_target == "wasm":
                probe_suffix = ".wasm"
            elif probe_target == "luau":
                probe_suffix = ".luau"
            elif probe_target == "rust":
                probe_suffix = ".rs"
            probe_tmp = tempfile.NamedTemporaryFile(
                prefix="molt_backend_probe_",
                suffix=probe_suffix,
                delete=False,
            )
            probe_path = Path(probe_tmp.name)
            probe_tmp.close()
            probe_cmd = [str(binary_path or backend_bin), "--output", str(probe_path)]
            if probe_target == "wasm":
                probe_cmd.extend(["--target", "wasm"])
            elif probe_target == "luau":
                probe_cmd.extend(["--target", "luau"])
            elif probe_target == "rust":
                probe_cmd.extend(["--target", "rust"])
            try:
                probe = _run_subprocess_captured_to_tempfiles(
                    probe_cmd,
                    input=probe_ir,
                    cwd=project_root,
                    timeout=10,
                    memory_guard_prefix="MOLT_BUILD",
                )
            except (subprocess.TimeoutExpired, OSError) as exc:
                return False, str(exc)
            finally:
                try:
                    probe_path.unlink()
                except OSError:
                    pass
            stderr = probe.stderr.decode(errors="replace")
            stdout = probe.stdout.decode(errors="replace")
            return probe.returncode == 0, (stderr or stdout).strip()

        def _refresh_feature_tagged_backend_alias(probe_target: str) -> None:
            if backend_features == _DEFAULT_BACKEND_FEATURES:
                return
            cargo_output = _canonical_cargo_backend_output()
            if cargo_output == backend_bin or not cargo_output.exists():
                return
            try:
                cargo_mtime = cargo_output.stat().st_mtime_ns
            except OSError:
                return
            try:
                alias_mtime = backend_bin.stat().st_mtime_ns
            except OSError:
                alias_mtime = -1
            if alias_mtime >= cargo_mtime:
                return
            probe_ok, _probe_detail = _probe_backend_binary_support(
                probe_target,
                binary_path=cargo_output,
            )
            if probe_ok:
                _materialize_backend_binary_from(cargo_output)

        if stored_fingerprint is None:
            stored_fingerprint = _read_runtime_fingerprint(fingerprint_path)
        if not _artifact_needs_rebuild(backend_bin, fingerprint, stored_fingerprint):
            # Force a real compile-path probe. An empty stdin-only probe can
            # miss feature-lane poisoning because it never exercises output
            # emission for the requested target.
            _quick_target = _backend_probe_target()
            _refresh_feature_tagged_backend_alias(_quick_target)
            _probe_ok, _probe_detail = _probe_backend_binary_support(_quick_target)
            if _probe_ok:
                return _backend_ensure_success()
        canonical_target_root = _canonical_target_root(project_root)
        canonical_backend_bin = (
            canonical_target_root / _cargo_profile_dir(cargo_profile) / backend_bin.name
        )
        canonical_fingerprint_path = _artifact_state_path_for_build_state_root(
            _canonical_build_state_root(project_root),
            canonical_backend_bin,
            subdir="backend_fingerprints",
            stem_suffix=f"{cargo_profile}",
            extension="fingerprint",
        )
        if _maybe_hydrate_artifact_from_canonical_target(
            artifact=backend_bin,
            fingerprint=fingerprint,
            fingerprint_path=fingerprint_path,
            candidate_artifact=canonical_backend_bin,
            candidate_fingerprint_path=canonical_fingerprint_path,
        ):
            _probe_target = _backend_probe_target()
            _probe_ok, _probe_detail = _probe_backend_binary_support(_probe_target)
            if _probe_ok:
                return _backend_ensure_success()
        # Cargo always writes the executable as `molt-backend`; Molt keeps
        # feature-specific aliases beside it so native/wasm/rust lanes cannot
        # poison each other.  When CI or a developer prebuilds the correct
        # feature lane with cargo, materialize the alias after probing the
        # canonical binary instead of rebuilding the backend.
        if backend_features != _DEFAULT_BACKEND_FEATURES:
            cargo_output = _canonical_cargo_backend_output()
            if _artifact_newer_than_sources(
                cargo_output,
                _backend_source_paths(project_root, backend_features),
            ):
                _probe_target = _backend_probe_target()
                _probe_ok, _probe_detail = _probe_backend_binary_support(
                    _probe_target,
                    binary_path=cargo_output,
                )
                if _probe_ok and _materialize_backend_binary_from(cargo_output):
                    if fingerprint is not None:
                        try:
                            fingerprint_path.parent.mkdir(
                                parents=True,
                                exist_ok=True,
                            )
                            _write_runtime_fingerprint(fingerprint_path, fingerprint)
                        except OSError:
                            if not json_output:
                                print(
                                    "Warning: failed to write backend fingerprint metadata.",
                                    file=sys.stderr,
                                )
                    return _backend_ensure_success()
        # Fast path: if the backend binary exists and is newer than every
        # source file that contributes to the fingerprint, skip the expensive
        # cargo build and just update the stored fingerprint.  This handles
        # the common case of running `cargo build` manually before `molt build`.
        if _artifact_newer_than_sources(
            backend_bin,
            _backend_source_paths(project_root, backend_features),
        ):
            _probe_target = _backend_probe_target()
            _probe_ok, _probe_detail = _probe_backend_binary_support(_probe_target)
            if _probe_ok:
                assert fingerprint is not None
                _write_runtime_fingerprint(fingerprint_path, fingerprint)
                return _backend_ensure_success()
        if not json_output:
            print("Backend sources changed; rebuilding backend...", file=sys.stderr)
        if "llvm" in backend_features:
            llvm_message = _llvm_backend_unavailable_message(project_root)
            if llvm_message is not None:
                return _backend_ensure_failure("backend_toolchain", llvm_message)
        # Cache entries include backend/tooling/runtime identity in their keys.
        # A backend rebuild therefore invalidates by selecting new keys, not by
        # deleting shared immutable cache artifacts that concurrent sessions may
        # still be reading. Size/age retention belongs to `molt clean`.
        cmd = [
            "cargo",
            "build",
            "--package",
            "molt-backend",
            "--profile",
            cargo_profile,
        ]
        if backend_features:
            cmd.append("--no-default-features")
            cmd.extend(["--features", ",".join(backend_features)])
        build_env = _cargo_build_env()
        # Per-session build isolation: route cargo output to
        # target/sessions/<id>/ under the canonical target root
        # when MOLT_SESSION_ID is active to prevent concurrent agents from
        # clobbering each other's backend artifacts.
        build_env["CARGO_TARGET_DIR"] = str(_cargo_target_root(project_root))
        # When building with the LLVM feature, ensure the pinned llvm-sys
        # prefix env var points at the matching Homebrew install so
        # inkwell/llvm-sys can link without extra shell setup.
        if "llvm" in backend_features:
            try:
                llvm_pin = required_llvm_backend_pin(project_root)
            except LlvmToolchainConfigError:
                llvm_pin = None
            if llvm_pin is not None and llvm_pin.env_var not in build_env:
                llvm_prefix = f"/opt/homebrew/opt/llvm@{llvm_pin.major}"
                if os.path.isdir(llvm_prefix):
                    build_env[llvm_pin.env_var] = llvm_prefix
        _maybe_enable_sccache(build_env)
        _maybe_enable_native_cpu(build_env)
        try:
            build = _run_cargo_with_sccache_retry(
                cmd,
                cwd=project_root,
                env=build_env,
                timeout=cargo_timeout,
                json_output=json_output,
                label="Backend build",
            )
        except subprocess.TimeoutExpired:
            timeout_note = (
                f"Backend build timed out after {cargo_timeout:.1f}s."
                if cargo_timeout is not None
                else "Backend build timed out."
            )
            return _backend_ensure_failure(
                "backend_cargo_build",
                timeout_note,
                command=cmd,
            )
        if build.returncode != 0:
            return _backend_ensure_failure(
                "backend_cargo_build",
                _completed_process_failure_detail("Backend cargo build", build),
                returncode=build.returncode,
                command=cmd,
            )
        # Cargo always produces target/<profile>/molt-backend regardless of
        # features.  When the requested feature set is non-default, copy
        # the freshly-built binary to the feature-tagged path so that
        # concurrent or sequential builds with different feature sets
        # (native vs wasm vs rust) do not overwrite each other.
        if not _materialize_rebuilt_backend_binary():
            return _backend_ensure_failure(
                "backend_artifact",
                "Backend binary missing after rebuild.",
                command=cmd,
            )
        # -- Post-build feature probe (defense-in-depth) -----------------
        # Cargo's incremental cache may skip recompilation when only
        # features change, leaving a binary built for the wrong target.
        # Probe the binary and, on mismatch, clean + rebuild once.
        _probe_target = _backend_probe_target()
        _probe_ok, _probe_detail = _probe_backend_binary_support(_probe_target)
        if not _probe_ok:
            if not json_output:
                print(
                    "Backend feature mismatch detected; cleaning and rebuilding...",
                    file=sys.stderr,
                )
            # Skip cargo clean: the deterministic rebuild path plus post-build
            # feature probe is the authority, while cargo clean would hold the
            # Cargo lock and block concurrent sessions.
            try:
                rebuild = _run_cargo_with_sccache_retry(
                    cmd,
                    cwd=project_root,
                    env=build_env,
                    timeout=cargo_timeout,
                    json_output=json_output,
                    label="Backend rebuild (feature fix)",
                )
            except subprocess.TimeoutExpired:
                return _backend_ensure_failure(
                    "backend_feature_rebuild",
                    "Backend rebuild timed out.",
                    command=cmd,
                )
            if rebuild.returncode != 0:
                return _backend_ensure_failure(
                    "backend_feature_rebuild",
                    _completed_process_failure_detail(
                        "Backend feature rebuild", rebuild
                    ),
                    returncode=rebuild.returncode,
                    command=cmd,
                )
            if not _materialize_rebuilt_backend_binary():
                return _backend_ensure_failure(
                    "backend_artifact",
                    "Backend binary missing after rebuild.",
                    command=cmd,
                )
            _reprobe_ok, _reprobe_detail = _probe_backend_binary_support(_probe_target)
            if not _reprobe_ok:
                detail = "Backend feature probe failed after rebuild."
                if _reprobe_detail:
                    detail = f"{detail}\n{_reprobe_detail}"
                return _backend_ensure_failure(
                    "backend_feature_probe",
                    detail,
                    command=cmd,
                )
        # -- End post-build feature probe --------------------------------
        if fingerprint is not None:
            try:
                fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
                _write_runtime_fingerprint(fingerprint_path, fingerprint)
            except OSError:
                if not json_output:
                    print(
                        "Warning: failed to write backend fingerprint metadata.",
                        file=sys.stderr,
                    )
    return _backend_ensure_success()


def _artifact_newer_than_sources(
    artifact: Path,
    source_paths: list[Path],
) -> bool:
    """Return True if *artifact* exists and is newer than every file in *source_paths*.

    Handles both individual files and directories (recursed for all files).
    Returns False on any OS error or if no source files are found.
    """
    try:
        lib_mtime = artifact.stat().st_mtime
    except OSError:
        return False
    if not _artifact_content_looks_valid(artifact):
        return False
    newest_src = 0.0
    for path in source_paths:
        try:
            if path.is_dir():
                for item in path.rglob("*"):
                    if item.is_file():
                        newest_src = max(newest_src, item.stat().st_mtime)
            elif path.exists():
                newest_src = max(newest_src, path.stat().st_mtime)
        except OSError:
            return False
    return newest_src > 0 and lib_mtime > newest_src
