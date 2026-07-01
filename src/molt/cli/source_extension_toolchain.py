from __future__ import annotations

import hashlib
import json
import os
import shlex
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from molt.cli.file_hashing import _sha256_file
from molt.cli.native_toolchain import _zig_target_query
from molt.cli.wasm_toolchain import resolve_wasi_sysroot as _resolve_wasi_sysroot

_SOURCE_EXTENSION_ABI_TIERS = {"source-compat", "cpython-abi"}
_SOURCE_EXTENSION_INCLUDE_FILE_SUFFIXES = {
    ".c",
    ".h",
    ".hh",
    ".hpp",
    ".hxx",
    ".inc",
}


@dataclass(frozen=True)
class _SourceExtensionWasmToolchain:
    ok: bool
    compiler_kind: str | None
    compiler_cmd: tuple[str, ...]
    wasm_ld: str | None
    wasi_sysroot: Path | None
    detail: str


@dataclass(frozen=True)
class _SourceExtensionTargetMetadata:
    target_triple: str
    abi_tier: str
    out_dir: Path
    pkg_config_dir: Path
    python_pc: Path
    meson_cross: Path
    sidecar: Path
    digest: str
    payload: dict[str, Any]


def _wasi_sysroot_setup_advice(_system: str) -> list[str]:
    return [
        "Set MOLT_WASI_SYSROOT=<path-to-wasi-sysroot>",
        "Set WASI_SYSROOT=<path-to-wasi-sysroot>",
        "or set WASI_SDK_PATH=<path-to-wasi-sdk>",
        "or set MOLT_TARGET_ROOT=<path-with-toolchains/wasi-sysroot*>",
        "or install zig for the wasm source-extension compiler path",
        "or set MOLT_WASM_CC=<wasm-capable-compiler-with-sysroot>",
        "or set MOLT_CROSS_CC=<wasm-capable-compiler-with-sysroot>",
    ]


def _path_like_command(command: str) -> bool:
    return (
        Path(command).is_absolute()
        or "/" in command
        or "\\" in command
        or (os.altsep is not None and os.altsep in command)
    )


def _resolve_tool_command(raw_command: str, *, label: str) -> tuple[str, ...] | str:
    try:
        argv = shlex.split(raw_command)
    except ValueError as exc:
        return f"{label} is not a valid shell command: {exc}"
    if not argv:
        return f"{label} is empty"
    executable = argv[0]
    if _path_like_command(executable):
        path = Path(executable).expanduser()
        if not path.exists() or not path.is_file():
            return f"{label} executable not found: {executable}"
        return (str(path.resolve()), *argv[1:])
    resolved = shutil.which(executable)
    if resolved is None:
        return f"{label} executable not found on PATH: {executable}"
    return (resolved, *argv[1:])


def _source_extension_toolchain_advice() -> str:
    return "; ".join(_wasi_sysroot_setup_advice(os.name))


def _wasm_compiler_probe_target_args(command: tuple[str, ...]) -> tuple[str, ...]:
    has_target = any(
        arg == "-target" or arg.startswith("--target") for arg in command
    )
    return () if has_target else ("-target", "wasm32-wasip1")


def _probe_wasm_source_extension_compiler(
    compiler_cmd: tuple[str, ...],
) -> str | None:
    with tempfile.TemporaryDirectory(prefix="molt_wasm_cc_probe_") as td:
        workdir = Path(td)
        source = workdir / "probe.c"
        obj = workdir / "probe.o"
        source.write_text(
            "#include <errno.h>\nint main(void) { return EINVAL; }\n",
            encoding="ascii",
        )
        cmd = [
            *compiler_cmd,
            *_wasm_compiler_probe_target_args(compiler_cmd),
            "-c",
            str(source),
            "-o",
            str(obj),
        ]
        try:
            result = subprocess.run(
                cmd,
                cwd=workdir,
                capture_output=True,
                text=True,
                timeout=20,
                check=False,
            )
        except (OSError, subprocess.SubprocessError) as exc:
            return str(exc)
    if result.returncode == 0:
        return None
    detail = (result.stderr or result.stdout or "").strip()
    if not detail:
        detail = f"compiler exited with code {result.returncode}"
    return detail.splitlines()[0]


