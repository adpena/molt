from __future__ import annotations

import os
from pathlib import Path
import platform
import shlex
import sys
from typing import Mapping, Sequence

from molt.cli.atomic_io import _atomic_write_text
from molt.cli.llvm_wasi_tools import (
    llvm_linker_candidates,
    llvm_named_tool_candidates,
    llvm_tool_candidates,
    resolve_explicit_tool_command,
)
from molt.llvm_linker_roles import (
    LlvmLinkerRole,
    executable_entrypoint_name,
    executable_selects_linker_role,
    is_llvm_linker_role,
    llvm_linker_role_for_object_format,
)
from molt.cli.native_link_deps import _collect_cargo_native_link_deps
from molt.cli.native_link_plan import (
    NativeLinkPlan,
    NativeObjectFormat,
    native_link_capabilities,
    native_link_policy_flags,
    native_linker_name_from_driver_command,
    native_link_policy,
    resolve_native_target_spec,
)
from molt.cli.source_extension_link_requirements import (
    validate_source_extension_link_arguments,
)
from molt.cli.native_toolchain import (
    _append_darwin_runtime_frameworks,
    _detect_macos_arch,
    _detect_macos_deployment_target,
    _strip_arch_flags,
    _zig_target_query,
)


_CPYTHON_SINGLETON_CANONICAL_ALIASES = (
    ("_Py_NoneStruct", "Py_None"),
    ("_Py_NotImplementedStruct", "Py_NotImplementedSentinel"),
    ("_Py_EllipsisObject", "Py_EllipsisObject"),
)


def _tool_sibling_directories(command: Sequence[str]) -> tuple[Path, ...]:
    if not command:
        return ()
    executable = Path(command[0])
    return (executable.parent,) if executable.is_absolute() else ()


def _resolve_available_fast_linker(
    driver_command: Sequence[str] = (),
    *,
    host_platform: str | None = None,
    linker_role: LlvmLinkerRole | None = None,
) -> str | None:
    host_platform = sys.platform if host_platform is None else host_platform
    sibling_directories = _tool_sibling_directories(driver_command)
    if host_platform.startswith("linux") and llvm_named_tool_candidates(
        "mold", sibling_directories=sibling_directories
    ):
        return "mold"
    if linker_role is None:
        target = resolve_native_target_spec(
            None,
            host_platform=host_platform,
            host_arch=platform.machine(),
        )
        linker_role = llvm_linker_role_for_object_format(target.object_format.value)
    if llvm_linker_candidates(linker_role, sibling_directories=sibling_directories):
        return "lld"
    return None


def _resolve_dev_linker(
    driver_command: Sequence[str] = (),
    *,
    host_platform: str | None = None,
    linker_role: LlvmLinkerRole | None = None,
) -> str | None:
    raw = os.environ.get("MOLT_DEV_LINKER", "auto").strip().lower()
    if raw in {"0", "false", "no", "off", "none", "disable"}:
        return None
    if raw == "mold":
        return raw
    if raw == "lld":
        if linker_role is None:
            target = resolve_native_target_spec(
                None,
                host_platform=sys.platform if host_platform is None else host_platform,
                host_arch=platform.machine(),
            )
            linker_role = llvm_linker_role_for_object_format(target.object_format.value)
        return (
            "lld"
            if llvm_linker_candidates(
                linker_role,
                sibling_directories=_tool_sibling_directories(driver_command),
            )
            else None
        )
    if raw != "auto":
        return None
    return _resolve_available_fast_linker(
        driver_command,
        host_platform=host_platform,
        linker_role=linker_role,
    )


def _resolve_native_linker_hint(
    *,
    profile: str,
    target_triple: str | None,
    driver_command: Sequence[str] = (),
    host_platform: str | None = None,
) -> str | None:
    host_platform = sys.platform if host_platform is None else host_platform
    target = resolve_native_target_spec(
        target_triple,
        host_platform=host_platform,
        host_arch=platform.machine(),
    )
    linker_role = llvm_linker_role_for_object_format(target.object_format.value)
    if profile == "dev":
        raw = os.environ.get("MOLT_DEV_LINKER", "auto").strip().lower()
        is_host_linux = target_triple is None and host_platform.startswith("linux")
        is_host_windows = target_triple is None and host_platform == "win32"
        if raw == "auto" and not (is_host_linux or is_host_windows):
            return None
        selected = _resolve_dev_linker(
            driver_command,
            host_platform=host_platform,
            linker_role=linker_role,
        )
        target_is_linux = (target_triple is not None and "linux" in target_triple) or (
            target_triple is None and host_platform.startswith("linux")
        )
        if selected == "mold" and not target_is_linux:
            raise RuntimeError("mold is supported only for Linux ELF link targets.")
        return selected
    is_host_fast_linker = target_triple is None and (
        host_platform.startswith("linux") or host_platform == "win32"
    )
    if is_host_fast_linker:
        return _resolve_available_fast_linker(
            driver_command,
            host_platform=host_platform,
            linker_role=linker_role,
        )
    return None


