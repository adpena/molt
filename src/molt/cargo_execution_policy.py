"""Shared Cargo subprocess, resource, and timeout authority."""

from __future__ import annotations

# tools/command_execution.py loads this file under a per-checkout neutral
# package (tools/import_file.py require_repository_identity, HF-223).
__molt_neutral_authority__ = True

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
import hashlib
import math
import os
from pathlib import Path
import re
import tomllib

from .disk_capacity import (
    DiskCapacityReceipt,
    FreeSpaceMeasurement,
    require_build_capacity,
)
from .source_root import compiler_source_root
from .process_environment import NameRule, ProcessEnvironment, ProcessEnvironmentBuilder


CI_CARGO_POLICY_SCHEMA = "molt.ci-resource-policy.v2"
SCCACHE_INCREMENTAL_POLICY = "sccache-disables-incremental"
SCCACHE_SERVER_TEMP_POLICY = "sccache-server-temp-dir"
SCCACHE_SERVER_ENDPOINT_POLICY = "sccache-server-private-endpoint"
SCCACHE_SERVER_ENDPOINT_ENV_NAMES = ("SCCACHE_SERVER_UDS", "SCCACHE_SERVER_PORT")
# sun_path holds 104 bytes on macOS and 108 on Linux, terminator included.
_UNIX_SOCKET_PATH_LIMIT = 103
# The pinned sccache serves Unix sockets on POSIX hosts only.
_SCCACHE_SERVER_SOCKETS = os.name != "nt"
# A client that starts the server waits for it on `<temp dir>/sccacheXXXXXX/sock`
# (a tempfile directory with the prefix "sccache"), 19 bytes past the temp dir.
_SCCACHE_STARTUP_SOCKET_SUFFIX_BYTES = len("/sccacheXXXXXX/sock")
TEMPORARY_DIRECTORY_ENV_NAMES = ("TMPDIR", "TMP", "TEMP")
DIRECT_RUSTC_INCREMENTAL_POLICY = "direct-rustc-enables-incremental"
CARGO_WRAPPER_ENV_NAMES = (
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
)
AWS_LC_SYS_ENV_PREFIX = "AWS_LC_SYS_"
CARGO_NATIVE_DEPENDENCY_ENV_PREFIXES = (
    AWS_LC_SYS_ENV_PREFIX,
    "HOST_CC_",
    "TARGET_CC_",
    "HOST_CXX_",
    "TARGET_CXX_",
    "HOST_CFLAGS_",
    "TARGET_CFLAGS_",
)
PROOF_COMMAND_TIMEOUT_ENV = "MOLT_PROOF_COMMAND_TIMEOUT_SEC"
NESTED_PROCESS_BUDGET_BY_ROLE = {
    "build": "cold",
    "execution": "warm",
}


def cargo_native_dependency_environment_name(name: str) -> bool:
    """Output-bearing upstream native dependency controls, including targets."""
    return NameRule.host().key(name).startswith(CARGO_NATIVE_DEPENDENCY_ENV_PREFIXES)


