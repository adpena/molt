from __future__ import annotations

import os
from pathlib import Path
import platform
import sys
from typing import Sequence

from molt.flag_strings import FlagStringRule
from molt.cli.compiler_target import (
    compiler_target_triple,
    validate_compiler_target,
    _zig_target_query,
)
from molt.llvm_wasi_tools import (
    llvm_linker_candidates,
    llvm_named_tool_candidates,
    llvm_tool_candidates,
)
from molt.cli.source_extension_compiler_inputs import (
    compiler_sysroot_arg_value,
    darwin_compiler_commands,
)
from molt.toolchain_identity import resolve_explicit_tool_command
from molt.llvm_linker_roles import (
    LlvmLinkerRole,
    executable_entrypoint_name,
    executable_selects_linker_role,
    is_llvm_linker_role,
)
from molt.cli.native_link_deps import _collect_cargo_native_link_deps
from molt.cli.runtime_native_codegen import NativeRuntimeCodegenBinding
from molt.cli.runtime_identity_schema import RuntimeBuildIdentity
from molt.cli.native_link_plan import (
    NativeLinkPlan,
    NativeLinkSidecar,
    NativeSymbolStrip,
    native_link_capabilities,
    native_artifact_link_arguments,
    native_link_output_flags,
    native_link_policy_flags,
    native_link_strip_flags,
    native_linker_name_from_driver_command,
    native_link_policy,
)
from molt.native_target import (
    LinkDialect,
    NativeArtifactKind,
    NativeTargetSpec,
    resolve_native_target_spec,
)
from molt.native_target_shape import NativeObjectFormat
from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkRequirements,
    merge_source_extension_link_requirements,
    render_source_extension_link_arguments,
    validate_source_extension_link_input_files,
)
from molt.cli.native_toolchain import _append_darwin_runtime_frameworks


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
        linker_role = target.link_dialect.llvm_linker_role
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
            linker_role = target.link_dialect.llvm_linker_role
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
    target: NativeTargetSpec,
    driver_command: Sequence[str] = (),
    host_platform: str | None = None,
) -> str | None:
    host_platform = sys.platform if host_platform is None else host_platform
    linker_role = target.link_dialect.llvm_linker_role
    if profile == "dev":
        raw = os.environ.get("MOLT_DEV_LINKER", "auto").strip().lower()
        is_host_linux = target.is_host and host_platform.startswith("linux")
        is_host_windows = target.is_host and host_platform == "win32"
        if raw == "auto" and not (is_host_linux or is_host_windows):
            return None
        selected = _resolve_dev_linker(
            driver_command,
            host_platform=host_platform,
            linker_role=linker_role,
        )
        if selected == "mold" and target.os != "linux":
            raise RuntimeError("mold is supported only for Linux ELF link targets.")
        return selected
    is_host_fast_linker = target.is_host and (
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
    target: NativeTargetSpec,
    sysroot_path: Path | None,
    profile: str,
    host_platform: str | None = None,
) -> tuple[list[str], str | None, str | None]:
    host_platform = sys.platform if host_platform is None else host_platform
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
    normalized_target: str | None = None if target.is_host else target.triple
    if not target.is_host:
        cross_cc = os.environ.get("MOLT_CROSS_CC")
        target_arg = target.triple
        if cross_cc:
            link_cmd = list(
                resolve_explicit_tool_command(cross_cc, label="MOLT_CROSS_CC")
            )
        elif zig := llvm_named_tool_candidates("zig"):
            link_cmd = [str(zig[0]), "cc"]
            target_arg = _zig_target_query(target.triple)
            normalized_target = target_arg
        else:
            raise RuntimeError(
                f"Cross-target build requires zig or MOLT_CROSS_CC (missing for {target.triple})."
            )
        link_cmd.extend(["-target", target_arg])
    if sysroot_path is not None:
        sysroot_flag = "--sysroot"
        if target.os == "macos":
            sysroot_flag = "-isysroot"
        link_cmd.extend([sysroot_flag, str(sysroot_path)])
    # The link driver reads CFLAGS as cc-rs reads it for the runtime's C code,
    # so one CFLAGS value means the same arguments to both compilers.
    link_cmd.extend(FlagStringRule.cc(os.environ).decode(os.environ.get("CFLAGS", "")))
    if target.os == "macos":
        # One selected SDK supplies headers, libraries, framework search paths
        # and the default version. An argv sysroot overrides ambient SDKROOT.
        # Backend admission already checked every application archive member
        # against this target. Project its architecture at the Darwin argv
        # boundary; reopening the archive cannot select a different target.
        arch = "arm64" if target.arch == "aarch64" else target.arch
        if target.is_host:
            arch = os.environ.get("MOLT_ARCH") or arch
        try:
            link_cmd = list(
                darwin_compiler_commands(
                    {"link": tuple(link_cmd)},
                    environment=os.environ,
                    arch=arch,
                )["link"]
            )
        except (OSError, ValueError) as exc:
            raise RuntimeError(f"Darwin native link SDK selection: {exc}") from exc
        if target.is_host:
            link_cmd.extend(["-arch", arch])
        if not target.is_host:
            sdk_root = compiler_sysroot_arg_value(link_cmd)
            assert sdk_root is not None
            link_cmd.extend(
                [
                    "-F",
                    str(Path(sdk_root) / "System/Library/Frameworks"),
                    "-L",
                    str(Path(sdk_root) / "usr/lib"),
                ]
            )
    try:
        validate_compiler_target(
            link_cmd,
            compiler_target_triple(
                link_cmd,
                target.triple,
            ),
        )
    except ValueError as exc:
        raise RuntimeError(f"Native link compiler target custody: {exc}") from exc
    linker_hint = _resolve_native_linker_hint(
        profile=profile,
        target=target,
        driver_command=link_cmd,
        host_platform=host_platform,
    )
    if linker_hint and not any(arg.startswith("-fuse-ld=") for arg in link_cmd):
        link_cmd.append(f"-fuse-ld={linker_hint}")
    return link_cmd, linker_hint, normalized_target


