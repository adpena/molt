from __future__ import annotations

from molt.native_target import NativeTargetSpec
from molt.compilation_target import target_name, Target, TargetKind, is_wasm_target

from typing import get_args
from molt.cli.build_request import BuildProfile

from contextlib import ExitStack, contextmanager
from contextvars import ContextVar
from dataclasses import dataclass, field
import functools
import hashlib
import json
import os
import sys
from collections.abc import Generator, Mapping
from pathlib import Path
import re

from molt.cli.config_resolution import AUTO_STDLIB_PROFILE
from molt.default_paths import (
    _default_molt_bin,
    _default_molt_cache,
    _default_molt_home,
)
from molt.dx import configured_artifact_root, project_cargo_target_dir


@dataclass(frozen=True)
class _BuildOutputLayout:
    is_wasm: bool
    is_wasm_freestanding: bool
    is_rust_transpile: bool
    is_luau_transpile: bool
    is_mlir_emit: bool
    split_runtime: bool
    linked: bool
    target_triple: str | None
    emit_mode: str
    output_artifact: Path
    output_binary: Path | None
    linked_output_path: Path | None
    emit_ir_path: Path | None


_OUTPUT_BASE_SAFE_RE = re.compile(r"[^A-Za-z0-9._-]+")
# The default runtime WASM directory inside a project's Cargo target.
WASM_RUNTIME_TARGET_DIR = "molt-runtime-wasm"

_DEPLOY_PROFILE_DEFAULTS: dict[str, dict[str, object]] = {
    "cloudflare": {
        "wasm_opt_level": "Oz",
        "wasm_profile": "pure",
        "precompile": False,
        "tmp_quota_mb": 32,
        "stdlib_profile": AUTO_STDLIB_PROFILE,
    },
    "browser": {
        "wasm_opt_level": "Oz",
        "wasm_profile": "auto",
        "precompile": False,
        "split_runtime": True,
        "tmp_quota_mb": 64,
        "stdlib_profile": AUTO_STDLIB_PROFILE,
    },
    "wasi": {
        "wasm_opt_level": "O3",
        "wasm_profile": "auto",
        "precompile": False,
        "tmp_quota_mb": 256,
        "stdlib_profile": AUTO_STDLIB_PROFILE,
    },
    "fastly": {
        "wasm_opt_level": "Oz",
        "wasm_profile": "pure",
        "precompile": False,
        "tmp_quota_mb": 64,
        "stdlib_profile": AUTO_STDLIB_PROFILE,
    },
}
_BUILD_PROFILE_CHOICES = get_args(BuildProfile)
_DEPLOY_PROFILE_CHOICES = tuple(_DEPLOY_PROFILE_DEFAULTS)
_BUILD_OR_DEPLOY_PROFILE_CHOICES = (*_BUILD_PROFILE_CHOICES, *_DEPLOY_PROFILE_CHOICES)


@functools.lru_cache(maxsize=512)
def _safe_output_base(name: str) -> str:
    cleaned = _OUTPUT_BASE_SAFE_RE.sub("_", name)
    return cleaned or "molt"


def _wasm_runtime_root(project_root: Path) -> Path:
    return _wasm_runtime_root_from_env(project_root, os.environ)


def _wasm_runtime_root_from_env(project_root: Path, env: Mapping[str, str]) -> Path:
    """The one root of runtime WASM artifacts: build, link and tools agree on it.

    An explicit ``MOLT_WASM_RUNTIME_DIR`` wins. An artifact root
    (``MOLT_EXT_ROOT``) holds one content-addressed runtime directory that its
    checkouts share. Otherwise the runtime sits in the project's Cargo target
    beside the native runtime libraries, so a build never writes into the
    source tree or the working directory.
    """
    explicit = env.get("MOLT_WASM_RUNTIME_DIR", "").strip()
    if explicit:
        return Path(explicit).expanduser()
    external = configured_artifact_root(env, relative_to=project_root)
    if external is not None:
        return external / "wasm"
    return project_cargo_target_dir(project_root, env) / WASM_RUNTIME_TARGET_DIR