def validate_cargo_native_dependency_environment(
    environ: Mapping[str, str],
    *,
    target: str | None = None,
    compiler_host: str | None = None,
    native_bound: bool = False,
) -> None:
    """Refuse AWS-LC providers/tools outside the captured source/cc capability.

    The source default belongs only to .cargo/config.toml. Validate effective
    Cargo [env] values, never install defaults here. Target controls follow
    aws-lc-sys 0.45's exact lowercase, hyphen-to-underscore suffix. Other target
    values stay semantic identity inputs but do not select this build's tools.
    """
    if native_bound and (not target or not compiler_host):
        raise ValueError(
            "bound native dependency validation requires captured Rust target and host"
        )
    env = ProcessEnvironment(environ)
    prefix = AWS_LC_SYS_ENV_PREFIX
    suffix = "_" + target.lower().replace("-", "_") if target else None

    def selected(control: str, *, crate: bool = True) -> tuple[str, str] | None:
        name = (prefix if crate else "") + control
        specific = name + suffix if suffix else None
        if specific is not None:
            if specific in env:
                return specific, env[specific]
        elif any(env.rule.key(key).startswith(env.rule.key(name + "_")) for key in env):
            # An implicit Rust target is not known until captured compiler
            # metadata arrives. Do not mistake a possibly shadowed generic
            # value for the selected control; validate again with that target.
            return None
        return (name, env[name]) if name in env else None

    def boolean(value: str) -> bool | None:
        # Upstream parse_to_bool is prefix-based and does not trim whitespace.
        lowered = value.lower()
        if lowered.startswith(("0", "n", "off", "f")):
            return False
        if lowered.startswith(tuple("123456789")) or lowered.startswith(
            ("y", "on", "t")
        ):
            return True
        return None

    errors: dict[str, str] = {}
    provider_names = []
    for control in ("USE_SYSTEM", "SYSTEM_DIR"):
        item = selected(control)
        if item is None:
            continue
        name, value = item
        provider_names.append(name)
        if control == "SYSTEM_DIR" or boolean(value) is not False:
            errors[name] = "system/automatic provider has no native input custody"
    if errors:
        for name in provider_names:
            errors.setdefault(name, "part of the conflicting provider selection")
    if native_bound and compiler_host is not None and target is not None:
        normalized_target = target.lower().replace("-", "_")
        for control in ("CC", "CXX", "CFLAGS"):
            role = ("HOST_" if compiler_host == target else "TARGET_") + control
            item = (
                selected(role)
                or selected(role, crate=False)
                or selected(control)
                or selected(control, crate=False)
            )
            if item is None:
                continue
            raw = f"{control}_{target}"
            rewritten = f"{control}_{normalized_target}"
            # AWS-LC rewrites only the normalized selector. Published raw-target
            # CC/CXX remains first in cc-rs, so an ignored private driver is not
            # an additional capability. Flags accumulate every form, however;
            # changing that entry invalidates the captured ordered C witness.
            if control == "CFLAGS":
                existing = env.get(rewritten, "")
                modeled_names = {"CFLAGS", role}
                if item[1] != existing and (existing or item[0] not in modeled_names):
                    errors[item[0]] = (
                        "rewrites CFLAGS outside the captured ordered native flag projection"
                    )
            elif item[0].startswith(prefix) and (
                not env.get(raw)
                or (
                    env.rule.key(raw) == env.rule.key(rewritten) and item[1] != env[raw]
                )
            ):
                errors[item[0]] = (
                    "compiler override has no pinned raw-target native driver"
                )
    if (item := selected("INCLUDES")) is not None:
        errors[item[0]] = "private includes bypass captured native header custody"
    for control in (
        "CMAKE_BUILDER",
        "NO_PREGENERATED_SRC",
        "NO_PREFIX",
        "EXTERNAL_BINDGEN",
        "PREGENERATING_BINDINGS",
    ):
        if (item := selected(control)) is not None and boolean(item[1]) is True:
            errors[item[0]] = (
                "requires an unmodeled CMake/bindgen/source-generation capability"
            )
    if (item := selected("STATIC")) is not None and boolean(item[1]) is False:
        errors[item[0]] = "dynamic AWS-LC requires an unmodeled CMake capability"
    cmake = selected("CMAKE_BUILDER")
    if target is not None and (cmake is None or boolean(cmake[1]) is not False):
        if (item := selected("NO_ASM")) is not None and boolean(item[1]) is True:
            errors[item[0]] = "requires an unmodeled CMake capability"
        if (item := selected("SANITIZER")) is not None:
            errors[item[0]] = "requires an unmodeled CMake/sanitizer capability"
    if target is not None and (item := selected("EFFECTIVE_TARGET")) is not None:
        if item[1] and item[1] != target:
            errors[item[0]] = (
                "private target selection differs from captured Rust/C target"
            )
    if errors:
        details = "; ".join(f"{name}: {errors[name]}" for name in sorted(errors))
        raise ValueError(
            "Molt's captured AWS-LC build requires the pinned source provider and "
            f"native C tools; unsupported controls: {details}. "
            "Use captured CC/CXX/CFLAGS for native tool inputs. "
            "Custom raw Cargo provider cells require separate qualification."
        )