def _build_native_link_plan(
    *,
    output_obj: Path,
    stub_path: Path,
    runtime_lib: Path,
    output_binary: Path,
    target: NativeTargetSpec,
    sysroot_path: Path | None,
    profile: str,
    runtime_build_identity: RuntimeBuildIdentity,
    runtime_codegen_binding: NativeRuntimeCodegenBinding | None = None,
    output_kind: NativeArtifactKind = NativeArtifactKind.ARCHIVE,
    stdlib_kind: NativeArtifactKind = NativeArtifactKind.ARCHIVE,
    stdlib_obj_path: Path | None = None,
    external_link_requirements: Sequence[SourceExtensionLinkRequirements] = (),
    bolt_requested: bool = False,
    host_platform: str | None = None,
) -> NativeLinkPlan:
    host_platform = sys.platform if host_platform is None else host_platform
    link_cmd, linker_hint, normalized_target = _build_native_link_driver_command(
        target=target,
        sysroot_path=sysroot_path,
        profile=profile,
        host_platform=host_platform,
    )
    link_inputs = [
        str(stub_path),
        *native_artifact_link_arguments(output_obj, kind=output_kind, target=target),
    ]
    if stdlib_obj_path is not None:
        if not stdlib_obj_path.is_file():
            raise RuntimeError(
                f"Shared stdlib artifact is unavailable: {stdlib_obj_path}"
            )
        link_inputs.extend(
            native_artifact_link_arguments(
                stdlib_obj_path, kind=stdlib_kind, target=target
            )
        )
    try:
        external_inputs = merge_source_extension_link_requirements(
            external_link_requirements, target_triple=target.triple
        )
        validate_source_extension_link_input_files(external_inputs)
    except (OSError, ValueError) as exc:
        raise RuntimeError(str(exc)) from exc
    external_link_arguments = render_source_extension_link_arguments(external_inputs)
    has_external_inputs = bool(external_inputs.inputs)
    selected_linker_name = native_linker_name_from_driver_command(
        link_cmd,
        hinted=linker_hint,
    )
    expected_role = target.link_dialect.llvm_linker_role
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
        capabilities=capabilities,
        profile=profile,
        keep_symbols=os.environ.get("MOLT_KEEP_SYMBOLS") == "1",
        bolt_requested=bolt_requested,
    )
    runtime_lib_str = str(runtime_lib)
    if target.object_format is NativeObjectFormat.ELF:
        link_inputs.extend(
            [
                "-Wl,--start-group",
                *external_link_arguments,
                runtime_lib_str,
                "-Wl,--end-group",
                "-o",
                str(output_binary),
            ]
        )
    elif target.object_format is NativeObjectFormat.COFF:
        link_inputs.extend(
            [
                *external_link_arguments,
                runtime_lib_str,
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
                *external_link_arguments,
                runtime_lib_str,
                runtime_lib_str,
                "-o",
                str(output_binary),
            ]
        )
    link_inputs.extend(
        native_link_output_flags(capabilities=capabilities, output=output_binary)
    )
    link_cmd.extend(link_inputs)
    sidecars: list[NativeLinkSidecar] = []

    if target.object_format is NativeObjectFormat.MACHO:
        exported_symbols_path = output_binary.parent / ".molt_exports.exp"
        exported_symbols = ["_main"]
        if has_external_inputs:
            for canonical, storage in _CPYTHON_SINGLETON_CANONICAL_ALIASES:
                link_cmd.append(f"-Wl,-alias,_{storage},_{canonical}")
        sidecars.append(
            NativeLinkSidecar(
                role="macho-exports",
                planned_path=exported_symbols_path,
                content=("\n".join(exported_symbols) + "\n").encode("utf-8"),
                command_index=len(link_cmd),
                operand_prefix="-Wl,-exported_symbols_list,",
            )
        )
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
        if has_external_inputs:
            singleton_globals = " ".join(
                f"{canonical}; {storage};"
                for canonical, storage in _CPYTHON_SINGLETON_CANONICAL_ALIASES
            )
            globals = f"main; {singleton_globals}"
            link_cmd.extend(
                f"-Wl,--defsym={canonical}={storage}"
                for canonical, storage in _CPYTHON_SINGLETON_CANONICAL_ALIASES
            )
        sidecars.append(
            NativeLinkSidecar(
                role="elf-version-script",
                planned_path=version_script_path,
                content=f"{{ global: {globals} local: *; }};\n".encode("utf-8"),
                command_index=len(link_cmd),
                operand_prefix="-Wl,--version-script=",
            )
        )
        link_cmd.append(f"-Wl,--version-script={version_script_path}")
        link_cmd.append("-lstdc++")
        link_cmd.append("-lm")
    elif target.object_format is NativeObjectFormat.COFF:
        if has_external_inputs:
            def_path = output_binary.parent / ".molt_exports.def"
            exports = "\n".join(
                (
                    *(
                        f"{canonical}={storage}"
                        for canonical, storage in _CPYTHON_SINGLETON_CANONICAL_ALIASES
                    ),
                )
            )
            sidecars.append(
                NativeLinkSidecar(
                    role="coff-exports",
                    planned_path=def_path,
                    content=f"EXPORTS\n{exports}\n".encode("utf-8"),
                    command_index=(
                        len(link_cmd) + 1
                        if target.link_dialect is LinkDialect.COFF_MSVC
                        else len(link_cmd)
                    ),
                    operand_prefix=(
                        "/DEF:" if target.link_dialect is LinkDialect.COFF_MSVC else ""
                    ),
                )
            )
            if target.link_dialect is LinkDialect.COFF_MSVC:
                link_cmd.extend(("-Xlinker", f"/DEF:{def_path}"))
            else:
                link_cmd.append(str(def_path))
    link_cmd.extend(
        native_link_policy_flags(
            target=target,
            capabilities=capabilities,
            dead_strip=policy.dead_strip,
        )
    )
    if policy.symbol_strip is NativeSymbolStrip.AT_LINK:
        link_cmd.extend(native_link_strip_flags(target))
    _append_darwin_runtime_frameworks(link_cmd, target=target)
    runtime_inputs = _collect_cargo_native_link_deps(
        runtime_lib,
        target_triple=target.cargo_target,
        object_format=target.object_format.value,
        runtime_build_identity=runtime_build_identity,
        runtime_codegen_binding=runtime_codegen_binding,
    )
    link_cmd.extend(runtime_inputs.flags)
    return NativeLinkPlan(
        target=target,
        capabilities=capabilities,
        policy=policy,
        command=tuple(link_cmd),
        linker_hint=selected_linker_name,
        normalized_target=normalized_target,
        sidecars=tuple(sidecars),
        selection_requirements=external_inputs if external_inputs.items else None,
        runtime_inputs=runtime_inputs,
    )