@functools.lru_cache(maxsize=8)
def _default_build_base_cached(home_root_str: str) -> Path:
    return Path(home_root_str) / "build"


def _default_build_base() -> Path:
    """Where builds without ``--out-dir`` keep their intermediates."""
    return _default_build_base_cached(os.fspath(_default_molt_home()))


def artifacts_identity(
    *, project_root: Path, source_path: Path, target: Target, profile: str
) -> str:
    """Name one build configuration's intermediates.

    Two builds share an artifacts root only when they build the same entry of
    the same project for the same target and profile, as Bazel keys an output
    base by its workspace. An entry's file name alone is shared by unrelated
    programs, which then linked each other's objects.
    """
    material = json.dumps(
        {
            "profile": profile,
            "project": os.fspath(project_root.resolve()),
            "source": os.fspath(source_path.resolve()),
            "target": target_name(target),
        },
        sort_keys=True,
    )
    return hashlib.sha256(material.encode("utf-8")).hexdigest()[:16]


@dataclass
class _BuildScope:
    stack: ExitStack
    claimed: set[Path] = field(default_factory=set)


_BUILD_SCOPE: ContextVar[_BuildScope | None] = ContextVar(
    "molt_build_scope", default=None
)


@contextmanager
def build_scope() -> Generator[None]:
    """Hold one build's claims on its artifacts roots until the build returns."""
    with ExitStack() as stack:
        token = _BUILD_SCOPE.set(_BuildScope(stack))
        try:
            yield
        finally:
            _BUILD_SCOPE.reset(token)


def _claim_artifacts_root(root: Path) -> None:
    """Serialize concurrent builds of one configuration, as Cargo locks its
    build directory, so no build reads another's half-written objects."""
    scope = _BUILD_SCOPE.get()
    if scope is None:
        raise RuntimeError("an artifacts root is claimed only inside build_scope()")
    if root in scope.claimed:
        return
    from molt.file_locks import (
        _acquire_file_lock,
        _release_file_lock,
        _try_acquire_file_lock,
    )

    lock_path = root / ".molt-build.lock"
    handle = _try_acquire_file_lock(lock_path)
    if handle is None:
        print(
            f"molt: waiting for another build of this program to release {root}",
            file=sys.stderr,
        )
        handle = _acquire_file_lock(
            lock_path,
            timeout_s=None,
            timeout_message=f"artifacts root stayed claimed: {root}",
        )
    scope.claimed.add(root)
    scope.stack.callback(_release_file_lock, handle)


@functools.lru_cache(maxsize=256)
def _resolve_cache_root_cached(
    project_root_str: str, cache_dir: str | None, default_cache_str: str
) -> Path:
    if not cache_dir:
        return Path(default_cache_str)
    project_root = Path(project_root_str)
    path = Path(cache_dir).expanduser()
    if not path.is_absolute():
        path = (project_root / path).absolute()
    return path


def _resolve_cache_root(project_root: Path, cache_dir: str | None) -> Path:
    return _resolve_cache_root_cached(
        os.fspath(project_root),
        cache_dir,
        "" if cache_dir else os.fspath(_default_molt_cache()),
    )


@functools.lru_cache(maxsize=256)
def _resolve_out_dir_cached(
    project_root_str: str,
    out_dir: str | None,
) -> Path | None:
    if not out_dir:
        return None
    project_root = Path(project_root_str)
    path = Path(out_dir).expanduser()
    if not path.is_absolute():
        path = (project_root / path).absolute()
    return path


def _resolve_out_dir(project_root: Path, out_dir: str | Path | None) -> Path | None:
    if not out_dir:
        return None
    path = _resolve_out_dir_cached(os.fspath(project_root), os.fspath(out_dir))
    assert path is not None
    path.mkdir(parents=True, exist_ok=True)
    return path


@functools.lru_cache(maxsize=256)
def _resolve_sysroot_cached(
    project_root_str: str,
    sysroot: str | None,
    env_sysroot: str | None,
) -> Path | None:
    raw = sysroot or env_sysroot
    if not raw:
        return None
    project_root = Path(project_root_str)
    path = Path(raw).expanduser()
    if not path.is_absolute():
        path = (project_root / path).absolute()
    return path