def source_build_disabled_reason(artifact: str) -> str | None:
    """Restrict source builds without changing artifact admission requirements.

    Call after compatible artifact reuse/hydration, before invoking Cargo.
    Provenance refresh through Cargo is also a source build, even if Cargo
    could discover that none of its inputs changed.
    """
    if os.environ.get("MOLT_SKIP_RUNTIME_REBUILD") != "1":
        return None
    return (
        f"{artifact} requires a source build or provenance refresh, but "
        "MOLT_SKIP_RUNTIME_REBUILD=1 disables source builds. "
        "Provide an admitted artifact for the selected inputs or unset "
        "MOLT_SKIP_RUNTIME_REBUILD to allow building from source."
    )


def _wrapper_is_sccache(wrapper: str) -> bool:
    name = wrapper.strip().strip('"').replace("\\", "/").rsplit("/", 1)[-1]
    return name.lower() in {"sccache", "sccache.exe"}


def cargo_compiler_wrappers(environ: Mapping[str, str]) -> tuple[tuple[str, str], ...]:
    return tuple(
        (name, value)
        for name in CARGO_WRAPPER_ENV_NAMES
        if (value := environ.get(name, "").strip())
    )


def sccache_compiler_wrappers(
    environ: Mapping[str, str],
) -> tuple[tuple[str, str], ...]:
    return tuple(
        (name, value)
        for name, value in cargo_compiler_wrappers(environ)
        if _wrapper_is_sccache(value)
    )


def sccache_server_temp_dir(environ: Mapping[str, str]) -> Path | None:
    """Return the temporary directory of the sccache server for ``SCCACHE_DIR``.

    The pinned sccache (0.18.0) has no temporary-directory setting. Its server
    takes ``std::env::temp_dir()`` (``TMPDIR``; ``TMP``/``TEMP`` on Windows)
    from whichever client process starts it, and it outlives that client. It
    writes each rustc dep-info probe below that directory. A client whose
    ``TMPDIR`` is one run's scratch therefore leaves a shared server whose
    temporary directory disappears with the scratch, and every later compile
    on the host fails with "Failed to create temp dir". All clients of one
    cache use this one durable directory beside the cache instead, so the
    server gets it whichever client starts it. It is not inside the cache:
    sccache's disk cache counts and evicts every file under ``SCCACHE_DIR``.

    Molt sets ``SCCACHE_DIR`` wherever it selects sccache; without it the
    cache, and its server, are not Molt's to configure.

    A client that starts the server waits for it on a Unix socket inside this
    directory, so the socket path must fit ``sun_path``. When the directory
    beside a deep cache would not, the server uses a durable directory named
    by the cache path under the user's cache home instead.
    """

    raw = environ.get("SCCACHE_DIR", "").strip()
    if not raw:
        return None
    cache = Path(os.path.abspath(Path(raw).expanduser()))
    if not cache.name:
        raise ValueError(f"SCCACHE_DIR must name a directory below a root: {raw!r}")
    beside = cache.with_name(f"{cache.name}-tmp")
    if not _SCCACHE_SERVER_SOCKETS or _startup_socket_fits(beside):
        return beside
    home = Path(environ.get("HOME") or os.path.expanduser("~"))
    digest = hashlib.sha256(os.fsencode(cache)).hexdigest()[:16]
    short = home / ".cache" / "molt" / "sccache-tmp" / digest
    if not _startup_socket_fits(short):
        raise ValueError(
            "sccache cannot start a server for SCCACHE_DIR "
            f"{cache}: its startup socket under {short} exceeds the "
            f"{_UNIX_SOCKET_PATH_LIMIT}-byte socket path limit"
        )
    return short


def _startup_socket_fits(temporary: Path) -> bool:
    return (
        len(os.fsencode(temporary)) + _SCCACHE_STARTUP_SOCKET_SUFFIX_BYTES
        <= _UNIX_SOCKET_PATH_LIMIT
    )


