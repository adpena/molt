from __future__ import annotations

import hashlib
import json
import os
import shlex
import shutil
import subprocess
import tempfile
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

from molt.cli.file_hashing import _sha256_file
from molt.cli.llvm_wasi_tools import (
    LlvmToolRole,
    LlvmWasiToolFamily,
    resolve_explicit_tool_command,
    resolve_llvm_wasi_tool_family,
)
from molt.cli.native_toolchain import _zig_target_query
from molt.cli.wasm_toolchain import resolve_wasi_sysroot as _resolve_wasi_sysroot
from molt.scientific_stack_versions import (
    resolve_scientific_stack,
    verify_cpython_abi_headers,
)

_SOURCE_EXTENSION_ABI_TIERS = {"source-compat", "cpython-abi"}
MOLT_PKGCONF_REQUIREMENT = "pkgconf==3.0.1.post0"
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
    tools: LlvmWasiToolFamily
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


def _source_extension_toolchain_advice() -> str:
    return "; ".join(_wasi_sysroot_setup_advice(os.name))


def _wasm_compiler_probe_target_args(command: tuple[str, ...]) -> tuple[str, ...]:
    has_target = any(
        arg in {"-target", "--target"}
        or arg.startswith("-target=")
        or arg.startswith("--target=")
        for arg in command
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
) -> _SourceExtensionWasmToolchain:
    try:
        compiler = resolve_explicit_tool_command(raw_command, label=env_name)
    except ValueError as exc:
        return _SourceExtensionWasmToolchain(
            ok=False,
            compiler_kind=env_name.lower(),
            tools=resolve_llvm_wasi_tool_family(),
            wasi_sysroot=None,
            detail=str(exc),
        )
    tools = resolve_llvm_wasi_tool_family(explicit_commands={"cc": compiler})
    missing = tools.missing_roles()
    if missing:
        return _SourceExtensionWasmToolchain(
            ok=False,
            compiler_kind=env_name.lower(),
            tools=tools,
            wasi_sysroot=None,
            detail=(
                "missing LLVM/WASI tools "
                + ", ".join(missing)
                + f"; {env_name} is configured"
            ),
        )
    probe_error = _probe_wasm_source_extension_compiler(compiler)
    if probe_error is not None:
        return _SourceExtensionWasmToolchain(
            ok=False,
            compiler_kind=env_name.lower(),
            tools=tools,
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
        tools=tools,
        wasi_sysroot=None,
        detail=(
            f"{_llvm_wasi_tool_family_detail(tools)}; {env_name}="
            + " ".join(shlex.quote(arg) for arg in compiler)
        ),
    )


def _llvm_wasi_tool_family_detail(tools: LlvmWasiToolFamily) -> str:
    details: list[str] = []
    for role in ("cc", "cxx", "wasm_ld", "ar", "ranlib", "nm", "strip"):
        tool = getattr(tools, role)
        if tool is None:
            details.append(f"{role}=missing")
            continue
        version = tool.version or "unattested"
        details.append(f"{role}={tool.path} version={version}")
    return "; ".join(details)


def _with_compiler_command(
    tools: LlvmWasiToolFamily,
    command: tuple[str, ...],
) -> LlvmWasiToolFamily:
    assert tools.cc is not None
    return replace(tools, cc=replace(tools.cc, command=command))