def _resolve_sysroot(project_root: Path, sysroot: str | None) -> Path | None:
    return _resolve_sysroot_cached(
        os.fspath(project_root),
        sysroot,
        os.environ.get("MOLT_SYSROOT"),
    )


def _resolve_output_roots(
    project_root: Path, out_dir: Path | None, output_base: str, *, identity: str
) -> tuple[Path, Path, Path]:
    """The build's artifacts, binary and output roots; claims the artifacts root.

    ``identity`` (``artifacts_identity``) keeps two configurations' objects
    apart; the claim keeps two builds of one configuration apart.
    """
    name = f"{_safe_output_base(output_base)}-{identity}"
    if out_dir is not None:
        artifacts_root = out_dir / ".molt_build" / name
        bin_root = out_dir
        output_root = out_dir
    else:
        artifacts_root = _default_build_base() / name
        bin_root = _default_molt_bin()
        output_root = project_root / "dist"

    # The default binary and output roots are created only when a default
    # destination is chosen (`_resolve_output_path`): a build that names its
    # output leaves no empty `dist/` or binary directory behind.
    _mkdir_resilient(artifacts_root)
    _claim_artifacts_root(artifacts_root)
    return artifacts_root, bin_root, output_root


def _repair_broken_symlink_parents(path: Path) -> bool:
    repaired = False
    chain = list(path.parents)
    chain.reverse()
    for parent in chain:
        try:
            if parent.is_symlink() and not parent.exists():
                parent.unlink()
                parent.mkdir(parents=True, exist_ok=True)
                repaired = True
        except OSError:
            continue
    return repaired


def _mkdir_resilient(path: Path) -> None:
    try:
        path.mkdir(parents=True, exist_ok=True)
    except (FileExistsError, NotADirectoryError):
        if _repair_broken_symlink_parents(path):
            path.mkdir(parents=True, exist_ok=True)
        else:
            raise


def _resolve_output_path(
    output: str | None,
    default: Path,
    *,
    out_dir: Path | None,
    project_root: Path,
) -> Path:
    if not output:
        _mkdir_resilient(default.parent)
        return default
    path = Path(output).expanduser()
    if not path.is_absolute():
        base = out_dir if out_dir is not None else project_root
        path = base / path
    if output.endswith(os.sep) or (os.altsep and output.endswith(os.altsep)):
        return path / default.name
    try:
        if path.exists() and path.is_dir():
            return path / default.name
    except OSError:
        pass
    return path


def _backend_artifact_suffix(*, target: Target, emit_mode: str) -> str:
    """Name producer bytes consistently in user outputs and backend caches."""
    from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract

    return resolve_backend_artifact_contract(target=target, emit_mode=emit_mode).suffix


