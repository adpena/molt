from __future__ import annotations

import functools
import os
from pathlib import Path
import re
import sys

from molt.cli.default_paths import (
    _default_home_str,
    _default_molt_bin,
    _default_molt_cache_cached,
    _default_molt_home_cached,
)
from molt.cli.models import _BuildOutputLayout

_OUTPUT_BASE_SAFE_RE = re.compile(r"[^A-Za-z0-9._-]+")

_DEPLOY_PROFILE_DEFAULTS: dict[str, dict[str, object]] = {
    "cloudflare": {
        "wasm_opt_level": "Oz",
        "wasm_profile": "pure",
        "precompile": True,
        "tmp_quota_mb": 32,
        "stdlib_profile": "micro",
    },
    "browser": {
        "wasm_opt_level": "Oz",
        "wasm_profile": "auto",
        "precompile": False,
        "tmp_quota_mb": 64,
        "stdlib_profile": "micro",
    },
    "wasi": {
        "wasm_opt_level": "O3",
        "wasm_profile": "auto",
        "precompile": False,
        "tmp_quota_mb": 256,
        "stdlib_profile": "full",
    },
    "fastly": {
        "wasm_opt_level": "Oz",
        "wasm_profile": "pure",
        "precompile": True,
        "tmp_quota_mb": 64,
        "stdlib_profile": "micro",
    },
}
_BUILD_PROFILE_CHOICES = ("dev", "release")
_DEPLOY_PROFILE_CHOICES = tuple(_DEPLOY_PROFILE_DEFAULTS)
_BUILD_OR_DEPLOY_PROFILE_CHOICES = (*_BUILD_PROFILE_CHOICES, *_DEPLOY_PROFILE_CHOICES)


@functools.lru_cache(maxsize=512)
def _safe_output_base(name: str) -> str:
    cleaned = _OUTPUT_BASE_SAFE_RE.sub("_", name)
    return cleaned or "molt"


@functools.lru_cache(maxsize=128)
def _wasm_runtime_root_cached(
    project_root_str: str,
    env_root: str | None,
    ext_root: str | None,
    cwd_str: str,
) -> Path:
    if env_root:
        return Path(env_root).expanduser()
    project_root = Path(project_root_str)
    external_root = Path(ext_root).expanduser() if ext_root else Path(cwd_str)
    if external_root.is_dir():
        return external_root / "wasm"
    return project_root / "wasm"


def _wasm_runtime_root(project_root: Path) -> Path:
    return _wasm_runtime_root_cached(
        os.fspath(project_root),
        os.environ.get("MOLT_WASM_RUNTIME_DIR"),
        os.environ.get("MOLT_EXT_ROOT"),
        os.fspath(Path.cwd()),
    )


@functools.lru_cache(maxsize=256)
def _default_build_root_cached(
    output_base: str,
    home_override: str | None,
    cache_override: str | None,
    xdg_cache_home: str | None,
    cwd_str: str,
    home_str: str | None,
    platform_name: str,
    ext_root_str: str | None,
) -> Path:
    safe_base = _safe_output_base(output_base)
    home_root = _default_molt_home_cached(
        home_override,
        cache_override,
        xdg_cache_home,
        cwd_str,
        home_str,
        platform_name,
        ext_root_str,
    )
    return home_root / "build" / safe_base


def _default_build_root(output_base: str) -> Path:
    return _default_build_root_cached(
        output_base,
        os.environ.get("MOLT_HOME"),
        os.environ.get("MOLT_CACHE"),
        os.environ.get("XDG_CACHE_HOME"),
        os.fspath(Path.cwd()),
        _default_home_str(),
        sys.platform,
        os.environ.get("MOLT_EXT_ROOT"),
    )


@functools.lru_cache(maxsize=256)
def _resolve_cache_root_cached(
    project_root_str: str,
    cache_dir: str | None,
    cache_override: str | None,
    xdg_cache_home: str | None,
    cwd_str: str,
    home_str: str | None,
    platform_name: str,
    ext_root_str: str | None,
) -> Path:
    if not cache_dir:
        return _default_molt_cache_cached(
            cache_override,
            xdg_cache_home,
            cwd_str,
            home_str,
            platform_name,
            ext_root_str,
        )
    project_root = Path(project_root_str)
    path = Path(cache_dir).expanduser()
    if not path.is_absolute():
        path = (project_root / path).absolute()
    return path


def _resolve_cache_root(project_root: Path, cache_dir: str | None) -> Path:
    return _resolve_cache_root_cached(
        os.fspath(project_root),
        cache_dir,
        os.environ.get("MOLT_CACHE"),
        os.environ.get("XDG_CACHE_HOME"),
        os.fspath(Path.cwd()),
        _default_home_str(),
        sys.platform,
        os.environ.get("MOLT_EXT_ROOT"),
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
    env_cross_sysroot: str | None,
) -> Path | None:
    raw = sysroot or env_sysroot or env_cross_sysroot
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
        os.environ.get("MOLT_CROSS_SYSROOT"),
    )


def _resolve_output_roots(
    project_root: Path, out_dir: Path | None, output_base: str
) -> tuple[Path, Path, Path]:
    if out_dir is not None:
        artifacts_root = out_dir / ".molt_build" / _safe_output_base(output_base)
        bin_root = out_dir
        output_root = out_dir
    else:
        artifacts_root = _default_build_root(output_base)
        bin_root = _default_molt_bin()
        output_root = project_root / "dist"

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

    _mkdir_resilient(artifacts_root)
    _mkdir_resilient(bin_root)
    if output_root != bin_root:
        _mkdir_resilient(output_root)
    return artifacts_root, bin_root, output_root


def _resolve_output_path(
    output: str | None,
    default: Path,
    *,
    out_dir: Path | None,
    project_root: Path,
) -> Path:
    if not output:
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


def _resolve_build_output_layout(
    *,
    target: str,
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
    is_wasm = target in {"wasm", "wasm-freestanding"}
    is_wasm_freestanding = target == "wasm-freestanding"
    is_rust_transpile = target in {"rust", "luau"}
    is_luau_transpile = target == "luau"
    is_mlir_emit = target == "mlir"
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
        None
        if target in {"native", "wasm", "wasm-freestanding", "rust", "luau", "mlir"}
        else target
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

    output_binary: Path | None = None
    linked_output_path: Path | None = None
    if is_luau_transpile and "MOLT_MODULE_CHUNK_OPS" not in os.environ:
        os.environ["MOLT_MODULE_CHUNK_OPS"] = "1500"
    if is_mlir_emit:
        output_artifact = _resolve_output_path(
            output,
            output_root / f"{output_base}.mlir",
            out_dir=out_dir_path,
            project_root=project_root,
        )
    elif is_luau_transpile:
        output_artifact = _resolve_output_path(
            output,
            output_root / f"{output_base}.luau",
            out_dir=out_dir_path,
            project_root=project_root,
        )
    elif is_rust_transpile:
        output_artifact = _resolve_output_path(
            output,
            output_root / f"{output_base}.rs",
            out_dir=out_dir_path,
            project_root=project_root,
        )
    elif is_wasm:
        output_wasm = _resolve_output_path(
            output,
            output_root / "output.wasm",
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
        output_obj = artifacts_root / "output.o"
        if emit_mode == "obj":
            output_obj = _resolve_output_path(
                output,
                output_root / "output.o",
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
