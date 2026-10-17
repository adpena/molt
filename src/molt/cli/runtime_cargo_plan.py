"""Resolve one executable Cargo plan for both runtime capture and execution.

The plan is live host state, never embedded in a portable artifact receipt.
Configuration bytes are parsed and fingerprinted from the same stable snapshot.
"""

from __future__ import annotations

import os
import re
import shlex
import sys
import tempfile
import tomllib
from collections.abc import Generator, Iterator, MutableMapping
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass, field, replace
from enum import Enum
from pathlib import Path
from types import MappingProxyType
from typing import Callable, Mapping, Sequence, cast

from molt import process_guard
from molt.cli import cargo_command as cargo_argv
from molt.cargo_execution_policy import validate_cargo_native_dependency_environment
from molt.cli.cargo_target_cfg import (
    RustcTargetMetadata,
    cargo_target_query_arguments,
    parse_rustc_target_metadata,
    select_cargo_target_flags,
    rust_probe_arguments,
)
from molt.exact_json import canonical_json_sha256, string_keyed_mapping
from molt.rust_toolchain import (
    canonical_rust_codegen_option,
    rust_flag_spans,
    cargo_configuration_paths,
    cargo_configuration_value,
    cargo_selected_value,
    resolve_rustup_proxy,
    rust_toolchain_library_environment,
)
from molt.cli.cargo_resource_custody import (
    CargoExecutableCustody,
    CargoResourceCustody,
    CargoResourceIdentity,
    CargoResourceRoot,
    is_apple_sdk,
)
from molt.cli.runtime_identity_schema import _freeze_json
from molt.cli.wasm_link_args import wasi_external_libc_rustflags
from molt.llvm_toolchain import (
    LlvmToolchainConfigError,
    WasiSdkInstallation,
    apply_provisioned_wasm_toolchain,
    selected_wasi_sdk_installation,
    wasi_c_abi_plan,
)
from molt.source_root import compiler_source_root
from molt.wasi_sdk_identity import WasiCAbiProjection, wasi_c_abi_projection
from molt.toolchain_identity import (
    ExecutableIdentity,
    find_executable,
    StableRegularFileIdentity,
    capture_stable_regular_file,
    resolve_executable,
    stable_executable_probe,
    verify_stable_regular_file_identity,
)


class _CargoEnvironment(MutableMapping[str, str]):
    """One platform-correct environment map, including Windows key overwrite."""

    def __init__(
        self, values: Mapping[str, str], *, case_insensitive: bool | None = None
    ) -> None:
        self.case_insensitive = (
            os.name == "nt" if case_insensitive is None else case_insensitive
        )
        self._values: dict[str, str] = {}
        self.update(values)

    def canonical_key(self, name: str) -> str:
        return name.upper() if self.case_insensitive else name

    def __getitem__(self, name: str) -> str:
        return self._values[self.canonical_key(name)]

    def __setitem__(self, name: str, value: str) -> None:
        self._values[self.canonical_key(name)] = value

    def __delitem__(self, name: str) -> None:
        del self._values[self.canonical_key(name)]

    def __iter__(self) -> Iterator[str]:
        return iter(self._values)

    def __len__(self) -> int:
        return len(self._values)


@dataclass(frozen=True, slots=True)
class CargoConfigurationInput:
    label: str
    identity: StableRegularFileIdentity
    document: Mapping[str, object]

    @property
    def path(self) -> Path:
        return self.identity.path


@dataclass(frozen=True, slots=True)
class CargoConfigurationSource:
    """Frozen configuration input, without fabricated live file identity."""

    label: str
    path: Path
    document: Mapping[str, object]


_C_FLAG_PREFIXES = (
    "CFLAGS",
    "CXXFLAGS",
    "CPPFLAGS",
    "LDFLAGS",
    "ARFLAGS",
    "RANLIBFLAGS",
    "ASFLAGS",
)
_C_FLAG_CONTROLS = (
    "CC_SHELL_ESCAPED_FLAGS",
    "CC_FORCE_DISABLE",
    "CRATE_CC_NO_DEFAULTS",
    "CC_ENABLE_DEBUG_OUTPUT",
    "SOURCE_DATE_EPOCH",
    "MACOSX_DEPLOYMENT_TARGET",
    "IPHONEOS_DEPLOYMENT_TARGET",
    "WATCHOS_DEPLOYMENT_TARGET",
    "TVOS_DEPLOYMENT_TARGET",
    "XROS_DEPLOYMENT_TARGET",
)
_C_SEARCH_ENVIRONMENTS = (
    "CPATH",
    "C_INCLUDE_PATH",
    "CPLUS_INCLUDE_PATH",
    "OBJC_INCLUDE_PATH",
    "LIBRARY_PATH",
    "INCLUDE",
    "LIB",
    "LIBPATH",
    "SDKROOT",
)


class CResourceKind(Enum):
    DIRECTORY = "directory"
    FILE = "file"
    EITHER = "either"


_C_RESOURCE_ARGUMENTS: Mapping[str, CResourceKind] = MappingProxyType(
    {
        "-I": CResourceKind.DIRECTORY,
        "-L": CResourceKind.DIRECTORY,
        "-F": CResourceKind.DIRECTORY,
        "-isystem": CResourceKind.DIRECTORY,
        "-iquote": CResourceKind.DIRECTORY,
        "-idirafter": CResourceKind.DIRECTORY,
        "-iframework": CResourceKind.DIRECTORY,
        "-iframeworkwithsysroot": CResourceKind.DIRECTORY,
        "-iprefix": CResourceKind.DIRECTORY,
        "-iwithprefix": CResourceKind.DIRECTORY,
        "-iwithprefixbefore": CResourceKind.DIRECTORY,
        "-isysroot": CResourceKind.DIRECTORY,
        "--sysroot": CResourceKind.DIRECTORY,
        "-resource-dir": CResourceKind.DIRECTORY,
        "--gcc-toolchain": CResourceKind.DIRECTORY,
        "-gcc-toolchain": CResourceKind.DIRECTORY,
        "-B": CResourceKind.DIRECTORY,
        "-include": CResourceKind.FILE,
        "-imacros": CResourceKind.FILE,
        "-include-pch": CResourceKind.FILE,
        "-ivfsoverlay": CResourceKind.FILE,
        "-fmodule-map-file": CResourceKind.FILE,
        "-fmodule-file": CResourceKind.FILE,
        "-fprofile-use": CResourceKind.EITHER,
        "-fprofile-instr-use": CResourceKind.FILE,
        "-fprofile-sample-use": CResourceKind.FILE,
        "-specs": CResourceKind.FILE,
        "--specs": CResourceKind.FILE,
        "-T": CResourceKind.FILE,
        "--script": CResourceKind.FILE,
        "--version-script": CResourceKind.FILE,
        "--dynamic-list": CResourceKind.FILE,
        "--retain-symbols-file": CResourceKind.FILE,
        "-rpath-link": CResourceKind.DIRECTORY,
        "--rpath-link": CResourceKind.DIRECTORY,
        "/I": CResourceKind.DIRECTORY,
        "/external:I": CResourceKind.DIRECTORY,
        "/AI": CResourceKind.DIRECTORY,
        "/FI": CResourceKind.FILE,
        "/FU": CResourceKind.FILE,
        "/LIBPATH:": CResourceKind.DIRECTORY,
        "/DEF:": CResourceKind.FILE,
    }
)


# A leading '=' is only an option/value separator for these grammars. In
# joined search flags such as -I=/include and -L=/lib it belongs to the
# operand and asks the compiler to apply its effective sysroot instead.
_C_VALUE_SEPARATOR_ARGUMENTS = frozenset(
    {
        "--sysroot",
        "-resource-dir",
        "--gcc-toolchain",
        "-gcc-toolchain",
        "-fmodule-map-file",
        "-fmodule-file",
        "-fprofile-use",
        "-fprofile-instr-use",
        "-fprofile-sample-use",
        "-specs",
        "--specs",
        "--script",
        "--version-script",
        "--dynamic-list",
        "--retain-symbols-file",
        "-rpath-link",
        "--rpath-link",
    }
)
_C_IMPLICIT_PREFIX_ARGUMENTS = frozenset(
    {
        "-iframeworkwithsysroot",
        "-iprefix",
        "-iwithprefix",
        "-iwithprefixbefore",
    }
)


_C_LITERAL_ARGUMENT_OPTIONS = frozenset(
    {
        "-o",
        "-MF",
        "-MT",
        "-MQ",
        "-MJ",
        "-D",
        "-U",
        "-z",
        "-u",
        "--undefined",
        "-rpath",
        "--rpath",
        "-soname",
        "--soname",
        "-install_name",
        "-dynamic-linker",
        "--dynamic-linker",
        "-arch",
        "-target",
        "--target",
    }
)


def runtime_c_flag_environment_names(
    target: str, host_target: str | None = None
) -> tuple[str, ...]:
    """Canonical cc-rs flag variables and parsing controls (not PATH selectors)."""
    forms = tuple(
        dict.fromkeys(
            value
            for triple in dict.fromkeys((target, host_target or target))
            for value in (triple, triple.replace("-", "_").replace(".", "_"))
        )
    )
    names = set(_C_FLAG_CONTROLS)
    for prefix in _C_FLAG_PREFIXES:
        names.update((prefix, "HOST_" + prefix, "TARGET_" + prefix))
        names.update(prefix + "_" + form for form in forms)
    return tuple(sorted(names))


def runtime_c_flag_tokens(value: str, *, shell_escaped: bool) -> tuple[str, ...]:
    if shell_escaped:
        return tuple(shlex.split(value, comments=True, posix=True))
    return tuple(item for item in re.split(r"[ \t\n\r\v\f]+", value) if item)


@dataclass(frozen=True, slots=True)
class RuntimeCResourcePlan:
    roots: tuple[CargoResourceRoot, ...]
    logical_paths: tuple[tuple[str, Path], ...]
    environment: Mapping[str, tuple[str, ...]]