def _build_native_link_driver_command(
    *,
    output_obj: Path | None,
    target_triple: str | None,
    sysroot_path: Path | None,
    profile: str,
    host_platform: str | None = None,
    host_arch: str | None = None,
) -> tuple[list[str], str | None, str | None]:
    host_platform = sys.platform if host_platform is None else host_platform
    host_arch = platform.machine() if host_arch is None else host_arch
    explicit_cc = os.environ.get("CC", "").strip()
    if explicit_cc:
        link_cmd = list(resolve_explicit_tool_command(explicit_cc, label="CC"))
    else:
        candidates = llvm_tool_candidates("cc")
        if not candidates:
            raise RuntimeError(
                "Native link requires Clang in the managed LLVM toolchain or PATH."
            )
        link_cmd = [str(candidates[0])]
    normalized_target: str | None = target_triple
    if target_triple:
        cross_cc = os.environ.get("MOLT_CROSS_CC")
        target_arg = target_triple
        if cross_cc:
            link_cmd = list(
                resolve_explicit_tool_command(cross_cc, label="MOLT_CROSS_CC")
            )
        elif zig := llvm_named_tool_candidates("zig"):
            link_cmd = [str(zig[0]), "cc"]
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
        ) or (not target_triple and host_platform == "darwin"):
            sysroot_flag = "-isysroot"
        link_cmd.extend([sysroot_flag, str(sysroot_path)])
    cflags = os.environ.get("CFLAGS", "")
    if cflags:
        link_cmd.extend(shlex.split(cflags))
    linker_hint = _resolve_native_linker_hint(
        profile=profile,
        target_triple=target_triple,
        driver_command=link_cmd,
        host_platform=host_platform,
    )
    if linker_hint and not any(arg.startswith("-fuse-ld=") for arg in link_cmd):
        link_cmd.append(f"-fuse-ld={linker_hint}")
    if host_platform == "darwin" and not target_triple:
        link_cmd = _strip_arch_flags(link_cmd)
        arch = (
            os.environ.get("MOLT_ARCH")
            or (None if output_obj is None else _detect_macos_arch(output_obj))
            or host_arch
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
            *resolve_explicit_tool_command(override, label="MOLT_COFF_LIB"),
            f"/OUT:{output_path}",
            *[str(path) for path in input_objects],
        ]
    candidates = llvm_named_tool_candidates("llvm-lib", "lib")
    if not candidates:
        candidates = llvm_linker_candidates("lld-link")
    if candidates:
        tool = candidates[0]
        is_lld_link = tool.stem.lower() == "lld-link"
        return [
            str(tool),
            *(("/lib",) if is_lld_link else ()),
            f"/OUT:{output_path}",
            *[str(path) for path in input_objects],
        ]
    raise RuntimeError(
        "Windows native object emission requires llvm-lib, lib.exe, or lld-link "
        "to combine COFF objects."
    )