def _resolve_env_wasm_compiler(
    *,
    env_name: str,
    raw_command: str,
    wasm_ld_path: str | None,
) -> _SourceExtensionWasmToolchain:
    compiler = _resolve_tool_command(raw_command, label=env_name)
    if isinstance(compiler, str):
        return _SourceExtensionWasmToolchain(
            ok=False,
            compiler_kind=env_name.lower(),
            compiler_cmd=(),
            wasm_ld=wasm_ld_path,
            wasi_sysroot=None,
            detail=compiler,
        )
    if wasm_ld_path is None:
        return _SourceExtensionWasmToolchain(
            ok=False,
            compiler_kind=env_name.lower(),
            compiler_cmd=compiler,
            wasm_ld=None,
            wasi_sysroot=None,
            detail=f"missing wasm-ld; {env_name} is configured",
        )
    probe_error = _probe_wasm_source_extension_compiler(compiler)
    if probe_error is not None:
        return _SourceExtensionWasmToolchain(
            ok=False,
            compiler_kind=env_name.lower(),
            compiler_cmd=compiler,
            wasm_ld=wasm_ld_path,
            wasi_sysroot=None,
            detail=(
                f"{env_name} cannot compile the WASI source-extension probe "
                f"including <errno.h>: {probe_error}; "
                + _source_extension_toolchain_advice()
            ),
        )
    return _SourceExtensionWasmToolchain(
        ok=True,
        compiler_kind=env_name.lower(),
        compiler_cmd=compiler,
        wasm_ld=wasm_ld_path,
        wasi_sysroot=None,
        detail=(
            f"wasm-ld={wasm_ld_path}; {env_name}="
            + " ".join(shlex.quote(arg) for arg in compiler)
        ),
    )


def _resolve_source_extension_wasm_toolchain() -> _SourceExtensionWasmToolchain:
    wasm_ld_path = shutil.which("wasm-ld")
    raw_wasm_cc = os.environ.get("MOLT_WASM_CC", "").strip()
    if raw_wasm_cc:
        return _resolve_env_wasm_compiler(
            env_name="MOLT_WASM_CC",
            raw_command=raw_wasm_cc,
            wasm_ld_path=wasm_ld_path,
        )

    raw_cross_cc = os.environ.get("MOLT_CROSS_CC", "").strip()
    if raw_cross_cc:
        return _resolve_env_wasm_compiler(
            env_name="MOLT_CROSS_CC",
            raw_command=raw_cross_cc,
            wasm_ld_path=wasm_ld_path,
        )

    zig_path = shutil.which("zig")
    if zig_path is not None:
        if wasm_ld_path is None:
            return _SourceExtensionWasmToolchain(
                ok=False,
                compiler_kind="zig",
                compiler_cmd=(zig_path, "cc"),
                wasm_ld=None,
                wasi_sysroot=None,
                detail="missing wasm-ld; zig is available",
            )
        return _SourceExtensionWasmToolchain(
            ok=True,
            compiler_kind="zig",
            compiler_cmd=(zig_path, "cc"),
            wasm_ld=wasm_ld_path,
            wasi_sysroot=None,
            detail=f"wasm-ld={wasm_ld_path}; zig={zig_path}",
        )

    clang_path = shutil.which("clang")
    wasi_sysroot = _resolve_wasi_sysroot()
    if clang_path is not None and wasi_sysroot is not None:
        clang_cmd = (clang_path, "--sysroot", str(wasi_sysroot))
        if wasm_ld_path is None:
            return _SourceExtensionWasmToolchain(
                ok=False,
                compiler_kind="clang",
                compiler_cmd=clang_cmd,
                wasm_ld=None,
                wasi_sysroot=wasi_sysroot,
                detail="missing wasm-ld; clang and WASI sysroot are available",
            )
        probe_error = _probe_wasm_source_extension_compiler(clang_cmd)
        if probe_error is not None:
            return _SourceExtensionWasmToolchain(
                ok=False,
                compiler_kind="clang",
                compiler_cmd=clang_cmd,
                wasm_ld=wasm_ld_path,
                wasi_sysroot=wasi_sysroot,
                detail=(
                    "clang+WASI sysroot cannot compile the source-extension "
                    f"probe including <errno.h>: {probe_error}; "
                    + _source_extension_toolchain_advice()
                ),
            )
        return _SourceExtensionWasmToolchain(
            ok=True,
            compiler_kind="clang",
            compiler_cmd=clang_cmd,
            wasm_ld=wasm_ld_path,
            wasi_sysroot=wasi_sysroot,
            detail=(
                f"wasm-ld={wasm_ld_path}; clang={clang_path}; "
                f"WASI sysroot={wasi_sysroot}"
            ),
        )

    missing: list[str] = []
    if wasm_ld_path is None:
        missing.append("wasm-ld")
    missing.append(
        "zig, valid MOLT_WASM_CC, valid MOLT_CROSS_CC, or clang+WASI sysroot"
    )
    return _SourceExtensionWasmToolchain(
        ok=False,
        compiler_kind=None,
        compiler_cmd=(),
        wasm_ld=wasm_ld_path,
        wasi_sysroot=wasi_sysroot,
        detail="missing " + ", ".join(missing) + "; " + _source_extension_toolchain_advice(),
    )