def _resolve_c_build_resources(
    env: _CargoEnvironment, *, target: str, host_target: str | None = None
) -> RuntimeCResourcePlan:
    """Fence C search inputs without changing the child compiler's semantics.

    Cargo's build-script cwd is each package directory, not the workspace.
    The local source closure excludes registry dependencies, so it is not an
    authority for resolving relative paths or GCC's empty (cwd) search entry.
    Those selectors require a complete build-script cwd plan before admission.
    Sysroot/prefix-relative operands also require a proven effective C sysroot
    or include-prefix selection; a host-absolute suffix is not that authority.
    MSVC's empty semicolon search entries are inert and remain in projection.
    """
    resources: list[CargoResourceRoot] = []
    paths: list[tuple[str, Path]] = []
    projection: dict[str, tuple[str, ...]] = {}

    def resource(
        value: str,
        *,
        label: str,
        kind: CResourceKind,
        must_exist: bool = False,
        identity: CargoResourceIdentity = CargoResourceIdentity.CONTENT,
    ) -> str:
        if value.startswith(("=", "$SYSROOT", "${SYSROOT}")):
            raise ValueError(
                f"runtime C resource {label} has unbound sysroot-relative operand: {value!r}; "
                "an effective C sysroot authority is required before resolving this input"
            )
        path = Path(value)
        if not value or not path.is_absolute():
            raise ValueError(
                f"runtime C resource {label} has unbound build-script cwd: {value!r}; "
                "select an absolute input path or provide a complete package cwd authority"
            )
        if path.exists() and (
            (kind is CResourceKind.DIRECTORY and not path.is_dir())
            or (kind is CResourceKind.FILE and not path.is_file())
        ):
            raise ValueError(f"runtime C resource {label} has the wrong kind: {path}")
        required = kind is not CResourceKind.DIRECTORY or must_exist
        if required and not path.exists():
            raise ValueError(f"runtime C resource {label} is missing: {path}")
        resources.append(
            CargoResourceRoot(label, path, required=required, identity=identity)
        )
        paths.append((label, path))
        return "${" + label + "}"

    def flags(
        tokens: Sequence[str], *, name: str, lane: str = "flags", forwarded: str = ""
    ) -> tuple[str, ...]:
        result: list[str] = []
        iterator = iter(enumerate(tokens))
        joined_options = sorted(_C_RESOURCE_ARGUMENTS, key=len, reverse=True)
        msvc = target.endswith("-msvc")
        linker_lane = forwarded == "-Wl" or "LDFLAGS" in name
        for index, token in iterator:
            if msvc and token == "/link":
                linker_lane = True
                result.append(token)
                continue
            if token in _C_LITERAL_ARGUMENT_OPTIONS:
                next_item = next(iterator, None)
                if next_item is None:
                    raise ValueError(f"runtime C {name} {token} requires a value")
                result.extend((token, next_item[1]))
                continue
            if token.startswith("@"):
                raise ValueError(
                    f"runtime C {name} response file requires compiler-specific argument custody"
                )
            if token in {"-Xclang", "-Xlinker", "-Xpreprocessor", "-Xassembler"}:
                raise ValueError(
                    f"runtime C {name} forwarded arguments require a typed {token} resource plan"
                )
            if token.startswith(("-Wl,", "-Wp,", "-Wa,")):
                prefix, values = token.split(",", 1)
                result.append(
                    prefix
                    + ","
                    + ",".join(
                        flags(
                            values.split(","),
                            name=name,
                            lane=f"{lane}/{index}",
                            forwarded=prefix,
                        )
                    )
                )
                continue
            option = next(
                (
                    candidate
                    for candidate in joined_options
                    if not (forwarded == "-Wl" and candidate in {"-I", "-F", "-B"})
                    and not (forwarded == "-Wa" and candidate not in {"-I"})
                    and not (
                        msvc
                        and linker_lane
                        and candidate in {"/I", "/AI", "/FI", "/FU", "/external:I"}
                    )
                    and (
                        token == candidate
                        or token.startswith(candidate + "=")
                        or (
                            candidate
                            in {
                                "-I",
                                "-L",
                                "-F",
                                "-B",
                                "-T",
                                "-isystem",
                                "-iquote",
                                "-idirafter",
                                "-iframework",
                                "-iframeworkwithsysroot",
                                "-iprefix",
                                "-iwithprefix",
                                "-iwithprefixbefore",
                                "-isysroot",
                                "-include",
                                "-imacros",
                                "/I",
                                "/AI",
                                "/FI",
                                "/FU",
                                "/external:I",
                                "/LIBPATH:",
                                "/DEF:",
                            }
                            and token.startswith(candidate)
                        )
                    )
                ),
                None,
            )
            if option is None:
                # Ordinary macro values and output filenames are semantic text,
                # not guessed input selectors. Positional object/archive inputs
                # are real linker resources, including absolute paths.
                if not token.startswith("-") and (
                    (Path(token).is_absolute() and not (msvc and token.startswith("/")))
                    or Path(token).suffix.lower()
                    in {
                        ".a",
                        ".lib",
                        ".o",
                        ".obj",
                        ".so",
                        ".dylib",
                        ".bc",
                        ".c",
                        ".cpp",
                        ".s",
                    }
                    or ".so." in Path(token).name
                ):
                    result.append(
                        resource(
                            token,
                            label=f"c/{name}/{lane}/{index}",
                            kind=CResourceKind.FILE,
                        )
                    )
                else:
                    result.append(token)
                continue
            value = token[len(option) :]
            if value.startswith("=") and option in _C_VALUE_SEPARATOR_ARGUMENTS:
                value = value[1:]
            if not value:
                next_item = next(iterator, None)
                if next_item is None:
                    raise ValueError(
                        f"runtime C {name} {option} requires an input path"
                    )
                _value_index, value = next_item
            if option in _C_IMPLICIT_PREFIX_ARGUMENTS:
                raise ValueError(
                    f"runtime C {name} {option} has unbound sysroot/include-prefix resource selection; "
                    "an effective C prefix authority is required before resolving this input"
                )
            if option == "-I" and value == "-":
                result.append("-I-")
                continue
            module = ""
            if option == "-fmodule-file" and "=" in value:
                module, value = value.split("=", 1)
                module += "="
            selected = resource(
                value,
                label=f"c/{name}/{lane}/{index}",
                kind=_C_RESOURCE_ARGUMENTS[option],
                must_exist=option
                in {
                    "-isysroot",
                    "--sysroot",
                    "-resource-dir",
                    "--gcc-toolchain",
                    "-gcc-toolchain",
                },
            )
            # This is identity-only syntax; the exact original environment is
            # executed unchanged. Equal selectors share one portable projection.
            result.extend((option, module + selected))
        return tuple(result)

    shell_flags = env.get("CC_SHELL_ESCAPED_FLAGS", "") not in {"", "0", "false", "no"}
    for name in runtime_c_flag_environment_names(target, host_target):
        key = env.canonical_key(name)
        if key not in env or key in projection:
            continue
        value = env[key]
        if name in _C_FLAG_CONTROLS:
            projection[key] = (value,)
            continue
        try:
            tokens = runtime_c_flag_tokens(value, shell_escaped=shell_flags)
        except ValueError as exc:
            raise ValueError(
                f"runtime C {name} has invalid shell-escaped flags: {exc}"
            ) from exc
        projection[key] = flags(tokens, name=key)
    for name in _C_SEARCH_ENVIRONMENTS:
        if name not in env:
            continue
        value = env[name]
        if name == "SDKROOT" and not value:
            projection[name] = ()
            continue
        separator = ";" if env.case_insensitive else ":"
        entries = (value,) if name == "SDKROOT" else value.split(separator)
        selected_entries = []
        for index, entry in enumerate(entries):
            if not entry and name in {"INCLUDE", "LIB", "LIBPATH"}:
                selected_entries.append("")
                continue
            sdk = name == "SDKROOT"
            selected_entries.append(
                resource(
                    entry,
                    label=f"c/{name}/search/{index}",
                    kind=CResourceKind.DIRECTORY,
                    must_exist=sdk,
                    identity=CargoResourceIdentity.APPLE_SDK
                    if sdk and is_apple_sdk(Path(entry))
                    else CargoResourceIdentity.CONTENT,
                )
            )
        projection[name] = tuple(selected_entries)
    return RuntimeCResourcePlan(
        tuple(resources), tuple(paths), MappingProxyType(projection)
    )


@contextmanager
def _rust_probe_command(
    rustc: Path, wrappers: Mapping[str, Path], *, label: str
) -> Generator[list[str]]:
    """Cargo workspace_process order, with every executable fenced together."""
    with ExitStack() as stack:
        command = []
        for role, path in (
            *(
                (role, wrappers[role])
                for role in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER")
                if role in wrappers
            ),
            ("rustc", rustc),
        ):
            entrypoint, _identity = stack.enter_context(
                stable_executable_probe(path, label=f"{label}/{role}")
            )
            command.append(os.fspath(entrypoint))
        yield command


def _rust_resource_roots(
    rustc: Path,
    *,
    root: Path,
    env: Mapping[str, str],
    target: str,
    host: str,
    flag_lanes: tuple[tuple[str, ...], ...] = ((),),
    wrappers: Mapping[str, Path] = MappingProxyType({}),
    target_argument: str | None,
    metadata_probe: Callable[[str | None, tuple[str, ...], bool], RustcTargetMetadata]
    | None = None,
) -> tuple[CargoResourceRoot, ...]:
    def metadata(
        query_target: str | None, flags: tuple[str, ...], wrapped: bool
    ) -> RustcTargetMetadata:
        if metadata_probe is not None:
            return metadata_probe(query_target, flags, wrapped)
        return _rust_target_metadata(
            rustc,
            query_target,
            flags,
            root=root,
            env=env,
            wrappers=wrappers if wrapped else {},
        )

    # The selected compiler's installed driver remains a resource even when a
    # wrapper or explicit flags select a different target sysroot. Query the
    # compiler itself here, not the wrapper's virtualized target selection.
    sysroot = metadata(None, (), False).sysroot.resolve(strict=True)
    selected: list[CargoResourceRoot] = []
    for index, flags in enumerate(flag_lanes):
        info = metadata(target_argument, flags, True)
        target_libdir = info.target_libdir(target).resolve(strict=True)
        selected.append(CargoResourceRoot(f"rust/target-libdir/{index}", target_libdir))
        if info.sysroot.resolve(strict=True) != sysroot:
            selected.append(
                CargoResourceRoot(
                    f"rust/target-codegen/{index}",
                    info.sysroot / "lib" / "rustlib" / host / "codegen-backends",
                    required=False,
                )
            )
    lib = sysroot / "lib"
    roots = [
        *selected,
        CargoResourceRoot("rust/driver", lib, recursive=False),
        CargoResourceRoot(
            "rust/driver-bin",
            sysroot / "bin",
            recursive=False,
            dynamic_libraries_only=True,
        ),
        CargoResourceRoot(
            "rust/codegen-backends",
            lib / "rustlib" / host / "codegen-backends",
            required=False,
        ),
    ]
    if target != host:
        host_info = metadata(None, (), True)
        roots.append(
            CargoResourceRoot(
                "rust/host-libdir", host_info.target_libdir(host).resolve(strict=True)
            )
        )
        if host_info.sysroot.resolve(strict=True) != sysroot:
            roots.append(
                CargoResourceRoot(
                    "rust/host-codegen",
                    host_info.sysroot / "lib" / "rustlib" / host / "codegen-backends",
                    required=False,
                )
            )
    return tuple(roots)


