"""Rust-owned host tool lookup for compiler-selected linker commands.

Rust 1.99 Session::get_tools_search_paths uses each selected sysroot's
lib/rustlib/<compiler host>/bin, not the compilation target's bin directory.
Keep this lookup separate from PATH discovery and preserve its provenance.
"""

from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path
import re
import sys
import tomllib
from typing import Iterator, Mapping, Sequence

from molt.exact_json import string_keyed_mapping
from molt.toolchain_identity import (
    executable_environment_value,
    find_executable,
    resolve_executable,
    stable_executable_probe,
)


def cargo_configuration_value(
    document: Mapping[str, object], *keys: str
) -> object | None:
    """Read one Cargo configuration path without selecting or probing tools."""
    value: object = document
    for key in keys:
        if not isinstance(value, Mapping) or key not in value:
            return None
        table = string_keyed_mapping(value)
        if table is None:
            raise ValueError(f"runtime Cargo {key} must be a string-keyed table")
        value = table[key]
    return value


def cargo_selected_value(
    config: Mapping[str, object],
    cli: Mapping[str, object],
    env: Mapping[str, str],
    keys: tuple[str, ...],
    names: tuple[str, ...],
    default: object = None,
) -> object:
    """Apply Cargo command-line, environment, configuration, then default priority."""
    cli_value = cargo_configuration_value(cli, *keys)
    if cli_value is not None:
        return cli_value
    for name in names:
        if name in env:
            return env[name]
    value = cargo_configuration_value(config, *keys)
    return default if value is None else value


def rust_channel(data: bytes) -> str:
    """Return the exact ``X.Y.Z`` channel a ``rust-toolchain.toml`` pins.

    The repository's ``rust-toolchain.toml`` is the one Rust version authority;
    CI setup, the toolchain contract check and release builds all read it here.
    """
    try:
        channel = tomllib.loads(data.decode("utf-8"))["toolchain"]["channel"]
    except (KeyError, TypeError, UnicodeError, tomllib.TOMLDecodeError) as exc:
        raise ValueError("rust-toolchain.toml has no [toolchain] channel") from exc
    if not isinstance(channel, str) or re.fullmatch(r"\d+\.\d+\.\d+", channel) is None:
        raise ValueError(
            f"rust-toolchain.toml must pin an exact X.Y.Z channel, got {channel!r}"
        )
    return channel


class RustupProxyUnavailable(ValueError):
    """A selected Rustup proxy could not provide the requested component."""

    def __init__(self, role: str, diagnostic: dict[str, object]) -> None:
        super().__init__(f"rustup {role} selection failed: {diagnostic['stderr']}")
        self.diagnostic = diagnostic


def resolve_rustup_proxy(
    path: Path, *, role: str, root: Path, env: Mapping[str, str]
) -> Path:
    """Resolve a content-proven rustup proxy, preserving custom tool binaries."""
    name = "rustup.exe" if os.name == "nt" else "rustup"
    candidates = tuple(
        candidate
        for candidate in dict.fromkeys(
            (path.parent / name, path.resolve(strict=True).parent / name)
        )
        if candidate.is_file()
    )
    if not candidates:
        return path
    with stable_executable_probe(path, label=f"Rust {role} selection") as (
        _,
        selected,
    ):
        for rustup in candidates:
            with stable_executable_probe(rustup, label="rustup selection") as (
                entrypoint,
                proxy,
            ):
                if selected.sha256 != proxy.sha256:
                    continue
                from molt import process_guard

                result = process_guard.run_completed_command(
                    [os.fspath(entrypoint), "which", role],
                    cwd=root,
                    env=dict(env),
                    check=False,
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    timeout=30,
                    memory_guard_prefix=None,
                )
                if result.returncode != 0:
                    raise RustupProxyUnavailable(
                        role,
                        {
                            "phase": "rustup-component-selection",
                            "unit": role,
                            "argv": [os.fspath(entrypoint), "which", role],
                            "cwd": os.fspath(root),
                            "returncode": result.returncode,
                            "stdout": result.stdout,
                            "stderr": result.stderr,
                        },
                    )
                value = result.stdout.strip()
                if (
                    not value
                    or "\n" in value
                    or "\r" in value
                    or not Path(value).is_absolute()
                ):
                    raise ValueError(
                        f"rustup {role} selector must print one absolute path"
                    )
                return resolve_executable(
                    value, environment=env, label=f"selected Rust {role}"
                )
    return path


