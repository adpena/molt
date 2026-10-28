"""Rust-owned host tool lookup for compiler-selected linker commands.

Rust 1.99 Session::get_tools_search_paths uses each selected sysroot's
lib/rustlib/<compiler host>/bin, not the compilation target's bin directory.
Keep this lookup separate from PATH discovery and preserve its provenance.
"""

from __future__ import annotations

from molt.process_environment import (
    NameRule,
    ProcessEnvironment,
    ProcessEnvironmentBuilder,
)

from contextlib import contextmanager
from dataclasses import dataclass
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import subprocess
import sys
import tomllib
from typing import Generator, Iterator, Mapping, Sequence, assert_never

from molt.exact_json import string_keyed_mapping
from molt.toolchain_facts import (
    RUSTUP_PROBE_ENVIRONMENT,
    FileInput,
    ProbeAnswer,
    ProbeKey,
    ProbeTool,
    completed_probe,
    environment_inputs,
    file_input,
    host_toolchain_facts,
)
from molt.toolchain_identity import (
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


@dataclass(frozen=True, slots=True)
class RustupProxySelection:
    """A content-proven rustup proxy and the rustup CLI that resolves it."""

    entrypoint: Path
    proxy: ProbeTool
    rustup: ProbeTool

    @contextmanager
    def fenced(self) -> Generator[Path, None, None]:
        """Yield the rustup CLI while both captured generations are current."""
        with (
            self.proxy.fenced(label="Rust proxy selection"),
            self.rustup.fenced(label="rustup selection") as entrypoint,
        ):
            yield entrypoint


def rustup_proxy_selection(path: Path) -> RustupProxySelection | None:
    """The named rustup CLI, only when its bytes match the selected proxy."""
    from molt import input_report

    name = "rustup.exe" if os.name == "nt" else "rustup"
    candidates = []
    for candidate in dict.fromkeys(
        (path.parent / name, path.resolve(strict=True).parent / name)
    ):
        input_report.report_probe(candidate)
        if candidate.is_file():
            candidates.append(candidate)
    if not candidates:
        return None
    with stable_executable_probe(path, label="Rust proxy selection") as (
        invoked,
        selected,
    ):
        if selected.path != path:
            input_report.report_probe(path)
        input_report.report_captured_file(
            selected.path, selected.stat_token(), selected.sha256
        )
        for rustup in candidates:
            with stable_executable_probe(rustup, label="rustup selection") as (
                entrypoint,
                proxy,
            ):
                input_report.report_captured_file(
                    proxy.path, proxy.stat_token(), proxy.sha256
                )
                if selected.sha256 == proxy.sha256:
                    return RustupProxySelection(
                        entrypoint,
                        ProbeTool.from_identity(invoked, selected),
                        ProbeTool.from_identity(entrypoint, proxy),
                    )
    return None


def _rustup_home_inputs(rustup_home: Path) -> tuple[FileInput, ...]:
    """What rustup reads under its home: settings, the installed toolchains
    and each toolchain's ``bin`` listing (a component that appears there
    changes which binary ``which`` selects, including the cargo fallback)."""
    toolchains = rustup_home / "toolchains"
    try:
        with os.scandir(toolchains) as iterator:
            names = sorted(entry.name for entry in iterator)
    except OSError:
        names = []
    return (
        file_input(rustup_home / "settings.toml"),
        file_input(toolchains),
        *(file_input(toolchains / name / "bin") for name in names),
    )


def _user_home(environment: Mapping[str, str], *, windows: bool) -> Path | None:
    """The home directory rustup reads (the ``home`` crate): ``USERPROFILE``
    on Windows and ``HOME`` elsewhere, when set and non-empty, else the
    account's own record; None when there is none (rustup then reports)."""
    value = environment.get("USERPROFILE" if windows else "HOME") or ""
    if value:
        return Path(value)
    if windows:
        # The crate asks the shell for the profile folder; Python's lookup is
        # its nearest equivalent (unverified on Windows).
        profile = os.path.expanduser("~")
        return None if profile == "~" else Path(profile)
    import pwd

    try:
        return Path(pwd.getpwuid(os.getuid()).pw_dir)
    except KeyError:
        return None


def rustup_home(
    environment: Mapping[str, str], cwd: Path, *, windows: bool = os.name == "nt"
) -> Path | None:
    """The rustup home rustup itself selects (``home::rustup_home_with_cwd``):
    ``RUSTUP_HOME`` when set and non-empty, joined to the working directory
    when relative, else ``.rustup`` in the user's home directory; None when
    there is no home."""
    configured = environment.get("RUSTUP_HOME") or ""
    if configured:
        home = Path(configured)
        return home if home.is_absolute() else cwd / home
    user = _user_home(environment, windows=windows)
    return None if user is None else user / ".rustup"


def rustup_fallback_settings(
    environment: Mapping[str, str], cwd: Path, *, windows: bool = os.name == "nt"
) -> Path | None:
    """The settings rustup falls back to on Unix for a default toolchain
    (rustup-rs ``config.rs``): ``RUSTUP_OVERRIDE_UNIX_FALLBACK_SETTINGS``
    (joined to the working directory when relative, as rustup opens it), else
    ``/etc/rustup/settings.toml``. Windows reads none."""
    if windows:
        return None
    configured = Path(
        environment.get("RUSTUP_OVERRIDE_UNIX_FALLBACK_SETTINGS")
        or "/etc/rustup/settings.toml"
    )
    return configured if configured.is_absolute() else cwd / configured


def _printed_executable(stdout: str) -> Path | None:
    """The one absolute path a selector printed, while it names a file."""
    value = stdout.strip()
    if not value or "\n" in value or "\r" in value or not Path(value).is_absolute():
        return None
    path = Path(value)
    return path if path.is_file() else None


def resolve_rustup_proxy(
    path: Path, *, role: str, root: Path, env: Mapping[str, str]
) -> Path:
    """Resolve a content-proven rustup proxy, preserving custom tool binaries.

    ``rustup which`` reads the explicit toolchain, the ``rust-toolchain``
    file chain from the root upward, its settings and the installed
    toolchains; the answer is keyed by all of them (rustup-rs ``config.rs``).
    """
    selection = rustup_proxy_selection(path)
    if selection is None:
        return path
    from molt import process_guard
    from molt import input_report

    environment = ProcessEnvironment(env)
    entrypoint = selection.entrypoint
    # rustup walks its working directory upward: the physical path on POSIX
    # (getcwd), the path as spelled on Windows (GetCurrentDirectoryW keeps a
    # junction or substituted drive). Key both chains; one extra lstat per
    # level of the spelled chain.
    physical_root = Path(os.path.realpath(root))
    directories = dict.fromkeys(
        (physical_root, *physical_root.parents, root, *root.parents)
    )
    toolchain_files = tuple(
        directory / name
        for directory in directories
        for name in ("rust-toolchain.toml", "rust-toolchain")
    )
    home = rustup_home(environment, physical_root)
    fallback = rustup_fallback_settings(environment, physical_root)
    consulted = (*toolchain_files, *(() if fallback is None else (fallback,)))
    if input_report.active():
        # rustup selects by toolchain files from the root upward and by
        # its settings; the record must see each lookup.
        input_report.report_file(Path(entrypoint))
        for file in consulted:
            input_report.report_data_file(file)
        if home is not None:
            input_report.report_data_file(home / "settings.toml")
    argv = ("which", role)
    key = ProbeKey(
        "rustup-which",
        (selection.proxy, selection.rustup),
        argv,
        os.fspath(root),
        environment_inputs(environment, RUSTUP_PROBE_ENVIRONMENT),
        (
            *(file_input(file) for file in consulted),
            # No home: the environment inputs key its absence; rustup reports.
            *(() if home is None else _rustup_home_inputs(home)),
        ),
    )

    def run() -> subprocess.CompletedProcess[str]:
        with selection.fenced():
            return process_guard.run_completed_command(
                [os.fspath(entrypoint), *argv],
                cwd=root,
                env=environment,
                check=False,
                capture_output=True,
                text=True,
                encoding="utf-8",
                timeout=30,
                memory_guard_prefix=None,
            )

    def current(answer: ProbeAnswer) -> bool:
        return _printed_executable(answer.stdout) is not None

    result, _cached = completed_probe(host_toolchain_facts(), key, run, accept=current)
    if result.returncode != 0:
        raise RustupProxyUnavailable(
            role,
            {
                "phase": "rustup-component-selection",
                "unit": role,
                "argv": [os.fspath(entrypoint), *argv],
                "cwd": os.fspath(root),
                "returncode": result.returncode,
                "stdout": result.stdout,
                "stderr": result.stderr,
            },
        )
    value = result.stdout.strip()
    if not value or "\n" in value or "\r" in value or not Path(value).is_absolute():
        raise ValueError(f"rustup {role} selector must print one absolute path")
    selected = resolve_executable(value, environment=env, label=f"selected Rust {role}")
    input_report.report_file(selected)
    return selected


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


def _platform_environment(env: Mapping[str, str]) -> ProcessEnvironment:
    """``env`` under the platform this module selects Cargo's tools on.

    A typed environment keeps its rule. A plain mapping takes the rule of the
    ``os`` that decides the PATH grammar and the home variable here, so a
    captured Windows selection reads ``Path`` the way Cargo does on Windows.
    """
    if isinstance(env, (ProcessEnvironment, ProcessEnvironmentBuilder)):
        return ProcessEnvironment(env)
    return ProcessEnvironment(
        env, rule=NameRule.WINDOWS if os.name == "nt" else NameRule.EXACT
    )


def cargo_home_path(root: Path, env: Mapping[str, str]) -> Path:
    """Cargo home selection shared by config and external-subcommand lookup."""
    environment = _platform_environment(env)
    home_value = environment.get("CARGO_HOME", "")
    if home_value:
        home = Path(home_value)
    else:
        profile = environment.get("USERPROFILE" if os.name == "nt" else "HOME", "")
        home = (Path(profile) if profile else Path.home()) / ".cargo"
    return home if home.is_absolute() else root / home


def cargo_external_subcommand_path(
    command: str, *, cwd: Path, env: Mapping[str, str]
) -> Path:
    """Cargo 1.99 search_directories: home/bin first unless already on PATH.

    Keep lexical PATH order, including a deliberately lower-priority Cargo home;
    resolving aliases before the membership check changes Cargo's selection.
    """
    environment = _platform_environment(env)
    path_value = environment.get("PATH", "")
    directories: list[Path] = []
    if "PATH" in environment:
        # Rust's Windows split_paths ignores quotes and splits semicolons only
        # outside them. Cargo adds no implicit cwd beyond an empty PATH entry.
        if os.name == "nt":
            chunks, current, quoted = [], [], False
            for char in path_value:
                if char == '"':
                    quoted = not quoted
                elif char == ";" and not quoted:
                    chunks.append("".join(current))
                    current = []
                else:
                    current.append(char)
            chunks.append("".join(current))
        else:
            chunks = path_value.split(":")
        directories = [Path(value) for value in chunks]
    home_bin = cargo_home_path(cwd, environment) / "bin"
    if str(home_bin) not in {str(directory) for directory in directories}:
        directories.insert(0, home_bin)
    name = f"cargo-{command}" + (".exe" if os.name == "nt" else "")
    for directory in directories:
        candidate = directory / name
        if not candidate.is_absolute():
            candidate = cwd / candidate
        if candidate.is_file() and (os.name == "nt" or os.access(candidate, os.X_OK)):
            return candidate
    raise ValueError(f"Cargo requires an available {name} executable before capture")


def rust_component_executables(component: str) -> tuple[str, ...]:
    """Installed component executable closure, not just its version wrapper."""
    return {
        "rustfmt": ("rustfmt",),
        "clippy": ("cargo-clippy", "clippy-driver"),
        "miri": ("cargo-miri", "miri"),
    }[component]


def rust_component_version_arguments(tool: str) -> tuple[str, ...]:
    return {
        "rustfmt": ("--version",),
        "cargo-clippy": ("--version",),
        "clippy-driver": ("--rustc", "--version", "--verbose"),
        "cargo-miri": ("miri", "--version"),
        "miri": ("--version", "--verbose"),
    }[tool]


def rust_component_version_matches(
    tool: str, output: str, compiler_fields: Mapping[str, str]
) -> bool:
    if tool in {"clippy-driver", "miri"}:
        fields = dict(
            line.split(": ", 1) for line in output.splitlines() if ": " in line
        )
        return all(
            compiler_fields.get(key) and fields.get(key) == compiler_fields[key]
            for key in ("host", "release", "commit-hash")
        )
    name = {"rustfmt": "rustfmt", "cargo-clippy": "clippy", "cargo-miri": "miri"}[tool]
    return (
        re.fullmatch(
            rf"{name} [0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?"
            r"(?: \([0-9a-f]{7,40} [0-9]{4}-[0-9]{2}-[0-9]{2}\))?",
            output.strip(),
        )
        is not None
    )


def cargo_configuration_paths(root: Path, env: Mapping[str, str]) -> tuple[Path, ...]:
    """Cargo's conventional configuration files, in low-to-high priority order.

    Explicit --config files are separate invocation inputs. An empty CARGO_HOME
    selects the default home, and Cargo does not expand a literal tilde. Each
    lookup is reported to a recording build: creating a configuration file
    changes what Cargo builds.
    """
    from molt import input_report

    home = cargo_home_path(root, env)
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
            input_report.report_data_file(candidate)
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


@dataclass(frozen=True, slots=True)
class RustLogicalRemapSource:
    label: str
    suffix: str


@dataclass(frozen=True, slots=True)
class RustLiteralRemapSource:
    value: str


@dataclass(frozen=True, slots=True)
class RustPathRemap:
    """rustc's textual FROM=TO operation, never a resolved filesystem path."""

    source: str
    destination: str

    @classmethod
    def parse(cls, value: str) -> RustPathRemap:
        source, separator, destination = value.rpartition("=")
        if not separator:
            raise ValueError("Rust --remap-path-prefix requires FROM=TO")
        return cls(source, destination)

    @property
    def source_is_absolute(self) -> bool:
        return (
            PureWindowsPath(self.source).is_absolute()
            or self.source.startswith("\\")
            or PurePosixPath(self.source).is_absolute()
        )

    def project(
        self, logical_roots: Sequence[tuple[str, str]]
    ) -> RustLogicalRemapSource | RustLiteralRemapSource:
        # Preserve the existing owned-root boundary, spelling and longest-root
        # precedence. In particular, '..', quotes and case are semantic text.
        if self.source_is_absolute:
            for label, root in sorted(
                logical_roots, key=lambda item: len(item[1]), reverse=True
            ):
                if self.source == root or self.source.startswith(root + os.sep):
                    return RustLogicalRemapSource(label, self.source[len(root) :])
        return RustLiteralRemapSource(self.source)


def rust_flag_remap_identity(
    flags: Sequence[str], logical_roots: Sequence[tuple[str, str]]
) -> list[dict[str, object]]:
    """Bind host flags exactly except for typed owned remap source prefixes.

    Unknown paths remain literal inputs. This is not portable runtime flag
    admission: cfg values, response-looking text and other operands stay exact.
    The existing lexical parser owns option values and the '--' terminator.
    """
    if isinstance(flags, str):
        raise TypeError("Rust flag vectors are token sequences, not strings")
    result: list[dict[str, object]] = []
    for span in rust_flag_spans(flags):
        if span.option != "--remap-path-prefix":
            result.append(
                {"kind": "literal", "tokens": list(flags[span.start : span.stop])}
            )
            continue
        remap = RustPathRemap.parse(span.value or "")
        source = remap.project(logical_roots)
        source_record: dict[str, str]
        if isinstance(source, RustLogicalRemapSource):
            source_record = {
                "kind": "logical-root",
                "label": source.label,
                "suffix": source.suffix,
            }
        elif isinstance(source, RustLiteralRemapSource):
            source_record = {"kind": "literal", "value": source.value}
        else:
            assert_never(source)
        result.append(
            {"kind": "remap", "source": source_record, "destination": remap.destination}
        )
    return result


def rust_remap_path_flags(mappings: Sequence[tuple[Path, str]]) -> tuple[str, ...]:
    """Encode ordered rustc prefix mappings without shell or path rewriting.

    rustc splits at the last '=' and the last matching mapping wins. The
    default scope includes both diagnostics and object metadata. Keep these
    tokens intact through Cargo's encoded flag transport, including spaces.
    """
    for source, destination in mappings:
        if not os.fspath(source) or "=" in destination or "\0" in destination:
            raise ValueError("invalid Rust path remapping")
    return tuple(
        f"--remap-path-prefix={source}={destination}"
        for source, destination in mappings
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

# Rustdoc 1.99 stable option arity (librustdoc/lib.rs::opts). This extends
# lexical boundaries only when the caller selects the documenter grammar.
_RUSTDOC_VALUE_OPTIONS = frozenset(
    {
        "--output-format",
        "--output",
        "--library-path",
        "--test-args",
        "--test-run-directory",
        "--markdown-css",
        "--html-in-header",
        "--html-before-content",
        "--html-after-content",
        "--markdown-playground-url",
        "--extend-css",
        "--crate-version",
        "--default-theme",
        "--theme",
        "--check-theme",
        "--test-runtool",
        "--test-runtool-arg",
        "--plugin-path",
        "--passes",
        "--plugins",
        "--input-format",
    }
)


def rust_flag_spans(
    arguments: Sequence[str], *, documenter: bool = False
) -> Iterator[RustFlagSpan]:
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
            if (
                key == "--codegen"
                or key in _RUST_VALUE_OPTIONS
                or documenter
                and key in _RUSTDOC_VALUE_OPTIONS
            ):
                option = "-C" if key == "--codegen" else key
                if separator:
                    value = operand
        elif argument.startswith("-"):
            for offset, character in enumerate(argument[1:], start=1):
                if character in _RUST_SHORT_FLAGS:
                    continue
                if (
                    character in _RUST_SHORT_VALUE_OPTIONS
                    or documenter
                    and character in "wer"
                ):
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