@dataclass(frozen=True, slots=True)
class RustFlagResourcePlan:
    flags: tuple[str, ...]
    command: tuple[str, ...]
    roots: tuple[CargoResourceRoot, ...]
    logical_paths: tuple[tuple[str, Path], ...]
    flag_lanes: tuple[tuple[str, ...], ...]
    dependency_linker: Path | None
    final_linker: Path | None
    link_roots: tuple[CargoResourceRoot, ...]


def _resolve_rust_flag_resources(
    flags: tuple[str, ...],
    command: Sequence[str],
    *,
    root: Path,
    env: Mapping[str, str],
    wasi_plan: WasiCAbiProjection | None,
) -> RustFlagResourcePlan:
    """Resolve resource selectors in dependency flags and final-crate flags.

    The lanes stay distinct: a final-crate override must not discard the
    dependency lane's linker/sysroot/codegen resources.
    """
    resources: list[CargoResourceRoot] = []
    link_resources: list[CargoResourceRoot] = []
    logical_paths: list[tuple[str, Path]] = []
    linker: Path | None = None
    managed_search = () if wasi_plan is None else wasi_plan.native_search_directories()

    def local_path(value: str, *, label: str, directory: bool) -> Path:
        if not value:
            raise ValueError(f"runtime Rust {label} requires a resource path")
        path = Path(value)
        path = path if path.is_absolute() else root / path
        valid = path.is_dir() if directory else path.is_file()
        if not valid:
            raise ValueError(
                f"runtime Rust {label} resource is missing or has the wrong kind: {path}"
            )
        return Path(os.path.abspath(path))

    def lane(
        tokens: Sequence[str], name: str, inherited_sysroot: Path | None
    ) -> tuple[tuple[str, ...], Path | None]:
        nonlocal linker
        if any(token.startswith("@") for token in tokens):
            raise ValueError(
                "runtime Rust argument response files require parsed argument custody; linker response files use -C link-arg=@file"
            )
        selected_sysroot = inherited_sysroot
        if wasi_plan is not None and (tokens or name == "dependencies"):
            tokens = wasi_external_libc_rustflags(
                tokens,
                plan=wasi_plan,
                include_search=name == "dependencies",
            )
        result: list[str] = []
        managed_native_seen: set[Path] = set()
        entries: list[tuple[str, str, tuple[str, ...]]] = []
        for span in rust_flag_spans(tokens):
            token = tokens[span.start]
            original = tuple(tokens[span.start : span.stop])
            if token == "--":
                entries.append(("", "", original))
                break
            if span.codegen is not None:
                key, value = "-C", span.codegen
            elif span.option in {"--sysroot", "--extern", "-L", "-Z"}:
                key, value = span.option, span.value
                if not value:
                    raise ValueError(f"runtime Rust {key} requires an operand")
                if key == "-Z":
                    value = canonical_rust_codegen_option(value)
            else:
                entries.append(("", "", original))
                continue
            if span.leading:
                entries.append(("", "", span.leading))
            original = (key, value)
            if key in {"-C", "-Z"}:
                option, separator, operand = value.partition("=")
                if option in {"link-arg", "link-args"} and "@" in operand:
                    if option != "link-arg" or not operand.startswith("@"):
                        raise ValueError(
                            "runtime linker response requires one explicit -C link-arg=@path operand"
                        )
                    path = local_path(
                        operand[1:], label="linker response", directory=False
                    )
                    response = CargoResourceRoot(
                        f"rust/link-response/{name}/{len(link_resources) + len(resources)}",
                        path,
                    )
                    (resources if name == "dependencies" else link_resources).append(
                        response
                    )
                    entries.append(("", "", (key, "link-arg=@" + os.fspath(path))))
                    continue
                if option not in {"linker", "codegen-backend"}:
                    entries.append(("", "", original))
                    continue
                if not separator or not operand:
                    raise ValueError(
                        f"runtime Rust {option} requires a resource selector"
                    )
                key, value = key + ":" + option, operand
            entries.append((key, value, original))
        # rustc registers --sysroot as Opt (not Multi), so a dependency-lane
        # selector plus another final-crate selector is invalid too.
        sysroot_count = sum(key == "--sysroot" for key, _, _ in entries)
        if sysroot_count > 1 or (sysroot_count and inherited_sysroot is not None):
            raise ValueError("runtime Rust duplicate --sysroot selectors are ambiguous")
        last = {
            key: index
            for index, (key, _, _) in enumerate(entries)
            if key
            in {"-C:linker", "-Z:linker", "-C:codegen-backend", "-Z:codegen-backend"}
        }
        resource_index = 0
        for index, (key, value, original) in enumerate(entries):
            if not key:
                result.extend(original)
                continue
            if key in last and index != last[key]:
                continue  # rustc codegen option precedence is last-value wins.
            label = f"rust/flags/{name}/{resource_index}"
            resource_index += 1
            if key == "--sysroot":
                selected_sysroot = local_path(value, label="sysroot", directory=True)
                logical_paths.append((label + "/sysroot", selected_sysroot))
                result.extend(("--sysroot", os.fspath(selected_sysroot)))
            elif key == "-L":
                kind, separator, operand = value.partition("=")
                if not separator or kind not in {
                    "dependency",
                    "crate",
                    "native",
                    "framework",
                    "all",
                }:
                    kind, operand = "all", value
                path = Path(operand)
                if path not in managed_search:
                    path = local_path(operand, label="-L " + kind, directory=True)
                    if kind in {"native", "all"} and managed_search:
                        resolved = path.resolve()
                        if resolved in managed_search:
                            path = resolved
                if kind in {"native", "all"} and path in managed_search:
                    if kind == "native" and path in managed_native_seen:
                        continue
                    if kind == "native":
                        managed_native_seen.add(path)
                    logical_paths.append(
                        (f"wasi/search/{managed_search.index(path)}", path)
                    )
                    result.extend(("-L", kind + "=" + os.fspath(path)))
                    continue
                resources.append(CargoResourceRoot(label + "/search-" + kind, path))
                logical_paths.append((label + "/search-" + kind, path))
                result.extend(("-L", kind + "=" + os.fspath(path)))
            elif key == "--extern":
                crate, separator, operand = value.partition("=")
                if not separator or not crate:
                    raise ValueError(
                        "runtime Rust --extern requires an explicit crate=artifact path for content custody"
                    )
                path = local_path(operand, label="--extern " + crate, directory=False)
                resources.append(CargoResourceRoot(label + "/extern", path))
                logical_paths.append((label + "/extern", path))
                result.extend(("--extern", crate + "=" + os.fspath(path)))
            else:
                switch, option = key.split(":", 1)
                if option == "linker":
                    path = _tool_path(
                        value, root=root, env=env, role="Rust flag linker"
                    )
                    linker = path
                    result.extend((switch, option + "=" + os.fspath(path)))
                    continue  # Executable custody belongs to the selected tool role.
                elif (
                    any(separator in value for separator in ("/", "\\"))
                    or Path(value).suffix
                ):
                    path = local_path(value, label="codegen-backend", directory=False)
                else:
                    if not re.fullmatch(r"[A-Za-z0-9_-]+", value):
                        raise ValueError(
                            f"runtime Rust named codegen backend is invalid: {value}"
                        )
                    result.extend((switch, option + "=" + value))
                    continue  # Named backend lookup is covered by selected sysroots.
                resources.append(CargoResourceRoot(label + "/" + option, path))
                logical_paths.append((label + "/" + option, path))
                result.extend((switch, option + "=" + os.fspath(path)))
        return tuple(result), selected_sysroot

    normalized, dependency_sysroot = lane(flags, "dependencies", None)
    dependency_linker = linker
    linker = None
    separator = command.index("--") if "--" in command else len(command)
    final_args, _final_sysroot = lane(
        command[separator + 1 :], "final-crate", dependency_sysroot
    )
    normalized_command = (
        *command[:separator],
        *(("--", *final_args) if separator < len(command) else ()),
    )
    return RustFlagResourcePlan(
        normalized,
        normalized_command,
        tuple(resources),
        tuple(logical_paths),
        tuple(dict.fromkeys((normalized, (*normalized, *final_args)))),
        dependency_linker,
        linker,
        tuple(link_resources),
    )


def _profile_environment_prefix(profile: str) -> str:
    return "CARGO_PROFILE_" + profile.upper().replace("-", "_") + "_"


def _table(value: object, *, label: str) -> Mapping[str, object]:
    table = string_keyed_mapping(value)
    if table is None:
        raise ValueError(f"runtime Cargo {label} must be a string-keyed table")
    return table


def _merge(
    left: Mapping[str, object], right: Mapping[str, object]
) -> dict[str, object]:
    result = dict(left)
    for key, value in right.items():
        old = result.get(key)
        if isinstance(old, Mapping) and isinstance(value, Mapping):
            result[key] = _merge(_table(old, label=key), _table(value, label=key))
        elif isinstance(old, (list, tuple)) and isinstance(value, (list, tuple)):
            result[key] = (*old, *value)
        else:
            result[key] = value
    return result


def _environment_origins(
    document: Mapping[str, object], base: Path, origins: dict[str, Path]
) -> None:
    for name, definition in _table(document.get("env", {}), label="env").items():
        if isinstance(definition, str) or (
            isinstance(definition, Mapping) and "value" in definition
        ):
            origins[name] = base