# dyld's search list when DYLD_FALLBACK_LIBRARY_PATH is unset.
_DYLD_DEFAULT_FALLBACK = ("~/lib", "/usr/local/lib", "/usr/lib")


def rust_toolchain_library_environment(
    rustc: Path, env: Mapping[str, str]
) -> dict[str, str]:
    """The library path a Rust toolchain needs when run without rustup.

    Rustup's proxies put ``<toolchain>/lib`` on the dynamic-library search
    path; Molt runs the resolved binaries directly, so it does the same.
    Rust 1.99's macOS ``rust-lld`` loads ``libLLVM.dylib`` from that
    directory and carries no rpath to it (the Linux build has one), so every
    wasm link fails without it. ``<rustc>/../../lib`` is the directory rustc
    derives its own default sysroot from.
    """
    if sys.platform != "darwin":
        return {}
    library = Path(rustc).resolve(strict=True).parent.parent / "lib"
    current = env.get("DYLD_FALLBACK_LIBRARY_PATH")
    fallback = (
        current.split(os.pathsep)
        if current
        else [os.path.expanduser(entry) for entry in _DYLD_DEFAULT_FALLBACK]
    )
    if os.fspath(library) in fallback:
        return {}
    return {
        "DYLD_FALLBACK_LIBRARY_PATH": os.pathsep.join([os.fspath(library), *fallback])
    }


def rustc_host(version: str) -> str:
    hosts = re.findall(r"(?m)^host:\s*([A-Za-z0-9_.-]+)\s*$", version)
    if len(hosts) != 1:
        raise ValueError("selected rustc has no unique compiler host triple")
    return hosts[0]


def rustc_printed_sysroot(stdout: str, *, cwd: Path) -> Path:
    lines = [line.strip() for line in stdout.splitlines() if line.strip()]
    if len(lines) != 1:
        raise ValueError("selected rustc has no unique printed sysroot")
    path = Path(lines[0])
    path = path if path.is_absolute() else cwd / path
    if not path.is_dir():
        raise ValueError("selected rustc printed an unavailable sysroot directory")
    return path.resolve(strict=True)


def cargo_config_arguments(command: Sequence[str], *, cwd: Path) -> list[str]:
    """Retain Cargo's invocation-relative --config files and inline TOML."""
    result: list[str] = []
    index = 1
    while index < len(command) and command[index] != "--":
        value = command[index]
        if value == "--config":
            index += 1
            if index == len(command):
                raise ValueError("Cargo --config requires a value")
            argument = command[index]
        elif value.startswith("--config="):
            argument = value.split("=", 1)[1]
        else:
            index += 1
            continue
        if "=" not in argument:
            path = Path(argument)
            path = path if path.is_absolute() else cwd / path
            if not path.is_file():
                raise ValueError(f"Cargo --config file is unavailable: {path}")
            argument = str(path.absolute())
        result.extend(("--config", argument))
        index += 1
    return result


def cargo_configuration_paths(root: Path, env: Mapping[str, str]) -> tuple[Path, ...]:
    """Cargo's conventional configuration files, in low-to-high priority order.

    Explicit --config files are separate invocation inputs. An empty CARGO_HOME
    selects the default home, and Cargo does not expand a literal tilde.
    """
    home_value = executable_environment_value(env, "CARGO_HOME")
    if home_value:
        home = Path(home_value)
    else:
        profile = executable_environment_value(
            env, "USERPROFILE" if os.name == "nt" else "HOME"
        )
        home = (Path(profile) if profile else Path.home()) / ".cargo"
    if not home.is_absolute():
        home = root / home
    locations = (home, *(path / ".cargo" for path in reversed((root, *root.parents))))
    paths: list[Path] = []
    seen: set[Path] = set()
    for location in locations:
        location = location.resolve(strict=False)
        if location in seen:
            continue
        seen.add(location)
        for name in ("config", "config.toml"):
            candidate = location / name
            if candidate.is_file():
                paths.append(candidate)
                break
    return tuple(paths)


@dataclass(frozen=True, slots=True)
class RustFlagSpan:
    """An exact Rust argument span with any preceding short no-value flags.

    Values retain their original bytes. ``codegen`` is a semantic projection;
    original argv and relative-tool indexes never use that projection.
    """

    start: int
    stop: int
    option: str | None = None
    value: str | None = None
    leading: tuple[str, ...] = ()

    @property
    def codegen(self) -> str | None:
        return (
            canonical_rust_codegen_option(self.value)
            if self.option == "-C" and self.value is not None
            else None
        )