def _normalize_source_extension_metadata_target(target: str | None) -> str:
    requested = (target or "wasm").strip().lower()
    if requested == "wasm":
        return "wasm32-wasip1"
    if requested.startswith("wasm32"):
        return requested
    raise ValueError(
        "source-extension target metadata currently supports wasm or wasm32 triples"
    )


def _normalize_source_extension_abi_tier(abi_tier: str | None) -> str:
    requested = (abi_tier or "source-compat").strip().lower().replace("_", "-")
    aliases = {
        "molt": "source-compat",
        "molt-source": "source-compat",
        "source": "source-compat",
        "source-compatible": "source-compat",
        "cpython": "cpython-abi",
        "cpython-layout": "cpython-abi",
        "python-abi": "cpython-abi",
    }
    normalized = aliases.get(requested, requested)
    if normalized in _SOURCE_EXTENSION_ABI_TIERS:
        return normalized
    raise ValueError("source-extension ABI tier must be source-compat or cpython-abi")


def _source_extension_include_dirs_for_abi_tier(
    *,
    molt_root: Path,
    abi_tier: str,
) -> tuple[Path, ...]:
    normalized = _normalize_source_extension_abi_tier(abi_tier)
    root = molt_root.resolve()
    if normalized == "cpython-abi":
        return (
            root / "runtime" / "molt-cpython-abi" / "include",
            root / "include",
        )
    return (root / "include",)


def _source_extension_python_header_for_abi_tier(
    *,
    molt_root: Path,
    abi_tier: str,
) -> Path:
    normalized = _normalize_source_extension_abi_tier(abi_tier)
    root = molt_root.resolve()
    if normalized == "cpython-abi":
        return root / "runtime" / "molt-cpython-abi" / "include" / "Python.h"
    return root / "include" / "molt" / "Python.h"


