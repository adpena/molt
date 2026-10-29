"""Resolve one executable Cargo plan for both runtime capture and execution.

The plan is live host state, never embedded in a portable artifact receipt.
Configuration bytes are parsed and fingerprinted from the same stable snapshot.
"""

from __future__ import annotations

from molt.cargo_execution_policy import (
    cargo_command_options,
    cargo_c_tool_environment_names,
    validate_sccache_epoch_environment,
)

import os
import re
import sys
import tomllib
from dataclasses import dataclass, field, replace
from pathlib import Path
from types import MappingProxyType
from typing import Callable, Literal, Mapping, Sequence, cast

from molt.flag_strings import FlagStringRule
from molt.process_environment import (
    ProcessEnvironment,
    ProcessEnvironmentBuilder,
)
from molt.cli import cargo_command as cargo_argv
from molt.cli import cargo_capabilities
from molt.cli import runtime_tool_selection
from molt.cli import rustc_probes
from molt.cli.cargo_search_path import cargo_search_path, derives_cargo_search_path
from molt.cargo_execution_policy import validate_cargo_native_dependency_environment
from molt.cli.cargo_target_cfg import (
    RustcTargetMetadata,
    select_cargo_target_flags,
)
from molt.exact_json import canonical_json_sha256, string_keyed_mapping
from molt.rust_toolchain import (
    canonical_rust_codegen_option,
    rust_flag_spans,
    rust_remap_path_flags,
    cargo_configuration_paths,
    cargo_configuration_value,
    cargo_selected_value,
    resolve_rustup_proxy,
    rust_toolchain_library_environment,
)
from molt.cli.cargo_resource_custody import (
    CargoExecutableCustody,
    CargoResourceCustody,
    CargoResourceRoot,
)
from molt.cli.runtime_identity_schema import _freeze_json
from molt.cli.runtime_c_resources import (
    _C_FLAG_CONTROLS,
    _C_FLAG_PREFIXES,
    _C_SEARCH_ENVIRONMENTS,
    _resolve_c_build_resources,
)
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
    verify_stable_regular_file_identity,
)


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
        return rustc_probes.rust_target_metadata(
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
                    path = runtime_tool_selection.runtime_tool_path(
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
) -> tuple[ProcessEnvironmentBuilder, tuple[str, ...], tuple[CargoResourceRoot, ...]]:
    definitions = _merge(
        _table(configuration.get("env", {}), label="env"),
        _table(cli.get("env", {}), label="CLI env"),
    )
    result = ProcessEnvironmentBuilder(env)
    forced: list[str] = []
    resources: list[CargoResourceRoot] = []
    seen: dict[str, str] = {}
    for name, raw in definitions.items():
        if not name or "=" in name or "\0" in name:
            raise ValueError(f"runtime Cargo environment key is invalid: {name!r}")
        canonical = result.rule.key(name)
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


def _flags(value: object, *, label: str, rule: FlagStringRule) -> tuple[str, ...]:
    if isinstance(value, str):
        # Cargo splits string flags with its own rule; it does not invoke a shell.
        return rule.decode(value)
    if isinstance(value, (tuple, list)) and all(
        isinstance(item, str) and item for item in value
    ):
        return cast(tuple[str, ...], tuple(value))
    raise ValueError(f"runtime {label} must be a flag string or string array")


def cargo_cfg_flags(value: object) -> tuple[str, ...]:
    """A ``target.'cfg(...)'`` flag value, read as Cargo's StringList."""
    return _flags(value, label="cfg rustflags", rule=FlagStringRule.CARGO_LIST)


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
    configured = _flags(
        value if value is not None else (),
        label=".".join(keys),
        rule=FlagStringRule.CARGO_LIST,
    )
    return configured + (
        _flags(
            env[environment_name],
            label=environment_name,
            rule=FlagStringRule.CARGO_LIST,
        )
        if environment_name in env
        else ()
    )


def _cargo_build_directory_base(root: Path, environment: Mapping[str, str]) -> Path:
    raw = (
        environment.get("MOLT_CARGO_BUILD_DIR_BASE")
        or environment.get("CARGO_BUILD_BUILD_DIR")
        or environment.get("CARGO_TARGET_DIR")
        or "target"
    )
    path = Path(raw)
    return Path(os.path.abspath(path if path.is_absolute() else root / path))


def _cargo_profile_package_selector_name(spec: str) -> str | None:
    """Proven package-name projection of Cargo's package-ID specification.

    Only the name is needed to exclude sibling controls. A matching qualified
    selector still needs resolved version/source identity before it can govern
    a target package. Grammar: https://doc.rust-lang.org/cargo/reference/pkgid-spec.html
    """
    from urllib.parse import urlsplit

    name_pattern = r"[A-Za-z0-9_-]+"
    version_pattern = (
        r"[0-9]+(?:\.[0-9]+){0,2}(?:-[A-Za-z0-9.-]+)?(?:\+[A-Za-z0-9.-]+)?"
    )
    if "://" not in spec:
        match = re.fullmatch(rf"({name_pattern})(?:[:@]{version_pattern})?", spec)
        return match.group(1) if match is not None else None
    try:
        url = urlsplit(spec)
    except ValueError:
        return None
    fragment = url.fragment
    if fragment and re.fullmatch(version_pattern, fragment) is None:
        match = re.fullmatch(rf"({name_pattern})(?:[:@]{version_pattern})?", fragment)
        return match.group(1) if match is not None else None
    # Version-only and absent fragments inherit the terminal URL path segment.
    # Encoded or non-package segments remain unproven rather than guessed.
    tail = url.path.rsplit("/", 1)[-1]
    return tail if re.fullmatch(name_pattern, tail) is not None else None


@dataclass(frozen=True, slots=True)
class RuntimeCargoPlan:
    project_root: Path
    command: tuple[str, ...]
    environment: ProcessEnvironment
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

    def __post_init__(self) -> None:
        # A plain mapping looks the same on POSIX and loses cc-rs's
        # target-suffixed selectors only on Windows.
        if not isinstance(self.environment, ProcessEnvironment):
            raise TypeError(
                "runtime Cargo plan environment must be a ProcessEnvironment"
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
        pinned = {
            "CARGO_BUILD_BUILD_DIR": os.fspath(self.build_directory),
            "MOLT_CARGO_BUILD_DIR_BASE": os.fspath(self.build_directory_base),
        }
        if self.target_directory is not None:
            pinned["CARGO_TARGET_DIR"] = os.fspath(self.target_directory)
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
            environment=self.environment.updated(pinned),
        )

    def _without_pinned_configuration(
        self,
        command: tuple[str, ...] | None = None,
    ) -> tuple[str, ...]:
        self.configuration_arguments()
        selected = self.command if command is None else command
        separator = selected.index("--") if "--" in selected else len(selected)
        pins = cargo_argv._pinned_tool_configurations(
            self.tools,
            self.wrappers,
            self.target,
            self.forced_environment,
            self.build_directory,
            self.target_directory,
        )
        return (*selected[: separator - 2 * len(pins)], *selected[separator:])

    def with_final_link_response(
        self,
        replacement: Path,
        *,
        previous: Path | None,
    ) -> RuntimeCargoPlan:
        """Rebind final-link response custody without recapturing the SDK graph.

        The caller owns its response argument language. This operation changes
        only that response operand, normalizes final-crate target policy, captures
        every selected response file and repins the resulting resource generation.
        """
        self.verify()
        replacement = Path(os.path.abspath(replacement))
        argument = "link-arg=@" + os.fspath(replacement)
        if previous is None:
            command = (
                *self.command,
                *(("--",) if "--" not in self.command else ()),
                "-C",
                argument,
            )
        else:
            old = "link-arg=@" + os.fspath(previous)
            if self.command.count(old) != 1:
                raise ValueError(
                    "runtime link response replacement requires one admitted operand"
                )
            command = tuple(
                argument if token == old else token for token in self.command
            )
        flags = _resolve_rust_flag_resources(
            self.rustflags,
            command,
            root=self.project_root,
            env=self.environment,
            wasi_plan=self.wasi_c_abi,
        )
        if (
            flags.flags != self.rustflags
            or any(root not in self.rust_resources.roots for root in flags.roots)
            or flags.dependency_linker is not None
            and flags.dependency_linker != self.tools.get("linker")
            or flags.final_linker != self.tools.get("final_linker")
        ):
            raise ValueError(
                "runtime link response changed admitted compiler resources"
            )
        result = replace(
            self,
            command=self._without_pinned_configuration(flags.command),
            link_resources=CargoResourceCustody.capture(flags.link_roots),
        ).with_build_directory()
        result.verify()
        return result

    def with_build_resources(self, resources: CargoResourceCustody) -> RuntimeCargoPlan:
        """Join an already admitted producer SDK to the same Cargo authority."""
        unbound = replace(
            self,
            command=self._without_pinned_configuration(),
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
            ("cargo-build", self.build_directory_base),
            ("source", self.project_root),
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
        overrides = cargo_command_options(self.command)[1]
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
        if self.environment.get(
            "CARGO_ENCODED_RUSTFLAGS"
        ) != FlagStringRule.CARGO_ENCODED.encode(
            self.rustflags, variable="CARGO_ENCODED_RUSTFLAGS"
        ):
            raise ValueError("runtime Cargo execution differs from selected flags")
        for name in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
            if self.environment.get(name) != (
                os.fspath(self.wrappers[name]) if name in self.wrappers else ""
            ):
                raise ValueError(
                    "runtime Cargo execution differs from selected wrappers"
                )
        if derives_cargo_search_path() and self.environment.get(
            "PATH"
        ) != cargo_search_path(
            self.environment, (*self.tools.values(), *self.wrappers.values())
        ):
            raise ValueError("runtime Cargo PATH differs from its selected tools")
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
            selector = cargo_c_tool_environment_names(
                c_role, target=target, host_target=self.host_target
            )[0]
            if self.environment.get(selector) != os.fspath(path):
                raise ValueError(
                    f"runtime Cargo {role} execution differs from selected tool"
                )
        c_resources = _resolve_c_build_resources(
            self.environment,
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
        for spelling, value in self.environment.items():
            name = self.environment.rule.key(spelling)
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

    def incremental_for_profile(
        self, cargo_profile: str, *, package_name: str, workspace_package: bool
    ) -> bool:
        """Effective incrementality for one package in this captured Cargo plan.

        CARGO_INCREMENTAL overrides build.incremental and all profile settings.
        Named package overrides precede wildcard and base-profile settings;
        Cargo's wildcard excludes workspace packages. Qualified package IDs need
        resolved package identity, so refuse those controls rather than guess.
        """
        global_incremental = self.environment.get("CARGO_INCREMENTAL")
        if global_incremental is not None:
            # Cargo's global switch enables only the literal "1"; profile
            # and build configuration use their separate boolean grammar.
            return global_incremental == "1"
        value = cargo_selected_value(
            self.profile_configuration,
            self.cli_configuration,
            self.environment,
            ("build", "incremental"),
            ("CARGO_BUILD_INCREMENTAL",),
        )
        if value is None:
            value = self._profile_package_value(
                cargo_profile,
                "incremental",
                package_name=package_name,
                workspace_package=workspace_package,
            )
        if value is None:
            return self.profile_ancestry(cargo_profile)[-1] == "dev"
        if value in (True, "true", "1"):
            return True
        if value in (False, "false", "0"):
            return False
        raise ValueError(f"runtime Cargo incremental setting is invalid: {value!r}")

    def _profile_package_value(
        self,
        cargo_profile: str,
        setting: Literal["incremental", "strip"],
        *,
        package_name: str,
        workspace_package: bool,
    ) -> object:
        """Resolve a package setting from the one captured Cargo configuration."""
        ancestry = self.profile_ancestry(cargo_profile)
        value = None
        for profile in ancestry:
            overrides = cargo_selected_value(
                self.profile_configuration,
                self.cli_configuration,
                {},
                ("profile", profile, "package"),
                (),
                {},
            )
            for spec, settings in _table(overrides, label="profile.package").items():
                settings = _table(settings, label="profile.package settings")
                selected_name = _cargo_profile_package_selector_name(spec)
                if selected_name is not None and selected_name != package_name:
                    # A version qualification cannot change the named package.
                    # These sibling settings do not govern this package.
                    continue
                if setting in settings and (
                    not isinstance(spec, str)
                    or any(character in spec for character in ":/@#")
                ):
                    raise ValueError(
                        f"runtime Cargo {setting} package-ID override requires "
                        f"resolved package identity: {spec!r}"
                    )
        for selector in (package_name,) if workspace_package else (package_name, "*"):
            for profile in ancestry:
                value = cargo_selected_value(
                    self.profile_configuration,
                    self.cli_configuration,
                    {},
                    ("profile", profile, "package", selector, setting),
                    (),
                )
                if value is not None:
                    break
            if value is not None:
                break
        if value is None:
            for profile in ancestry:
                value = cargo_selected_value(
                    self.profile_configuration,
                    self.cli_configuration,
                    self.environment,
                    ("profile", profile, setting),
                    (_profile_environment_prefix(profile) + setting.upper(),),
                )
                if value is not None:
                    break
        return value

    def strips_debug_for_profile(
        self,
        cargo_profile: str,
        *,
        package_name: str,
        workspace_package: bool,
    ) -> bool:
        """Whether the final package linker strips all DWARF, even with debug on."""
        value = None
        separator = (
            self.command.index("--") if "--" in self.command else len(self.command)
        )
        for tokens in (self.rustflags, self.command[separator + 1 :]):
            for span in rust_flag_spans(tokens):
                if (
                    span.codegen is not None
                    and span.codegen.partition("=")[0] == "strip"
                ):
                    value = span.codegen.partition("=")[2]
        if value is None:
            value = self._profile_package_value(
                cargo_profile,
                "strip",
                package_name=package_name,
                workspace_package=workspace_package,
            )
        if value is None or value is False or value in ("false", "none"):
            return False
        if value is True or value in ("true", "symbols", "debuginfo"):
            return True
        raise ValueError(f"runtime Cargo strip setting is invalid: {value!r}")

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
        environment_flags = FlagStringRule.CARGO_ENCODED.decode(
            environment["CARGO_ENCODED_" + flag_kind.upper()]
        )
    elif flag_kind.upper() in environment:
        environment_flags = _flags(
            environment[flag_kind.upper()],
            label=flag_kind.upper(),
            rule=FlagStringRule.CARGO_RUSTFLAGS,
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
    environment = ProcessEnvironmentBuilder(env)
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
    command_target, overrides = cargo_command_options(cargo_command)
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
    environment_transform: Callable[[ProcessEnvironment], Mapping[str, str]]
    | None = None,
    rustflags_transform: Callable[[tuple[str, ...]], tuple[str, ...]] | None = None,
    required_cargo_capabilities: tuple[cargo_capabilities.CargoCapability, ...] = (),
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
        environment = ProcessEnvironmentBuilder(
            environment_transform(ProcessEnvironment(environment)),
            rule=environment.rule,
        )
    # Resolve Cargo's effective selectors before probing either compiler. The
    # source environment remains exact; this projection only admits the plan.
    wrapper_selectors: dict[str, str] = {}
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
        wrapper_selectors[name] = value
    if "SOURCE_DATE_EPOCH" in environment:
        admission_environment = ProcessEnvironmentBuilder(environment)
        admission_environment.update(wrapper_selectors)
        validate_sccache_epoch_environment(
            admission_environment,
            cwd=root,
            admit_c_tools=False,
        )
    rustc_selector = cargo_selected_value(
        configuration,
        cli,
        environment,
        ("build", "rustc"),
        ("RUSTC", "CARGO_BUILD_RUSTC"),
        "rustc",
    )
    rustc = runtime_tool_selection.runtime_tool_path(
        rustc_selector, root=root, env=environment, role="rustc"
    )
    rustc = resolve_rustup_proxy(rustc, role="rustc", root=root, env=environment)
    rustc_custody = CargoExecutableCustody.capture("tool/rustc", rustc)
    environment["RUSTC"] = os.fspath(rustc)
    environment.update(rust_toolchain_library_environment(rustc, environment))
    wrappers: dict[str, Path] = {}
    for name, value in wrapper_selectors.items():
        environment[name] = ""  # Explicit empty disables inherited config.
        if value:
            wrappers[name] = runtime_tool_selection.runtime_tool_path(
                value, root=root, env=environment, role=name
            )
            environment[name] = os.fspath(wrappers[name])
    wrapper_custody = tuple(
        CargoExecutableCustody.capture("wrapper/" + role, path)
        for role, path in wrappers.items()
    )
    if host_target is None:
        host_target = rustc_probes.rustc_host_target(
            rustc, wrappers, root=root, env=environment
        )
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
    validate_sccache_epoch_environment(
        environment, cwd=root, target=target, host_target=host_target
    )
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
    probe_environment = ProcessEnvironment(environment)

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
            metadata_answers[key] = rustc_probes.rust_target_metadata(
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
        # Compiler metadata must not depend on checkout/intermediate custody.
        # Cargo's selected flags remain intact. Apply the more specific root
        # last; if both roots coincide, source spelling owns that directory.
        # This also covers generated Rust in OUT_DIR, outside the source tree.
        build_root = _cargo_build_directory_base(root, environment)
        mappings = ((root, "/molt/source"), (build_root, "/molt/build"))
        if root.is_relative_to(build_root):
            mappings = mappings[::-1]
        remaps = rust_remap_path_flags(mappings)
        # A retained plan can re-enter target policy (WASM adds codegen flags
        # after its inherited vector). Remove only our exact mapping spans,
        # then append the owned policy once; values of other options are not
        # flags, and user mappings with different operands retain their order.
        transformed = tuple(
            token
            for span in rust_flag_spans(transformed)
            if span.option != "--remap-path-prefix"
            or "--remap-path-prefix=" + (span.value or "") not in remaps
            for token in transformed[span.start : span.stop]
        )
        flag_plan = _resolve_rust_flag_resources(
            transformed,
            cargo_command,
            root=root,
            env=environment,
            wasi_plan=wasi_plan,
        )
        # SDK mode normalization can append flags too. Add resource-free
        # remaps after it, to both the dependency vector and its combined
        # final-crate metadata lane, without a second resource walk.
        dependency_flags = flag_plan.flags
        flag_plan = replace(
            flag_plan,
            flags=(*dependency_flags, *remaps),
            flag_lanes=tuple(
                (*dependency_flags, *remaps, *lane[len(dependency_flags) :])
                for lane in flag_plan.flag_lanes
            ),
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
        flags=cargo_cfg_flags,
        probe=lambda flags: metadata_for(target_argument, flags, True).cfg,
        transform=finalize_flags,
    )
    assert flag_plan is not None
    cargo = runtime_tool_selection.runtime_tool_path(
        cargo_command[0], root=root, env=environment, role="cargo"
    )
    cargo = resolve_rustup_proxy(cargo, role="cargo", root=root, env=environment)
    cargo_custody = CargoExecutableCustody.capture("tool/cargo", cargo)
    cargo_capabilities.require_cargo_capabilities(
        cargo_custody,
        root=root,
        env=environment,
        capabilities=required_cargo_capabilities,
    )
    tools: dict[str, Path] = {"rustc": rustc, "cargo": cargo}
    defaults = runtime_tool_selection.target_tool_defaults(target)
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
            names = cargo_c_tool_environment_names(
                role, target=target, host_target=host_target
            )
            selected_c_tool = runtime_tool_selection.runtime_c_tool_selection(
                role, root=root, env=environment, target=target, host_target=host_target
            )
            value = None if selected_c_tool is None else os.fspath(selected_c_tool)
        if role == "linker" and value is None and target == host_target:
            value = next(
                (
                    name
                    for name in defaults[role]
                    if find_executable(name, environment=environment, cwd=root)
                    is not None
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
            runtime_tool_selection.runtime_tool_path(
                value, root=root, env=environment, role=role
            )
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
    runtime_tool_selection.select_host_c_tools(
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
    if derives_cargo_search_path():
        # Tools are final: the caller's PATH stops being a build input (HF-385).
        environment["PATH"] = cargo_search_path(
            environment, (*tools.values(), *wrappers.values())
        )
    environment["CARGO_ENCODED_RUSTFLAGS"] = FlagStringRule.CARGO_ENCODED.encode(
        rustflags, variable="CARGO_ENCODED_RUSTFLAGS"
    )
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
            qualified = rustc_probes.rust_target_metadata(
                rustc,
                query_target,
                flags,
                root=root,
                env=probe_environment,
                wrappers=wrappers if wrapped else {},
                qualify=True,
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
        ProcessEnvironment(environment),
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