def canonical_rust_codegen_option(value: str) -> str:
    """rustc accepts underscores and hyphens in keys, not rewritten values."""
    key, separator, operand = value.partition("=")
    return key.replace("_", "-") + separator + operand


# rustc 1.99 --help -v outer arity, not a codegen option-name registry.
# -Z retains the existing unstable-resource lane. A value-taking short option
# ends a cluster; preceding no-value flags remain part of the raw argument.
_RUST_VALUE_OPTIONS = frozenset(
    {
        "--cfg",
        "--check-cfg",
        "--crate-type",
        "--crate-name",
        "--edition",
        "--emit",
        "--print",
        "--sysroot",
        "--target",
        "--extern",
        "--out-dir",
        "--explain",
        "--color",
        "--error-format",
        "--json",
        "--diagnostic-width",
        "--remap-path-prefix",
        "--remap-path-scope",
        "--cap-lints",
        "--force-warn",
        "--allow",
        "--warn",
        "--deny",
        "--forbid",
    }
)
_RUST_SHORT_VALUE_OPTIONS = frozenset("CLloAWDFZ")
_RUST_SHORT_FLAGS = frozenset("hVvgO")


def rust_flag_spans(arguments: Sequence[str]) -> Iterator[RustFlagSpan]:
    """Consume rustc's lexical option boundaries without shell tokenization."""
    index = 0
    while index < len(arguments):
        start = index
        argument = arguments[index]
        index += 1
        if argument == "--":
            yield RustFlagSpan(start, len(arguments))
            return
        option = value = None
        leading: tuple[str, ...] = ()
        if argument.startswith("--"):
            key, separator, operand = argument.partition("=")
            if key == "--codegen" or key in _RUST_VALUE_OPTIONS:
                option = "-C" if key == "--codegen" else key
                if separator:
                    value = operand
        elif argument.startswith("-"):
            for offset, character in enumerate(argument[1:], start=1):
                if character in _RUST_SHORT_FLAGS:
                    continue
                if character in _RUST_SHORT_VALUE_OPTIONS:
                    option = "-" + character
                    leading = (argument[:offset],) if offset > 1 else ()
                    value = argument[offset + 1 :] or None
                break  # A value consumes the rest; unknown options stay rustc-owned.
        if option is not None and value is None:
            if index == len(arguments):
                label = "a codegen option" if option == "-C" else "an operand"
                raise ValueError(f"Rust {option} requires {label}")
            value = arguments[index]
            index += 1
        if option == "-C" and (not value or value.startswith(("=", "-"))):
            raise ValueError(f"invalid Rust codegen option: {argument!r}")
        yield RustFlagSpan(start, index, option, value, leading)


def canonical_rust_codegen_flags(arguments: Sequence[str]) -> tuple[str, ...]:
    """Project codegen spelling only; preserve operand bytes and other tokens."""
    result: list[str] = []
    for span in rust_flag_spans(arguments):
        result.extend(
            (*span.leading, "-C", span.codegen)
            if span.codegen is not None
            else arguments[span.start : span.stop]
        )
    return tuple(result)


def relative_rustc_tool_paths(arguments: Sequence[str]) -> list[tuple[int, str, str]]:
    """Find tool operands at their original indexes without Cargo precedence."""
    result = []
    for span in rust_flag_spans(arguments):
        index = span.start
        argument = arguments[index]
        prefix = ""
        if span.codegen is not None:
            if not span.codegen.startswith("linker="):
                continue
            index = span.stop - 1
            path = span.codegen[len("linker=") :]
            prefix = arguments[index][: len(arguments[index]) - len(path)]
            if not any(separator in path for separator in "/\\"):
                continue  # Bare linker names select a tool, not a relative path.
        elif argument == "--":
            break
        elif span.option == "--sysroot":
            assert span.value is not None
            index, path = span.stop - 1, span.value
            prefix = arguments[index][: len(arguments[index]) - len(path)]
        else:
            continue
        if not Path(path).is_absolute():
            result.append((index, prefix, path))
    return result