def _apply_cargo_environment(
    configuration: Mapping[str, object],
    cli: Mapping[str, object],
    env: Mapping[str, str],
    origins: Mapping[str, Path],
) -> tuple[_CargoEnvironment, tuple[str, ...], tuple[CargoResourceRoot, ...]]:
    definitions = _merge(
        _table(configuration.get("env", {}), label="env"),
        _table(cli.get("env", {}), label="CLI env"),
    )
    result = _CargoEnvironment(
        env,
        case_insensitive=env.case_insensitive
        if isinstance(env, _CargoEnvironment)
        else None,
    )
    forced: list[str] = []
    resources: list[CargoResourceRoot] = []
    seen: dict[str, str] = {}
    for name, raw in definitions.items():
        if not name or "=" in name or "\0" in name:
            raise ValueError(f"runtime Cargo environment key is invalid: {name!r}")
        canonical = result.canonical_key(name)
        if canonical in seen:
            raise ValueError(
                f"runtime Cargo environment keys are case-ambiguous: {seen[canonical]!r} and {name!r}"
            )
        seen[canonical] = name
        if isinstance(raw, str):
            value, force, relative = raw, False, False
        else:
            definition = _table(raw, label="environment " + name)
            if set(definition) - {"value", "force", "relative"}:
                raise ValueError(f"runtime Cargo environment {name} has unknown fields")
            value = definition.get("value")
            force, relative = (
                definition.get("force", False),
                definition.get("relative", False),
            )
            if (
                not isinstance(value, str)
                or type(force) is not bool
                or type(relative) is not bool
            ):
                raise ValueError(
                    f"runtime Cargo environment {name} has invalid value/force/relative"
                )
        if "\0" in value:
            raise ValueError(f"runtime Cargo environment {name} contains NUL")
        if not force and name in result:
            continue
        if relative:
            base = origins.get(name)
            if base is None:
                raise ValueError(
                    f"runtime Cargo relative environment {name} has no source custody"
                )
            path = (base / value).absolute()
            resources.append(CargoResourceRoot("cargo/env/" + name, path))
            value = os.fspath(path)
        result[name] = value
        if force:
            forced.append(name)
    return result, tuple(sorted(forced)), tuple(resources)


def _capture_config(path: Path, *, label: str) -> CargoConfigurationInput:
    identity, raw = capture_stable_regular_file(path, label=label)
    return CargoConfigurationInput(
        label, identity, cargo_configuration_document(raw, path)
    )


def cargo_configuration_document(raw: bytes, path: Path) -> Mapping[str, object]:
    """Parse captured Cargo bytes with their existing path-origin semantics."""
    try:
        value = tomllib.loads(raw.decode("utf-8"))
    except (UnicodeError, tomllib.TOMLDecodeError) as exc:
        raise ValueError(f"runtime Cargo configuration is invalid: {path}") from exc
    if "include" in value:
        raise ValueError(
            f"runtime Cargo configuration include needs resolved custody: {path}"
        )
    # Cargo resolves executable paths relative to the parent of the directory
    # containing a config file; bare names remain PATH lookups.
    tables = []
    build = value.get("build")
    if isinstance(build, dict):
        tables.extend(
            (build, name)
            for name in ("rustc", "rustc-wrapper", "rustc-workspace-wrapper")
        )
    targets = value.get("target")
    if isinstance(targets, dict):
        tables.extend(
            (table, "linker") for table in targets.values() if isinstance(table, dict)
        )
    for table, key in tables:
        command = table.get(key)
        if (
            isinstance(command, str)
            and command
            and any(separator in command for separator in ("/", "\\"))
            and not Path(command).is_absolute()
        ):
            table[key] = str((path.parent.parent / command).resolve(strict=False))
    return _freeze_json(value)


def _tool_path(value: object, *, root: Path, env: Mapping[str, str], role: str) -> Path:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"runtime {role} executable is not selected")
    command = value.strip()
    candidate = Path(command)
    if not candidate.is_absolute() and any(part in command for part in ("/", "\\")):
        candidate = root / candidate
        command = os.fspath(candidate)
    # Cargo executable selectors are paths, not shell commands. Compound C
    # wrapper commands require a separate attested executable dependency plan.
    return resolve_executable(command, environment=env, label=f"runtime {role}")


def _c_tool_environment_names(
    role: str, *, target: str, host_target: str
) -> tuple[str, ...]:
    prefix = role.upper()
    return (
        f"{prefix}_{target}",
        f"{prefix}_{target.replace('-', '_').replace('.', '_')}",
        f"{'HOST' if target == host_target else 'TARGET'}_{prefix}",
        prefix,
    )


def runtime_c_tool_selection(
    role: str,
    *,
    root: Path,
    env: Mapping[str, str],
    target: str,
    host_target: str,
    allow_default: bool = True,
) -> Path | None:
    """One cc-rs selector precedence for Cargo and its development proof."""
    names = _c_tool_environment_names(role, target=target, host_target=host_target)
    value = next((env[name] for name in names if env.get(name)), None)
    if value is None and allow_default and target == host_target:
        value = next(
            (
                name
                for name in _target_tool_defaults(target)[role]
                if find_executable(name, environment=env) is not None
            ),
            None,
        )
    return None if value is None else _tool_path(value, root=root, env=env, role=role)


def runtime_c_environment_name(name: str) -> bool:
    """Selection, flag, and search controls shared by proof environment custody."""
    if name in {*_C_FLAG_CONTROLS, *_C_SEARCH_ENVIRONMENTS}:
        return True
    prefixes = ("CC", "CXX", "AR", "RANLIB", *_C_FLAG_PREFIXES)
    return any(
        name == prefix
        or name.startswith(prefix + "_")
        or name in ("HOST_" + prefix, "TARGET_" + prefix)
        for prefix in prefixes
    )


def _select_host_c_tools(
    tools: dict[str, Path],
    environment: _CargoEnvironment,
    *,
    root: Path,
    target: str,
    host_target: str,
) -> None:
    """Pin explicit host C selections for cross-target build dependencies.

    Native builds already select the host lane as their target lane. A cross
    build cannot use its target compiler receipt to attest HOST_CC et al.
    Implicit host compiler SDK/resource discovery remains a separate admission
    boundary; do not pretend the target compiler's sysroot supplies it.
    """
    if target == host_target:
        return
    for role in ("cc", "cxx", "ar", "ranlib"):
        names = _c_tool_environment_names(
            role, target=host_target, host_target=host_target
        )
        path = runtime_c_tool_selection(
            role,
            root=root,
            env=environment,
            target=host_target,
            host_target=host_target,
            allow_default=False,
        )
        if path is None:
            continue
        tools["host_" + role] = path
        for name in names[:2]:
            environment[name] = os.fspath(path)


def _target_tool_defaults(target: str) -> Mapping[str, tuple[str, ...]]:
    if target.endswith("-pc-windows-msvc"):
        return {
            "cc": ("cl",),
            "cxx": ("cl",),
            "ar": ("lib",),
            "ranlib": ("lib",),
            "linker": ("link",),
        }
    if "windows-gnu" in target or "windows-gnullvm" in target:
        return {
            "cc": ("gcc", "clang"),
            "cxx": ("g++", "clang++"),
            "ar": ("ar", "llvm-ar"),
            "ranlib": ("ranlib", "llvm-ranlib"),
            "linker": ("gcc", "clang"),
        }
    if "-apple-" in target or target.endswith("-darwin"):
        return {
            "cc": ("cc", "clang"),
            "cxx": ("c++", "clang++"),
            "ar": ("ar", "llvm-ar"),
            "ranlib": ("ranlib", "llvm-ranlib"),
            "linker": ("cc", "clang"),
        }
    if "-linux-" in target:
        return {
            "cc": ("cc", "gcc", "clang"),
            "cxx": ("c++", "g++", "clang++"),
            "ar": ("ar", "llvm-ar"),
            "ranlib": ("ranlib", "llvm-ranlib"),
            "linker": ("cc", "gcc", "clang"),
        }
    # WebAssembly C tools are never host guesses: the manifest-pinned WASI SDK
    # owns them and `llvm_toolchain.apply_provisioned_wasm_toolchain` selects
    # them, like every other target without a host default.
    return {role: () for role in ("cc", "cxx", "ar", "ranlib", "linker")}


def _flags(value: object, *, label: str) -> tuple[str, ...]:
    if isinstance(value, str):
        # Cargo splits string flags at whitespace; it does not invoke a shell.
        return tuple(value.split())
    if isinstance(value, (tuple, list)) and all(
        isinstance(item, str) and item for item in value
    ):
        return cast(tuple[str, ...], tuple(value))
    raise ValueError(f"runtime {label} must be a flag string or string array")


def _cargo_string_list(
    configuration: Mapping[str, object],
    cli: Mapping[str, object],
    env: Mapping[str, str],
    keys: tuple[str, ...],
    environment_name: str,
) -> tuple[str, ...]:
    # Cargo StringList deserializes the merged file/CLI value, then appends
    # its config environment variable. This is not scalar selector precedence.
    value = cargo_configuration_value(_merge(configuration, cli), *keys)
    configured = _flags(value if value is not None else (), label=".".join(keys))
    return configured + (
        _flags(env[environment_name], label=environment_name)
        if environment_name in env
        else ()
    )


def _rust_target_metadata(
    rustc: Path,
    target: str | None,
    flags: tuple[str, ...],
    *,
    root: Path,
    env: Mapping[str, str],
    wrappers: Mapping[str, Path] = MappingProxyType({}),
) -> RustcTargetMetadata:
    root = Path(os.path.abspath(root))
    with (
        tempfile.TemporaryDirectory(prefix="molt-rust-cfg-") as scratch,
        _rust_probe_command(
            rustc, wrappers, label="runtime rustc target metadata"
        ) as command,
    ):
        scratch_root = Path(scratch)
        (scratch_root / "output").mkdir()
        safe_flags = rust_probe_arguments(flags, scratch_root)
        probe_environment = dict(env)
        probe_environment.pop("RUSTC_LOG", None)
        result = process_guard.run_completed_command(
            [*command, *cargo_target_query_arguments(target, safe_flags)],
            cwd=root,
            env=probe_environment,
            input="",
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=30,
            memory_guard_prefix=None,
            check=False,
        )
    if result.returncode != 0:
        raise ValueError(
            f"runtime rustc target metadata failed ({result.returncode}): {result.stderr.strip()}"
        )
    return parse_rustc_target_metadata(result.stdout, result.stderr)


def _cargo_build_directory_base(root: Path, environment: Mapping[str, str]) -> Path:
    raw = (
        environment.get("MOLT_CARGO_BUILD_DIR_BASE")
        or environment.get("CARGO_BUILD_BUILD_DIR")
        or environment.get("CARGO_TARGET_DIR")
        or "target"
    )
    path = Path(raw)
    return Path(os.path.abspath(path if path.is_absolute() else root / path))