def _resolve_build_output_layout(
    *,
    target: Target,
    trusted: bool,
    split_runtime: bool = False,
    require_linked: bool,
    linked: bool,
    linked_output: str | None,
    emit: str | None,
    output: str | None,
    emit_ir: str | None,
    artifacts_root: Path,
    bin_root: Path,
    output_root: Path,
    output_base: str,
    out_dir_path: Path | None,
    project_root: Path,
) -> _BuildOutputLayout:
    is_wasm = is_wasm_target(target)
    is_wasm_freestanding = target in {
        TargetKind.WASM_FREESTANDING,
        TargetKind.WASM_UNKNOWN,
    }
    is_rust_transpile = target in {TargetKind.RUST, TargetKind.LUAU}
    is_luau_transpile = target is TargetKind.LUAU
    is_mlir_emit = target is TargetKind.MLIR
    if trusted and is_wasm:
        raise ValueError("Trusted mode is not supported for wasm targets")
    if require_linked and not is_wasm:
        raise ValueError("--require-linked is only supported for wasm targets")
    if linked_output and is_wasm:
        linked = True
    if linked_output and not linked and not require_linked:
        raise ValueError("--linked-output requires --linked")
    if linked and not is_wasm and not is_rust_transpile:
        raise ValueError("Linked output is only supported for wasm targets")
    if require_linked and not linked:
        linked = True
    if split_runtime and is_wasm:
        linked = True
    if is_wasm and not linked:
        wasm_linked_env = os.environ.get("MOLT_WASM_LINKED", "1").strip().lower()
        if wasm_linked_env not in {"0", "false", "no", "off"}:
            linked = True
    target_triple = (
        target.triple
        if isinstance(target, NativeTargetSpec)
        else target.value
        if target in {TargetKind.WASI, TargetKind.WASM_UNKNOWN}
        else None
    )
    is_transpile = is_rust_transpile or is_luau_transpile
    emit_mode = (
        "bin"
        if is_transpile or is_mlir_emit
        else (emit or ("wasm" if is_wasm else "bin"))
    )
    if (
        not is_transpile
        and not is_mlir_emit
        and emit_mode not in {"bin", "obj", "wasm"}
    ):
        raise ValueError(f"Invalid emit mode: {emit_mode}")
    if is_wasm and emit_mode != "wasm":
        raise ValueError(f"Invalid emit mode for wasm target: {emit_mode}")
    if not is_wasm and not is_transpile and not is_mlir_emit and emit_mode == "wasm":
        raise ValueError("emit=wasm requires --target wasm")

    artifact_suffix = _backend_artifact_suffix(target=target, emit_mode=emit_mode)
    output_binary: Path | None = None
    linked_output_path: Path | None = None
    if is_mlir_emit:
        output_artifact = _resolve_output_path(
            output,
            output_root / f"{output_base}{artifact_suffix}",
            out_dir=out_dir_path,
            project_root=project_root,
        )
    elif is_luau_transpile:
        output_artifact = _resolve_output_path(
            output,
            output_root / f"{output_base}{artifact_suffix}",
            out_dir=out_dir_path,
            project_root=project_root,
        )
    elif is_rust_transpile:
        output_artifact = _resolve_output_path(
            output,
            output_root / f"{output_base}{artifact_suffix}",
            out_dir=out_dir_path,
            project_root=project_root,
        )
    elif is_wasm:
        output_wasm = _resolve_output_path(
            output,
            output_root / f"output{artifact_suffix}",
            out_dir=out_dir_path,
            project_root=project_root,
        )
        output_artifact = output_wasm
        if linked:
            stem = output_wasm.stem
            if stem.endswith("_linked"):
                stem = stem[: -len("_linked")]
            linked_output_path = output_wasm.with_name(
                f"{stem}_linked{output_wasm.suffix}"
            )
            if linked_output is not None:
                linked_output_path = _resolve_output_path(
                    linked_output,
                    linked_output_path,
                    out_dir=out_dir_path,
                    project_root=project_root,
                )
    else:
        output_obj = artifacts_root / f"output{artifact_suffix}"
        if emit_mode == "obj":
            output_obj = _resolve_output_path(
                output,
                output_root / f"output{artifact_suffix}",
                out_dir=out_dir_path,
                project_root=project_root,
            )
        output_artifact = output_obj
        if emit_mode == "bin":
            output_binary = _resolve_output_path(
                output,
                bin_root / f"{output_base}_molt",
                out_dir=out_dir_path,
                project_root=project_root,
            )
    for path in (output_artifact, output_binary):
        if path is not None and path.parent != Path("."):
            path.parent.mkdir(parents=True, exist_ok=True)
    emit_ir_path: Path | None = None
    if emit_ir:
        emit_ir_path = Path(emit_ir)
        if not emit_ir_path.is_absolute():
            emit_ir_path = artifacts_root / emit_ir_path
        if emit_ir_path.parent != Path("."):
            emit_ir_path.parent.mkdir(parents=True, exist_ok=True)
    return _BuildOutputLayout(
        is_wasm=is_wasm,
        is_wasm_freestanding=is_wasm_freestanding,
        is_rust_transpile=is_rust_transpile,
        is_luau_transpile=is_luau_transpile,
        is_mlir_emit=is_mlir_emit,
        split_runtime=split_runtime,
        linked=linked,
        target_triple=target_triple,
        emit_mode=emit_mode,
        output_artifact=output_artifact,
        output_binary=output_binary,
        linked_output_path=linked_output_path,
        emit_ir_path=emit_ir_path,
    )