@dataclass(frozen=True, slots=True)
class RustToolSearch:
    host: str
    selected_sysroot: Path
    compiler_sysroot: Path

    def directories(self) -> tuple[Path, ...]:
        return tuple(
            dict.fromkeys(
                root / "lib" / "rustlib" / self.host / "bin"
                for root in (self.selected_sysroot, self.compiler_sysroot)
            )
        )

    def resolve(
        self, executable: str, *, cwd: Path, env: Mapping[str, str]
    ) -> tuple[Path, dict[str, object]]:
        candidate = Path(executable)
        if candidate.is_absolute() or "/" in executable or "\\" in executable:
            path = find_executable(executable, cwd=cwd, environment=env)
            if path is None:
                raise ValueError(
                    f"selected explicit Rust process image is unavailable: {executable!r}"
                )
            origin = "explicit-path"
        else:
            # Tool suffix belongs to the compiler host even for a WASM target.
            name = executable
            if "windows" in self.host.split("-") and not name.lower().endswith(".exe"):
                name += ".exe"
            path = next(
                (root / name for root in self.directories() if (root / name).is_file()),
                None,
            )
            origin = "rust-sysroot-host-tool"
            if path is None:
                selected = find_executable(executable, cwd=cwd, environment=env)
                if selected is None:
                    raise ValueError(
                        f"selected Rust process image is unavailable: {executable!r}"
                    )
                path, origin = Path(selected), "process-path"
        # Invocation spelling selects driver mode (clang++ versus clang, etc.).
        # Image custody separately follows this entrypoint to its content bytes.
        path = path.absolute()
        if not path.is_file() or not os.access(path, os.X_OK):
            raise ValueError(f"selected Rust process image is not executable: {path}")
        return path, self._evidence(executable, path, origin)

    def bundled_lld(self, wrapper: Path) -> tuple[Path, dict[str, object]] | None:
        """Return the rust-lld image that a Rust `gcc-ld` wrapper executes.

        rustc adds `-B<host tool dir>/gcc-ld` for a self-contained linker (the
        x86_64-unknown-linux-gnu default since Rust 1.90), so the C driver or
        GCC's collect2 runs `gcc-ld/ld.lld`. rust-lang/rust
        src/tools/lld-wrapper derives its child from its own executable path:
        `<wrapper dir>/../rust-lld` plus the host suffix, exec'd on Unix and
        spawned on Windows. Any other path, including a `gcc-ld` entry linked
        to a system lld, selects no further image.
        """
        content = wrapper.resolve(strict=True)
        tool_directories = {
            directory.resolve()
            for directory in self.directories()
            if directory.is_dir()
        }
        windows = "windows" in self.host.split("-")
        name = content.name.casefold() if windows else content.name
        if windows and name.endswith(".exe"):
            name = name[: -len(".exe")]
        if (
            content.parent.name != "gcc-ld"
            or content.parent.parent not in tool_directories
            or name not in _RUST_LLD_WRAPPER_NAMES
        ):
            return None
        child = "rust-lld.exe" if windows else "rust-lld"
        target = content.parent.parent / child
        if not target.is_file() or not os.access(target, os.X_OK):
            raise ValueError(
                f"Rust lld wrapper {content} has no rust-lld beside its directory: "
                f"{target}"
            )
        # current_exe() is canonical on Linux but the invoked spelling on macOS
        # and Windows. Both spellings must select one image.
        lexical = wrapper.absolute().parent.parent / child
        if not lexical.is_file() or lexical.resolve() != target.resolve():
            raise ValueError(
                f"Rust lld wrapper {wrapper} selects {target} through its content "
                f"path but {lexical} through its invoked path"
            )
        return target, self._evidence(str(wrapper), target, "rust-lld-wrapper")

    def _evidence(self, requested: str, path: Path, origin: str) -> dict[str, object]:
        return {
            "requested": requested,
            "path": str(path),
            "content_path": str(path.resolve(strict=True)),
            "origin": origin,
            "compiler_host": self.host,
            "selected_sysroot": str(self.selected_sysroot),
            "compiler_sysroot": str(self.compiler_sysroot),
            "tool_search_directories": [str(value) for value in self.directories()],
        }


# The executable names rust-lang/rust src/tools/lld-wrapper maps to an lld flavor.
_RUST_LLD_WRAPPER_NAMES = frozenset({"ld.lld", "ld64.lld", "lld-link", "wasm-ld"})