def require_cargo_build_directory_version(version: str) -> None:
    """Admit the Cargo capability from its captured short or verbose banner."""
    banner = version.partition("\n")[0].rstrip("\r")
    match = re.fullmatch(r"cargo (\d+)\.(\d+)\.(\d+)(?:[-+ ][^\r\n]*)?", banner)
    if match is None or tuple(map(int, match.groups())) < (1, 91, 0):
        raise ValueError(
            "runtime Cargo requires Cargo 1.91 or newer for resource-keyed build directories"
        )


def _require_cargo_build_directory(
    cargo: CargoExecutableCustody, *, root: Path, env: Mapping[str, str]
) -> None:
    # Older Cargo ignores unknown configuration keys instead of proving that
    # the selected intermediate graph is used. Admit the capability up front.
    with stable_executable_probe(
        cargo.entrypoint, label="runtime Cargo version", identity=cargo.identity
    ):
        result = process_guard.run_completed_command(
            [os.fspath(cargo.entrypoint), "--version"],
            cwd=root,
            env=env,
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=15,
            memory_guard_prefix=None,
            check=False,
        )
    if result.returncode != 0:
        raise ValueError(
            "runtime Cargo requires Cargo 1.91 or newer; its version probe failed"
        )
    require_cargo_build_directory_version(result.stdout)