def sccache_server_endpoint(environ: Mapping[str, str]) -> tuple[str, str] | None:
    """Return the private server endpoint of ``SCCACHE_DIR``'s clients.

    sccache's default endpoint, TCP 127.0.0.1:4226, is shared by every sccache
    on the host. Any server listening there answers Molt's clients and runs
    their compiles in its own environment: on 2026-10-16 a Lima Linux guest
    forwarded its port 4226 to the macOS host, and every macOS rustc probe
    through sccache failed with "Compiler not supported". Each cache therefore
    gets its own Unix socket beside it, or, where a socket cannot hold the path
    or the platform has none, a port derived from the cache path. An endpoint
    the caller already chose is kept; without ``SCCACHE_DIR`` the server is not
    Molt's to configure.
    """

    if any(environ.get(name) for name in SCCACHE_SERVER_ENDPOINT_ENV_NAMES):
        return None
    raw = environ.get("SCCACHE_DIR", "").strip()
    if not raw:
        return None
    cache = Path(os.path.abspath(Path(raw).expanduser()))
    if not cache.name:
        return None
    if _SCCACHE_SERVER_SOCKETS:
        socket = cache.with_name(f"{cache.name}-server.sock")
        if len(os.fsencode(socket)) <= _UNIX_SOCKET_PATH_LIMIT:
            return "SCCACHE_SERVER_UDS", str(socket)
    digest = hashlib.sha256(os.fsencode(cache)).digest()
    # The IANA dynamic range, away from sccache's default.
    return "SCCACHE_SERVER_PORT", str(49152 + int.from_bytes(digest[:2]) % 16383)


def sccache_client_environment(
    environ: Mapping[str, str],
) -> tuple[dict[str, str], tuple[str, ...]]:
    """Return ``environ`` with the sccache server's temp dir and endpoint pinned.

    Every process that may start or reach the shared server (a Cargo child
    whose compiler wrapper is sccache, a rustc probe through that wrapper, or
    an sccache probe) runs with this environment, so the directory exists
    whenever the environment names it and every client reaches the same
    private server. The second value names the policies applied.
    """

    child = ProcessEnvironmentBuilder(environ)
    temporary = sccache_server_temp_dir(child)
    if temporary is None:
        return ProcessEnvironment(child).child_block(), ()
    temporary.mkdir(parents=True, exist_ok=True)
    for name in TEMPORARY_DIRECTORY_ENV_NAMES:
        child[name] = str(temporary)
    endpoint = sccache_server_endpoint(child)
    if endpoint is None:
        return ProcessEnvironment(child).child_block(), (SCCACHE_SERVER_TEMP_POLICY,)
    child[endpoint[0]] = endpoint[1]
    return ProcessEnvironment(child).child_block(), (
        SCCACHE_SERVER_TEMP_POLICY,
        SCCACHE_SERVER_ENDPOINT_POLICY,
    )


def normalize_cargo_environment(
    environ: Mapping[str, str] | None,
    *,
    default_incremental: str | None = None,
) -> tuple[dict[str, str], tuple[str, ...]]:
    """Return one executable Cargo environment and its applied policy names.

    Cargo accepts compiler wrappers through both direct Rust variables and
    ``build.*`` configuration environment variables.  Every subprocess boundary
    must inspect the complete family: checking only ``RUSTC_WRAPPER`` leaves
    probes and workspace wrappers able to inherit the invalid
    sccache-plus-incremental combination, or a caller's scratch ``TMPDIR`` that
    a server it starts would keep (see ``sccache_server_temp_dir``). When
    sccache is selected, normalization also prepares its durable temporary
    directory through ``sccache_client_environment``; it starts no process.
    """

    child = ProcessEnvironmentBuilder(os.environ if environ is None else environ)
    if sccache_compiler_wrappers(child):
        child["CARGO_INCREMENTAL"] = "0"
        selected, pinned = sccache_client_environment(child)
        return selected, (SCCACHE_INCREMENTAL_POLICY, *pinned)
    if default_incremental is not None and "CARGO_INCREMENTAL" not in child:
        child["CARGO_INCREMENTAL"] = default_incremental
        return ProcessEnvironment(child).child_block(), (
            DIRECT_RUSTC_INCREMENTAL_POLICY,
        )
    return ProcessEnvironment(child).child_block(), ()


def without_sccache_compiler_wrappers(
    environ: Mapping[str, str],
) -> dict[str, str]:
    child = ProcessEnvironmentBuilder(environ)
    for name, _value in sccache_compiler_wrappers(child):
        child.pop(name, None)
    return ProcessEnvironment(child).child_block()