def _source_extension_include_surface(include_dirs: tuple[Path, ...]) -> dict[str, Any]:
    entries: list[dict[str, Any]] = []
    for index, include_dir in enumerate(include_dirs):
        root = include_dir.resolve()
        for path in root.rglob("*"):
            if not path.is_file():
                continue
            if path.suffix.lower() not in _SOURCE_EXTENSION_INCLUDE_FILE_SUFFIXES:
                continue
            entries.append(
                {
                    "include_dir_index": index,
                    "relative_path": path.relative_to(root).as_posix(),
                    "sha256": _sha256_file(path),
                }
            )
    entries.sort(key=lambda entry: (entry["include_dir_index"], entry["relative_path"]))
    encoded = json.dumps(entries, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return {
        "sha256": hashlib.sha256(encoded).hexdigest(),
        "file_count": len(entries),
        "files": entries,
    }


def _pc_path(path: Path) -> str:
    return str(path.resolve()).replace("\\", "/")


def _meson_quote(value: str) -> str:
    return "'" + value.replace("\\", "\\\\").replace("'", "\\'") + "'"


def _meson_array(items: tuple[str, ...] | list[str]) -> str:
    return "[" + ", ".join(_meson_quote(str(item)) for item in items) + "]"


def _meson_value(value: object) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    return _meson_quote(str(value))


def _tool_variant(path: str, *, basename: str) -> str:
    candidate = Path(path)
    suffix = candidate.suffix
    replacement = basename + suffix
    if candidate.parent == Path("."):
        return replacement
    return str(candidate.with_name(replacement))


def _source_extension_c_commands(
    *,
    toolchain: _SourceExtensionWasmToolchain,
    target_triple: str,
) -> dict[str, tuple[str, ...]]:
    target_arg = (
        _zig_target_query(target_triple)
        if toolchain.compiler_kind == "zig"
        else target_triple
    )
    c_cmd = (*toolchain.compiler_cmd, "-target", target_arg)
    cpp_cmd: tuple[str, ...] | None = None
    ar_cmd: tuple[str, ...] | None = None
    strip_cmd: tuple[str, ...] | None = None
    if toolchain.compiler_kind == "zig":
        zig = toolchain.compiler_cmd[0]
        cpp_cmd = (zig, "c++", "-target", target_arg)
        ar_cmd = (zig, "ar")
        strip_cmd = (zig, "strip")
    elif toolchain.compiler_cmd:
        compiler = Path(toolchain.compiler_cmd[0]).name.lower()
        if compiler in {"clang", "clang.exe"}:
            cpp = shutil.which(
                _tool_variant(toolchain.compiler_cmd[0], basename="clang++")
            )
            if cpp is not None:
                cpp_cmd = (cpp, *toolchain.compiler_cmd[1:], "-target", target_arg)
        llvm_ar = shutil.which("llvm-ar")
        llvm_strip = shutil.which("llvm-strip")
        if llvm_ar is not None:
            ar_cmd = (llvm_ar,)
        if llvm_strip is not None:
            strip_cmd = (llvm_strip,)
    commands: dict[str, tuple[str, ...]] = {"c": c_cmd}
    if cpp_cmd is not None:
        commands["cpp"] = cpp_cmd
    if ar_cmd is not None:
        commands["ar"] = ar_cmd
    if strip_cmd is not None:
        commands["strip"] = strip_cmd
    pkg_config = shutil.which("pkg-config") or shutil.which("pkgconf")
    if pkg_config is not None:
        commands["pkg-config"] = (pkg_config,)
    return commands


def _source_extension_meson_cross_properties(target_triple: str) -> dict[str, object]:
    normalized = _normalize_source_extension_metadata_target(target_triple)
    properties: dict[str, object] = {
        "needs_exe_wrapper": True,
        "skip_sanity_check": True,
    }
    if normalized.startswith("wasm32"):
        properties["longdouble_format"] = "IEEE_QUAD_LE"
    return properties


def _python_pc_text(*, molt_root: Path, abi_tier: str) -> str:
    prefix = _pc_path(molt_root)
    include_dirs = _source_extension_include_dirs_for_abi_tier(
        molt_root=molt_root,
        abi_tier=abi_tier,
    )
    include_dir = _pc_path(include_dirs[0])
    cflags = " ".join(f"-I{_pc_path(path)}" for path in include_dirs)
    return (
        f"prefix={prefix}\n"
        "exec_prefix=${prefix}\n"
        f"includedir={include_dir}\n"
        "\n"
        "Name: Python\n"
        "Description: Molt Python C API for source-recompiled extensions\n"
        "Version: 3.12\n"
        f"Cflags: {cflags}\n"
        "Libs:\n"
    )


def _meson_cross_text(
    *,
    target_triple: str,
    pkg_config_dir: Path,
    toolchain: _SourceExtensionWasmToolchain,
) -> str:
    commands = _source_extension_c_commands(
        toolchain=toolchain,
        target_triple=target_triple,
    )
    binaries = "\n".join(
        f"{name} = {_meson_array(command)}"
        for name, command in sorted(commands.items())
    )
    properties = _source_extension_meson_cross_properties(target_triple)
    property_lines = "\n".join(
        f"{name} = {_meson_value(value)}" for name, value in sorted(properties.items())
    )
    return (
        "[binaries]\n"
        f"{binaries}\n"
        "\n"
        "[built-in options]\n"
        f"pkg_config_path = {_meson_array([_pc_path(pkg_config_dir)])}\n"
        "\n"
        "[host_machine]\n"
        "system = 'wasi'\n"
        "cpu_family = 'wasm32'\n"
        "cpu = 'wasm32'\n"
        "endian = 'little'\n"
        "\n"
        "[properties]\n"
        f"{property_lines}\n"
    )


def _materialize_source_extension_target_metadata(
    *,
    molt_root: Path,
    out_dir: Path,
    target_triple: str,
    abi_tier: str = "source-compat",
) -> tuple[_SourceExtensionTargetMetadata | None, list[str]]:
    toolchain = _resolve_source_extension_wasm_toolchain()
    if not toolchain.ok:
        return None, [
            "source-extension target metadata requires a valid wasm compiler "
            "and linker toolchain: " + toolchain.detail
        ]
    resolved_target = _normalize_source_extension_metadata_target(target_triple)
    resolved_abi_tier = _normalize_source_extension_abi_tier(abi_tier)
    include_dirs = _source_extension_include_dirs_for_abi_tier(
        molt_root=molt_root,
        abi_tier=resolved_abi_tier,
    )
    missing_include_dirs = [path for path in include_dirs if not path.is_dir()]
    if missing_include_dirs:
        return None, [
            "source-extension ABI tier "
            f"{resolved_abi_tier} has missing include directories: "
            + ", ".join(str(path) for path in missing_include_dirs)
        ]
    resolved_out = out_dir.resolve()
    pkg_config_dir = resolved_out / "pkgconfig"
    python_pc = pkg_config_dir / "python3.pc"
    meson_cross = resolved_out / "meson.cross"
    sidecar = resolved_out / "source-extension-target-metadata.json"
    python_header = _source_extension_python_header_for_abi_tier(
        molt_root=molt_root,
        abi_tier=resolved_abi_tier,
    )
    include_surface = _source_extension_include_surface(include_dirs)
    meson_cross_properties = _source_extension_meson_cross_properties(resolved_target)
    pkg_config_dir.mkdir(parents=True, exist_ok=True)
    python_pc.write_text(
        _python_pc_text(
            molt_root=molt_root.resolve(),
            abi_tier=resolved_abi_tier,
        ),
        encoding="utf-8",
    )
    meson_cross.write_text(
        _meson_cross_text(
            target_triple=resolved_target,
            pkg_config_dir=pkg_config_dir,
            toolchain=toolchain,
        ),
        encoding="utf-8",
    )
    payload: dict[str, Any] = {
        "schema_version": 1,
        "kind": "molt-source-extension-target-metadata",
        "target_triple": resolved_target,
        "abi": {
            "tier": resolved_abi_tier,
            "include_dirs": [str(path) for path in include_dirs],
            "python_header": str(python_header),
            "python_header_sha256": _sha256_file(python_header),
            "include_surface": include_surface,
        },
        "toolchain": {
            "compiler_kind": toolchain.compiler_kind,
            "compiler_cmd": list(toolchain.compiler_cmd),
            "wasm_ld": toolchain.wasm_ld,
            "wasi_sysroot": str(toolchain.wasi_sysroot)
            if toolchain.wasi_sysroot is not None
            else None,
            "detail": toolchain.detail,
        },
        "meson_cross_properties": meson_cross_properties,
        "paths": {
            "out_dir": str(resolved_out),
            "pkg_config_dir": str(pkg_config_dir),
            "python_pc": str(python_pc),
            "meson_cross": str(meson_cross),
            "sidecar": str(sidecar),
        },
        "env": {
            "PKG_CONFIG_PATH": str(pkg_config_dir),
            "PKG_CONFIG_LIBDIR": str(pkg_config_dir),
        },
        "digests": {
            "python_pc_sha256": _sha256_file(python_pc),
            "meson_cross_sha256": _sha256_file(meson_cross),
        },
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    payload["digest"] = hashlib.sha256(encoded).hexdigest()
    sidecar.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return (
        _SourceExtensionTargetMetadata(
            target_triple=resolved_target,
            abi_tier=resolved_abi_tier,
            out_dir=resolved_out,
            pkg_config_dir=pkg_config_dir,
            python_pc=python_pc,
            meson_cross=meson_cross,
            sidecar=sidecar,
            digest=payload["digest"],
            payload=payload,
        ),
        [],
    )