def _build_native_link_plan(
    *,
    output_obj: Path,
    stub_path: Path,
    runtime_lib: Path,
    output_binary: Path,
    target_triple: str | None,
    sysroot_path: Path | None,
    profile: str,
    source_root: Path,
    source_fingerprint: Mapping[str, object],
    stdlib_obj_path: Path | None = None,
    external_static_archives: Sequence[Path] = (),
    external_link_arguments: Sequence[str] = (),
    bolt_requested: bool = False,
    host_platform: str | None = None,
    host_arch: str | None = None,
) -> NativeLinkPlan:
    host_platform = sys.platform if host_platform is None else host_platform
    host_arch = platform.machine() if host_arch is None else host_arch
    link_cmd, linker_hint, normalized_target = _build_native_link_driver_command(
        output_obj=output_obj,
        target_triple=target_triple,
        sysroot_path=sysroot_path,
        profile=profile,
        host_platform=host_platform,
        host_arch=host_arch,
    )
    link_inputs = [str(stub_path), str(output_obj)]
    if stdlib_obj_path is not None and stdlib_obj_path.exists():
        link_inputs.append(str(stdlib_obj_path))
    target = resolve_native_target_spec(
        target_triple,
        host_platform=host_platform,
        host_arch=host_arch,
    )
    try:
        external_link_arguments = validate_source_extension_link_arguments(
            external_link_arguments
        )
    except ValueError as exc:
        raise RuntimeError(
            f"External source-extension link requirements are invalid: {exc}"
        ) from exc
    selected_linker_name = native_linker_name_from_driver_command(
        link_cmd,
        hinted=linker_hint,
    )
    expected_role = llvm_linker_role_for_object_format(target.object_format.value)
    explicit_selectors = tuple(
        arg.split("=", 1)[1].strip() for arg in link_cmd if arg.startswith("-fuse-ld=")
    )
    for selector in explicit_selectors:
        # The conventional clang selector names a target-aware linker mode; an
        # explicit executable path must instead name the exact object-format role.
        if selector.lower() == "lld":
            continue
        selector_entrypoint = executable_entrypoint_name(Path(selector))
        if (
            selector_entrypoint == "lld" or is_llvm_linker_role(selector_entrypoint)
        ) and not executable_selects_linker_role(Path(selector), expected_role):
            raise RuntimeError(
                "Native LLVM linker selection crosses object-format roles: "
                f"expected {expected_role}, found {selector}. The generic lld "
                "driver is not a role-specific linker entrypoint."
            )
    capabilities = native_link_capabilities(
        target=target,
        linker_hint=selected_linker_name,
    )
    policy = native_link_policy(
        target=target,
        profile=profile,
        keep_symbols=os.environ.get("MOLT_KEEP_SYMBOLS") == "1",
        bolt_requested=bolt_requested,
    )
    try:
        resolved_external_archives = tuple(
            archive.resolve(strict=True) for archive in external_static_archives
        )
    except OSError as exc:
        raise RuntimeError(f"External static archive is unavailable: {exc}") from exc
    runtime_lib_str = str(runtime_lib)
    if target.object_format is NativeObjectFormat.ELF:
        link_inputs.extend(
            [
                "-Wl,--start-group",
                *[str(archive) for archive in resolved_external_archives],
                runtime_lib_str,
                "-Wl,--end-group",
                *external_link_arguments,
                "-o",
                str(output_binary),
            ]
        )
    elif target.object_format is NativeObjectFormat.COFF:
        link_inputs.extend(
            [
                *[
                    f"-Wl,/WHOLEARCHIVE:{archive}"
                    for archive in resolved_external_archives
                ],
                runtime_lib_str,
                *external_link_arguments,
                "-o",
                str(output_binary),
            ]
        )
    else:
        # ld64 has no ELF-style archive group.  Preserve its explicit second
        # archive pass until the Mach-O runtime is represented as one acyclic
        # archive graph or a measured -force_load policy replaces it.
        link_inputs.extend(
            [
                *[
                    f"-Wl,-force_load,{archive}"
                    for archive in resolved_external_archives
                ],
                runtime_lib_str,
                runtime_lib_str,
                *external_link_arguments,
                "-o",
                str(output_binary),
            ]
        )
    link_cmd.extend(link_inputs)

    if target.object_format is NativeObjectFormat.MACHO:
        exported_symbols_path = output_binary.parent / ".molt_exports.exp"
        exported_symbols = ["_main"]
        if resolved_external_archives:
            for canonical, storage in _CPYTHON_SINGLETON_CANONICAL_ALIASES:
                link_cmd.append(f"-Wl,-alias,_{storage},_{canonical}")
        _atomic_write_text(exported_symbols_path, "\n".join(exported_symbols) + "\n")
        link_cmd.append(f"-Wl,-exported_symbols_list,{exported_symbols_path}")
        link_cmd.append("-lc++")
    elif target.object_format is NativeObjectFormat.ELF:
        link_cmd.extend(["-fdata-sections", "-ffunction-sections"])
        link_cmd.append("-Wl,--as-needed")
        link_cmd.append("-Wl,-O2")
        if policy.emit_relocations:
            link_cmd.append("-Wl,--emit-relocs")
        version_script_path = output_binary.parent / ".molt_version.ver"
        globals = "main;"
        if resolved_external_archives:
            singleton_globals = " ".join(
                f"{canonical}; {storage};"
                for canonical, storage in _CPYTHON_SINGLETON_CANONICAL_ALIASES
            )
            globals = f"main; {singleton_globals}"
            link_cmd.extend(
                f"-Wl,--defsym={canonical}={storage}"
                for canonical, storage in _CPYTHON_SINGLETON_CANONICAL_ALIASES
            )
        _atomic_write_text(version_script_path, f"{{ global: {globals} local: *; }};\n")
        link_cmd.append(f"-Wl,--version-script={version_script_path}")
        link_cmd.append("-lstdc++")
        link_cmd.append("-lm")
    elif target.object_format is NativeObjectFormat.COFF:
        if resolved_external_archives:
            def_path = output_binary.parent / ".molt_exports.def"
            exports = "\n".join(
                (
                    *(
                        f"{canonical}={storage}"
                        for canonical, storage in _CPYTHON_SINGLETON_CANONICAL_ALIASES
                    ),
                )
            )
            _atomic_write_text(def_path, f"EXPORTS\n{exports}\n")
            link_cmd.append(f"-Wl,/DEF:{def_path}")
    link_cmd.extend(
        native_link_policy_flags(
            target=target,
            capabilities=capabilities,
            dead_strip=policy.dead_strip,
        )
    )
    _append_darwin_runtime_frameworks(link_cmd, target_triple=target_triple)
    cargo_native_link_flags = _collect_cargo_native_link_deps(
        runtime_lib,
        target_triple=target_triple,
        object_format=target.object_format.value,
        source_root=source_root,
        source_fingerprint=source_fingerprint,
    )
    link_cmd.extend(cargo_native_link_flags)
    return NativeLinkPlan(
        target=target,
        capabilities=capabilities,
        policy=policy,
        command=tuple(link_cmd),
        linker_hint=selected_linker_name,
        normalized_target=normalized_target,
    )