@dataclass(frozen=True, slots=True)
class RuntimeCargoPlan:
    project_root: Path
    command: tuple[str, ...]
    environment: Mapping[str, str]
    target: str
    host_target: str
    rustc: Path
    tools: Mapping[str, Path]
    wrappers: Mapping[str, Path]
    rustflags: tuple[str, ...]
    configuration: tuple[CargoConfigurationInput, ...]
    configuration_paths: tuple[Path, ...]
    cli_configuration: Mapping[str, object]
    profile_configuration: Mapping[str, object]
    executable_custody: tuple[CargoExecutableCustody, ...]
    rust_resources: CargoResourceCustody
    resource_paths: tuple[tuple[str, Path], ...]
    link_resources: CargoResourceCustody
    forced_environment: tuple[str, ...]
    c_environment: Mapping[str, tuple[str, ...]]
    wasi_sdk: WasiSdkInstallation | None
    build_directory_base: Path
    _build_directory: Path | None = field(
        default=None, init=False, repr=False, compare=False
    )

    @property
    def target_directory(self) -> Path | None:
        raw = self.environment.get("CARGO_TARGET_DIR")
        if raw is None:
            return None
        if not raw.strip():
            raise ValueError("runtime Cargo CARGO_TARGET_DIR must be nonempty")
        path = Path(raw)
        return Path(
            os.path.abspath(path if path.is_absolute() else self.project_root / path)
        )

    @property
    def build_directory(self) -> Path:
        """One host/target intermediate graph for these external input bytes.

        Source edits, Python generators, job controls and output paths do not
        select a new graph. Cargo owns their normal incremental invalidation.
        A resource generation also gives nested CMake builds a fresh OUT_DIR;
        target Rust flags alone cannot invalidate Cargo's compiled host units.
        """
        if self._build_directory is not None:
            return self._build_directory
        toolchain = self.toolchain_content_identity()
        digest = canonical_json_sha256(
            {
                "schema": "molt.cargo-external-inputs.v1",
                **toolchain,
                # Some native C producers deliberately suppress cc-rs Cargo
                # metadata to own archive linking. Their flags still select
                # outputs even when no resource file is added or changed.
                "c_environment": {
                    name: tokens
                    for name, tokens in self.c_environment.items()
                    if name != "CC_ENABLE_DEBUG_OUTPUT"
                },
                "link_resources": self.link_resources.content_identity(),
            }
        )
        directory = self.build_directory_base / ".molt-toolchains" / digest
        object.__setattr__(self, "_build_directory", directory)
        return directory

    def with_build_directory(self) -> RuntimeCargoPlan:
        """Pin intermediates before identity capture, leases or execution."""
        environment = dict(self.environment)
        environment["CARGO_BUILD_BUILD_DIR"] = os.fspath(self.build_directory)
        environment["MOLT_CARGO_BUILD_DIR_BASE"] = os.fspath(self.build_directory_base)
        if self.target_directory is not None:
            environment["CARGO_TARGET_DIR"] = os.fspath(self.target_directory)
        return replace(
            self,
            command=cargo_argv._pin_cargo_command(
                self.command,
                self.tools,
                self.wrappers,
                self.target,
                self.forced_environment,
                self.build_directory,
                self.target_directory,
            ),
            environment=MappingProxyType(environment),
        )

    def with_build_resources(self, resources: CargoResourceCustody) -> RuntimeCargoPlan:
        """Join an already admitted producer SDK to the same Cargo authority."""
        # Check the tail before replacing it. The joined result below fences
        # both captures once before the caller computes its final identity.
        self.configuration_arguments()
        separator = (
            self.command.index("--") if "--" in self.command else len(self.command)
        )
        pins = cargo_argv._pinned_tool_configurations(
            self.tools,
            self.wrappers,
            self.target,
            self.forced_environment,
            self.build_directory,
            self.target_directory,
        )
        # The verified tail consists of the canonical --config/value pairs.
        unbound = replace(
            self,
            command=(
                *self.command[: separator - 2 * len(pins)],
                *self.command[separator:],
            ),
            # Both captures already own each root's exact walk and file bytes.
            # Joining them must not enumerate SDKs again (including Windows,
            # where directory timestamps cannot fence cached listings).
            rust_resources=CargoResourceCustody(
                (*self.rust_resources.roots, *resources.roots),
                (*self.rust_resources.files, *resources.files),
                (*self.rust_resources.walks, *resources.walks),
            ),
        )
        result = unbound.with_build_directory()
        result.verify()
        return result

    @property
    def wasi_c_abi(self) -> WasiCAbiProjection | None:
        return None if self.wasi_sdk is None else wasi_c_abi_plan(self.wasi_sdk)

    def managed_tool_identities(self) -> dict[str, dict[str, object]]:
        if self.wasi_sdk is None:
            return {}
        result: dict[str, dict[str, object]] = {}
        for role, sdk_role in (
            ("cc", "clang"),
            ("cxx", "clang++"),
            ("ar", "llvm-ar"),
            ("ranlib", "llvm-ranlib"),
            ("linker", "wasm-ld"),
            ("final_linker", "wasm-ld"),
        ):
            fact = self.wasi_sdk.tool_fact(sdk_role)
            if self.tools.get(role) == self.wasi_sdk.sdk / fact["path"]:
                result[role] = {
                    "logical_name": role,
                    **ExecutableIdentity(
                        self.wasi_sdk.sdk / fact["path"],
                        self.wasi_sdk.sdk / fact["content_path"],
                        fact["size"],
                        fact["sha256"],
                        "",
                    ).content_record(),
                }
        return result

    @property
    def cargo_profile(self) -> str:
        """Present the profile selected by the admitted Cargo command.

        Rustc arguments after `--` never select a Cargo profile. This is a
        command projection only; it performs no configuration or identity work.
        """
        profile = "dev"
        arguments = iter(self.command[1:])
        for argument in arguments:
            if argument == "--":
                break
            if argument == "--release":
                profile = "release"
            elif argument == "--profile":
                profile = next(arguments)
            elif argument.startswith("--profile="):
                profile = argument.partition("=")[2]
        return profile

    @property
    def logical_paths(self) -> tuple[tuple[str, Path], ...]:
        return (
            *self.resource_paths,
            *(("tool/" + role, path) for role, path in self.tools.items()),
            *(("wrapper/" + role, path) for role, path in self.wrappers.items()),
        )

    def project_link_arguments(self, arguments: Sequence[str]) -> tuple[str, ...]:
        """Project response bytes from live custody, never independently reread."""
        resources = {
            item.entrypoint: item
            for item in (*self.rust_resources.files, *self.link_resources.files)
            if item.label.startswith("rust/link-response/")
        }
        result: list[str] = []
        for token in arguments:
            if "@" not in token:
                result.append(token)
                continue
            prefix, raw = token.rsplit("@", 1)
            path = Path(raw.strip('"'))
            if not path.is_absolute():
                path = self.project_root / path
            selected = resources.get(Path(os.path.abspath(path)))
            if selected is None:
                raise ValueError(
                    f"runtime linker response has no captured Cargo plan custody: {path}"
                )
            selected.verify()
            result.append(
                f"{prefix}@response:sha256={selected.identity.sha256}:size={selected.identity.size}"
            )
        return tuple(result)

    def partition_command(self) -> tuple[tuple[str, ...], tuple[str, ...]]:
        """Separate final-link rustc arguments from compile-affecting arguments."""
        before, after, linking = [], [], []
        configurations = iter(self.configuration_arguments())
        command = iter(self.command[1:])
        for token in command:
            if token == "--":
                break
            if token == "--config":
                next(command)
                before.extend((token, "config:" + next(configurations)))
            elif token.startswith("--config="):
                before.append("--config=config:" + next(configurations))
            else:
                before.append(token)
        rust_arguments = tuple(command)
        for span in rust_flag_spans(rust_arguments):
            value = span.codegen
            if value is None:
                after.extend(rust_arguments[span.start : span.stop])
            else:
                after.extend(span.leading)
                destination = (
                    linking if value.startswith(("link-arg=", "link-args=")) else after
                )
                destination.extend(("-C", value))
        return ("cargo", *before, *(("--", *after) if after else ())), tuple(linking)

    def configuration_arguments(self) -> tuple[str, ...]:
        result = []
        overrides = cargo_argv._command_options(self.command)[1]
        pinned = cargo_argv._pinned_tool_configurations(
            self.tools,
            self.wrappers,
            self.target,
            self.forced_environment,
            self.build_directory,
            self.target_directory,
        )
        if overrides[-len(pinned) :] != tuple(raw for raw, _logical in pinned):
            raise ValueError("runtime Cargo pinned executable configuration changed")
        for index, raw in enumerate(overrides[: -len(pinned)]):
            captured = next(
                (
                    item
                    for item in self.configuration
                    if item.label.startswith(f"cargo-cli-config/{index}/")
                ),
                None,
            )
            result.append(
                captured.identity.sha256
                if captured is not None
                else canonical_json_sha256(tomllib.loads(raw))
            )
        result.extend(
            canonical_json_sha256(tomllib.loads(logical)) for _raw, logical in pinned
        )
        return tuple(result)

    def verify(self) -> None:
        if self.environment.get("CARGO_BUILD_BUILD_DIR") != os.fspath(
            self.build_directory
        ) or self.environment.get("MOLT_CARGO_BUILD_DIR_BASE") != os.fspath(
            self.build_directory_base
        ):
            raise ValueError("runtime Cargo intermediate generation changed")
        if (
            self.command[0] != os.fspath(self.tools["cargo"])
            or self.rustc != self.tools["rustc"]
            or self.environment.get("RUSTC") != os.fspath(self.rustc)
        ):
            raise ValueError("runtime Cargo execution differs from selected tools")
        if self.environment.get("CARGO_ENCODED_RUSTFLAGS") != "\x1f".join(
            self.rustflags
        ):
            raise ValueError("runtime Cargo execution differs from selected flags")
        for name in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
            if self.environment.get(name) != (
                os.fspath(self.wrappers[name]) if name in self.wrappers else ""
            ):
                raise ValueError(
                    "runtime Cargo execution differs from selected wrappers"
                )
        self.configuration_arguments()
        flag_plan = _resolve_rust_flag_resources(
            self.rustflags,
            self.command,
            root=self.project_root,
            env=self.environment,
            wasi_plan=self.wasi_c_abi,
        )
        if flag_plan.flags != self.rustflags or flag_plan.command != self.command:
            raise ValueError(
                "runtime Rust flags differ from their admitted target context"
            )
        if (
            flag_plan.dependency_linker is not None
            and flag_plan.dependency_linker != self.tools.get("linker")
        ) or flag_plan.final_linker != self.tools.get("final_linker"):
            raise ValueError(
                "runtime Rust linker selection differs from captured tools"
            )
        if (
            cargo_configuration_paths(self.project_root, self.environment)
            != self.configuration_paths
        ):
            raise ValueError(
                "runtime Cargo configuration selection changed during build"
            )
        for item in self.configuration:
            verify_stable_regular_file_identity(item.identity, label=item.label)
        expected_tools = {
            "tool/" + role: path
            for role, path in self.tools.items()
            if role not in self.managed_tool_identities()
        }
        expected_tools.update(
            {"wrapper/" + role: path for role, path in self.wrappers.items()}
        )
        if expected_tools != {
            item.label: item.entrypoint for item in self.executable_custody
        }:
            raise ValueError("runtime Cargo executable custody is incomplete")
        for item in self.executable_custody:
            item.verify()
        for role, path in self.tools.items():
            c_role = role.removeprefix("host_")
            if c_role not in {"cc", "cxx", "ar", "ranlib"}:
                continue
            target = self.host_target if role.startswith("host_") else self.target
            selector = _c_tool_environment_names(
                c_role, target=target, host_target=self.host_target
            )[0]
            if self.environment.get(selector) != os.fspath(path):
                raise ValueError(
                    f"runtime Cargo {role} execution differs from selected tool"
                )
        c_resources = _resolve_c_build_resources(
            _CargoEnvironment(self.environment),
            target=self.target,
            host_target=self.host_target,
        )
        if dict(c_resources.environment) != dict(
            self.c_environment
        ) or c_resources.roots != tuple(
            root for root in self.rust_resources.roots if root.label.startswith("c/")
        ):
            raise ValueError(
                "runtime C input custody differs from captured environment"
            )
        if self.wasi_c_abi is not None:
            if wasi_c_abi_projection(self.environment) != self.wasi_c_abi:
                raise ValueError(
                    "runtime WASI projection differs from captured selection"
                )
        self.rust_resources.verify()
        self.link_resources.verify()

    def rust_resource_identity(self) -> dict[str, object]:
        return {
            "host_triple": self.host_target,
            "selected_target": self.target,
            "content": self.rust_resources.content_identity(),
        }

    def toolchain_content_identity(self) -> dict[str, object]:
        """Project Cargo's admitted tools/resources, without consumer extras.

        A runtime build may additionally execute Python generators. The compiler
        build does not inherit those runtime-only inputs or interpreter probes.
        """
        tools: dict[str, object] = dict(self.managed_tool_identities())
        wrappers: dict[str, object] = {}
        for item in self.executable_custody:
            group, role = item.label.split("/", 1)
            destination = tools if group == "tool" else wrappers
            destination[role] = {
                "logical_name": role if group == "tool" else role.casefold(),
                **item.content_record(),
            }
        return {
            "tools": tools,
            "wrappers": wrappers,
            "effective_target": self.target,
            "rust_resources": self.rust_resource_identity(),
            "sysroots": {}
            if self.wasi_c_abi is None
            else {"wasi": self.wasi_c_abi.content_identity()},
            "archives": [],
        }

    def toolchain_identity(self) -> dict[str, object]:
        return {
            **self.toolchain_content_identity(),
            "cargo_configuration": self.configuration_identity(),
        }

    def configuration_identity(self) -> dict[str, object]:
        entries = [
            {
                "label": item.label,
                "size": item.identity.size,
                "sha256": item.identity.sha256,
            }
            for item in self.configuration
        ]
        # Inline --config inputs are already exact command inputs. Include them
        # here too because they select the toolchain before command projection.
        digest = canonical_json_sha256(
            {"files": entries, "cli": self.configuration_arguments()}
        )
        return {
            "digest": digest,
            "file_count": len(entries),
            "total_size": sum(item.identity.size for item in self.configuration),
            "roots": sorted(item.label for item in self.configuration),
            "missing": [],
        }

    def profile_ancestry(self, cargo_profile: str) -> tuple[str, ...]:
        """One child-first profile chain for output identity and debug policy."""
        seen: set[str] = set()
        ancestry: list[str] = []
        profile = cargo_profile
        while True:
            if profile in seen:
                raise ValueError("runtime Cargo profile inheritance cycle")
            seen.add(profile)
            ancestry.append(profile)
            parent = cargo_selected_value(
                self.profile_configuration,
                self.cli_configuration,
                {},
                ("profile", profile, "inherits"),
                (),
            )
            if parent is None:
                if profile in {"dev", "release"}:
                    return tuple(ancestry)
                if profile in {"test", "bench"}:
                    parent = "dev" if profile == "test" else "release"
                else:
                    raise ValueError(
                        f"runtime Cargo custom profile has no proven inheritance: {profile}"
                    )
            if not isinstance(parent, str) or not parent:
                raise ValueError("runtime Cargo profile inheritance is invalid")
            profile = parent

    def profile_environment(self, cargo_profile: str) -> dict[str, str]:
        """Attribute controls before filtering the complete profile namespace."""
        controls = {
            "OPT_LEVEL",
            "DEBUG",
            "SPLIT_DEBUGINFO",
            "STRIP",
            "DEBUG_ASSERTIONS",
            "OVERFLOW_CHECKS",
            "LTO",
            "PANIC",
            "INCREMENTAL",
            "CODEGEN_UNITS",
            "RPATH",
        }
        # Cargo disallows link-wide settings in build-script/proc-macro
        # overrides, just as it does in per-package overrides.
        controls |= {
            "BUILD_OVERRIDE_" + control
            for control in controls - {"PANIC", "LTO", "RPATH"}
        }
        ancestry = self.profile_ancestry(cargo_profile)
        selected_prefixes = {_profile_environment_prefix(name) for name in ancestry}
        known_profiles = {"dev", "release", "test", "bench", *ancestry}
        for configuration in (self.profile_configuration, self.cli_configuration):
            known_profiles.update(
                _table(configuration.get("profile", {}), label="profile")
            )
        prefixes = sorted(
            {_profile_environment_prefix(name) for name in known_profiles},
            key=lambda prefix: (-len(prefix), prefix),
        )
        known_controls = {
            prefix + control for prefix in prefixes for control in controls
        }
        selected_controls = {
            prefix + control for prefix in selected_prefixes for control in controls
        }
        result: dict[str, str] = {}
        for name, value in self.environment.items():
            # Cargo constructs a key for each profile field. A legal ancestor
            # control can also match a longer profile name (release-build-override
            # or release-debug); longest-prefix selection must not hide it.
            if name in known_controls:
                if name in selected_controls:
                    result[name] = value
                continue
            prefix = next(
                (prefix for prefix in prefixes if name.startswith(prefix)), None
            )
            # Discover siblings from every captured configuration source before
            # rejecting unknown controls, so RELEASE_SIZE_LTO is not diagnosed
            # as an unknown RELEASE control when release-size is not selected.
            if prefix in selected_prefixes:
                raise ValueError(
                    f"unsupported output-bearing Cargo profile override: {name}"
                )
        return result

    def preserve_debug_for_profile(self, cargo_profile: str) -> bool:
        """Resolve Cargo profile inheritance and overrides, never name substrings."""
        ancestry = self.profile_ancestry(cargo_profile)
        for profile in ancestry:
            value = cargo_selected_value(
                self.profile_configuration,
                self.cli_configuration,
                self.environment,
                ("profile", profile, "debug"),
                (_profile_environment_prefix(profile) + "DEBUG",),
            )
            if value is not None:
                if value in (False, 0, "0", "false", "none"):
                    return False
                if value in (
                    True,
                    1,
                    2,
                    "1",
                    "2",
                    "true",
                    "limited",
                    "full",
                    "line-tables-only",
                    "line-directives-only",
                ):
                    return True
                raise ValueError(
                    f"runtime Cargo profile debug setting is invalid: {value!r}"
                )
        return ancestry[-1] == "dev"


def cargo_requested_target(
    configuration: Mapping[str, object],
    cli: Mapping[str, object],
    environment: Mapping[str, str],
    command_target: str | None,
) -> str | None:
    """One Cargo target precedence authority for runtime and proof capture."""
    configured = cargo_selected_value(
        configuration, cli, environment, ("build", "target"), ("CARGO_BUILD_TARGET",)
    )
    selected = command_target or configured
    if selected is not None and (
        not isinstance(selected, str) or not selected or selected.endswith(".json")
    ):
        raise ValueError("Cargo requires one content-known target triple")
    return selected