def _executable_name(value: str) -> str:
    name = value.replace("\\", "/").rsplit("/", 1)[-1].lower()
    return name[:-4] if name.endswith(".exe") else name


def is_cargo_command(command: Sequence[str]) -> bool:
    if not command:
        return False
    if _executable_name(str(command[0])) == "cargo":
        return True
    if _executable_name(str(command[0])) != "rustup":
        return False
    return any(_executable_name(str(part)) == "cargo" for part in command[1:])


# Cargo subcommands that compile, so each one writes below a target directory.
CARGO_COMPILING_SUBCOMMANDS = frozenset(
    {
        "b",
        "bench",
        "build",
        "c",
        "check",
        "clippy",
        "d",
        "doc",
        "fix",
        "fuzz",
        "install",
        "llvm-cov",
        "miri",
        "nextest",
        "r",
        "run",
        "rustc",
        "rustdoc",
        "t",
        "test",
    }
)
_CARGO_GLOBAL_OPTIONS_WITH_VALUE = frozenset({"-C", "-Z", "--color", "--config"})
_CARGO_INFORMATION_FLAGS = frozenset({"-V", "--version", "-h", "--help"})


def cargo_compiles(command: Sequence[str]) -> bool:
    """Return whether a Cargo invocation compiles, and so needs admission."""

    parts = [str(part) for part in command]
    if not is_cargo_command(parts):
        return False
    index = 1 + next(
        position
        for position, part in enumerate(parts)
        if _executable_name(part) == "cargo"
    )
    while index < len(parts):
        part = parts[index]
        if part in _CARGO_GLOBAL_OPTIONS_WITH_VALUE:
            index += 2
        elif part.startswith(("+", "-")):
            index += 1
        else:
            options = parts[index + 1 :]
            if "--" in options:
                options = options[: options.index("--")]
            return part in CARGO_COMPILING_SUBCOMMANDS and not (
                _CARGO_INFORMATION_FLAGS.intersection(options)
            )
    return False


def _cargo_output_path(raw: str, *, cwd: Path, label: str) -> Path:
    if not isinstance(raw, str) or not raw.strip():
        raise ValueError(f"Cargo execution requires a non-empty {label}")
    path = Path(raw)
    if not path.is_absolute():
        path = cwd / path
    try:
        return path.resolve(strict=False)
    except (OSError, RuntimeError, ValueError) as exc:
        raise ValueError(f"cannot resolve Cargo {label} {raw!r}: {exc}") from exc


def _command_target_dirs(command: Sequence[str]) -> tuple[str, ...]:
    declarations: list[str] = []
    index = 1
    while index < len(command):
        argument = command[index]
        if argument == "--":
            break
        if argument == "--target-dir":
            index += 1
            if index >= len(command) or not command[index]:
                raise ValueError("Cargo --target-dir requires a non-empty path")
            declarations.append(command[index])
        elif argument.startswith("--target-dir="):
            value = argument.partition("=")[2]
            if not value:
                raise ValueError("Cargo --target-dir= requires a non-empty path")
            declarations.append(value)
        index += 1
    return tuple(declarations)


def cargo_output_paths(
    command: Sequence[str], *, cwd: Path, env: Mapping[str, str]
) -> tuple[Path, ...]:
    """Return the directories one Cargo invocation writes build output below.

    The target is ``--target-dir``, else ``CARGO_TARGET_DIR``; two declarations
    that disagree are refused. Without either, Cargo writes below the workspace
    that contains ``cwd``, on the same filesystem, so ``cwd`` stands for it.
    ``CARGO_BUILD_BUILD_DIR`` adds its own root.
    """

    command = [str(part) for part in command]
    declared = {
        _cargo_output_path(value, cwd=cwd, label="--target-dir")
        for value in _command_target_dirs(command)
    }
    raw_target = env.get("CARGO_TARGET_DIR")
    if raw_target is not None:
        declared.add(_cargo_output_path(raw_target, cwd=cwd, label="CARGO_TARGET_DIR"))
    if len(declared) > 1:
        raise ValueError(
            "Cargo --target-dir conflicts with the resolved CARGO_TARGET_DIR: "
            + " != ".join(str(path) for path in sorted(declared))
        )
    outputs = list(declared) or [_cargo_output_path(str(cwd), cwd=cwd, label="cwd")]
    if "CARGO_BUILD_BUILD_DIR" in env:
        outputs.append(
            _cargo_output_path(
                env["CARGO_BUILD_BUILD_DIR"], cwd=cwd, label="CARGO_BUILD_BUILD_DIR"
            )
        )
    return tuple(outputs)