def _resolve_source_extension_wasm_toolchain() -> _SourceExtensionWasmToolchain:
    raw_wasm_cc = os.environ.get("MOLT_WASM_CC", "").strip()
    if raw_wasm_cc:
        return _resolve_env_wasm_compiler(
            env_name="MOLT_WASM_CC",
            raw_command=raw_wasm_cc,
        )

    raw_cross_cc = os.environ.get("MOLT_CROSS_CC", "").strip()
    if raw_cross_cc:
        return _resolve_env_wasm_compiler(
            env_name="MOLT_CROSS_CC",
            raw_command=raw_cross_cc,
        )

    tools = resolve_llvm_wasi_tool_family()
    wasi_sysroot = _resolve_wasi_sysroot()
    if tools.cc is not None and wasi_sysroot is not None:
        clang_cmd = (*tools.cc.command, "--sysroot", str(wasi_sysroot))
        tools = _with_compiler_command(tools, clang_cmd)
        missing = tools.missing_roles()
        if missing:
            return _SourceExtensionWasmToolchain(
                ok=False,
                compiler_kind="clang",
                tools=tools,
                wasi_sysroot=wasi_sysroot,
                detail=(
                    "missing LLVM/WASI tools "
                    + ", ".join(missing)
                    + "; clang and WASI sysroot are available"
                ),
            )
        probe_error = _probe_wasm_source_extension_compiler(clang_cmd)
        if probe_error is not None:
            return _SourceExtensionWasmToolchain(
                ok=False,
                compiler_kind="clang",
                tools=tools,
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
            tools=tools,
            wasi_sysroot=wasi_sysroot,
            detail=(
                f"{_llvm_wasi_tool_family_detail(tools)}; WASI sysroot={wasi_sysroot}"
            ),
        )

    try:
        zig_command = resolve_explicit_tool_command("zig", label="zig")
    except ValueError:
        zig_command = None
    if zig_command is not None:
        zig = zig_command[0]
        zig_explicit_commands: dict[LlvmToolRole, tuple[str, ...]] = {
            "cc": (zig, "cc"),
            "cxx": (zig, "c++"),
            "ar": (zig, "ar"),
            "ranlib": (zig, "ranlib"),
            "strip": (zig, "strip"),
        }
        zig_tools = resolve_llvm_wasi_tool_family(
            explicit_commands=zig_explicit_commands
        )
        missing = zig_tools.missing_roles()
        if missing:
            return _SourceExtensionWasmToolchain(
                ok=False,
                compiler_kind="zig",
                tools=zig_tools,
                wasi_sysroot=None,
                detail="missing LLVM/WASI tools "
                + ", ".join(missing)
                + "; zig is available",
            )
        return _SourceExtensionWasmToolchain(
            ok=True,
            compiler_kind="zig",
            tools=zig_tools,
            wasi_sysroot=None,
            detail=_llvm_wasi_tool_family_detail(zig_tools),
        )

    missing: list[str] = list(tools.missing_roles())
    missing.append(
        "zig, valid MOLT_WASM_CC, valid MOLT_CROSS_CC, or clang+WASI sysroot"
    )
    return _SourceExtensionWasmToolchain(
        ok=False,
        compiler_kind=None,
        tools=tools,
        wasi_sysroot=wasi_sysroot,
        detail="missing "
        + ", ".join(missing)
        + "; "
        + _source_extension_toolchain_advice(),
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
        # The CPython-ABI tier is a SINGLE self-complete header authority:
        # ``runtime/molt-cpython-abi/include`` supplies the full stock-CPython
        # public surface (Python.h, structmember.h, pymem.h, pyerrors.h, ...).
        # It must NOT also inject the repo-root ``include/`` tier: that tier
        # is the libmolt source-compat header surface. Package headers such as
        # ``numpy/*`` are admitted through the package build/source plan, never
        # through either Molt ABI tier. One header home per tier; no cross-tier
        # drag-in.
        return (root / "runtime" / "molt-cpython-abi" / "include",)
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


def _compiler_command_with_target(
    command: tuple[str, ...],
    target: str,
) -> tuple[str, ...]:
    if not _wasm_compiler_probe_target_args(command):
        return command
    return (*command, "-target", target)


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
    tools = toolchain.tools
    if tools.missing_roles():
        raise ValueError(
            "source-extension command materialization requires complete LLVM/WASI tools"
        )
    assert tools.cc is not None
    assert tools.cxx is not None
    assert tools.wasm_ld is not None
    assert tools.ar is not None
    assert tools.ranlib is not None
    assert tools.nm is not None
    assert tools.strip is not None
    c_cmd = _compiler_command_with_target(tools.cc.command, target_arg)
    if toolchain.compiler_kind == "zig" or len(tools.cxx.command) > 1:
        cxx_base = tools.cxx.command
    else:
        cxx_base = (*tools.cxx.command, *tools.cc.command[1:])
    cpp_cmd = _compiler_command_with_target(cxx_base, target_arg)
    commands: dict[str, tuple[str, ...]] = {
        "ar": tools.ar.command,
        "c": c_cmd,
        "cpp": cpp_cmd,
        "ld": tools.wasm_ld.command,
        "nm": tools.nm.command,
        "ranlib": tools.ranlib.command,
        "strip": tools.strip.command,
    }
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
    stack = resolve_scientific_stack()
    verify_cpython_abi_headers(stack=stack, repo_root=molt_root)
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
        f"Version: {stack.cpython}\n"
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
    materialized_commands = _source_extension_c_commands(
        toolchain=toolchain,
        target_triple=resolved_target,
    )
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
        "schema_version": 2,
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
            "tools": toolchain.tools.metadata(),
            "commands": {
                role: list(command)
                for role, command in sorted(materialized_commands.items())
            },
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
