"""Compact one-capture toolchain custody and frozen-manifest verification."""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import time
from typing import Mapping, Sequence, cast

from molt.exact_json import canonical_json_sha256
from molt.toolchain_identity import executable_environment_value, find_executable
from molt.rust_toolchain import (
    RustToolSearch,
    RustupProxyUnavailable,
    cargo_config_arguments,
    resolve_rustup_proxy,
    relative_rustc_tool_paths,
    rustc_host,
    rustc_printed_sysroot,
)
from tools.command_execution import CommandExecutor
from tools.proof_queue_pkg import custody_cas
from tools.proof_queue_pkg.process_image_capture import (
    canonical_images,
    capture_image,
    revalidate_images,
)


CAPTURE_SCHEMA = "molt.proof-toolchain-capture.v1"
VERIFICATION_SCHEMA = "molt.proof-toolchain-verification.v1"
_COMMANDS = CommandExecutor.for_file(__file__)


class RustLinkCaptureError(ValueError):
    """Compact failure with full probe transcripts for the queue's existing CAS."""

    def __init__(self, message: str, *, unit: str, probes: list[dict[str, object]]):
        unit_probes = [probe for probe in probes if probe.get("unit") == unit]
        phase = str(unit_probes[-1]["phase"]) if unit_probes else "configuration"
        super().__init__(f"Rust linker capture {unit}/{phase}: {message}")
        self.diagnostic = {
            "schema": "molt.proof-rust-link-capture-failure.v1",
            "unit": unit,
            "phase": phase,
            "reason": message,
            "probes": list(probes),
        }


