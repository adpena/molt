from __future__ import annotations

import os
from pathlib import Path
import platform
import shlex
import shutil
import sys
from typing import Sequence

from molt.cli.atomic_io import _atomic_write_text
from molt.cli.native_link_deps import (
    _collect_cargo_native_link_deps,
    _native_windows_system_link_libs,
)
from molt.cli.native_toolchain import (
    _append_darwin_runtime_frameworks,
    _detect_macos_arch,
    _detect_macos_deployment_target,
    _strip_arch_flags,
    _zig_target_query,
)


def _resolve_available_fast_linker() -> str | None:
    if shutil.which("mold"):
        return "mold"
    if shutil.which("ld.lld") or shutil.which("lld"):
        return "lld"
    return None


def _resolve_dev_linker() -> str | None:
    raw = os.environ.get("MOLT_DEV_LINKER", "auto").strip().lower()
    if raw in {"0", "false", "no", "off", "none", "disable"}:
        return None
    if raw in {"mold", "lld"}:
        return raw
    if raw != "auto":
        return None
    return _resolve_available_fast_linker()


def _resolve_native_linker_hint(
    *,
    profile: str,
    target_triple: str | None,
) -> str | None:
    if profile == "dev":
        return _resolve_dev_linker()
    is_host_linux = target_triple is None and sys.platform.startswith("linux")
    if is_host_linux:
        return _resolve_available_fast_linker()
    return None


def _build_native_link_driver_command(
    *,
    output_obj: Path | None,
    target_triple: str | None,
    sysroot_path: Path | None,
    profile: str,
) -> tuple[list[str], str | None, str | None]:
    cc = os.environ.get("CC", "clang")
    link_cmd = shlex.split(cc)
    normalized_target: str | None = target_triple
    if target_triple:
        cross_cc = os.environ.get("MOLT_CROSS_CC")
        target_arg = target_triple
        if cross_cc:
            link_cmd = shlex.split(cross_cc)
        elif shutil.which("zig"):
            link_cmd = ["zig", "cc"]
            target_arg = _zig_target_query(target_triple)
            normalized_target = target_arg
        else:
            raise RuntimeError(
                f"Cross-target build requires zig or MOLT_CROSS_CC (missing for {target_triple})."
            )
        link_cmd.extend(["-target", target_arg])
    if sysroot_path is not None:
        sysroot_flag = "--sysroot"
        if (
            target_triple and ("apple" in target_triple or "darwin" in target_triple)
        ) or (not target_triple and sys.platform == "darwin"):
            sysroot_flag = "-isysroot"
        link_cmd.extend([sysroot_flag, str(sysroot_path)])
    cflags = os.environ.get("CFLAGS", "")
    if cflags:
        link_cmd.extend(shlex.split(cflags))
    linker_hint = _resolve_native_linker_hint(
        profile=profile,
        target_triple=target_triple,
    )
    if linker_hint and not any(arg.startswith("-fuse-ld=") for arg in link_cmd):
        link_cmd.append(f"-fuse-ld={linker_hint}")
    if sys.platform == "darwin" and not target_triple:
        link_cmd = _strip_arch_flags(link_cmd)
        arch = (
            os.environ.get("MOLT_ARCH")
            or (None if output_obj is None else _detect_macos_arch(output_obj))
            or platform.machine()
        )
        link_cmd.extend(["-arch", arch])
        deployment_target = _detect_macos_deployment_target(arch)
        if deployment_target:
            link_cmd.append(f"-mmacosx-version-min={deployment_target}")
    return link_cmd, linker_hint, normalized_target