def cargo_flag_inputs(
    configuration: Mapping[str, object],
    cli: Mapping[str, object],
    environment: Mapping[str, str],
    target: str,
    *,
    flag_kind: str = "rustflags",
) -> tuple:
    """Select Cargo flag sources once; cfg matching remains rustc-derived."""
    if flag_kind not in {"rustflags", "rustdocflags"}:
        raise ValueError("unknown Cargo compiler flag family")
    cargo_target = target.upper().replace("-", "_")
    configured_targets, cli_targets = (
        configuration.get("target", {}),
        cli.get("target", {}),
    )
    if not isinstance(configured_targets, Mapping) or not isinstance(
        cli_targets, Mapping
    ):
        raise ValueError("runtime Cargo target configuration must be a table")
    target_tables = _merge(
        _table(configured_targets, label="target"),
        _table(cli_targets, label="CLI target"),
    )
    cfg_tables = {
        key: _table(value, label=f"target.{key}")
        for key, value in target_tables.items()
        if key.startswith("cfg(")
    }
    target_flags = _cargo_string_list(
        configuration,
        cli,
        environment,
        ("target", target, flag_kind),
        f"CARGO_TARGET_{cargo_target}_{flag_kind.upper()}",
    )
    build_flags = _cargo_string_list(
        configuration,
        cli,
        environment,
        ("build", flag_kind),
        "CARGO_BUILD_" + flag_kind.upper(),
    )
    environment_flags = None
    if "CARGO_ENCODED_" + flag_kind.upper() in environment:
        encoded = environment["CARGO_ENCODED_" + flag_kind.upper()]
        environment_flags = tuple(encoded.split("\x1f")) if encoded else ()
    elif flag_kind.upper() in environment:
        environment_flags = _flags(
            environment[flag_kind.upper()], label=flag_kind.upper()
        )
    return cfg_tables, target_flags, build_flags, environment_flags


def capture_cargo_configuration_inputs(
    project_root: Path,
    *,
    env: Mapping[str, str],
    cargo_command: Sequence[str],
    captured_sources: Mapping[str, CargoConfigurationSource] | None = None,
    captured_paths: Sequence[Path] | None = None,
) -> tuple:
    """Capture Cargo configuration before any compiler or native tool probe.

    Shared by runtime planning and proof capability selection. The returned
    documents retain the existing merge and configured-environment authority.
    """
    root = (
        project_root.resolve(strict=True) if captured_sources is None else project_root
    )
    if not cargo_command or not all(
        isinstance(item, str) and item for item in cargo_command
    ):
        raise ValueError("runtime Cargo command is empty or malformed")
    environment = _CargoEnvironment(env)
    paths = (
        cargo_configuration_paths(root, environment)
        if captured_paths is None
        else tuple(captured_paths)
    )

    def read(path: Path, *, label: str):
        if captured_sources is None:
            return _capture_config(path, label=label)
        value = captured_sources.get(label)
        if value is None or value.label != label:
            raise ValueError("Cargo configuration source custody is incomplete")
        return value

    def exists(path: Path) -> bool:
        return (
            path.is_file()
            if captured_sources is None
            else any(source.path == path for source in captured_sources.values())
        )

    inputs = [
        read(path, label=f"cargo-config/{index}/{path.name}")
        for index, path in enumerate(paths)
    ]
    configuration: dict[str, object] = {}
    environment_origins: dict[str, Path] = {}
    for item in inputs:
        configuration = _merge(configuration, item.document)
        _environment_origins(
            item.document, item.path.parent.parent, environment_origins
        )
    profiles: dict[str, object] = {}
    manifest = root / "Cargo.toml"
    if exists(manifest):
        profile_input = read(manifest, label="cargo-profile-manifest/Cargo.toml")
        inputs.append(profile_input)
        profiles = dict(profile_input.document)
    profiles = _merge(profiles, configuration)
    command_target, overrides = cargo_argv._command_options(cargo_command)
    cli: dict[str, object] = {}
    for index, override in enumerate(overrides):
        candidate = Path(override)
        if not candidate.is_absolute():
            candidate = root / candidate
        if "=" not in override or exists(candidate):
            captured = read(
                candidate, label=f"cargo-cli-config/{index}/{candidate.name}"
            )
            inputs.append(captured)
            document = captured.document
            environment_base = captured.path.parent.parent
        else:
            try:
                document = tomllib.loads(override)
            except tomllib.TOMLDecodeError as exc:
                raise ValueError(
                    "runtime Cargo inline configuration is invalid"
                ) from exc
            environment_base = root
        if "include" in document:
            raise ValueError(
                "runtime Cargo CLI configuration includes need resolved custody"
            )
        cli = _merge(cli, document)
        _environment_origins(document, environment_base, environment_origins)
    environment, forced_environment, environment_resources = _apply_cargo_environment(
        configuration, cli, environment, environment_origins
    )
    validate_cargo_native_dependency_environment(
        environment,
        target=cargo_requested_target(configuration, cli, environment, command_target),
    )
    return (
        root,
        environment,
        paths,
        inputs,
        configuration,
        profiles,
        command_target,
        cli,
        forced_environment,
        environment_resources,
    )