# The runtime leaves the application initializer to the final image. A JIT
# runner exports it, so a runtime image binds it when the runner loads it.
APP_ISOLATE_BOOTSTRAP_SYMBOL = "molt_isolate_bootstrap"


def runtime_image_suffix(target: NativeTargetSpec) -> str:
    if target.object_format is NativeObjectFormat.MACHO:
        return ".dylib"
    if target.object_format is NativeObjectFormat.ELF:
        return ".so"
    raise RuntimeError(
        f"The JIT tier has no {target.object_format.value} runtime image yet"
    )


def _build_native_runtime_image_link_plan(
    *,
    runtime_lib: Path,
    output_image: Path,
    target: NativeTargetSpec,
    sysroot_path: Path | None,
    profile: str,
    runtime_codegen_binding: NativeRuntimeCodegenBinding,
) -> NativeLinkPlan:
    """Link the whole AOT runtime archive into one shared image (JIT tier).

    Every archive member stays in the image and nothing is dead-stripped, so
    the image defines exactly the symbols an AOT link resolves generated code
    against. The archive's own Cargo native link plan and the AOT driver,
    SDK and framework selection apply unchanged.
    """
    if not target.is_host:
        raise RuntimeError("The JIT tier runs programs on the host target only")
    runtime_image_suffix(target)
    link_cmd, linker_hint, normalized_target = _build_native_link_driver_command(
        target=target,
        sysroot_path=sysroot_path,
        profile=profile,
    )
    selected_linker_name = native_linker_name_from_driver_command(
        link_cmd,
        hinted=linker_hint,
    )
    capabilities = native_link_capabilities(
        target=target,
        linker_hint=selected_linker_name,
    )
    runtime_lib_str = str(runtime_lib)
    if target.object_format is NativeObjectFormat.MACHO:
        link_cmd.extend(
            [
                "-dynamiclib",
                f"-Wl,-force_load,{runtime_lib_str}",
                f"-Wl,-U,_{APP_ISOLATE_BOOTSTRAP_SYMBOL}",
                "-o",
                str(output_image),
                *native_link_output_flags(
                    capabilities=capabilities, output=output_image
                ),
                "-lc++",
            ]
        )
        _append_darwin_runtime_frameworks(link_cmd, target=target)
    else:
        # A shared object may leave the bootstrap undefined; the dynamic
        # loader binds it to the runner's export.
        link_cmd.extend(
            [
                "-shared",
                "-Wl,--whole-archive",
                runtime_lib_str,
                "-Wl,--no-whole-archive",
                "-o",
                str(output_image),
                "-lstdc++",
                "-lm",
            ]
        )
    runtime_inputs = _collect_cargo_native_link_deps(
        runtime_lib,
        target_triple=target.cargo_target,
        object_format=target.object_format.value,
        runtime_build_identity=runtime_codegen_binding.build_identity,
        runtime_codegen_binding=runtime_codegen_binding,
    )
    link_cmd.extend(runtime_inputs.flags)
    return NativeLinkPlan(
        target=target,
        capabilities=capabilities,
        policy=native_link_policy(
            target=target,
            capabilities=capabilities,
            profile=profile,
            keep_symbols=True,
            bolt_requested=False,
        ),
        command=tuple(link_cmd),
        linker_hint=selected_linker_name,
        normalized_target=normalized_target,
        runtime_inputs=runtime_inputs,
    )