def _windows_coff_library_command(
    *,
    input_objects: Sequence[Path],
    output_path: Path,
) -> list[str]:
    override = os.environ.get("MOLT_COFF_LIB")
    if override:
        return [
            *shlex.split(override),
            f"/OUT:{output_path}",
            *[str(path) for path in input_objects],
        ]
    for tool_name in ("llvm-lib", "lib"):
        tool = shutil.which(tool_name)
        if tool:
            return [
                tool,
                f"/OUT:{output_path}",
                *[str(path) for path in input_objects],
            ]
    lld_link = shutil.which("lld-link")
    if lld_link:
        return [
            lld_link,
            "/lib",
            f"/OUT:{output_path}",
            *[str(path) for path in input_objects],
        ]
    raise RuntimeError(
        "Windows native object emission requires llvm-lib, lib.exe, or lld-link "
        "to combine COFF objects."
    )


def _build_native_link_command(
    *,
    output_obj: Path,
    stub_path: Path,
    runtime_lib: Path,
    output_binary: Path,
    target_triple: str | None,
    sysroot_path: Path | None,
    profile: str,
    stdlib_obj_path: Path | None = None,
) -> tuple[list[str], str | None, str | None]:
    link_cmd, linker_hint, normalized_target = _build_native_link_driver_command(
        output_obj=output_obj,
        target_triple=target_triple,
        sysroot_path=sysroot_path,
        profile=profile,
    )
    link_inputs = [str(stub_path), str(output_obj)]
    if stdlib_obj_path is not None and stdlib_obj_path.exists():
        link_inputs.append(str(stdlib_obj_path))
    is_darwin = (
        target_triple and ("apple" in target_triple or "darwin" in target_triple)
    ) or (not target_triple and sys.platform == "darwin")
    is_linux = (target_triple and "linux" in target_triple) or (
        not target_triple and sys.platform.startswith("linux")
    )
    is_windows = (
        target_triple and ("windows" in target_triple or "msvc" in target_triple)
    ) or (not target_triple and sys.platform == "win32")
    runtime_lib_str = str(runtime_lib)
    if is_linux:
        link_inputs.extend(
            [
                "-Wl,--start-group",
                runtime_lib_str,
                "-Wl,--end-group",
                "-o",
                str(output_binary),
            ]
        )
    else:
        link_inputs.extend([runtime_lib_str, runtime_lib_str, "-o", str(output_binary)])
    link_cmd.extend(link_inputs)

    suppress_linker_warnings = os.environ.get("MOLT_LINKER_WARNINGS") != "1"
    if is_darwin:
        link_cmd.append("-Wl,-dead_strip")
        exported_symbols_path = output_binary.parent / ".molt_exports.exp"
        _atomic_write_text(exported_symbols_path, "_main\n")
        link_cmd.append(f"-Wl,-exported_symbols_list,{exported_symbols_path}")
        if os.environ.get("MOLT_KEEP_SYMBOLS") != "1":
            link_cmd.extend(["-Wl,-x", "-Wl,-S"])
        if suppress_linker_warnings:
            link_cmd.append("-Wl,-w")
        link_cmd.append("-lc++")
    elif is_linux:
        link_cmd.extend(["-fdata-sections", "-ffunction-sections"])
        link_cmd.append("-Wl,--gc-sections")
        link_cmd.append("-Wl,--strip-all")
        link_cmd.append("-Wl,--as-needed")
        link_cmd.append("-Wl,-O2")
        version_script_path = output_binary.parent / ".molt_version.ver"
        _atomic_write_text(version_script_path, "{ global: main; local: *; };\n")
        link_cmd.append(f"-Wl,--version-script={version_script_path}")
        link_cmd.append("-lstdc++")
        link_cmd.append("-lm")
    elif is_windows:
        link_cmd.extend(["-Wl,/OPT:REF"])
    _append_darwin_runtime_frameworks(link_cmd, target_triple=target_triple)
    cargo_search, cargo_libs = _collect_cargo_native_link_deps(runtime_lib)
    link_cmd.extend(cargo_search)
    link_cmd.extend(cargo_libs)
    link_cmd.extend(_native_windows_system_link_libs(target_triple))
    return link_cmd, linker_hint, normalized_target
