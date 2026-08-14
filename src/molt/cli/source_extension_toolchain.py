from __future__ import annotations

import hashlib
import json
import os
import re
import shlex
import subprocess
import tempfile
from collections.abc import Sequence
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

from molt.file_hashing import _sha256_file
from molt.cli.llvm_wasi_tools import (
    LlvmToolRole,
    LlvmWasiToolFamily,
    resolve_explicit_tool_command,
    resolve_llvm_wasi_tool_family,
)
from molt.cli.native_toolchain import _zig_target_query
from molt.cli.source_extension_target import SourceExtensionTargetPlan
from molt.target_python import _parse_target_python_version
from molt.cli.wasm_toolchain import (
    normalize_wasi_sysroot,
    resolve_wasi_sysroot as _resolve_wasi_sysroot,
    wasm_compiler_builtins_archive,
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
class _ResolvedSourceExtensionToolchain:
    target_plan: SourceExtensionTargetPlan
    compiler_kind: str
    tools: LlvmWasiToolFamily
    commands: dict[str, tuple[str, ...]]
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


def _compiler_target_values(command: tuple[str, ...]) -> tuple[str, ...]:
    targets: list[str] = []
    index = 0
    while index < len(command):
        argument = command[index]
        if argument in {"-target", "--target"}:
            if index + 1 >= len(command) or command[index + 1].startswith("-"):
                raise ValueError(
                    f"compiler command has {argument} without a target value"
                )
            targets.append(command[index + 1])
            index += 2
            continue
        for prefix in ("-target=", "--target="):
            if argument.startswith(prefix):
                value = argument.removeprefix(prefix)
                if not value:
                    raise ValueError(
                        f"compiler command has {prefix} without a target value"
                    )
                targets.append(value)
                break
        index += 1
    return tuple(targets)


def _compiler_probe_target_args(
    command: tuple[str, ...], target_triple: str
) -> tuple[str, ...]:
    configured_targets = _compiler_target_values(command)
    mismatched = sorted(
        {
            target
            for target in configured_targets
            if target.strip().lower() != target_triple.lower()
        }
    )
    if mismatched:
        raise ValueError(
            "compiler command target conflicts with source-extension target "
            f"{target_triple}: {', '.join(mismatched)}"
        )
    return () if configured_targets else ("-target", target_triple)


def _compiler_sysroot_arg_value(args: Sequence[str]) -> str | None:
    index = 0
    while index < len(args):
        argument = args[index]
        if argument in {"--sysroot", "-isysroot"}:
            if index + 1 >= len(args):
                return ""
            return args[index + 1]
        for prefix in ("--sysroot=", "-isysroot="):
            if argument.startswith(prefix):
                return argument.removeprefix(prefix)
        index += 1
    return None


def _probe_wasm_source_extension_compiler(
    compiler_cmd: tuple[str, ...],
    *,
    target_plan: SourceExtensionTargetPlan,
) -> str | None:
    with tempfile.TemporaryDirectory(prefix="molt_wasm_cc_probe_") as td:
        workdir = Path(td)
        source = workdir / "probe.c"
        obj = workdir / "probe.o"
        source.write_text(
            (
                "#include <errno.h>\nint molt_probe(void) { return EINVAL; }\n"
                if target_plan.target_triple == "wasm32-wasip1"
                else "int molt_probe(void) { return 0; }\n"
            ),
            encoding="ascii",
        )
        cmd = [
            *compiler_cmd,
            *_compiler_probe_target_args(
                compiler_cmd,
                target_plan.compiler_target_triple or target_plan.target_triple,
            ),
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
    target_plan: SourceExtensionTargetPlan,
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
    raw_sysroot = _compiler_sysroot_arg_value(compiler)
    wasi_sysroot = (
        normalize_wasi_sysroot(raw_sysroot)
        if target_plan.target_triple == "wasm32-wasip1" and raw_sysroot is not None
        else None
    )
    if (
        target_plan.target_triple == "wasm32-wasip1"
        and raw_sysroot is not None
        and wasi_sysroot is None
    ):
        return _SourceExtensionWasmToolchain(
            ok=False,
            compiler_kind=env_name.lower(),
            tools=resolve_llvm_wasi_tool_family(explicit_commands={"cc": compiler}),
            wasi_sysroot=None,
            detail=(
                f"{env_name} has an invalid WASI sysroot argument: {raw_sysroot!r}"
            ),
        )
    tools = resolve_llvm_wasi_tool_family(explicit_commands={"cc": compiler})
    missing = tools.missing_roles()
    if missing:
        return _SourceExtensionWasmToolchain(
            ok=False,
            compiler_kind=env_name.lower(),
            tools=tools,
            wasi_sysroot=wasi_sysroot,
            detail=(
                "missing LLVM/WASI tools "
                + ", ".join(missing)
                + f"; {env_name} is configured"
            ),
        )
    probe_error = _probe_wasm_source_extension_compiler(
        compiler, target_plan=target_plan
    )
    if probe_error is not None:
        probe_kind = (
            "WASI source-extension probe including <errno.h>"
            if target_plan.target_triple == "wasm32-wasip1"
            else f"{target_plan.target_triple} freestanding source-extension probe"
        )
        return _SourceExtensionWasmToolchain(
            ok=False,
            compiler_kind=env_name.lower(),
            tools=tools,
            wasi_sysroot=wasi_sysroot,
            detail=(
                f"{env_name} cannot compile the {probe_kind}: {probe_error}; "
                + _source_extension_toolchain_advice()
            ),
        )
    return _SourceExtensionWasmToolchain(
        ok=True,
        compiler_kind=env_name.lower(),
        tools=tools,
        wasi_sysroot=wasi_sysroot,
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


def _resolve_source_extension_wasm_toolchain(
    target_plan: SourceExtensionTargetPlan,
) -> _SourceExtensionWasmToolchain:
    if not target_plan.is_wasm:
        raise ValueError("WASM toolchain resolution requires a WASM target plan")
    raw_wasm_cc = os.environ.get("MOLT_WASM_CC", "").strip()
    if raw_wasm_cc:
        return _resolve_env_wasm_compiler(
            env_name="MOLT_WASM_CC",
            raw_command=raw_wasm_cc,
            target_plan=target_plan,
        )

    raw_cross_cc = os.environ.get("MOLT_CROSS_CC", "").strip()
    if raw_cross_cc:
        return _resolve_env_wasm_compiler(
            env_name="MOLT_CROSS_CC",
            raw_command=raw_cross_cc,
            target_plan=target_plan,
        )

    tools = resolve_llvm_wasi_tool_family()
    requires_wasi = target_plan.target_triple == "wasm32-wasip1"
    wasi_sysroot = _resolve_wasi_sysroot() if requires_wasi else None
    if tools.cc is not None and (wasi_sysroot is not None or not requires_wasi):
        clang_cmd = (
            (*tools.cc.command, "--sysroot", str(wasi_sysroot))
            if wasi_sysroot is not None
            else tools.cc.command
        )
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
        probe_error = _probe_wasm_source_extension_compiler(
            clang_cmd, target_plan=target_plan
        )
        if probe_error is not None:
            return _SourceExtensionWasmToolchain(
                ok=False,
                compiler_kind="clang",
                tools=tools,
                wasi_sysroot=wasi_sysroot,
                detail=(
                    f"clang cannot compile the {target_plan.target_triple} "
                    f"source-extension probe: {probe_error}; "
                    + _source_extension_toolchain_advice()
                ),
            )
        return _SourceExtensionWasmToolchain(
            ok=True,
            compiler_kind="clang",
            tools=tools,
            wasi_sysroot=wasi_sysroot,
            detail=(
                f"{_llvm_wasi_tool_family_detail(tools)}; "
                + (
                    f"WASI sysroot={wasi_sysroot}"
                    if wasi_sysroot is not None
                    else "freestanding target"
                )
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
        assert zig_tools.cc is not None
        zig_probe_command = (
            *zig_tools.cc.command,
            "-target",
            _zig_target_query(target_plan.target_triple),
        )
        probe_error = _probe_wasm_source_extension_compiler(
            zig_probe_command,
            target_plan=target_plan,
        )
        if probe_error is not None:
            return _SourceExtensionWasmToolchain(
                ok=False,
                compiler_kind="zig",
                tools=zig_tools,
                wasi_sysroot=None,
                detail=(
                    f"zig cannot compile the {target_plan.target_triple} "
                    f"source-extension probe: {probe_error}"
                ),
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


def _normalize_source_extension_python_version(python_version: str | None) -> str:
    if not isinstance(python_version, str) or not python_version:
        raise ValueError(
            "source-extension target metadata requires an explicit Python version"
        )
    if python_version != python_version.strip():
        raise ValueError(
            "source-extension Python version must not contain surrounding whitespace"
        )
    target_python = _parse_target_python_version(python_version)
    if python_version != target_python.short:
        raise ValueError(
            "source-extension Python version must use canonical major.minor syntax; "
            f"expected {target_python.short!r}, got {python_version!r}"
        )
    return target_python.short


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
    if not _compiler_probe_target_args(command, target):
        return command
    return (*command, "-target", target)


def _source_extension_c_commands(
    *,
    toolchain: _SourceExtensionWasmToolchain,
    target_plan: SourceExtensionTargetPlan,
) -> dict[str, tuple[str, ...]]:
    target_arg = (
        _zig_target_query(target_plan.target_triple)
        if toolchain.compiler_kind == "zig"
        else (target_plan.compiler_target_triple or target_plan.target_triple)
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
    return commands


def _resolve_source_extension_native_toolchain(
    target_plan: SourceExtensionTargetPlan,
) -> _ResolvedSourceExtensionToolchain:
    if target_plan.is_wasm or target_plan.native_target is None:
        raise ValueError("native toolchain resolution requires a native target plan")
    cross_target = target_plan.compiler_target_triple
    compiler_kind: str
    if cross_target is not None:
        raw_cross_cc = os.environ.get("MOLT_CROSS_CC", "").strip()
        if raw_cross_cc:
            c_base = resolve_explicit_tool_command(
                raw_cross_cc,
                label="MOLT_CROSS_CC",
            )
            compiler_kind = "molt_cross_cc"
        else:
            try:
                zig = resolve_explicit_tool_command("zig", label="zig")[0]
            except ValueError as exc:
                raise ValueError(
                    "cross-target source-extension builds require zig or "
                    f"MOLT_CROSS_CC for {cross_target}"
                ) from exc
            c_base = (zig, "cc")
            compiler_kind = "zig"
    else:
        c_base = resolve_explicit_tool_command(
            os.environ.get("CC", "clang"),
            label="CC",
        )
        compiler_kind = "host"

    compiler_target = (
        _zig_target_query(cross_target)
        if cross_target is not None and compiler_kind == "zig"
        else cross_target
    )
    c_command = (
        _compiler_command_with_target(c_base, compiler_target)
        if compiler_target is not None
        else c_base
    )
    explicit_tools: dict[LlvmToolRole, tuple[str, ...]] = {"cc": c_command}
    cxx_env_name = "MOLT_CROSS_CXX" if cross_target is not None else "CXX"
    configured_cxx = os.environ.get(cxx_env_name, "").strip()
    if configured_cxx:
        cxx_base = resolve_explicit_tool_command(
            configured_cxx,
            label=cxx_env_name,
        )
        explicit_tools["cxx"] = (
            _compiler_command_with_target(cxx_base, compiler_target)
            if compiler_target is not None
            else cxx_base
        )
    elif compiler_kind == "zig":
        explicit_tools["cxx"] = (
            c_command[0],
            "c++",
            *c_command[2:],
        )

    tools = resolve_llvm_wasi_tool_family(
        explicit_commands=explicit_tools,
        sibling_directories=(Path(c_command[0]).parent,),
    )
    commands: dict[str, tuple[str, ...]] = {}
    for role, tool in {
        "c": tools.cc,
        "cpp": tools.cxx,
        "ar": tools.ar,
        "nm": tools.nm,
        "ranlib": tools.ranlib,
        "strip": tools.strip,
    }.items():
        if tool is not None:
            commands[role] = tool.command
    # Compiler choice and target arguments are policy inputs. Family discovery
    # supplies sibling roles but may never replace either compiler projection.
    commands["c"] = c_command
    if "cxx" in explicit_tools:
        commands["cpp"] = explicit_tools["cxx"]
    elif tools.cxx is not None:
        cxx_base = tools.cxx.command
        if len(cxx_base) == 1 and len(c_command) > 1:
            cxx_base = (*cxx_base, *c_command[1:])
        commands["cpp"] = (
            _compiler_command_with_target(cxx_base, compiler_target)
            if compiler_target is not None
            else cxx_base
        )
    missing = sorted({"c", "ar", "nm"} - commands.keys())
    if missing:
        raise ValueError(
            "native source-extension tool family is incomplete; missing: "
            + ", ".join(missing)
        )
    return _ResolvedSourceExtensionToolchain(
        target_plan=target_plan,
        compiler_kind=compiler_kind,
        tools=tools,
        commands=commands,
        wasi_sysroot=None,
        detail=_llvm_wasi_tool_family_detail(tools),
    )


def _resolve_source_extension_toolchain(
    target_plan: SourceExtensionTargetPlan,
) -> _ResolvedSourceExtensionToolchain:
    if not target_plan.is_wasm:
        return _resolve_source_extension_native_toolchain(target_plan)
    wasm = _resolve_source_extension_wasm_toolchain(target_plan)
    if not wasm.ok:
        raise ValueError(wasm.detail)
    commands = _source_extension_c_commands(
        toolchain=wasm,
        target_plan=target_plan,
    )
    return _ResolvedSourceExtensionToolchain(
        target_plan=target_plan,
        compiler_kind=wasm.compiler_kind or "wasm",
        tools=wasm.tools,
        commands=commands,
        wasi_sysroot=wasm.wasi_sysroot,
        detail=wasm.detail,
    )


def _source_extension_meson_cross_properties(
    target_plan: SourceExtensionTargetPlan,
) -> dict[str, object]:
    properties: dict[str, object] = {
        "needs_exe_wrapper": (
            target_plan.is_wasm or target_plan.compiler_target_triple is not None
        ),
        "skip_sanity_check": (
            target_plan.is_wasm or target_plan.compiler_target_triple is not None
        ),
    }
    if target_plan.target_triple == "wasm32-wasip1":
        properties["longdouble_format"] = "IEEE_QUAD_LE"
    return properties


def _source_extension_meson_host_machine(
    target_plan: SourceExtensionTargetPlan,
) -> dict[str, str]:
    if target_plan.is_wasm:
        return {
            "system": (
                "wasi" if target_plan.target_triple == "wasm32-wasip1" else "none"
            ),
            "cpu_family": "wasm32",
            "cpu": "wasm32",
            "endian": "little",
        }
    assert target_plan.native_target is not None
    target = target_plan.native_target
    system = "darwin" if target.os == "macos" else target.os
    cpu_family = {
        "amd64": "x86_64",
        "arm64": "aarch64",
        "i386": "x86",
        "i486": "x86",
        "i586": "x86",
        "i686": "x86",
    }.get(target.arch, target.arch)
    endian = (
        "big"
        if cpu_family in {"powerpc", "powerpc64", "s390x", "sparc", "sparc64"}
        else "little"
    )
    return {
        "system": system,
        "cpu_family": cpu_family,
        "cpu": cpu_family,
        "endian": endian,
    }


def _python_pc_text(
    *,
    molt_root: Path,
    abi_tier: str,
    python_version: str,
) -> str:
    normalized_python_version = _normalize_source_extension_python_version(
        python_version
    )
    prefix = _pc_path(molt_root)
    include_dirs = _source_extension_include_dirs_for_abi_tier(
        molt_root=molt_root,
        abi_tier=abi_tier,
    )
    if _normalize_source_extension_abi_tier(abi_tier) == "cpython-abi":
        python_header = include_dirs[0] / "Python.h"
        header_text = python_header.read_text(encoding="utf-8")
        major = re.search(
            r"^#define PY_MAJOR_VERSION ([0-9]+)$",
            header_text,
            re.MULTILINE,
        )
        minor = re.search(
            r"^#define PY_MINOR_VERSION ([0-9]+)$",
            header_text,
            re.MULTILINE,
        )
        header_version = (
            f"{major.group(1)}.{minor.group(1)}"
            if major is not None and minor is not None
            else "<unresolved>"
        )
        if header_version != normalized_python_version:
            raise ValueError(
                f"CPython-ABI header {python_header} declares {header_version}, "
                f"not requested Python {normalized_python_version}"
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
        f"Version: {normalized_python_version}\n"
        f"Cflags: {cflags}\n"
        "Libs:\n"
    )


def _meson_cross_text(
    *,
    target_plan: SourceExtensionTargetPlan,
    pkg_config_dir: Path,
    toolchain: _ResolvedSourceExtensionToolchain,
    compiler_builtins: Path | None,
    include_dirs: tuple[Path, ...],
) -> str:
    commands = toolchain.commands
    binaries = "\n".join(
        f"{name} = {_meson_array(command)}"
        for name, command in sorted(commands.items())
    )
    properties = _source_extension_meson_cross_properties(target_plan)
    property_lines = "\n".join(
        f"{name} = {_meson_value(value)}" for name, value in sorted(properties.items())
    )
    built_in_options = [
        f"pkg_config_path = {_meson_array([_pc_path(pkg_config_dir)])}",
        f"c_args = {_meson_array(tuple(f'-I{_pc_path(path)}' for path in include_dirs))}",
        f"cpp_args = {_meson_array(tuple(f'-I{_pc_path(path)}' for path in include_dirs))}",
    ]
    if target_plan.target_triple == "wasm32-wasip1":
        assert compiler_builtins is not None
        built_in_options.extend(
            (
                "c_link_args = "
                + _meson_array(("-nodefaultlibs", "-lc", _pc_path(compiler_builtins))),
                "cpp_link_args = "
                + _meson_array(
                    (
                        "-nodefaultlibs",
                        "-lc",
                        "-lc++",
                        "-lc++abi",
                        _pc_path(compiler_builtins),
                    )
                ),
            )
        )
    elif target_plan.is_wasm:
        built_in_options.extend(
            (
                f"c_link_args = {_meson_array(('-nostdlib',))}",
                f"cpp_link_args = {_meson_array(('-nostdlib',))}",
            )
        )
    host_machine = _source_extension_meson_host_machine(target_plan)
    host_lines = "\n".join(
        f"{name} = {_meson_quote(value)}" for name, value in host_machine.items()
    )
    built_in_text = "\n".join(built_in_options)
    return (
        f"[binaries]\n{binaries}\n\n"
        f"[built-in options]\n{built_in_text}\n\n"
        f"[host_machine]\n{host_lines}\n\n"
        f"[properties]\n{property_lines}\n"
    )


def _materialize_source_extension_target_metadata(
    *,
    molt_root: Path,
    out_dir: Path,
    target_plan: SourceExtensionTargetPlan,
    python_version: str,
    abi_tier: str = "source-compat",
) -> tuple[_SourceExtensionTargetMetadata | None, list[str]]:
    try:
        normalized_python_version = _normalize_source_extension_python_version(
            python_version
        )
    except ValueError as exc:
        return None, [str(exc)]
    try:
        toolchain = _resolve_source_extension_toolchain(target_plan)
    except (OSError, RuntimeError, ValueError) as exc:
        return None, [
            "source-extension target metadata requires a valid canonical "
            f"compiler/archive/symbol tool family: {exc}"
        ]
    try:
        return _materialize_source_extension_target_metadata_with_toolchain(
            molt_root=molt_root,
            out_dir=out_dir,
            target_plan=target_plan,
            python_version=normalized_python_version,
            abi_tier=abi_tier,
            toolchain=toolchain,
        )
    except (OSError, RuntimeError, ValueError) as exc:
        return None, [
            "source-extension target metadata validation/materialization failed "
            f"for {target_plan.target_triple}: {exc}"
        ]


def _materialize_source_extension_target_metadata_with_toolchain(
    *,
    molt_root: Path,
    out_dir: Path,
    target_plan: SourceExtensionTargetPlan,
    python_version: str,
    abi_tier: str,
    toolchain: _ResolvedSourceExtensionToolchain,
) -> tuple[_SourceExtensionTargetMetadata | None, list[str]]:
    resolved_target = target_plan.target_triple
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
    meson_cross_properties = _source_extension_meson_cross_properties(target_plan)
    materialized_commands = toolchain.commands
    compiler_builtins = (
        wasm_compiler_builtins_archive(resolved_target)
        if resolved_target == "wasm32-wasip1"
        else None
    )
    if resolved_target == "wasm32-wasip1" and (
        compiler_builtins is None or not compiler_builtins.is_file()
    ):
        return None, [
            "WASI source-extension target metadata requires the target Rust "
            "compiler-builtins archive for Meson configure links"
        ]
    pkg_config_dir.mkdir(parents=True, exist_ok=True)
    python_pc.write_text(
        _python_pc_text(
            molt_root=molt_root.resolve(),
            abi_tier=resolved_abi_tier,
            python_version=python_version,
        ),
        encoding="utf-8",
    )
    meson_cross.write_text(
        _meson_cross_text(
            target_plan=target_plan,
            pkg_config_dir=pkg_config_dir,
            toolchain=toolchain,
            compiler_builtins=compiler_builtins,
            include_dirs=include_dirs,
        ),
        encoding="utf-8",
    )
    payload: dict[str, Any] = {
        "schema_version": 3,
        "kind": "molt-source-extension-target-metadata",
        "target_triple": resolved_target,
        "target": {
            "requested": target_plan.requested,
            "compiler_target_triple": target_plan.compiler_target_triple,
            "artifact_kind": target_plan.artifact_kind,
        },
        "python": {
            "implementation": "cpython",
            "version": python_version,
        },
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
            "link_probe_archives": (
                {
                    "compiler_builtins": {
                        "path": str(compiler_builtins.resolve()),
                        "sha256": _sha256_file(compiler_builtins),
                    }
                }
                if compiler_builtins is not None
                else {}
            ),
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