def _run_rust_link_probe(
    command: Sequence[str],
    *,
    phase: str,
    unit: str,
    cwd: Path,
    compiler_cwd: Path,
    env: Mapping[str, str],
    timeout: float,
    probes: list[dict[str, object]],
) -> subprocess.CompletedProcess[str]:
    """Retain command results, without logging environment values or retrying."""
    record: dict[str, object] = {
        "phase": phase,
        "unit": unit,
        "argv": list(command),
        "cwd": str(cwd),
        "compiler_cwd": str(compiler_cwd),
        "returncode": None,
        "stdout": "",
        "stderr": "",
    }
    probes.append(record)
    try:
        result = _COMMANDS.run(
            command,
            cwd=cwd,
            env=dict(env),
            check=False,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except Exception as exc:
        for stream in ("stdout", "stderr"):
            value = getattr(exc, stream, "") or ""
            record[stream] = (
                value.decode("utf-8", errors="replace")
                if isinstance(value, bytes)
                else str(value)
            )
        record["exception_type"] = type(exc).__name__
        record["exception_message"] = str(exc)
        raise RustLinkCaptureError(
            f"probe execution raised {type(exc).__name__}",
            unit=unit,
            probes=probes,
        ) from exc
    record.update(
        returncode=result.returncode, stdout=result.stdout, stderr=result.stderr
    )
    return result


def select_cargo_build_tool_environment(
    *, cwd: Path, env: Mapping[str, str], rustdoc_required: bool = False
) -> tuple[dict[str, str], dict[str, object]]:
    """Bind Cargo's header, formatting and documentation process selections.

    A Rust linker is not the bindgen compiler authority. The queue selects an
    optional host driver once and publishes the upstream explicit CLANG_PATH
    hook, including for cross-target headers. This is a pinned-driver policy,
    not a replay of every build script's target-prefixed ambient discovery.
    Existing executable-environment custody hashes, watches and admits it;
    no installation directory or ambient candidate set is admitted. Rustup
    proxies become their selected component before the same executable-input
    capture; Cargo documentation and doctests do not get a separate allowlist.
    """
    updates: dict[str, str] = {}
    probes: list[dict[str, object]] = []
    search_diagnostics: list[dict[str, object]] = []
    source = "unavailable"
    selected: Path | None = None

    def explicit_path(name: str, value: str) -> Path:
        path = Path(value)
        if not path.is_absolute():
            raise ValueError(
                f"Cargo build tool {name} requires an absolute executable path; "
                "build-script working directories differ from Cargo's invocation directory"
            )
        path = Path(os.path.abspath(path))
        if not path.is_file() or not os.access(path, os.X_OK):
            raise ValueError(f"Cargo build tool {name} must name one executable file")
        return path

    def query(path: Path, arguments: Sequence[str], *, required: bool) -> str | None:
        try:
            completed = _run_rust_link_probe(
                [str(path), *arguments],
                phase="header-driver-selection",
                unit="cargo-bindgen",
                cwd=cwd,
                compiler_cwd=cwd,
                env=env,
                timeout=30.0,
                probes=probes,
            )
        except RustLinkCaptureError:
            if required:
                raise
            return None  # The probe transcript retains the unavailable capability.
        if completed.returncode != 0:
            if not required:
                return None
            raise RustLinkCaptureError(
                "header-driver selector command failed",
                unit="cargo-bindgen",
                probes=probes,
            )
        lines = completed.stdout.splitlines()
        if len(lines) != 1 or not lines[0].strip():
            probes[-1]["selection_error"] = "selector did not print exactly one path"
            if not required:
                return None
            raise RustLinkCaptureError(
                "header-driver selector must print exactly one path",
                unit="cargo-bindgen",
                probes=probes,
            )
        return lines[0].strip()

    clang_value = executable_environment_value(env, "CLANG_PATH")
    config_value = executable_environment_value(env, "LLVM_CONFIG_PATH")
    config = None
    if config_value:
        config = explicit_path("LLVM_CONFIG_PATH", config_value)
    else:
        # Library discovery invokes llvm-config independently of Clang driver
        # discovery, even when the caller supplies an explicit CLANG_PATH.
        config = find_executable("llvm-config", environment=env, cwd=cwd)
    if config is not None:
        updates["LLVM_CONFIG_PATH"] = str(config)
    if clang_value:
        selected = explicit_path("CLANG_PATH", clang_value)
        source = "CLANG_PATH"
    else:
        search: list[tuple[Path, str]] = []
        if config is not None:
            output = query(config, ["--bindir"], required=bool(config_value))
            if output is not None:
                raw = Path(output)
                search.append(
                    (raw if raw.is_absolute() else cwd / raw, "llvm-config --bindir")
                )
        path_value = executable_environment_value(env, "PATH")
        if path_value:
            for value in path_value.split(os.pathsep):
                path = Path(value.strip('"') if os.name == "nt" else value)
                search.append((path if path.is_absolute() else cwd / path, "PATH"))
        suffix = ".exe" if os.name == "nt" else ""
        for directory, authority in search:
            try:
                # Do not enumerate a directory when its ordinary entrypoint
                # already suffices. A PATH entry can permit file execution but
                # deny directory enumeration, especially on Windows.
                ordinary = directory / f"clang{suffix}"
                if ordinary.is_file() and os.access(ordinary, os.X_OK):
                    selected = ordinary.absolute()
                else:
                    selected = next(
                        (
                            path.absolute()
                            for path in sorted(directory.glob(f"clang-[0-9]*{suffix}"))
                            if path.is_file() and os.access(path, os.X_OK)
                        ),
                        None,
                    )
            except OSError as exc:
                # An implicit search miss is not authority to change access
                # controls or fail an otherwise usable later PATH selection.
                search_diagnostics.append(
                    {
                        "source": authority,
                        "directory": str(directory),
                        "exception_type": type(exc).__name__,
                        "errno": exc.errno,
                        "winerror": getattr(exc, "winerror", None),
                    }
                )
                continue
            if selected is not None:
                source = authority
                break
        if selected is None and sys.platform == "darwin":
            xcrun = find_executable("xcrun", environment=env, cwd=cwd)
            if xcrun is not None:
                output = query(xcrun, ["--find", "clang"], required=False)
                if output is not None:
                    selected = explicit_path("xcrun-selected Clang", output)
                    source = "xcrun --find clang"
    if selected is not None:
        updates["CLANG_PATH"] = str(selected)
    rust_components: dict[str, bool] = {}
    for role, names, required in (
        ("rustfmt", ("RUSTFMT",), False),
        ("rustdoc", ("RUSTDOC", "CARGO_BUILD_RUSTDOC"), rustdoc_required),
    ):
        explicit = next(
            (
                (name, value)
                for name in names
                if (value := executable_environment_value(env, name))
            ),
            None,
        )
        component = (
            explicit_path(*explicit)
            if explicit is not None
            else find_executable(role, environment=env, cwd=cwd)
        )
        if component is not None:
            try:
                component = resolve_rustup_proxy(
                    component, role=role, root=cwd, env=env
                )
            except RustupProxyUnavailable as exc:
                if explicit is not None or required:
                    raise  # Never replace an explicit or required component.
                probes.append(exc.diagnostic)
                component = None
        if component is None and required:
            raise ValueError(
                f"Cargo requires an available {role} executable before capture"
            )
        if component is not None:
            updates[names[0]] = str(component)
            # RUSTDOC precedes Cargo's build.rustdoc hook. Collapse any supplied
            # lower-priority hook to that selection rather than admit a shadowed
            # executable as a second process authority.
            for name in names[1:]:
                if executable_environment_value(env, name):
                    updates[name] = str(component)
        rust_components[role] = component is not None
    return updates, {
        "schema": "molt.proof-cargo-build-tool-selection.v1",
        "available": selected is not None,
        "source": source,
        "formatter_available": rust_components["rustfmt"],
        "rustdoc_available": rust_components["rustdoc"],
        "rustdoc_required": rustdoc_required,
        "bound_names": sorted(updates),
        "probes": probes,
        "search_diagnostics": search_diagnostics,
    }


@dataclass(frozen=True)
class FrozenFile:
    path: str
    sha256: str
    size: int | None

    def as_dict(self) -> dict[str, object]:
        return {"path": self.path, "sha256": self.sha256, "size": self.size}


def _command_tokens(line: str) -> list[str]:
    """Decode rustc's quoted command-debug output without platform guessing."""
    tokens: list[str] = []
    decoder = json.JSONDecoder()
    index = 0
    while index < len(line):
        while index < len(line) and line[index].isspace():
            index += 1
        if index >= len(line):
            break
        if line[index] != '"':
            return shlex.split(line, posix=os.name != "nt")
        value, end = decoder.raw_decode(line, index)
        if not isinstance(value, str):
            raise ValueError("rust linker command contains a non-string argument")
        tokens.append(value)
        index = end
    return tokens


def _selected_command_lines(output: str) -> list[list[str]]:
    commands: list[list[str]] = []
    for line in output.splitlines():
        stripped = line.strip()
        if not stripped.startswith('"'):
            continue
        try:
            tokens = _command_tokens(stripped)
        except (ValueError, json.JSONDecodeError):
            continue
        if tokens:
            commands.append(tokens)
    return commands


def _selected_rust_link_command(stdout: str, stderr: str) -> list[str]:
    """Select exactly one rustc link command from its complete output stream."""
    commands = _selected_command_lines(stdout + "\n" + stderr)
    if len(commands) != 1:
        raise ValueError(
            f"synthetic Rust linker selection returned {len(commands)} commands"
        )
    return commands[0]


def _cargo_forwarded_compiler_context(
    cargo: Path,
    command: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    probes: list[dict[str, object]] | None = None,
    unit: str = "target",
) -> dict[str, object]:
    """Resolve the compiler cwd from Cargo-owned workspace/package provenance.

    Cargo 1.96 util/workspace.rs::path_args uses the workspace root for path
    sources below that root, otherwise the package root. The optional root-dir
    flag changes the first root, not the fallback. Never infer this from cwd or
    a hand-parsed Cargo.toml, and never compile an original package here.
    """
    context_args = cargo_config_arguments(command, cwd=cwd)
    packages: list[str] = []
    selectors: list[tuple[str, str | None]] = []
    root_override = None
    index = 1
    while index < len(command) and command[index] != "--":
        value = command[index]
        name, equal, inline = value.partition("=")
        if name in {
            "--manifest-path",
            "--package",
            "-p",
            "--bin",
            "--example",
            "--test",
            "--bench",
            "-Z",
        }:
            if equal:
                argument = inline
            else:
                index += 1
                if index >= len(command):
                    raise ValueError(f"Cargo {name} requires a value")
                argument = command[index]
            if name == "--manifest-path":
                path = Path(argument)
                context_args.extend(
                    (name, str(path if path.is_absolute() else cwd / path))
                )
            elif name in {"--package", "-p"}:
                packages.append(argument)
            elif name == "-Z":
                if argument.startswith("root-dir="):
                    root_override = cwd / argument.split("=", 1)[1]
            else:
                selectors.append((name[2:], argument))
        elif value == "--lib":
            selectors.append(("lib", None))
        elif value.startswith("-p") and len(value) > 2:
            packages.append(value[2:])
        elif value.startswith("-Zroot-dir="):
            root_override = cwd / value.split("=", 1)[1]
        index += 1

    def query(arguments: list[str]) -> object:
        result = _run_rust_link_probe(
            [str(cargo), *arguments, *context_args],
            phase="cargo-context-" + arguments[0],
            unit=unit,
            cwd=cwd,
            compiler_cwd=cwd,
            env=env,
            timeout=30.0,
            probes=probes if probes is not None else [],
        )
        if result.returncode != 0:
            raise ValueError(
                "Cargo compiler-cwd provenance unavailable: metadata command failed"
            )
        return result.stdout

    metadata = json.loads(
        str(
            query(
                [
                    "metadata",
                    "--offline",
                    "--locked",
                    "--no-deps",
                    "--format-version",
                    "1",
                ]
            )
        )
    )
    if not isinstance(metadata, dict):
        raise ValueError("Cargo compiler-cwd metadata is not an object")
    selected_ids = (
        [
            str(query(["pkgid", "--offline", "--locked", "--package", package])).strip()
            for package in packages
        ]
        if packages
        else metadata.get("workspace_default_members", [])
    )
    workspace = Path(str(metadata["workspace_root"]))
    root = workspace if root_override is None else root_override.absolute()
    contexts = []
    for package in metadata.get("packages", []):
        if package.get("id") not in selected_ids:
            continue
        package_root = Path(package["manifest_path"]).parent
        for target in package.get("targets", []):
            kinds = target.get("kind", [])
            if "custom-build" in kinds:
                continue  # Forwarded cargo-rustc arguments never apply here.
            if not selectors and all(
                kind in {"example", "test", "bench"} for kind in kinds
            ):
                continue
            if selectors and not any(
                (
                    kind in kinds
                    or kind == "lib"
                    and any(
                        k in kinds
                        for k in (
                            "lib",
                            "rlib",
                            "dylib",
                            "cdylib",
                            "staticlib",
                            "proc-macro",
                        )
                    )
                )
                and (name is None or name == target.get("name"))
                for kind, name in selectors
            ):
                continue
            source = Path(target["src_path"])
            compiler_cwd = (
                root
                if package.get("source") is None and source.is_relative_to(root)
                else package_root
            )
            contexts.append(
                {
                    "package_id": package["id"],
                    "source": str(source),
                    "compiler_cwd": str(compiler_cwd),
                }
            )
    directories = {row["compiler_cwd"] for row in contexts}
    if len(directories) != 1 or not Path(next(iter(directories))).is_absolute():
        raise ValueError(
            "Cargo forwarded relative tool path has no unique compiler-cwd provenance: "
            + json.dumps(contexts, sort_keys=True)
        )
    return {
        "compiler_cwd": next(iter(directories)),
        "workspace_root": str(workspace),
        "sources": contexts,
        "metadata_sha256": canonical_json_sha256(metadata),
    }


def capture_rust_link_process_images(
    *,
    rustc: Path,
    cargo: Path | None,
    cwd: Path,
    env: Mapping[str, str],
    target: str | None,
    command_argv: Sequence[str] = (),
    linker_process_helpers: Mapping[str, Sequence[str]] | None = None,
    linker_build_tools: Mapping[str, Mapping[str, str]] | None = None,
    rustc_version: str | None = None,
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Capture both Cargo target and host-unit linker families exactly once."""
    compiler_probes: list[dict[str, object]] = []

    def metadata(arguments: list[str]) -> str:
        result = _run_rust_link_probe(
            [str(rustc), *arguments],
            phase="compiler-metadata",
            unit="compiler",
            cwd=cwd,
            compiler_cwd=cwd,
            env=env,
            timeout=30.0,
            probes=compiler_probes,
        )
        if result.returncode != 0:
            raise RustLinkCaptureError(
                "selected Rust toolchain metadata command failed",
                unit="compiler",
                probes=compiler_probes,
            )
        return result.stdout

    try:
        host = rustc_host(
            rustc_version if rustc_version is not None else metadata(["-vV"])
        )
        compiler_sysroot = rustc_printed_sysroot(
            metadata(["--print", "sysroot"]), cwd=cwd
        )
    except RustLinkCaptureError:
        raise
    except Exception as exc:
        raise RustLinkCaptureError(
            str(exc), unit="compiler", probes=compiler_probes
        ) from exc
    units = ("target", "host-proc-macro") if cargo is not None else ("target",)
    images: list[dict[str, object]] = []
    selections: list[dict[str, object]] = []
    for unit in units:
        probes = list(compiler_probes)
        try:
            selected_images, selection = _capture_rust_link_unit(
                rustc=rustc,
                cargo=cargo,
                cwd=cwd,
                env=env,
                target=target,
                command_argv=command_argv,
                linker_process_helpers=linker_process_helpers,
                linker_build_tools=linker_build_tools,
                unit=unit,
                host=host,
                compiler_sysroot=compiler_sysroot,
                probes=probes,
            )
        except RustLinkCaptureError:
            raise
        except Exception as exc:
            raise RustLinkCaptureError(str(exc), unit=unit, probes=probes) from exc
        images.extend(selected_images)
        selections.append(selection)
    images = canonical_images(images)
    return images, {
        "schema": "molt.proof-rust-link-selection-telemetry.v2",
        "target": target,
        "compiler_host": host,
        "selection_probe_count": len(units),
        "selected_process_count": len(images),
        "units": selections,
        "command_semantics_sha256": hashlib.sha256(
            json.dumps(list(command_argv), separators=(",", ":")).encode()
        ).hexdigest(),
    }


def _capture_rust_link_unit(
    *,
    rustc: Path,
    cargo: Path | None,
    cwd: Path,
    env: Mapping[str, str],
    target: str | None,
    command_argv: Sequence[str],
    linker_process_helpers: Mapping[str, Sequence[str]] | None,
    linker_build_tools: Mapping[str, Mapping[str, str]] | None,
    unit: str,
    host: str,
    compiler_sysroot: Path,
    probes: list[dict[str, object]],
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Ask the selected Rust toolchain to link a zero-dependency synthetic crate.

    The probe runs outside the repository source/target and uses the exact captured
    environment and Cargo configuration. It executes no repository build script.
    `--print link-args` supplies the actual selected driver argv; compiler-driver
    dry-run output then exposes internally selected linker helpers such as mold.
    """
    started = time.perf_counter()
    probe_env = dict(env)
    with tempfile.TemporaryDirectory(prefix="molt-rust-link-capture-") as raw_root:
        root = Path(raw_root).resolve()
        compiler_cwd = root if cargo is not None else cwd
        forwarded_context: dict[str, object] | None = None
        path_projections: list[dict[str, object]] = []
        config_files: list[dict[str, object]] = []
        source = root / ("host.rs" if unit == "host-proc-macro" else "main.rs")
        source.write_text(
            "extern crate proc_macro;\n"
            if unit == "host-proc-macro"
            else "fn main() {}\n#[test]\nfn proof_link_test() {}\n",
            encoding="utf-8",
        )
        output = root / ("probe.exe" if os.name == "nt" else "probe")
        if cargo is not None:
            manifest = root / "Cargo.toml"
            feature_names: set[str] = set()
            custom_profiles: set[str] = set()
            cargo_profile_args: list[str] = []
            rustc_link_args: list[str] = []
            command = [str(value) for value in command_argv]
            config_args = cargo_config_arguments(command, cwd=cwd)
            for argument in config_args[1::2]:
                if "=" not in argument:
                    path = Path(argument)
                    config_files.append(
                        {
                            "path": str(path),
                            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                            "size": path.stat().st_size,
                        }
                    )
            try:
                separator = command.index("--")
            except ValueError:
                separator = len(command)
            cargo_args = command[1:separator]
            forwarded = command[separator + 1 :] if separator < len(command) else []
            index = 0
            while index < len(cargo_args):
                value = cargo_args[index]
                if value in {"--release", "--all-features", "--no-default-features"}:
                    cargo_profile_args.append(value)
                    index += 1
                    continue
                if value in {"--profile", "--features", "-F", "--config"}:
                    if index + 1 >= len(cargo_args):
                        raise ValueError(f"Cargo {value} requires a value")
                    argument = cargo_args[index + 1]
                    if value == "--config":
                        pass  # Invocation-relative files were bound above.
                    elif value == "--profile":
                        cargo_profile_args.extend((value, argument))
                        if argument not in {"dev", "release", "test", "bench"}:
                            if not argument or not all(
                                character.isalnum() or character in "_-"
                                for character in argument
                            ):
                                raise ValueError("Cargo profile name is not canonical")
                            custom_profiles.add(argument)
                    else:
                        selected_features: list[str] = []
                        for feature in argument.replace(",", " ").split():
                            local = feature.rsplit("/", 1)[-1]
                            if local and all(
                                character.isalnum() or character in "_-"
                                for character in local
                            ):
                                feature_names.add(local)
                                selected_features.append(local)
                        cargo_profile_args.extend((value, ",".join(selected_features)))
                    index += 2
                    continue
                if value.startswith("--config="):
                    pass
                elif value.startswith("--profile="):
                    cargo_profile_args.append(value)
                    profile = value.split("=", 1)[1]
                    if profile not in {"dev", "release", "test", "bench"}:
                        if not profile or not all(
                            character.isalnum() or character in "_-"
                            for character in profile
                        ):
                            raise ValueError("Cargo profile name is not canonical")
                        custom_profiles.add(profile)
                elif value.startswith("--features="):
                    raw_features = value.split("=", 1)[1]
                    selected_features = []
                    for feature in raw_features.replace(",", " ").split():
                        local = feature.rsplit("/", 1)[-1]
                        if local and all(
                            character.isalnum() or character in "_-"
                            for character in local
                        ):
                            feature_names.add(local)
                            selected_features.append(local)
                    cargo_profile_args.append(
                        "--features=" + ",".join(selected_features)
                    )
                index += 1
            index = 0
            while index < len(forwarded):
                value = forwarded[index]
                if value == "-C":
                    if index + 1 >= len(forwarded):
                        raise ValueError("rustc -C requires a value")
                    rustc_link_args.extend((value, forwarded[index + 1]))
                    index += 2
                    continue
                if value in {"--crate-type", "--sysroot"}:
                    if index + 1 >= len(forwarded):
                        raise ValueError(f"rustc {value} requires a value")
                    rustc_link_args.extend((value, forwarded[index + 1]))
                    index += 2
                    continue
                if value.startswith(("-C", "--crate-type=", "--sysroot=")):
                    rustc_link_args.append(value)
                index += 1
            relative_paths = (
                relative_rustc_tool_paths(rustc_link_args) if unit == "target" else []
            )
            if relative_paths:
                forwarded_context = _cargo_forwarded_compiler_context(
                    cargo,
                    command_argv,
                    cwd=cwd,
                    env=env,
                    probes=probes,
                    unit=unit,
                )
                original_cwd = Path(str(forwarded_context["compiler_cwd"]))
                for position, prefix, original in relative_paths:
                    projected = str((original_cwd / original).absolute())
                    rustc_link_args[position] = prefix + projected
                    path_projections.append(
                        {
                            "argument_index": position,
                            "original": original,
                            "projected": projected,
                            "compiler_cwd": str(original_cwd),
                        }
                    )
            feature_table = "".join(
                f"{json.dumps(name)}=[]\n" for name in sorted(feature_names)
            )
            profile_table = "".join(
                f'\n[profile.{name}]\ninherits="release"\n'
                for name in sorted(custom_profiles)
            )
            target_table = (
                '[lib]\nproc-macro=true\npath="host.rs"\n'
                if unit == "host-proc-macro"
                else '[[bin]]\nname="molt_link_capture"\npath="main.rs"\n'
            )
            manifest.write_text(
                '[package]\nname="molt_link_capture"\nversion="0.0.0"\n'
                'edition="2024"\npublish=false\n\n'
                + target_table
                + "\n[features]\n"
                + feature_table
                + profile_table,
                encoding="utf-8",
            )
            probe_env["CARGO_TARGET_DIR"] = str(root / "target")
            probe_env["CARGO_INCREMENTAL"] = "0"
            command = [
                str(cargo),
                "rustc",
                "--quiet",
                "--offline",
                "--manifest-path",
                str(manifest),
            ]
            if target:
                command.extend(("--target", target))
            command.extend(cargo_profile_args)
            command.extend(config_args)
            command.extend(
                ("--lib",)
                if unit == "host-proc-macro"
                else ("--bin", "molt_link_capture")
            )
            command.extend(
                (
                    "--",
                    *(rustc_link_args if unit == "target" else ()),
                )
            )
        else:
            command = [
                str(rustc),
                str(source),
                "--crate-name",
                "molt_link_capture",
                "-o",
                str(output),
            ]
            if target:
                command.extend(("--target", target))
            direct = [str(value) for value in command_argv[1:]]
            index = 0
            while index < len(direct):
                value = direct[index]
                if value == "-C" and index + 1 < len(direct):
                    command.extend((value, direct[index + 1]))
                    index += 2
                    continue
                if value in {"--crate-type", "--sysroot"}:
                    if index + 1 >= len(direct):
                        raise ValueError(f"rustc {value} requires a value")
                    command.extend((value, direct[index + 1]))
                    index += 2
                    continue
                if value.startswith(("-C", "--crate-type=", "--sysroot=")):
                    command.append(value)
                index += 1
        # rustc print_crate_info stops before compilation if ANY metadata-only
        # print is requested, even beside link-args. Keep the two command phases
        # disjoint for Cargo and direct rustc. Cargo fingerprints extra_args_for
        # the unit, so the changed print request is not a freshness retry.
        metadata_command = [*command, "--print", "sysroot"]
        command = [*command, "--print", "link-args"]
        metadata = _run_rust_link_probe(
            metadata_command,
            phase="selected-sysroot",
            unit=unit,
            cwd=cwd,
            compiler_cwd=compiler_cwd,
            env=probe_env,
            timeout=30.0,
            probes=probes,
        )
        if metadata.returncode != 0:
            raise ValueError("selected Rust sysroot metadata command failed")
        if cargo is not None:
            reported = metadata.stdout.strip()
            if reported and not Path(reported).is_absolute():
                raise ValueError(
                    "unsupported Cargo linker custody: configuration-selected relative "
                    f"sysroot {reported!r} for {unit}; synthetic compiler cwd is {compiler_cwd}, "
                    f"Cargo invocation cwd is {cwd}. Forwarded paths were projected from "
                    "Cargo metadata, but per-package configuration-relative paths require "
                    "a Cargo compiler-execution context hook before repository build."
                )
        selected_sysroot = rustc_printed_sysroot(metadata.stdout, cwd=compiler_cwd)
        completed = _run_rust_link_probe(
            command,
            phase="link-selection",
            unit=unit,
            cwd=cwd,
            compiler_cwd=compiler_cwd,
            env=probe_env,
            timeout=120.0,
            probes=probes,
        )
        if cargo is not None:
            printed = _selected_command_lines(
                completed.stdout + "\n" + completed.stderr
            )
            if (
                len(printed) == 1
                and not Path(printed[0][0]).is_absolute()
                and any(separator in printed[0][0] for separator in "/\\")
            ):
                raise ValueError(
                    "unsupported Cargo linker custody: configuration-selected relative "
                    f"linker {printed[0][0]!r} for {unit}; synthetic compiler cwd {compiler_cwd}, "
                    f"Cargo invocation cwd {cwd}. Original per-package execution contexts "
                    "require a Cargo compiler-execution context hook before repository build."
                )
        if completed.returncode != 0:
            raise ValueError("synthetic Rust linker selection command failed")
        selected = _selected_rust_link_command(completed.stdout, completed.stderr)
        search = RustToolSearch(host, selected_sysroot, compiler_sysroot)
        primary, resolution = search.resolve(
            selected[0], cwd=compiler_cwd, env=probe_env
        )
        resolutions = [resolution]
        selected_paths = [primary]
        driver_name = primary.name.casefold()
        if any(token in driver_name for token in ("clang", "gcc", "cc", "c++")):
            dry_run = _run_rust_link_probe(
                [str(primary), "-###", *selected[1:]],
                phase="driver-helpers",
                unit=unit,
                cwd=compiler_cwd,
                compiler_cwd=compiler_cwd,
                env=probe_env,
                timeout=30.0,
                probes=probes,
            )
            nested_commands = _selected_command_lines(
                dry_run.stdout + "\n" + dry_run.stderr
            )
            if dry_run.returncode != 0 or not nested_commands:
                raise ValueError(
                    "selected compiler driver did not expose its exact helper commands"
                )
            for nested in nested_commands:
                selected_path, resolution = search.resolve(
                    nested[0], cwd=compiler_cwd, env=probe_env
                )
                selected_paths.append(selected_path)
                resolutions.append(resolution)
        helper_policy = {
            str(linker).casefold(): tuple(str(helper) for helper in helpers)
            for linker, helpers in (linker_process_helpers or {}).items()
        }
        declared_helpers = helper_policy.get(driver_name, ())
        selected_helpers: list[Path] = []
        for helper_name in declared_helpers:
            if Path(helper_name).name != helper_name:
                raise ValueError("Rust linker helper policy requires basenames")
            helper = primary.with_name(helper_name)
            if helper.is_file():
                selected_helpers.append(helper.absolute())
        selected_paths.extend(selected_helpers)
        unique_paths = list(dict.fromkeys(selected_paths))
        images = []
        auxiliary_keys = {os.path.normcase(str(path)) for path in selected_helpers}
        for index, path in enumerate(unique_paths):
            image = capture_image(
                "rust-linker" if index == 0 else "rust-link-helper",
                path,
                root_exit_disposition=(
                    "terminate"
                    if os.path.normcase(str(path)) in auxiliary_keys
                    else "require-exit"
                ),
            )
            images.append(image)
            if str(path.absolute()) != image["path"]:
                images.append(
                    capture_image(
                        str(image["role"]),
                        path,
                        root_exit_disposition=str(
                            image.get("root_exit_disposition", "require-exit")
                        ),
                        preserve_path=True,
                    )
                )
        build_tool_policy = {
            str(linker).casefold(): {
                str(tool): str(role) for tool, role in tools.items()
            }
            for linker, tools in (linker_build_tools or {}).items()
        }
        declared_build_tools = build_tool_policy.get(driver_name, {})
        selected_build_tools: list[Path] = []
        for tool_name, role in declared_build_tools.items():
            if Path(tool_name).name != tool_name:
                raise ValueError("Rust build-tool policy requires basenames")
            if not role.startswith("rust-build-"):
                raise ValueError(
                    "Rust build-tool policy requires typed rust-build roles"
                )
            tool = primary.with_name(tool_name)
            if not tool.is_file():
                raise ValueError(
                    f"selected Rust linker has no declared build tool {tool_name!r}"
                )
            selected_build_tools.append(tool.absolute())
            image = capture_image(role, tool, root_exit_disposition="require-exit")
            images.append(image)
            if str(tool.absolute()) != image["path"]:
                images.append(capture_image(role, tool, preserve_path=True))
        images = canonical_images(images)
        telemetry = {
            "schema": "molt.proof-rust-link-unit.v1",
            "unit": unit,
            "process_resolution": resolutions,
            "compiler_cwd": str(compiler_cwd),
            "forwarded_compiler_context": forwarded_context,
            "path_projections": path_projections,
            "configuration_files": config_files,
            "target": target,
            "probe": "cargo-rustc" if cargo is not None else "rustc",
            "selected_process_count": len(images),
            "selection_probe_count": 1,
            "metadata_probe_count": 1,
            "declared_helper_count": len(declared_helpers),
            "selected_helper_count": len(selected_helpers),
            "declared_build_tool_count": len(declared_build_tools),
            "selected_build_tool_count": len(selected_build_tools),
            "helper_policy_sha256": hashlib.sha256(
                json.dumps(
                    {
                        linker: list(helpers)
                        for linker, helpers in sorted(helper_policy.items())
                    },
                    sort_keys=True,
                    separators=(",", ":"),
                ).encode()
            ).hexdigest(),
            "build_tool_policy_sha256": hashlib.sha256(
                json.dumps(
                    {
                        linker: dict(sorted(tools.items()))
                        for linker, tools in sorted(build_tool_policy.items())
                    },
                    sort_keys=True,
                    separators=(",", ":"),
                ).encode()
            ).hexdigest(),
            "command_semantics_sha256": hashlib.sha256(
                json.dumps(
                    [str(value) for value in command_argv], separators=(",", ":")
                ).encode()
            ).hexdigest(),
            "link_argv_sha256": hashlib.sha256(
                json.dumps(selected, separators=(",", ":")).encode()
            ).hexdigest(),
            "capture_s": time.perf_counter() - started,
        }
        return images, telemetry


def revalidate_rust_link_process_images(
    selected_identity: Mapping[str, object],
    *,
    target: str | None,
    command_argv: Sequence[str] = (),
) -> tuple[list[dict[str, object]], dict[str, object]]:
    """Rehash frozen target/host linker selections without selecting again."""
    raw_telemetry = selected_identity.get("link_selection")
    if not isinstance(raw_telemetry, Mapping):
        raise ValueError("pre-arm Rust linker selection telemetry is unavailable")
    telemetry = dict(raw_telemetry)
    if telemetry.get("schema") != "molt.proof-rust-link-selection-telemetry.v2":
        raise ValueError("pre-arm Rust linker selection telemetry schema mismatch")
    units = telemetry.get("units")
    if (
        not isinstance(units, list)
        or not units
        or any(not isinstance(item, dict) for item in units)
        or [item.get("unit") for item in units if isinstance(item, dict)]
        not in (["target"], ["target", "host-proc-macro"])
        or telemetry.get("selection_probe_count") != len(units)
        or any(item.get("selection_probe_count") != 1 for item in units)
    ):
        raise ValueError("Rust linker units must each be selected exactly once pre-arm")
    if telemetry.get("target") != target:
        raise ValueError("Rust linker target changed while live custody armed")
    for unit in units:
        for config in unit.get("configuration_files", []):
            path = Path(config["path"])
            if (
                not path.is_file()
                or hashlib.sha256(path.read_bytes()).hexdigest() != config["sha256"]
            ):
                raise ValueError("Cargo --config file changed while live custody armed")
    command_semantics_sha256 = hashlib.sha256(
        json.dumps(
            [str(value) for value in command_argv], separators=(",", ":")
        ).encode()
    ).hexdigest()
    if telemetry.get("command_semantics_sha256") != command_semantics_sha256:
        raise ValueError(
            "Rust linker command semantics changed while live custody armed"
        )

    raw_images = selected_identity.get("process_images")
    if not isinstance(raw_images, list):
        raise ValueError("pre-arm Rust process-image selection is unavailable")
    selected_rows: list[Mapping[str, object]] = []
    for raw in raw_images:
        if not isinstance(raw, Mapping):
            raise ValueError("pre-arm Rust process-image row is malformed")
        role = raw.get("role")
        if role not in {"rust-linker", "rust-link-helper"} and not (
            isinstance(role, str) and role.startswith("rust-build-")
        ):
            continue
        selected_rows.append(raw)
    try:
        selected = revalidate_images(selected_rows)
    except ValueError as exc:
        raise ValueError(
            f"Rust linker process image changed while live custody armed: {exc}"
        ) from exc
    if not selected:
        raise ValueError("pre-arm Rust linker selection captured no process image")
    if telemetry.get("selected_process_count") != len(selected):
        raise ValueError("pre-arm Rust linker process count is inconsistent")
    return selected, telemetry


def _digest(value: object) -> str | None:
    return value if isinstance(value, str) and len(value) == 64 else None


def frozen_files(payload: object) -> list[FrozenFile]:
    """Project every captured file row into one deduplicated content manifest."""
    files: dict[str, FrozenFile] = {}

    def add(raw_path: object, raw_digest: object, raw_size: object = None) -> None:
        digest = _digest(raw_digest)
        if not isinstance(raw_path, str) or digest is None:
            return
        path = Path(raw_path)
        if not path.is_absolute():
            return
        normalized = os.path.normcase(os.path.abspath(path))
        size = raw_size if isinstance(raw_size, int) and raw_size >= 0 else None
        row = FrozenFile(str(path), digest, size)
        prior = files.get(normalized)
        if prior is not None and prior.sha256 != digest:
            raise ValueError(f"captured file has conflicting identities: {path}")
        files[normalized] = row if prior is None or prior.size is None else prior

    def visit(value: object) -> None:
        if isinstance(value, Mapping):
            if {"root", "files", "directories", "manifest_sha256"}.issubset(value):
                # Strict directory custody records relative members once. Bind
                # them to their owned root here, not in a second provider manifest.
                raw_root = value["root"]
                members = value["files"]
                directories = value["directories"]
                if (
                    not isinstance(raw_root, str)
                    or not Path(raw_root).is_absolute()
                    or not isinstance(members, list)
                    or not isinstance(directories, list)
                    or type(value.get("file_count")) is not int
                    or value["file_count"] != len(members)
                ):
                    raise ValueError(
                        "owned directory has malformed frozen file custody"
                    )
                root = Path(raw_root)
                if str(root.resolve(strict=False)) != raw_root:
                    raise ValueError("owned directory frozen root is not canonical")

                def member_path(relative: object) -> Path:
                    if (
                        not isinstance(relative, str)
                        or not relative
                        or "\\" in relative
                        or "\0" in relative
                        or any(part in {"", ".", ".."} for part in relative.split("/"))
                        or Path(relative).is_absolute()
                        or re.match(r"^[A-Za-z]:", relative) is not None
                    ):
                        raise ValueError(
                            "owned directory has invalid relative member path"
                        )
                    path = root.joinpath(*relative.split("/"))
                    if not path.resolve(strict=False).is_relative_to(root):
                        raise ValueError(
                            "owned directory relative member escapes its root"
                        )
                    return path

                for directory in directories:
                    member_path(directory)
                for member in members:
                    if not isinstance(member, Mapping):
                        raise ValueError("owned directory frozen member is malformed")
                    path = member_path(member.get("relative_path"))
                    size, digest = member.get("size"), member.get("sha256")
                    if (
                        type(size) is not int
                        or size < 0
                        or not isinstance(digest, str)
                        or re.fullmatch(r"[0-9a-f]{64}", digest) is None
                    ):
                        raise ValueError(
                            "owned directory frozen member identity is malformed"
                        )
                    add(str(path), digest, size)
                return
            if value.get("schema") == "molt.proof-python-toolchain.v3":
                custody = value.get("file_custody")
                if not isinstance(custody, list) or not custody:
                    raise ValueError("Python capture has no frozen file custody")
                for row in custody:
                    if not isinstance(row, Mapping):
                        raise ValueError(
                            "Python capture has malformed frozen file custody"
                        )
                    path = row.get("path")
                    size = row.get("size")
                    digest = row.get("sha256")
                    if (
                        not isinstance(path, str)
                        or not Path(path).is_absolute()
                        or not isinstance(size, int)
                        or isinstance(size, bool)
                        or size < 0
                        or not isinstance(digest, str)
                        or re.fullmatch(r"[0-9a-f]{64}", digest) is None
                    ):
                        raise ValueError(
                            "Python capture has malformed frozen file custody"
                        )
            path = value.get("resolved_path") or value.get("path")
            add(path, value.get("sha256"), value.get("size", value.get("size_bytes")))
            add(value.get("executable"), value.get("executable_sha256"))
            add(value.get("path"), value.get("launcher_sha256"))
            add(value.get("content_path"), value.get("executable_sha256"))
            for nested in value.values():
                visit(nested)
        elif isinstance(value, (list, tuple)):
            for nested in value:
                visit(nested)

    visit(payload)
    return [files[key] for key in sorted(files)]


def _compact_process_inventory(identity: Mapping[str, object]) -> dict[str, object]:
    """Project image custody without duplicating its full CAS-owned inventory.

    Process admission consumes the full capture, not these bounded summaries.
    Counts aid inspection; digests bind every row and all selection telemetry.
    """
    summaries: dict[str, object] = {}
    for name, expected in (
        ("process_images", list),
        ("process_image_inventories", list),
        ("link_selection", Mapping),
    ):
        if name not in identity:
            continue
        value = identity[name]
        if not isinstance(value, expected):
            raise ValueError(f"toolchain {name} has malformed inventory")
        assert isinstance(value, (list, Mapping))
        summaries[name] = {
            "count": len(value),
            "semantic_sha256": canonical_json_sha256(value),
        }
    return summaries


def _compact_python(identity: Mapping[str, object]) -> dict[str, object]:
    compact: dict[str, object] = {
        key: value
        for key, value in identity.items()
        if key
        not in {"environment", "process_images", "file_custody", "inventory_profile"}
        and not isinstance(value, (dict, list))
    }
    environment = identity.get("environment")
    if not isinstance(environment, Mapping):
        raise ValueError("python toolchain has no environment closure")
    environment = cast(Mapping[str, object], environment)
    environment_summary: dict[str, object] = {
        key: value
        for key, value in environment.items()
        if key
        in {
            "schema",
            "implementation",
            "version",
            "cache_tag",
            "soabi",
            "operating_system",
            "architecture",
            "pointer_bits",
            "gil_disabled",
            "environment_closure_sha256",
            "distribution_inventory_sha256",
        }
    }
    runtime = environment.get("runtime")
    if isinstance(runtime, Mapping):
        environment_summary["runtime"] = {
            key: value
            for key, value in runtime.items()
            if not isinstance(value, (dict, list))
        }
    distributions = environment.get("distributions")
    if not isinstance(distributions, list):
        raise ValueError("python toolchain has no distribution inventory")
    compact_distributions: list[dict[str, object]] = []
    for distribution in distributions:
        if not isinstance(distribution, Mapping):
            raise ValueError("python toolchain has a malformed distribution")
        external = distribution.get("external_source")
        if external is None:
            continue
        if not isinstance(external, Mapping):
            raise ValueError("python toolchain has a malformed external source")
        # Eligibility consumes editable-source ownership. The complete package
        # inventory is already bound by its digest and retained in the CAS blob.
        row = {
            key: distribution.get(key)
            for key in (
                "name",
                "version",
                "file_manifest_sha256",
                "direct_url_sha256",
                "record_sha256",
            )
            if distribution.get(key) is not None
        }
        row["external_source"] = dict(external)
        compact_distributions.append(row)
    environment_summary["distribution_count"] = len(distributions)
    environment_summary["distributions"] = compact_distributions
    external_roots = environment.get("external_roots")
    environment_summary["external_roots"] = (
        [dict(row) for row in external_roots if isinstance(row, Mapping)]
        if isinstance(external_roots, list)
        else []
    )
    compact["environment"] = environment_summary
    location = identity.get("location")
    if not isinstance(location, Mapping):
        raise ValueError("python toolchain has no pre-arm location receipt")
    compact["location"] = dict(location)
    compact["executable"] = location.get("selected_executable")
    compact["implementation"] = environment.get("implementation")
    compact["version"] = environment.get("version")
    profile = identity.get("inventory_profile")
    if not isinstance(profile, Mapping):
        raise ValueError("python toolchain has no capture profile")
    compact["inventory_profile"] = dict(profile)
    compact.update(_compact_process_inventory(identity))
    return compact


def compact_toolchains(toolchains: Mapping[str, object]) -> dict[str, object]:
    summaries: dict[str, object] = {}
    for name, raw in toolchains.items():
        if not isinstance(raw, Mapping):
            raise ValueError(f"toolchain {name!r} has malformed identity")
        if name == "python":
            summaries[name] = _compact_python(cast(Mapping[str, object], raw))
        else:
            summary = {
                key: value
                for key, value in raw.items()
                if not isinstance(value, (dict, list))
            }
            summary.update(_compact_process_inventory(raw))
            summaries[name] = summary
    return summaries


def publish_capture(
    cas_root: Path, toolchains: Mapping[str, object]
) -> tuple[dict[str, object], dict[str, object], dict[str, object]]:
    started = time.perf_counter()
    files = frozen_files(toolchains)
    artifact_payload: dict[str, object] = {
        "schema": custody_cas.ARTIFACT_SCHEMA,
        "kind": CAPTURE_SCHEMA,
        "toolchains": dict(toolchains),
        "files": [row.as_dict() for row in files],
    }
    reference = custody_cas.put_json(cas_root, artifact_payload).as_dict()
    summaries = compact_toolchains(toolchains)
    telemetry = {
        "schema": "molt.proof-toolchain-capture-telemetry.v1",
        "full_capture_count": 1,
        "frozen_file_count": len(files),
        "artifact_compressed_bytes": reference["compressed_bytes"],
        "artifact_uncompressed_bytes": reference["uncompressed_bytes"],
        "publish_s": time.perf_counter() - started,
    }
    return summaries, reference, telemetry


def load_capture(
    reference: Mapping[str, object], *, cas_root: Path
) -> dict[str, object]:
    payload = custody_cas.read_ref(reference, expected_root=cas_root)
    if payload.get("kind") != CAPTURE_SCHEMA:
        raise ValueError("proof toolchain capture artifact kind mismatch")
    toolchains = payload.get("toolchains")
    files = payload.get("files")
    if not isinstance(toolchains, dict) or not isinstance(files, list):
        raise ValueError("proof toolchain capture artifact is incomplete")
    return payload


def _rehash(
    row: Mapping[str, object],
) -> tuple[str, str | None, int | None, str | None]:
    raw_path = row.get("path")
    expected = row.get("sha256")
    if not isinstance(raw_path, str) or not isinstance(expected, str):
        return str(raw_path), None, None, "malformed-row"
    path = Path(raw_path)
    try:
        stat = path.stat()
        with path.open("rb") as stream:
            actual = hashlib.file_digest(stream, "sha256").hexdigest()
    except OSError as exc:
        return raw_path, None, None, type(exc).__name__
    return raw_path, actual, stat.st_size, None


def verify_capture(
    reference: Mapping[str, object], *, workers: int, cas_root: Path
) -> dict[str, object]:
    started = time.perf_counter()
    payload = load_capture(reference, cas_root=cas_root)
    rows = payload["files"]
    assert isinstance(rows, list)
    if workers < 1:
        raise ValueError("toolchain verification workers must be positive")
    with ThreadPoolExecutor(max_workers=workers) as executor:
        actual = list(executor.map(_rehash, rows))
    mismatches: list[dict[str, object]] = []
    bytes_hashed = 0
    for row, (path, digest, size, error) in zip(rows, actual, strict=True):
        assert isinstance(row, Mapping)
        if isinstance(size, int):
            bytes_hashed += size
        expected_size = row.get("size")
        if (
            error is not None
            or digest != row.get("sha256")
            or (isinstance(expected_size, int) and size != expected_size)
        ):
            mismatches.append(
                {
                    "path": path,
                    "expected_sha256": row.get("sha256"),
                    "actual_sha256": digest,
                    "expected_size": expected_size,
                    "actual_size": size,
                    "error": error,
                }
            )
    material = {
        "capture_semantic_sha256": reference.get("semantic_sha256"),
        "verified_file_count": len(rows),
        "bytes_hashed": bytes_hashed,
        "mismatches": mismatches,
    }
    return {
        "schema": VERIFICATION_SCHEMA,
        **material,
        "stable": not mismatches,
        "verification_s": time.perf_counter() - started,
        "identity_sha256": hashlib.sha256(
            json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest(),
    }