def require_cargo_build_capacity(
    command: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    measure_free_bytes: FreeSpaceMeasurement | None = None,
) -> DiskCapacityReceipt:
    """Admit build capacity for every output root of one Cargo build.

    Call it before Cargo starts, for every invocation that compiles (build,
    check, clippy, test, run, rustc, doc, bench, miri, fuzz, llvm-cov). A
    refusal raises ``DiskCapacityError`` and no Cargo process starts.
    """

    return require_build_capacity(
        cargo_output_paths(command, cwd=cwd, env=env),
        env=env,
        measure_free_bytes=measure_free_bytes,
    )


def admit_cargo_build(
    command: Sequence[str],
    *,
    cwd: str | Path | None,
    env: Mapping[str, str] | None,
) -> DiskCapacityReceipt | None:
    """Admit build capacity just before a launcher starts ``command``.

    Launchers call this for every command they start; it admits only Cargo
    invocations that compile and returns ``None`` for any other command.
    """

    if not cargo_compiles(command):
        return None
    return require_cargo_build_capacity(
        command,
        cwd=Path.cwd() if cwd is None else Path(cwd),
        env=os.environ if env is None else env,
    )


def cargo_subprocess_environment(
    command: Sequence[str],
    environ: Mapping[str, str] | None,
    *,
    default_incremental: str | None = None,
) -> tuple[Mapping[str, str] | None, tuple[str, ...]]:
    if not is_cargo_command(command):
        return environ, ()
    return normalize_cargo_environment(
        environ,
        default_incremental=default_incremental,
    )


def proof_command_timeout_seconds(
    environ: Mapping[str, str],
) -> float | None:
    raw = environ.get(PROOF_COMMAND_TIMEOUT_ENV)
    if raw is None or not raw.strip():
        return None
    try:
        value = float(raw)
    except ValueError as exc:
        raise ValueError(
            f"{PROOF_COMMAND_TIMEOUT_ENV} must be a positive finite number"
        ) from exc
    if not math.isfinite(value) or value <= 0.0:
        raise ValueError(
            f"{PROOF_COMMAND_TIMEOUT_ENV} must be a positive finite number"
        )
    return value


def default_nested_process_timeout_seconds(role: str) -> float:
    try:
        budget_class = NESTED_PROCESS_BUDGET_BY_ROLE[role]
    except KeyError as exc:
        choices = ", ".join(sorted(NESTED_PROCESS_BUDGET_BY_ROLE))
        raise ValueError(
            f"unknown nested process role {role!r}; expected {choices}"
        ) from exc
    return float(load_ci_cargo_policy().execution_budgets.timeout_seconds(budget_class))


def enforce_owning_proof_timeout(
    environ: Mapping[str, str],
    selected_timeout: float | None,
) -> float | None:
    """Keep a default/env-selected inner guard from undercutting its owner.

    Callers resolve an explicit function argument before invoking this helper;
    those deliberately narrow operation bounds remain authoritative. All
    default and environment-selected nested budgets inherit the proof command's
    calibrated envelope as a floor, while still remaining bounded by that outer
    command owner.
    """

    owner_timeout = proof_command_timeout_seconds(environ)
    if owner_timeout is None:
        return selected_timeout
    if selected_timeout is None:
        return owner_timeout
    return max(selected_timeout, owner_timeout)


@dataclass(frozen=True, slots=True)
class CargoBuildResourcePolicy:
    max_jobs: int
    measured_peak_rss_bytes: int
    headroom_ratio: float
    measurement_run_id: int
    measurement_commit: str
    measurement_command: str

    @property
    def gb_per_job(self) -> float:
        measured_gib = self.measured_peak_rss_bytes / float(1024**3)
        return measured_gib * (1.0 + self.headroom_ratio)


