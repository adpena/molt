"""The C toolchain resource language of runtime Cargo builds.

It names the C flag and search-path environments a runtime build reads, which
compiler and linker arguments name filesystem resources, and the admitted
resource plan `_resolve_c_build_resources` returns for one target.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from types import MappingProxyType
from typing import Mapping, Sequence

from molt.cli.cargo_resource_custody import (
    CargoResourceIdentity,
    CargoResourceRoot,
    is_apple_sdk,
)
from molt.flag_strings import FlagStringError, FlagStringRule
from molt.process_environment import (
    NameRule,
    ProcessEnvironment,
    ProcessEnvironmentBuilder,
)


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


@dataclass(frozen=True, slots=True)
class RuntimeCResourcePlan:
    roots: tuple[CargoResourceRoot, ...]
    logical_paths: tuple[tuple[str, Path], ...]
    environment: Mapping[str, tuple[str, ...]]


def _resolve_c_build_resources(
    env: ProcessEnvironment | ProcessEnvironmentBuilder,
    *,
    target: str,
    host_target: str | None = None,
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

    c_flag_rule = FlagStringRule.cc(env)
    # The projection names each variable as cc-rs spells it, as the search
    # entries below do, so readers and identities agree on every host. One
    # variable that two spellings select on Windows is projected once.
    selected: set[str] = set()
    for name in runtime_c_flag_environment_names(target, host_target):
        key = env.rule.key(name)
        if key not in env or key in selected:
            continue
        selected.add(key)
        value = env[key]
        if name in _C_FLAG_CONTROLS:
            projection[name] = (value,)
            continue
        try:
            tokens = c_flag_rule.decode(value)
        except FlagStringError as exc:
            raise ValueError(
                f"runtime C {name} has invalid shell-escaped flags: {exc}"
            ) from exc
        projection[name] = flags(tokens, name=name)
    for name in _C_SEARCH_ENVIRONMENTS:
        if name not in env:
            continue
        value = env[name]
        if name == "SDKROOT" and not value:
            projection[name] = ()
            continue
        separator = ";" if env.rule is NameRule.WINDOWS else ":"
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