def resolve_runtime_cargo_plan(
    project_root: Path,
    *,
    env: Mapping[str, str],
    cargo_command: Sequence[str],
    requested_target: str | None,
    host_target: str | None = None,
    environment_transform: Callable[[Mapping[str, str]], Mapping[str, str]]
    | None = None,
    rustflags_transform: Callable[[tuple[str, ...]], tuple[str, ...]] | None = None,
) -> RuntimeCargoPlan:
    if any(item.startswith("+") for item in cargo_command[1:2]):
        raise ValueError(
            "runtime Cargo toolchain selector must be resolved through RUSTUP_TOOLCHAIN"
        )
    (
        root,
        environment,
        paths,
        inputs,
        configuration,
        profiles,
        command_target,
        cli,
        forced_environment,
        environment_resources,
    ) = capture_cargo_configuration_inputs(
        project_root, env=env, cargo_command=cargo_command
    )
    if environment_transform is not None:
        environment = _CargoEnvironment(
            environment_transform(MappingProxyType(environment))
        )
    rustc_selector = cargo_selected_value(
        configuration,
        cli,
        environment,
        ("build", "rustc"),
        ("RUSTC", "CARGO_BUILD_RUSTC"),
        "rustc",
    )
    rustc = _tool_path(rustc_selector, root=root, env=environment, role="rustc")
    rustc = resolve_rustup_proxy(rustc, role="rustc", root=root, env=environment)
    rustc_custody = CargoExecutableCustody.capture("tool/rustc", rustc)
    environment["RUSTC"] = os.fspath(rustc)
    environment.update(rust_toolchain_library_environment(rustc, environment))
    wrappers: dict[str, Path] = {}
    for name, key in (
        ("RUSTC_WRAPPER", "rustc-wrapper"),
        ("RUSTC_WORKSPACE_WRAPPER", "rustc-workspace-wrapper"),
    ):
        value = cargo_selected_value(
            configuration,
            cli,
            environment,
            ("build", key),
            (name, "CARGO_BUILD_" + name),
            "",
        )
        if not isinstance(value, str):
            raise ValueError(f"runtime {name} must be an executable path or empty")
        environment[name] = ""  # Explicit empty disables inherited config.
        if value:
            wrappers[name] = _tool_path(value, root=root, env=environment, role=name)
            environment[name] = os.fspath(wrappers[name])
    wrapper_custody = tuple(
        CargoExecutableCustody.capture("wrapper/" + role, path)
        for role, path in wrappers.items()
    )
    if host_target is None:
        with _rust_probe_command(
            rustc, wrappers, label="runtime rustc host"
        ) as command:
            result = process_guard.run_completed_command(
                [*command, "-vV"],
                cwd=root,
                env=environment,
                capture_output=True,
                text=True,
                encoding="utf-8",
                timeout=30,
                memory_guard_prefix=None,
                check=False,
            )
        match = re.search(r"(?m)^host:\s*(\S+)\s*$", result.stdout)
        if result.returncode != 0 or match is None:
            raise ValueError("runtime rustc host selection could not be proven")
        host_target = match.group(1)
    configured_target = cargo_requested_target(configuration, cli, environment, None)
    target = (
        cargo_requested_target(configuration, cli, environment, command_target)
        or host_target
    )
    validate_cargo_native_dependency_environment(
        environment, target=target, compiler_host=host_target
    )
    if requested_target is None and target != host_target:
        raise ValueError(
            "runtime implicit cross target requires explicit target selection"
        )
    if requested_target is None and (
        command_target is not None or configured_target is not None
    ):
        raise ValueError(
            "runtime Cargo target selection requires an explicit artifact target"
        )
    if requested_target is not None and requested_target != target:
        raise ValueError("runtime Cargo target differs from requested target")
    wasi_plan = None
    wasi_sdk = None
    if target == "wasm32-wasip1" or (
        target == "wasm32-unknown-unknown" and "MOLT_WASI_C_ABI_PLAN" in environment
    ):
        try:
            wasi_sdk = selected_wasi_sdk_installation(
                compiler_source_root(), environ=environment
            )
            if wasi_sdk is None:
                raise ValueError(
                    "the pinned WASI SDK is missing; run tools/provision_wasi_sdk.py"
                )
            apply_provisioned_wasm_toolchain(
                compiler_source_root(),
                environment,
                installation=wasi_sdk,
                rust_target=target,
            )
            wasi_plan = wasi_c_abi_plan(wasi_sdk)
        except LlvmToolchainConfigError as exc:
            raise ValueError(str(exc)) from exc
    cargo_target = target.upper().replace("-", "_")
    if wasi_plan is not None:
        environment.setdefault(
            f"CARGO_TARGET_{cargo_target}_LINKER", str(wasi_plan.linker)
        )
    cfg_tables, target_flags, build_flags, environment_flags = cargo_flag_inputs(
        configuration, cli, environment, target
    )
    target_argument = (
        target if command_target is not None or configured_target is not None else None
    )

    # rustc's answers describe the compiler, not the Cargo settings this plan
    # adds below (linkers, C tools, encoded flags); every query sees the
    # environment as it stands here, so equal queries share one answer.
    probe_environment = MappingProxyType(dict(environment))

    metadata_answers: dict[
        tuple[str | None, tuple[str, ...], bool], RustcTargetMetadata
    ] = {}
    metadata_flag_roots: dict[tuple[str, ...], tuple[CargoResourceRoot, ...]] = {}

    def metadata_for(
        query_target: str | None, flags: tuple[str, ...], wrapped: bool
    ) -> RustcTargetMetadata:
        # Role names do not distinguish identical commands without wrappers.
        wrapped = wrapped and bool(wrappers)
        key = (query_target, flags, wrapped)
        if key not in metadata_answers:
            if flags not in metadata_flag_roots:
                # Cfg convergence can read provisional resources absent from
                # the final lane. Reuse its parser; retain all those inputs.
                separator = (
                    cargo_command.index("--")
                    if "--" in cargo_command
                    else len(cargo_command)
                )
                metadata_flag_roots[flags] = _resolve_rust_flag_resources(
                    flags,
                    cargo_command[:separator],
                    root=root,
                    env=probe_environment,
                    wasi_plan=wasi_plan,
                ).roots
            metadata_answers[key] = _rust_target_metadata(
                rustc,
                query_target,
                flags,
                root=root,
                env=probe_environment,
                wrappers=wrappers if wrapped else {},
            )
        return metadata_answers[key]

    flag_plan: RustFlagResourcePlan | None = None

    def finalize_flags(flags: tuple[str, ...]) -> tuple[str, ...]:
        nonlocal flag_plan
        transformed = (
            rustflags_transform(flags) if rustflags_transform is not None else flags
        )
        if not isinstance(transformed, tuple) or any(
            not isinstance(item, str) or not item or "\x1f" in item
            for item in transformed
        ):
            raise ValueError("runtime resolved Rust flag token is invalid")
        flag_plan = _resolve_rust_flag_resources(
            transformed,
            cargo_command,
            root=root,
            env=environment,
            wasi_plan=wasi_plan,
        )
        # These metadata vectors already have complete resource custody from
        # their original dependency/final-crate lanes. Their combined vector
        # may legitimately repeat an SDK mode once per lane; re-admitting it
        # as one dependency lane would reject our own validated projection.
        # Provisional cfg vectors still use metadata_for's resource parser.
        for lane_flags in flag_plan.flag_lanes:
            metadata_flag_roots[lane_flags] = flag_plan.roots
        return flag_plan.flags

    rustflags, matched_cfg = select_cargo_target_flags(
        cfg_tables,
        target_flags=target_flags,
        build_flags=build_flags,
        environment_flags=environment_flags,
        flags=lambda value: _flags(value, label="cfg rustflags"),
        probe=lambda flags: metadata_for(target_argument, flags, True).cfg,
        transform=finalize_flags,
    )
    assert flag_plan is not None
    cargo = _tool_path(cargo_command[0], root=root, env=environment, role="cargo")
    cargo = resolve_rustup_proxy(cargo, role="cargo", root=root, env=environment)
    cargo_custody = CargoExecutableCustody.capture("tool/cargo", cargo)
    _require_cargo_build_directory(cargo_custody, root=root, env=environment)
    tools: dict[str, Path] = {"rustc": rustc, "cargo": cargo}
    defaults = _target_tool_defaults(target)
    for role in ("cc", "cxx", "ar", "ranlib", "linker"):
        if role == "linker":
            value = cargo_selected_value(
                configuration,
                cli,
                environment,
                ("target", target, "linker"),
                (f"CARGO_TARGET_{cargo_target}_LINKER",),
            )
            if value is None:
                cfg_linkers = [
                    (key, table["linker"])
                    for key, table in matched_cfg
                    if "linker" in table
                ]
                if len(cfg_linkers) > 1:
                    raise ValueError(
                        "runtime Cargo target matches multiple cfg linkers: "
                        + ", ".join(key for key, _ in cfg_linkers)
                    )
                if cfg_linkers:
                    value = cfg_linkers[0][1]
        else:
            names = _c_tool_environment_names(
                role, target=target, host_target=host_target
            )
            selected_c_tool = runtime_c_tool_selection(
                role, root=root, env=environment, target=target, host_target=host_target
            )
            value = None if selected_c_tool is None else os.fspath(selected_c_tool)
        if role == "linker" and value is None and target == host_target:
            value = next(
                (
                    name
                    for name in defaults[role]
                    if find_executable(name, environment=environment) is not None
                ),
                None,
            )
        if value is None:
            if target == "wasm32-unknown-unknown" and wasi_plan is None:
                continue  # Pure freestanding Rust has no C-provider obligation.
            if role == "linker" and target.startswith("wasm32-"):
                continue  # WASM final linker has a separate explicit invocation.
            selectors = (
                (f"CARGO_TARGET_{cargo_target}_LINKER",) if role == "linker" else names
            )
            if target.startswith("wasm32-"):
                raise ValueError(
                    f"runtime target {target} takes its {role} from the pinned WASI "
                    f"SDK, which is not provisioned: run `{sys.executable} "
                    f"tools/provision_wasi_sdk.py`, or set {selectors[0]}"
                )
            if target != host_target:
                raise ValueError(
                    f"runtime cross target {target} needs an explicit {role}: set "
                    + " or ".join(selectors)
                )
            continue  # Native Rust-only builds need no absent C tool.
        selected_tool = (
            _tool_path(value, root=root, env=environment, role=role)
            if role == "linker"
            else selected_c_tool
        )
        assert selected_tool is not None
        tools[role] = selected_tool
        selected = os.fspath(tools[role])
        if role == "linker":
            environment[f"CARGO_TARGET_{cargo_target}_LINKER"] = selected
        else:
            environment[f"{role.upper()}_{target}"] = selected
            environment[f"{role.upper()}_{target.replace('-', '_')}"] = selected
    _select_host_c_tools(
        tools, environment, root=root, target=target, host_target=host_target
    )
    validate_cargo_native_dependency_environment(
        environment, target=target, compiler_host=host_target, native_bound=True
    )
    if flag_plan.dependency_linker is not None:
        tools["linker"] = flag_plan.dependency_linker
        environment[f"CARGO_TARGET_{cargo_target}_LINKER"] = os.fspath(
            flag_plan.dependency_linker
        )
    if flag_plan.final_linker is not None:
        tools["final_linker"] = flag_plan.final_linker
    if wasi_plan is not None:
        selected_linker = wasi_plan.linker
        if tools.get("linker") != selected_linker or (
            "final_linker" in tools and tools["final_linker"] != selected_linker
        ):
            raise ValueError(
                "WASI runtime linker differs from the selected SDK raw linker"
            )
    environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(rustflags)
    environment["RUSTC"] = os.fspath(rustc)
    # Final explicit selectors win over earlier --config operands as well as
    # environment/config defaults. In particular, CLI wrappers cannot silently
    # undo an explicit disabled wrapper, or reintroduce a mutable rustup proxy.
    command = flag_plan.command
    executable_custody = (
        rustc_custody,
        cargo_custody,
        *(
            CargoExecutableCustody.capture("tool/" + role, path)
            for role, path in tools.items()
            if role not in {"rustc", "cargo"}
            and not (
                wasi_sdk is not None
                and role in {"cc", "cxx", "ar", "ranlib", "linker", "final_linker"}
                and any(
                    path == wasi_sdk.sdk / fact["path"]
                    for fact in wasi_sdk.facts["tools"].values()
                    if fact is not None
                )
            )
        ),
        *wrapper_custody,
    )
    rust_roots = _rust_resource_roots(
        rustc,
        root=root,
        env=environment,
        target=target,
        host=host_target,
        flag_lanes=tuple(
            dict.fromkeys(
                (
                    *flag_plan.flag_lanes,
                    *(
                        flags
                        for query_target, flags, wrapped in metadata_answers
                        if query_target == target_argument
                    ),
                )
            )
        ),
        wrappers=wrappers,
        target_argument=target_argument,
        metadata_probe=metadata_for,
    )
    # rustc searches TARGET.json directly, never whole directory subtrees.
    # Capture candidates without reimplementing builtin/search precedence.
    # An implicit host question has no custom-target search input.
    metadata_target_roots: list[CargoResourceRoot] = []
    if search := probe_environment.get("RUST_TARGET_PATH"):
        target_names: dict[str, None] = {}
        for query_target, flags, _wrapped in metadata_answers:
            if query_target is not None:
                target_names[query_target] = None
            else:
                for span in rust_flag_spans(flags):
                    if span.option == "--target" and span.value is not None:
                        target_names[span.value] = None
        for index, entry in enumerate(search.split(os.pathsep)):
            path = Path(entry)
            directory = path if path.is_absolute() else root / path
            for target_index, name in enumerate(target_names):
                metadata_target_roots.append(
                    CargoResourceRoot(
                        f"rust/metadata/target-search/{index}/{target_index}",
                        directory / f"{name}.json",
                        required=False,
                    )
                )
    metadata_roots = tuple(
        dict.fromkeys(
            (
                *rust_roots,
                *flag_plan.roots,
                *environment_resources,
                *(
                    replace(
                        resource, label=f"rust/metadata/flags/{index}/{resource.label}"
                    )
                    for index, (flags, roots) in enumerate(metadata_flag_roots.items())
                    if flags not in flag_plan.flag_lanes
                    for resource in roots
                ),
                *metadata_target_roots,
            )
        )
    )
    metadata_resources = CargoResourceCustody.capture(metadata_roots)
    # Discovery reveals resource locations. Replay each distinct question once
    # after capturing the complete closure, before accepting its cfg selection.
    # Wrappers are covered by declared executable/resource inputs, not a claim
    # that arbitrary wrapper filesystem or network reads are hermetic.
    for (query_target, flags, wrapped), discovered in metadata_answers.items():
        metadata_resources.verify()
        rustc_custody.verify()
        for item in wrapper_custody:
            item.verify()
        try:
            qualified = _rust_target_metadata(
                rustc,
                query_target,
                flags,
                root=root,
                env=probe_environment,
                wrappers=wrappers if wrapped else {},
            )
        finally:
            metadata_resources.verify()
            rustc_custody.verify()
            for item in wrapper_custody:
                item.verify()
        if qualified != discovered:
            raise ValueError(
                "runtime Rust metadata changed between discovery and resource qualification"
            )
    c_resources = _resolve_c_build_resources(
        environment, target=target, host_target=host_target
    )
    resources = CargoResourceCustody.capture(
        (*metadata_resources.roots, *c_resources.roots),
    )
    resource_paths = (
        *flag_plan.logical_paths,
        *((resource.label, resource.path) for resource in environment_resources),
        *c_resources.logical_paths,
    )
    plan = RuntimeCargoPlan(
        root,
        command,
        MappingProxyType(environment),
        target,
        host_target,
        rustc,
        MappingProxyType(tools),
        MappingProxyType(wrappers),
        rustflags,
        tuple(inputs),
        paths,
        _freeze_json(cli),
        _freeze_json(profiles),
        executable_custody,
        resources,
        resource_paths,
        CargoResourceCustody.capture(flag_plan.link_roots),
        forced_environment,
        c_resources.environment,
        wasi_sdk,
        _cargo_build_directory_base(root, environment),
    ).with_build_directory()
    plan.verify()
    return plan