@dataclass(frozen=True, slots=True)
class CargoExecutionBudgetPolicy:
    timeout_seconds_by_class: Mapping[str, int]
    observed_cold_timeout_seconds: float
    minimum_cold_headroom_multiplier: float
    measurement_run_id: int
    measurement_job_id: int
    measurement_commit: str
    measurement_command: str

    def timeout_seconds(self, budget_class: str) -> int:
        try:
            return self.timeout_seconds_by_class[budget_class]
        except KeyError as exc:
            choices = ", ".join(sorted(self.timeout_seconds_by_class))
            raise ValueError(
                f"unknown Cargo execution budget {budget_class!r}; expected {choices}"
            ) from exc


@dataclass(frozen=True, slots=True)
class CargoEnvironmentPolicy:
    wrapper_environment_names: tuple[str, ...]
    incident_run_id: int
    incident_job_id: int
    incident_commit: str
    incident_command: str


@dataclass(frozen=True, slots=True)
class CiCargoPolicy:
    build_resources: CargoBuildResourcePolicy
    environment: CargoEnvironmentPolicy
    execution_budgets: CargoExecutionBudgetPolicy


def _positive_int(value: object, *, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise ValueError(f"CI Cargo policy {label} must be a positive integer")
    return value


def _positive_float(value: object, *, label: str) -> float:
    if (
        not isinstance(value, (int, float))
        or isinstance(value, bool)
        or not math.isfinite(float(value))
        or float(value) <= 0.0
    ):
        raise ValueError(f"CI Cargo policy {label} must be positive and finite")
    return float(value)


def _measurement_commit(value: object, *, label: str) -> str:
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{40}", value) is None:
        raise ValueError(f"CI Cargo policy {label} must be a lowercase 40-hex SHA")
    return value


def _measurement_command(value: object, *, label: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"CI Cargo policy {label} must be a non-empty string")
    return value


def load_ci_cargo_policy(
    path: Path | None = None,
) -> CiCargoPolicy:
    path = (
        path
        if path is not None
        else compiler_source_root() / "config/ci_resource_policy.toml"
    )
    try:
        payload = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as exc:
        raise ValueError(f"cannot read CI Cargo policy {path}: {exc}") from exc
    if payload.get("schema") != CI_CARGO_POLICY_SCHEMA:
        raise ValueError(f"CI Cargo policy schema must be {CI_CARGO_POLICY_SCHEMA!r}")
    if set(payload) != {
        "schema",
        "cargo_build",
        "cargo_environment",
        "cargo_execution",
    }:
        raise ValueError(
            "CI Cargo policy top-level fields must be exactly "
            "schema, cargo_build, cargo_environment, cargo_execution"
        )

    resources = payload.get("cargo_build")
    resource_fields = {
        "max_jobs",
        "measured_peak_rss_bytes",
        "headroom_ratio",
        "measurement_run_id",
        "measurement_commit",
        "measurement_command",
    }
    if not isinstance(resources, dict) or set(resources) != resource_fields:
        raise ValueError(
            "CI Cargo policy cargo_build fields must be exactly "
            + ", ".join(sorted(resource_fields))
        )
    headroom_ratio = _positive_float(
        resources["headroom_ratio"], label="cargo_build.headroom_ratio"
    )
    if not 0.10 <= headroom_ratio <= 2.0:
        raise ValueError(
            "CI Cargo policy cargo_build.headroom_ratio must be within [0.10, 2.0]"
        )
    build_resources = CargoBuildResourcePolicy(
        max_jobs=_positive_int(resources["max_jobs"], label="cargo_build.max_jobs"),
        measured_peak_rss_bytes=_positive_int(
            resources["measured_peak_rss_bytes"],
            label="cargo_build.measured_peak_rss_bytes",
        ),
        headroom_ratio=headroom_ratio,
        measurement_run_id=_positive_int(
            resources["measurement_run_id"],
            label="cargo_build.measurement_run_id",
        ),
        measurement_commit=_measurement_commit(
            resources["measurement_commit"],
            label="cargo_build.measurement_commit",
        ),
        measurement_command=_measurement_command(
            resources["measurement_command"],
            label="cargo_build.measurement_command",
        ),
    )

    environment = payload.get("cargo_environment")
    environment_fields = {
        "incident_run_id",
        "incident_job_id",
        "incident_commit",
        "incident_command",
    }
    if not isinstance(environment, dict) or set(environment) != environment_fields:
        raise ValueError(
            "CI Cargo policy cargo_environment fields must be exactly "
            + ", ".join(sorted(environment_fields))
        )
    environment_policy = CargoEnvironmentPolicy(
        wrapper_environment_names=CARGO_WRAPPER_ENV_NAMES,
        incident_run_id=_positive_int(
            environment["incident_run_id"],
            label="cargo_environment.incident_run_id",
        ),
        incident_job_id=_positive_int(
            environment["incident_job_id"],
            label="cargo_environment.incident_job_id",
        ),
        incident_commit=_measurement_commit(
            environment["incident_commit"],
            label="cargo_environment.incident_commit",
        ),
        incident_command=_measurement_command(
            environment["incident_command"],
            label="cargo_environment.incident_command",
        ),
    )

    execution = payload.get("cargo_execution")
    execution_fields = {
        "cross_check_timeout_seconds",
        "warm_timeout_seconds",
        "integration_timeout_seconds",
        "cold_timeout_seconds",
        "suite_timeout_seconds",
        "shipping_timeout_seconds",
        "observed_cold_timeout_seconds",
        "minimum_cold_headroom_multiplier",
        "measurement_run_id",
        "measurement_job_id",
        "measurement_commit",
        "measurement_command",
    }
    if not isinstance(execution, dict) or set(execution) != execution_fields:
        raise ValueError(
            "CI Cargo policy cargo_execution fields must be exactly "
            + ", ".join(sorted(execution_fields))
        )
    timeout_seconds_by_class = {
        "cross-check": _positive_int(
            execution["cross_check_timeout_seconds"],
            label="cargo_execution.cross_check_timeout_seconds",
        ),
        "warm": _positive_int(
            execution["warm_timeout_seconds"],
            label="cargo_execution.warm_timeout_seconds",
        ),
        "integration": _positive_int(
            execution["integration_timeout_seconds"],
            label="cargo_execution.integration_timeout_seconds",
        ),
        "cold": _positive_int(
            execution["cold_timeout_seconds"],
            label="cargo_execution.cold_timeout_seconds",
        ),
        "suite": _positive_int(
            execution["suite_timeout_seconds"],
            label="cargo_execution.suite_timeout_seconds",
        ),
        "shipping": _positive_int(
            execution["shipping_timeout_seconds"],
            label="cargo_execution.shipping_timeout_seconds",
        ),
    }
    ordered = tuple(timeout_seconds_by_class.values())
    if ordered != tuple(sorted(ordered)):
        raise ValueError(
            "CI Cargo execution budgets must be monotonic: "
            "cross-check <= warm <= integration <= cold <= suite <= shipping"
        )
    observed = _positive_float(
        execution["observed_cold_timeout_seconds"],
        label="cargo_execution.observed_cold_timeout_seconds",
    )
    multiplier = _positive_float(
        execution["minimum_cold_headroom_multiplier"],
        label="cargo_execution.minimum_cold_headroom_multiplier",
    )
    if timeout_seconds_by_class["cold"] < math.ceil(observed * multiplier):
        raise ValueError(
            "CI Cargo cold timeout is below the receipt-calibrated minimum headroom"
        )
    return CiCargoPolicy(
        build_resources=build_resources,
        environment=environment_policy,
        execution_budgets=CargoExecutionBudgetPolicy(
            timeout_seconds_by_class=timeout_seconds_by_class,
            observed_cold_timeout_seconds=observed,
            minimum_cold_headroom_multiplier=multiplier,
            measurement_run_id=_positive_int(
                execution["measurement_run_id"],
                label="cargo_execution.measurement_run_id",
            ),
            measurement_job_id=_positive_int(
                execution["measurement_job_id"],
                label="cargo_execution.measurement_job_id",
            ),
            measurement_commit=_measurement_commit(
                execution["measurement_commit"],
                label="cargo_execution.measurement_commit",
            ),
            measurement_command=_measurement_command(
                execution["measurement_command"],
                label="cargo_execution.measurement_command",
            ),
        ),
    )
