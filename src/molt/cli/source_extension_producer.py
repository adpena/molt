from __future__ import annotations

import contextlib
import hashlib
import io
import importlib.metadata as importlib_metadata
import json
import os
import subprocess
import sys
import sysconfig
import tempfile
import tomllib
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any, cast

from packaging.requirements import Requirement
from packaging.version import InvalidVersion

from molt.cli import commands
from molt.cli import source_extension_cython as _source_extension_cython
from molt.cli.atomic_io import (
    _atomic_copy_file,
    _atomic_write_bytes,
    _atomic_write_json,
    _atomic_write_text,
    _remove_file_or_tree,
)
from molt.cli.extension_manifest import (
    _default_molt_c_api_version,
    _manifest_dotted_name_tuple,
    _validate_extension_manifest,
)
from molt.cli.file_hashing import _sha256_file
from molt.cli.extension_wheel import _rewrite_staged_extension_wheel
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload
from molt.cli.source_extensions import (
    _MESON_EXTENSION_TARGET_TYPES,
    _load_ninja_build_all_inputs,
    _load_meson_intro_targets_source_extension_plan,
    _meson_linked_static_library_targets,
    _meson_target_filename_names,
    _meson_target_output_paths,
    canonicalize_source_extension_manifest_required_capsules,
)
from molt.cli.source_build_environment import (
    LockedSourceBuildEnvironment,
    SourceBuildEnvironmentError,
    active_source_build_requirements,
    canonical_source_marker_environment,
    provision_source_build_environment,
    source_build_environment,
)
from molt.cli.build_locks import _acquire_file_lock, _release_file_lock
from molt.cli.source_extension_reproducibility import (
    _canonical_extension_manifest_for_wheel,
    _canonicalize_location_string,
    _canonicalize_locations,
    _canonicalize_meson_metadata,
    _require_location_neutral,
)
from molt.cli.source_extension_manifest_codec import (
    _compact_source_extension_manifest,
    _manifest_dependencies,
    _manifest_sequence,
    _object_unit_sha256,
    _validate_compact_source_extension_manifest,
)
from molt.cli.source_extension_set_identity import (
    _require_expected_source_extension_set_identity,
    _source_extension_reproduction_comparison,
    _source_extension_set_identity,
)
from molt.cli.source_extension_publication import (
    SourceExtensionPublicationCustody,
    _source_extension_publication_custody,
    publish_source_extension_candidate,
    recover_source_extension_publication,
)
from molt.cli.source_extension_toolchain import (
    MOLT_PKGCONF_REQUIREMENT,
    _materialize_source_extension_target_metadata,
    _meson_array,
)
from molt.cli.source_package_seal import (
    SourcePackageInput,
    SourcePackageSealError,
    SourcePackageSealVerificationError,
    commit_source_package_seal,
    prepare_source_package_seal_commit,
    recover_source_package_seal_commits,
    stage_source_package_seal,
    validate_source_package_relative_path,
    verify_source_package_seal,
)
from molt.scientific_stack_versions import (
    ScientificExtensionSet,
    scientific_extension_set,
    scientific_extension_set_root,
    verify_cpython_abi_headers,
    verify_source_checkout,
)
from molt import process_guard


_REPO_ROOT = Path(__file__).resolve().parents[3]
_GENERATED_INPUT_SUFFIXES = {
    ".c",
    ".cc",
    ".cpp",
    ".cxx",
    ".h",
    ".hh",
    ".hpp",
    ".hxx",
    ".inc",
    ".pxd",
    ".py",
    ".pyi",
}


class SourceExtensionProducerError(ValueError):
    pass


SOURCE_EXTENSION_SET_SCHEMA_VERSION = 2


@dataclass(frozen=True)
class _ProducedExtension:
    module: str
    target: str
    capabilities: tuple[str, ...]
    output_root: Path
    manifest_path: Path
    artifact_path: Path
    artifact_manifest_path: Path
    wheel_path: Path
    artifact_sha256: str
    wheel_sha256: str
    object_closure_sha256: str


@dataclass(frozen=True)
class _ResolvedBuildRequirement:
    requirement: str
    distribution: str
    version: str

    def manifest_payload(self) -> dict[str, str]:
        return {
            "requirement": self.requirement,
            "distribution": self.distribution,
            "version": self.version,
        }


@dataclass(frozen=True)
class _SourceBuildEnvironment:
    python_executable: str
    requirements: tuple[str, ...]
    marker_environment: Mapping[str, str]
    active_requirements: tuple[str, ...]
    resolved: tuple[_ResolvedBuildRequirement, ...]
    custody: Mapping[str, object]

    def manifest_payload(self) -> dict[str, Any]:
        return {
            "python": {
                "implementation": sys.implementation.name,
                "version": (
                    f"{sys.version_info.major}.{sys.version_info.minor}."
                    f"{sys.version_info.micro}"
                ),
                "executable": Path(self.python_executable).name,
            },
            "requirements": list(self.requirements),
            "marker_environment": dict(self.marker_environment),
            "active_requirements": list(self.active_requirements),
            "resolved": [item.manifest_payload() for item in self.resolved],
            "custody": dict(self.custody),
        }


@dataclass(frozen=True)
class _SourceBuildConfigTool:
    name: str
    path: Path
    distribution: str
    version: str

    def manifest_payload(self) -> dict[str, str]:
        return {
            "name": self.name,
            "path": self.path.name,
            "distribution": self.distribution,
            "version": self.version,
            "sha256": _sha256_file(self.path),
        }


@dataclass(frozen=True)
class _SourceMesonDriver:
    command: tuple[str, ...]
    manifest: Mapping[str, str]

    def manifest_payload(self) -> dict[str, str]:
        return dict(self.manifest)


@dataclass(frozen=True)
class _SourceNinjaDriver:
    command: tuple[str, ...]
    manifest: Mapping[str, str]

    def manifest_payload(self) -> dict[str, str]:
        return dict(self.manifest)


@dataclass(frozen=True)
class _SourceSubmoduleIdentity:
    path: str
    commit: str

    def manifest_payload(self) -> dict[str, str]:
        return {"path": self.path, "commit": self.commit}


def _run_process(argv: Sequence[str], *, cwd: Path) -> subprocess.CompletedProcess[str]:
    return process_guard.run_completed_command(
        list(argv),
        cwd=cwd,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )


def _verify_recursive_submodules(
    source_root: Path,
) -> tuple[_SourceSubmoduleIdentity, ...]:
    result = _run_process(
        (
            "git",
            "-c",
            "core.longpaths=true",
            "-C",
            str(source_root),
            "submodule",
            "status",
            "--recursive",
        ),
        cwd=source_root,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise SourceExtensionProducerError(
            f"failed to verify recursive submodules for {source_root}: "
            f"{detail or f'returncode={result.returncode}'}"
        )
    raw_rows = tuple(line for line in result.stdout.splitlines() if line.strip())
    invalid = [row.strip() for row in raw_rows if row[0] in {"-", "+", "U"}]
    if invalid:
        raise SourceExtensionProducerError(
            "source checkout has missing, unpinned, or conflicted recursive "
            "submodules: " + "; ".join(invalid)
        )
    identities = _run_process(
        (
            "git",
            "-c",
            "core.longpaths=true",
            "-C",
            str(source_root),
            "submodule",
            "foreach",
            "--recursive",
            "--quiet",
            'printf "%s\\t%s\\n" "$displaypath" "$(git rev-parse HEAD)"',
        ),
        cwd=source_root,
    )
    if identities.returncode != 0:
        detail = (identities.stderr or identities.stdout).strip()
        raise SourceExtensionProducerError(
            "failed to attest recursive submodule identities: "
            f"{detail or f'returncode={identities.returncode}'}"
        )
    parsed: list[_SourceSubmoduleIdentity] = []
    seen_paths: set[str] = set()
    for row in identities.stdout.splitlines():
        fields = row.split("\t")
        if len(fields) != 2:
            raise SourceExtensionProducerError(
                f"cannot parse recursive submodule identity row: {row!r}"
            )
        try:
            path = validate_source_package_relative_path(
                fields[0], field="recursive submodule path"
            )
        except SourcePackageSealVerificationError as exc:
            raise SourceExtensionProducerError(str(exc)) from exc
        commit = fields[1]
        if (
            path in seen_paths
            or len(commit) != 40
            or any(character not in "0123456789abcdef" for character in commit)
        ):
            raise SourceExtensionProducerError(
                f"invalid or duplicate recursive submodule identity: {row!r}"
            )
        seen_paths.add(path)
        parsed.append(_SourceSubmoduleIdentity(path=path, commit=commit))
    dirty: list[str] = []
    for identity in parsed:
        submodule_path = source_root / identity.path
        tracked = _run_process(
            (
                "git",
                "-c",
                "core.longpaths=true",
                "-C",
                str(submodule_path),
                "status",
                "--porcelain=v1",
                "--untracked-files=no",
            ),
            cwd=source_root,
        )
        if tracked.returncode == 0 and tracked.stdout.strip():
            dirty.append(identity.path)
        elif tracked.returncode != 0:
            detail = (tracked.stderr or tracked.stdout).strip()
            raise SourceExtensionProducerError(
                f"failed to verify pinned submodule worktree {submodule_path}: "
                f"{detail or f'returncode={tracked.returncode}'}"
            )
    if dirty:
        raise SourceExtensionProducerError(
            "source checkout has modified or incomplete pinned submodule "
            "worktrees: " + ", ".join(dirty)
        )
    return tuple(sorted(parsed, key=lambda identity: identity.path))


def _provision_recursive_submodules(source_root: Path) -> None:
    result = _run_process(
        (
            "git",
            "-c",
            "core.longpaths=true",
            "-C",
            str(source_root),
            "submodule",
            "update",
            "--init",
            "--recursive",
        ),
        cwd=source_root,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise SourceExtensionProducerError(
            f"failed to provision pinned recursive submodules for {source_root}: "
            f"{detail or f'returncode={result.returncode}'}"
        )


def _git_head(source_root: Path) -> str:
    result = _run_process(
        ("git", "-C", str(source_root), "rev-parse", "HEAD"), cwd=source_root
    )
    head = result.stdout.strip()
    if result.returncode != 0 or not head:
        raise SourceExtensionProducerError(
            f"cannot resolve source checkout HEAD for {source_root}"
        )
    return head


def _require_fresh_build_root(build_root: Path) -> None:
    if build_root.exists():
        if not build_root.is_dir():
            raise SourceExtensionProducerError(
                f"Meson build root is not a directory: {build_root}"
            )
        if any(build_root.iterdir()):
            raise SourceExtensionProducerError(
                "Meson build root must be absent or empty so producer metadata "
                f"cannot mix with a prior configuration: {build_root}"
            )
    else:
        build_root.parent.mkdir(parents=True, exist_ok=True)


def _source_build_requirements(
    source_root: Path,
) -> tuple[
    tuple[str, ...],
    Mapping[str, str],
    tuple[tuple[str, Requirement], ...],
]:
    pyproject_path = source_root / "pyproject.toml"
    try:
        payload = tomllib.loads(pyproject_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as exc:
        raise SourceExtensionProducerError(
            f"failed to read source build requirements from {pyproject_path}: {exc}"
        ) from exc
    build_system = payload.get("build-system")
    if not isinstance(build_system, Mapping):
        raise SourceExtensionProducerError(
            f"source pyproject has no [build-system] table: {pyproject_path}"
        )
    raw_requirements = build_system.get("requires")
    if (
        not isinstance(raw_requirements, list)
        or not raw_requirements
        or not all(isinstance(item, str) and item.strip() for item in raw_requirements)
    ):
        raise SourceExtensionProducerError(
            f"source [build-system].requires must be a non-empty string array: "
            f"{pyproject_path}"
        )
    originals = tuple(item.strip() for item in raw_requirements)
    marker_environment = canonical_source_marker_environment()
    try:
        active = active_source_build_requirements(originals, marker_environment)
    except SourceBuildEnvironmentError as exc:
        raise SourceExtensionProducerError(str(exc)) from exc
    if not active:
        raise SourceExtensionProducerError(
            "source [build-system].requires has no requirements active for the "
            "current interpreter"
        )
    return originals, marker_environment, active


def _installed_build_requirement(
    raw: str, requirement: Requirement
) -> _ResolvedBuildRequirement | None:
    try:
        distribution = importlib_metadata.distribution(requirement.name)
    except importlib_metadata.PackageNotFoundError:
        return None
    version = distribution.version
    try:
        satisfied = not requirement.specifier or requirement.specifier.contains(
            version, prereleases=True
        )
    except InvalidVersion as exc:
        raise SourceExtensionProducerError(
            f"installed build requirement {requirement.name!r} has invalid version "
            f"{version!r}"
        ) from exc
    if not satisfied:
        return None
    distribution_name = distribution.metadata.get("Name")
    if not isinstance(distribution_name, str) or not distribution_name.strip():
        raise SourceExtensionProducerError(
            f"installed build requirement {requirement.name!r} has no distribution "
            "Name metadata"
        )
    return _ResolvedBuildRequirement(
        requirement=raw,
        distribution=distribution_name.strip(),
        version=version,
    )


def _ensure_source_build_environment(
    source_root: Path, *, custody: Mapping[str, object]
) -> _SourceBuildEnvironment:
    originals, marker_environment, active = _source_build_requirements(source_root)
    unsatisfied = [
        raw
        for raw, requirement in active
        if _installed_build_requirement(raw, requirement) is None
    ]
    if unsatisfied:
        raise SourceExtensionProducerError(
            "locked source-build environment does not satisfy upstream build "
            "requirements; its configured dependency group or frozen lock is "
            "incomplete: " + ", ".join(unsatisfied)
        )
    resolved: list[_ResolvedBuildRequirement] = []
    for raw, requirement in active:
        installed = _installed_build_requirement(raw, requirement)
        assert installed is not None
        resolved.append(installed)
    return _SourceBuildEnvironment(
        python_executable=sys.executable,
        requirements=originals,
        marker_environment=marker_environment,
        active_requirements=tuple(raw for raw, _requirement in active),
        resolved=tuple(resolved),
        custody=custody,
    )


def _run_locked_source_extension_producer(
    environment: LockedSourceBuildEnvironment,
    *,
    package: str,
    module_set: str,
    source: str,
    build_root: str,
    target: str,
    abi_tier: str,
    json_output: bool,
    expected_identity_sha256: str | None = None,
    expected_candidate_identity_sha256: str | None = None,
) -> int:
    argv = [
        str(environment.python_executable),
        "-P",
        "-m",
        "molt.cli",
        "extension",
        "produce-set",
        "--package",
        package,
        "--module-set",
        module_set,
        "--source",
        source,
        "--build-root",
        build_root,
        "--target",
        target,
        "--abi-tier",
        abi_tier,
    ]
    if json_output:
        argv.append("--json")
    if expected_identity_sha256 is not None:
        argv.extend(("--expected-identity-sha256", expected_identity_sha256))
    if expected_candidate_identity_sha256 is not None:
        argv.extend(
            (
                "--expected-candidate-identity-sha256",
                expected_candidate_identity_sha256,
            )
        )
    child_environment = os.environ.copy()
    current_src = str((_REPO_ROOT / "src").resolve())
    child_environment["PYTHONPATH"] = current_src
    child_environment.pop("PYTHONHOME", None)
    child_environment["PYTHONNOUSERSITE"] = "1"
    child_environment["VIRTUAL_ENV"] = str(environment.root)
    child_environment["PATH"] = _locked_console_tool_path(
        environment.python_executable.parent.resolve(),
        child_environment.get("PATH"),
    )
    return process_guard.run_completed_command(
        argv,
        cwd=_REPO_ROOT,
        env=child_environment,
        check=False,
    ).returncode


def _locked_console_tool_path(
    scripts_root: str | Path,
    inherited_path: str | None,
    *,
    separator: str = os.pathsep,
) -> str:
    """Put attested environment scripts ahead of intentional host tools.

    The inherited suffix retains system and cross-toolchain discovery (LLVM,
    Git, Rust, and platform SDKs). Only the locked environment's console-script
    directory gains precedence; ambient Python environments gain no authority.
    """
    locked = str(scripts_root)
    return separator.join((locked, inherited_path)) if inherited_path else locked


def _source_build_config_tools(
    environment: _SourceBuildEnvironment,
) -> tuple[_SourceBuildConfigTool, ...]:
    scripts_root = Path(sysconfig.get_path("scripts")).resolve()
    tools: dict[str, _SourceBuildConfigTool] = {}
    for resolved in environment.resolved:
        distribution = importlib_metadata.distribution(resolved.distribution)
        for entry_point in distribution.entry_points:
            if entry_point.group != "console_scripts" or not entry_point.name.endswith(
                "-config"
            ):
                continue
            path = _active_console_script(entry_point.name, scripts_root=scripts_root)
            tool = _SourceBuildConfigTool(
                name=entry_point.name,
                path=path,
                distribution=resolved.distribution,
                version=resolved.version,
            )
            previous = tools.get(tool.name)
            if previous is not None and previous != tool:
                raise SourceExtensionProducerError(
                    f"multiple build requirements own config tool {tool.name!r}: "
                    f"{previous.distribution}, {tool.distribution}"
                )
            tools[tool.name] = tool
    return tuple(tools[name] for name in sorted(tools))


def _active_console_script(name: str, *, scripts_root: Path | None = None) -> Path:
    root = (
        Path(sysconfig.get_path("scripts")).resolve()
        if scripts_root is None
        else scripts_root.resolve()
    )
    candidates = tuple(
        path
        for path in (
            root / name,
            root / f"{name}.exe",
            root / f"{name}.cmd",
            root / f"{name}.bat",
        )
        if path.is_file()
    )
    if len(candidates) != 1:
        raise SourceExtensionProducerError(
            f"active interpreter has {len(candidates)} matching console scripts "
            f"for {name!r} under {root}"
        )
    return candidates[0].resolve()


def _ensure_meson_pkg_config(source_root: Path) -> _SourceBuildConfigTool:
    requirement = Requirement(MOLT_PKGCONF_REQUIREMENT)
    resolved = _installed_build_requirement(MOLT_PKGCONF_REQUIREMENT, requirement)
    if resolved is None:
        raise SourceExtensionProducerError(
            "source build environment is missing Molt's locked Meson tool "
            f"requirement {MOLT_PKGCONF_REQUIREMENT}; the producer never installs "
            "into its active interpreter"
        )
    path = _active_console_script("pkg-config")
    version = _run_process((str(path), "--version"), cwd=source_root)
    if (
        version.returncode != 0
        or version.stdout.strip() != resolved.version.removesuffix(".post0")
    ):
        detail = (version.stderr or version.stdout).strip()
        raise SourceExtensionProducerError(
            f"pinned pkg-config executable {path} does not attest "
            f"{resolved.version}: {detail or f'returncode={version.returncode}'}"
        )
    return _SourceBuildConfigTool(
        name="pkg-config",
        path=path,
        distribution=resolved.distribution,
        version=resolved.version,
    )


def _materialize_meson_config_tool_cross(
    path: Path, tools: Sequence[_SourceBuildConfigTool]
) -> Path | None:
    if not tools:
        return None
    path.parent.mkdir(parents=True, exist_ok=True)
    lines = ["[binaries]"]
    lines.extend(
        f"{tool.name} = {_meson_array((str(tool.path),))}"
        for tool in sorted(tools, key=lambda item: item.name)
    )
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return path


def _run_meson_setup(
    *,
    source_root: Path,
    build_root: Path,
    meson_cross_files: Sequence[Path],
    setup_args: Sequence[str],
    driver: _SourceMesonDriver,
) -> None:
    argv: list[str] = [
        *driver.command,
        "setup",
        str(build_root),
        str(source_root),
    ]
    for meson_cross in meson_cross_files:
        argv.extend(("--cross-file", str(meson_cross)))
    argv.extend(setup_args)
    result = _run_process(argv, cwd=source_root)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise SourceExtensionProducerError(
            f"upstream Meson setup failed ({result.returncode}): {detail}"
        )


def _source_meson_driver(source_root: Path) -> _SourceMesonDriver:
    pyproject_path = source_root / "pyproject.toml"
    try:
        payload = tomllib.loads(pyproject_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as exc:
        raise SourceExtensionProducerError(
            f"failed to read upstream Meson authority from {pyproject_path}: {exc}"
        ) from exc
    tool = payload.get("tool")
    meson_python = tool.get("meson-python") if isinstance(tool, Mapping) else None
    raw_driver = (
        meson_python.get("meson") if isinstance(meson_python, Mapping) else None
    )
    if raw_driver is not None:
        if not isinstance(raw_driver, str) or not raw_driver.strip():
            raise SourceExtensionProducerError(
                f"{pyproject_path}: tool.meson-python.meson must be a relative path"
            )
        relative = Path(raw_driver.strip())
        if relative.is_absolute() or ".." in relative.parts:
            raise SourceExtensionProducerError(
                f"{pyproject_path}: tool.meson-python.meson escapes source custody"
            )
        driver = (source_root / relative).resolve()
        if not driver.is_relative_to(source_root.resolve()) or not driver.is_file():
            raise SourceExtensionProducerError(
                f"upstream-declared Meson driver is absent: {driver}"
            )
        return _SourceMesonDriver(
            command=(sys.executable, str(driver)),
            manifest={
                "kind": "source-vendored",
                "path": relative.as_posix(),
                "sha256": _sha256_file(driver),
            },
        )
    try:
        meson_version = importlib_metadata.version("meson")
    except importlib_metadata.PackageNotFoundError as exc:
        raise SourceExtensionProducerError(
            "source build environment has no Meson distribution and upstream did "
            "not declare tool.meson-python.meson"
        ) from exc
    return _SourceMesonDriver(
        command=(sys.executable, "-m", "mesonbuild.mesonmain"),
        manifest={
            "kind": "build-environment",
            "module": "mesonbuild.mesonmain",
            "distribution": "meson",
            "version": meson_version,
        },
    )


def _source_ninja_driver(source_root: Path) -> _SourceNinjaDriver:
    try:
        distribution = importlib_metadata.distribution("ninja")
    except importlib_metadata.PackageNotFoundError as exc:
        raise SourceExtensionProducerError(
            "source build environment has no Ninja backend distribution"
        ) from exc
    binaries = tuple(
        path.resolve()
        for item in (distribution.files or ())
        if Path(str(item)).name.lower() == "ninja.exe"
        and (path := Path(str(distribution.locate_file(item)))).is_file()
    )
    if len(binaries) != 1:
        raise SourceExtensionProducerError(
            f"Ninja distribution owns {len(binaries)} executable payloads"
        )
    path = binaries[0]
    command = (sys.executable, "-m", "ninja")
    result = _run_process((*command, "--version"), cwd=source_root)
    version = result.stdout.strip()
    if result.returncode != 0 or not version:
        detail = (result.stderr or result.stdout).strip()
        raise SourceExtensionProducerError(
            f"Ninja backend cannot attest its version: {detail}"
        )
    distribution_name = distribution.metadata.get("Name")
    return _SourceNinjaDriver(
        command=command,
        manifest={
            "distribution": (
                distribution_name.strip()
                if isinstance(distribution_name, str) and distribution_name.strip()
                else "ninja"
            ),
            "version": version,
            "path": path.name,
            "sha256": _sha256_file(path),
        },
    )


def _require_real_meson_metadata(build_root: Path) -> tuple[Path, Path, Path]:
    paths = (
        build_root / "meson-info" / "intro-targets.json",
        build_root / "compile_commands.json",
        build_root / "meson-info" / "intro-installed.json",
    )
    missing = [str(path) for path in paths if not path.is_file()]
    if missing:
        raise SourceExtensionProducerError(
            "Meson setup did not emit required real introspection metadata: "
            + ", ".join(missing)
        )
    return paths


def _installed_source_path(
    raw_path: str, *, source_root: Path, build_root: Path
) -> Path:
    path = Path(raw_path).expanduser()
    if path.is_absolute():
        return path.resolve()
    for root in (build_root, source_root):
        candidate = (root / path).resolve()
        if candidate.exists():
            return candidate
    return (build_root / path).resolve()


def _installed_package_relative_path(
    raw_destination: str, *, package: str
) -> Path | None:
    normalized = raw_destination.replace("\\", "/")
    parts = tuple(part for part in normalized.split("/") if part and part != ".")
    try:
        package_index = parts.index(package)
    except ValueError:
        return None
    relative = Path(*parts[package_index:])
    if any(part == ".." for part in relative.parts):
        raise SourceExtensionProducerError(
            f"Meson installed destination escapes package root: {raw_destination}"
        )
    return relative


def _stage_installed_package_files(
    *,
    intro_installed: Path,
    source_root: Path,
    build_root: Path,
    package: str,
    publish_root: Path,
    location_roots: Sequence[tuple[Path, str]],
    required_installed_files: Sequence[str],
) -> tuple[Path, ...]:
    try:
        payload = json.loads(intro_installed.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise SourceExtensionProducerError(
            f"failed to read Meson installed-file introspection {intro_installed}: {exc}"
        ) from exc
    if not isinstance(payload, Mapping):
        raise SourceExtensionProducerError(
            f"Meson installed-file introspection must be an object: {intro_installed}"
        )

    def installed_leaves(source: Path, relative: Path) -> tuple[tuple[Path, Path], ...]:
        if source.is_file():
            return ((source, relative),)
        if not source.is_dir():
            raise SourceExtensionProducerError(
                f"Meson installed source is missing: {source}"
            )
        return tuple(
            (child, relative / child.relative_to(source))
            for child in sorted(source.rglob("*"), key=lambda path: path.as_posix())
            if child.is_file()
        )

    def is_installed_package_support(relative: Path) -> bool:
        return (
            relative.suffix in {".py", ".pyi"}
            or relative.name == "py.typed"
            or "include" in relative.parts
        )

    staged_by_relative: dict[Path, Path] = {}
    for raw_source, raw_destination in sorted(
        payload.items(), key=lambda item: str(item[0])
    ):
        if not isinstance(raw_source, str) or not isinstance(raw_destination, str):
            raise SourceExtensionProducerError(
                "Meson installed-file introspection entries must map path strings "
                "to path strings"
            )
        relative = _installed_package_relative_path(raw_destination, package=package)
        if relative is None:
            continue
        source = _installed_source_path(
            raw_source, source_root=source_root, build_root=build_root
        )
        # Meson introspection covers the whole package build, including native
        # outputs outside this selected extension set. Preserve the established
        # support-file projection for leaf rows before requiring their source;
        # directory rows still expand recursively because Meson's install_subdir
        # entries are the authority for Python package structure.
        if not is_installed_package_support(relative) and not source.is_dir():
            continue
        for leaf_source, leaf_relative in installed_leaves(source, relative):
            if not is_installed_package_support(leaf_relative):
                continue
            try:
                raw_bytes = leaf_source.read_bytes()
            except OSError as exc:
                raise SourceExtensionProducerError(
                    f"cannot read installed package source {leaf_source}: {exc}"
                ) from exc
            try:
                canonical_bytes = _canonicalize_location_string(
                    raw_bytes.decode("utf-8"), location_roots
                ).encode("utf-8")
            except UnicodeError:
                canonical_bytes = raw_bytes
            previous = staged_by_relative.get(leaf_relative)
            if previous is not None:
                if (publish_root / leaf_relative).read_bytes() != canonical_bytes:
                    raise SourceExtensionProducerError(
                        "Meson installs different package files to the same path: "
                        f"{leaf_relative}"
                    )
                continue
            destination = publish_root / leaf_relative
            _atomic_write_bytes(destination, canonical_bytes)
            staged_by_relative[leaf_relative] = leaf_source

    required = tuple(required_installed_files)
    missing = [item for item in required if Path(item) not in staged_by_relative]
    if missing:
        raise SourceExtensionProducerError(
            "Meson installed-file introspection is incomplete for the package "
            f"root; missing: {', '.join(missing)}"
        )
    return tuple(sorted((publish_root / path) for path in staged_by_relative))


def _object_closure_digest(
    object_closure: Mapping[str, Any],
    *,
    manifest_dir: Path | None = None,
    manifest: Mapping[str, Any] | None = None,
) -> str:
    objects = object_closure.get("objects")
    runtime_symbols = object_closure.get("runtime_symbols")
    if not isinstance(objects, list) or not objects:
        raise SourceExtensionProducerError("extension object_closure.objects is empty")
    if not isinstance(runtime_symbols, list) or not all(
        isinstance(item, str) for item in runtime_symbols
    ):
        raise SourceExtensionProducerError(
            "extension object_closure.runtime_symbols must be a string array"
        )
    digest_objects: list[dict[str, Any]] = []
    for index, item in enumerate(objects):
        if not isinstance(item, Mapping):
            raise SourceExtensionProducerError(
                f"extension object_closure.objects[{index}] is not an object"
            )
        item = cast(Mapping[str, Any], item)
        source = item.get("source")
        object_path = item.get("object")
        source_sha256 = item.get("source_sha256")
        object_sha256 = item.get("object_sha256")
        if not (
            isinstance(source, str)
            and source
            and isinstance(object_path, str)
            and object_path
            and isinstance(source_sha256, str)
            and source_sha256
            and isinstance(object_sha256, str)
            and object_sha256
        ):
            raise SourceExtensionProducerError(
                f"extension object_closure.objects[{index}] lacks checksum custody"
            )
        source_path = Path(source)
        if not source_path.is_absolute() and manifest_dir is not None:
            source_path = manifest_dir / source_path
        if not source_path.is_file():
            raise SourceExtensionProducerError(
                f"extension object_closure source is missing: {source_path}"
            )
        if _sha256_file(source_path) != source_sha256:
            raise SourceExtensionProducerError(
                f"extension object_closure source checksum mismatch: {source_path}"
            )
        defined_symbols = item.get("defined_symbols")
        undefined_symbols = item.get("undefined_symbols")
        authority = (
            manifest if manifest is not None else {"object_closure": object_closure}
        )
        try:
            compile_command = _manifest_sequence(authority, item, "compile_command")
            symbol_command = _manifest_sequence(authority, item, "symbol_command")
        except ValueError as exc:
            raise SourceExtensionProducerError(str(exc)) from exc
        if not (
            isinstance(defined_symbols, list)
            and all(isinstance(value, str) for value in defined_symbols)
            and isinstance(undefined_symbols, list)
            and all(isinstance(value, str) for value in undefined_symbols)
            and isinstance(compile_command, list)
            and bool(compile_command)
            and all(isinstance(value, str) and value for value in compile_command)
            and isinstance(symbol_command, list)
            and bool(symbol_command)
            and all(isinstance(value, str) and value for value in symbol_command)
        ):
            raise SourceExtensionProducerError(
                f"extension object_closure.objects[{index}] has invalid symbols "
                "or tool command"
            )
        digest_object: dict[str, Any] = {
            "source": source,
            "object": object_path,
            "source_sha256": source_sha256,
            "object_sha256": object_sha256,
            "defined_symbols": defined_symbols,
            "undefined_symbols": undefined_symbols,
            "compile_command": compile_command,
            "symbol_command": symbol_command,
        }
        try:
            raw_dependencies = _manifest_dependencies(authority, item)
        except ValueError as exc:
            raise SourceExtensionProducerError(str(exc)) from exc
        dependencies: list[dict[str, str]] = []
        for dependency_index, raw_dependency in enumerate(raw_dependencies):
            if not isinstance(raw_dependency, Mapping):
                raise SourceExtensionProducerError(
                    "extension object_closure dependency is not an object"
                )
            dependency_path_raw = raw_dependency.get("path")
            dependency_sha256 = raw_dependency.get("sha256")
            if not (
                isinstance(dependency_path_raw, str)
                and dependency_path_raw
                and isinstance(dependency_sha256, str)
                and dependency_sha256
            ):
                raise SourceExtensionProducerError(
                    "extension object_closure dependency lacks path/checksum "
                    f"at objects[{index}].dependencies[{dependency_index}]"
                )
            dependency_path = Path(dependency_path_raw)
            if not dependency_path.is_absolute() and manifest_dir is not None:
                dependency_path = manifest_dir / dependency_path
            if not dependency_path.is_file():
                raise SourceExtensionProducerError(
                    f"extension object_closure dependency is missing: {dependency_path}"
                )
            if _sha256_file(dependency_path) != dependency_sha256:
                raise SourceExtensionProducerError(
                    "extension object_closure dependency checksum mismatch: "
                    f"{dependency_path}"
                )
            dependencies.append(
                {"path": dependency_path_raw, "sha256": dependency_sha256}
            )
        digest_object["dependencies"] = dependencies
        digest_objects.append(digest_object)
    digest_payload = {
        "schema_version": 1,
        "root_symbol": object_closure.get("root_symbol"),
        "objects": digest_objects,
        "runtime_symbols": runtime_symbols,
    }
    encoded = json.dumps(digest_payload, sort_keys=True, separators=(",", ":")).encode(
        "utf-8"
    )
    return hashlib.sha256(encoded).hexdigest()


def _declared_wheel_path(
    manifest: Mapping[str, Any], *, output_root: Path, module: str
) -> Path:
    raw_wheel = manifest.get("wheel")
    if not isinstance(raw_wheel, str) or not raw_wheel.strip():
        raise SourceExtensionProducerError(
            f"built extension {module} has no declared wheel path"
        )
    wheel = Path(raw_wheel).expanduser()
    if not wheel.is_absolute():
        wheel = output_root / wheel
    wheel = wheel.resolve()
    resolved_output_root = output_root.resolve()
    if not wheel.is_relative_to(resolved_output_root):
        raise SourceExtensionProducerError(
            f"built extension {module} wheel escapes transactional output: {wheel}"
        )
    if not wheel.is_file():
        raise SourceExtensionProducerError(
            f"built extension {module} wheel is missing: {wheel}"
        )
    return wheel


def _audit_declared_wheel(
    manifest: Mapping[str, Any], *, output_root: Path, module: str
) -> str:
    expected_sha256 = manifest.get("wheel_sha256")
    if not isinstance(expected_sha256, str) or not expected_sha256:
        raise SourceExtensionProducerError(
            f"built extension {module} has no wheel checksum"
        )
    wheel = _declared_wheel_path(
        manifest,
        output_root=output_root,
        module=module,
    )
    actual_sha256 = _sha256_file(wheel)
    if actual_sha256 != expected_sha256:
        raise SourceExtensionProducerError(
            f"built extension {module} wheel checksum mismatch"
        )
    return actual_sha256


def _audit_producer_contract(manifest: Mapping[str, Any], *, module: str) -> None:
    current_abi = _default_molt_c_api_version(_REPO_ROOT)
    expected = {
        "deterministic": True,
        "loader_kind": "libmolt_source",
        "runtime_linkage": "static_link",
        "artifact_kind": "wasm_relocatable_object",
        "target_triple": "wasm32-wasip1",
        "abi_tier": "cpython-abi",
        "molt_c_api_version": current_abi,
        "abi_tag": f"molt_abi{current_abi.split('.', 1)[0]}",
    }
    mismatches = [
        f"{field_name}: expected {expected_value!r}, got {manifest.get(field_name)!r}"
        for field_name, expected_value in expected.items()
        if manifest.get(field_name) != expected_value
    ]
    if mismatches:
        raise SourceExtensionProducerError(
            f"built extension {module} violates producer/consumer contract: "
            + "; ".join(mismatches)
        )


def _audit_extension_output(
    *,
    output_root: Path,
    module: str,
    target: str,
    python_exports: Sequence[str],
    capabilities: Sequence[str],
    provided_capsules: Sequence[str],
) -> _ProducedExtension:
    manifest_path = output_root / "extension_manifest.json"
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise SourceExtensionProducerError(
            f"failed to read built extension manifest {manifest_path}: {exc}"
        ) from exc
    if not isinstance(manifest, dict):
        raise SourceExtensionProducerError(
            f"built extension manifest is not an object: {manifest_path}"
        )
    validation = _validate_extension_manifest(
        manifest,
        manifest_dir=output_root,
        wheel_path=None,
        require_capabilities=False,
        required_abi=None,
        require_checksum=True,
        warn_missing_checksum=False,
    )
    if validation.errors:
        raise SourceExtensionProducerError(
            f"built extension manifest failed audit for {module}: "
            + "; ".join(validation.errors)
        )
    if manifest.get("module") != module:
        raise SourceExtensionProducerError(
            f"built extension module mismatch: expected {module!r}, "
            f"got {manifest.get('module')!r}"
        )
    if manifest.get("capabilities") != list(capabilities):
        raise SourceExtensionProducerError(
            f"built extension {module} capabilities differ from configured exact "
            f"contract: expected {list(capabilities)}, "
            f"got {manifest.get('capabilities')!r}"
        )
    _audit_producer_contract(manifest, module=module)
    source_plan = manifest.get("source_plan")
    if (
        not isinstance(source_plan, Mapping)
        or source_plan.get("target_selector") != target
    ):
        raise SourceExtensionProducerError(
            f"built extension {module} did not attest configured Meson target {target!r}"
        )
    export_errors: list[str] = []
    actual_exports = _manifest_dotted_name_tuple(
        manifest,
        "python_exports",
        package=module.split(".", 1)[0],
        errors=export_errors,
    )
    if export_errors:
        raise SourceExtensionProducerError(
            f"built extension {module} has invalid Python export custody: "
            + "; ".join(export_errors)
        )
    if set(actual_exports) != set(python_exports):
        raise SourceExtensionProducerError(
            f"built extension {module} python exports differ from configured exact "
            f"custody: expected {sorted(python_exports)}, got {sorted(actual_exports)}"
        )
    actual_capsules = manifest.get("provided_capsules")
    if actual_capsules != list(provided_capsules):
        raise SourceExtensionProducerError(
            f"built extension {module} capsule ownership differs from configured "
            f"exact custody: expected {list(provided_capsules)}, "
            f"got {actual_capsules!r}"
        )
    init_symbol = f"PyInit_{module.rsplit('.', 1)[-1]}"
    closure = manifest.get("object_closure")
    if not isinstance(closure, Mapping):
        raise SourceExtensionProducerError(
            f"built extension {module} has no object closure"
        )
    if manifest.get("init_symbol") != init_symbol:
        raise SourceExtensionProducerError(
            f"built extension {module} has wrong init symbol"
        )
    if closure.get("root_symbol") != init_symbol:
        raise SourceExtensionProducerError(
            f"built extension {module} object closure has wrong root symbol"
        )
    owner = closure.get("init_symbol_owner")
    if not isinstance(owner, str) or not owner:
        raise SourceExtensionProducerError(
            f"built extension {module} object closure has no init-symbol owner"
        )
    closure_digest = _object_closure_digest(closure, manifest_dir=output_root)
    if closure.get("closure_sha256") != closure_digest:
        raise SourceExtensionProducerError(
            f"built extension {module} object closure checksum mismatch"
        )
    extension = manifest.get("extension")
    if not isinstance(extension, str) or not extension:
        raise SourceExtensionProducerError(
            f"built extension {module} has no artifact path"
        )
    artifact_path = output_root / Path(extension)
    artifact_manifest_path = artifact_path.with_name(
        artifact_path.name + ".extension_manifest.json"
    )
    if not artifact_path.is_file() or not artifact_manifest_path.is_file():
        raise SourceExtensionProducerError(
            f"built extension {module} did not publish artifact plus sidecar"
        )
    try:
        artifact_manifest = json.loads(
            artifact_manifest_path.read_text(encoding="utf-8")
        )
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise SourceExtensionProducerError(
            f"failed to read built artifact sidecar {artifact_manifest_path}: {exc}"
        ) from exc
    expected_artifact_manifest = dict(manifest)
    expected_artifact_manifest["extension"] = artifact_path.name
    if artifact_manifest != expected_artifact_manifest:
        raise SourceExtensionProducerError(
            f"built extension {module} artifact sidecar differs from audited manifest"
        )
    artifact_sha256 = _sha256_file(artifact_path)
    if manifest.get("extension_sha256") != artifact_sha256:
        raise SourceExtensionProducerError(
            f"built extension {module} artifact checksum mismatch"
        )
    wheel_sha256 = _audit_declared_wheel(
        manifest, output_root=output_root, module=module
    )
    wheel_path = _declared_wheel_path(manifest, output_root=output_root, module=module)
    return _ProducedExtension(
        module=module,
        target=target,
        capabilities=tuple(capabilities),
        output_root=output_root,
        manifest_path=manifest_path,
        artifact_path=artifact_path,
        artifact_manifest_path=artifact_manifest_path,
        wheel_path=wheel_path,
        artifact_sha256=artifact_sha256,
        wheel_sha256=wheel_sha256,
        object_closure_sha256=closure_digest,
    )


def _build_extension(
    *,
    source_root: Path,
    build_root: Path,
    intro_targets: Path,
    compile_commands: Path,
    output_root: Path,
    module: str,
    target_name: str,
    python_exports: Sequence[str],
    capabilities: Sequence[str],
    provided_capsules: Sequence[str],
    exclude_linked_static_libraries: Sequence[str],
    target: str,
    abi_tier: str,
    tool_commands: Mapping[str, Sequence[str]],
    backend: _SourceNinjaDriver,
) -> _ProducedExtension:
    stdout = io.StringIO()
    stderr = io.StringIO()
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        rc = commands.extension_build(
            project=str(source_root),
            out_dir=str(output_root),
            module=module,
            capabilities=list(capabilities),
            provided_capsules=list(provided_capsules),
            python_export=list(python_exports),
            deterministic=True,
            target=target,
            source_plan=str(intro_targets),
            source_plan_target=target_name,
            source_plan_source_root=str(source_root),
            source_plan_build_root=str(build_root),
            source_plan_compile_commands=str(compile_commands),
            source_plan_exclude_linked_static_libraries=list(
                exclude_linked_static_libraries
            ),
            abi_tier=abi_tier,
            tool_commands=tool_commands,
            source_plan_ninja_command=backend.command,
            json_output=False,
            verbose=False,
        )
    if rc != 0:
        detail = (stderr.getvalue() or stdout.getvalue()).strip()
        raise SourceExtensionProducerError(
            f"extension build failed for {module} ({rc}): {detail}"
        )
    return _audit_extension_output(
        output_root=output_root,
        module=module,
        target=target_name,
        python_exports=python_exports,
        capabilities=capabilities,
        provided_capsules=provided_capsules,
    )


def _preflight_extension_set_plans(
    *,
    source_root: Path,
    build_root: Path,
    intro_targets: Path,
    compile_commands: Path,
    extension_set: ScientificExtensionSet,
) -> None:
    failures: list[str] = []
    for spec in extension_set.extensions:
        plan, errors = _load_meson_intro_targets_source_extension_plan(
            plan_path=intro_targets,
            project_root=source_root,
            module_name=spec.module,
            selector=spec.target,
            source_root=source_root,
            build_root=build_root,
            compile_commands=compile_commands,
            exclude_linked_static_libraries=(spec.exclude_linked_static_libraries),
        )
        if plan is None or errors:
            detail = "; ".join(errors) if errors else "no source plan"
            failures.append(f"{spec.module}: {detail}")
    if failures:
        raise SourceExtensionProducerError(
            "configured extension set has invalid Meson source plans before "
            "compilation: " + " | ".join(failures)
        )


def _manifest_source_candidates(manifest: Mapping[str, Any]) -> tuple[Path, ...]:
    raw_paths: list[str] = []
    closure = manifest.get("object_closure")
    if isinstance(closure, Mapping):
        objects = closure.get("objects")
        if isinstance(objects, list):
            raw_paths.extend(
                str(item["source"])
                for item in objects
                if isinstance(item, Mapping) and isinstance(item.get("source"), str)
            )
            for item in objects:
                dependencies = (
                    item.get("dependencies") if isinstance(item, Mapping) else None
                )
                if isinstance(dependencies, list):
                    raw_paths.extend(
                        str(dependency["path"])
                        for dependency in dependencies
                        if isinstance(dependency, Mapping)
                        and isinstance(dependency.get("path"), str)
                    )
    return tuple(
        sorted(
            {
                Path(raw).expanduser().resolve()
                for raw in raw_paths
                if Path(raw).expanduser().is_absolute()
            }
        )
    )


def _stage_compiled_inputs(
    manifest: Mapping[str, Any], *, publish_root: Path
) -> dict[Path, Path]:
    staged: dict[Path, Path] = {}
    for source in _manifest_source_candidates(manifest):
        if not source.is_file():
            raise SourceExtensionProducerError(
                f"source-extension manifest input is missing before sealing: {source}"
            )
        sha256 = _sha256_file(source)
        relative = (
            Path("provenance")
            / "compiled-inputs"
            / "sha256"
            / sha256[:2]
            / sha256
            / source.name
        )
        destination = publish_root / relative
        if destination.exists() and _sha256_file(destination) != sha256:
            raise SourceExtensionProducerError(
                f"content-addressed compiled input collision at {destination}"
            )
        if not destination.exists():
            _atomic_copy_file(source, destination)
        staged[source] = relative
    return staged


def _relative_manifest_path(path: Path, manifest_dir: Path) -> str:
    return os.path.relpath(path, manifest_dir).replace("\\", "/")


def _stage_extension(
    produced: _ProducedExtension,
    *,
    publish_root: Path,
    location_roots: Sequence[tuple[Path, str]],
    plan_metadata: Mapping[str, Path],
) -> _ProducedExtension:
    relative_artifact = produced.artifact_path.relative_to(produced.output_root)
    destination = publish_root / relative_artifact
    _atomic_copy_file(produced.artifact_path, destination)
    sidecar_path = destination.with_name(destination.name + ".extension_manifest.json")
    try:
        raw_manifest = json.loads(
            produced.artifact_manifest_path.read_text(encoding="utf-8")
        )
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise SourceExtensionProducerError(
            f"failed to reload audited extension manifest: {exc}"
        ) from exc
    if not isinstance(raw_manifest, dict):
        raise SourceExtensionProducerError(
            "audited extension manifest is not an object"
        )
    capsule_errors = canonicalize_source_extension_manifest_required_capsules(
        raw_manifest,
        manifest_path=produced.artifact_manifest_path,
    )
    if capsule_errors:
        raise SourceExtensionProducerError(
            "cannot canonicalize source-derived capsule custody: "
            + "; ".join(capsule_errors)
        )

    raw_source_plan = raw_manifest.get("source_plan")
    raw_plan_path = (
        Path(str(raw_source_plan["plan"]))
        if isinstance(raw_source_plan, Mapping) and raw_source_plan.get("plan")
        else None
    )
    raw_compile_commands_path = (
        Path(str(raw_source_plan["compile_commands"]))
        if isinstance(raw_source_plan, Mapping)
        and raw_source_plan.get("compile_commands")
        else None
    )
    canonical_embedded_manifest = _canonical_extension_manifest_for_wheel(
        raw_manifest,
        location_roots=location_roots,
        meson_plan_path=raw_plan_path,
        compile_commands_path=raw_compile_commands_path,
    )
    canonical_embedded_manifest["extension"] = destination.name
    canonical_embedded_manifest = _compact_source_extension_manifest(
        canonical_embedded_manifest
    )
    _require_location_neutral(
        canonical_embedded_manifest,
        authority=f"embedded extension manifest for {produced.module}",
    )

    staged_sources = _stage_compiled_inputs(raw_manifest, publish_root=publish_root)
    source_references = {
        source: _relative_manifest_path(publish_root / relative, sidecar_path.parent)
        for source, relative in staged_sources.items()
    }
    manifest = _canonicalize_locations(
        raw_manifest,
        location_roots,
        source_references,
    )
    assert isinstance(manifest, dict)
    manifest["extension"] = destination.name

    wheel_relative = (
        Path("provenance")
        / "wheels"
        / Path(*produced.module.split("."))
        / produced.wheel_path.name
    )
    wheel_destination = publish_root / wheel_relative
    manifest["wheel"] = _relative_manifest_path(wheel_destination, sidecar_path.parent)

    raw_source_plan = manifest.get("source_plan")
    if isinstance(raw_source_plan, dict):
        source_plan = {
            key: raw_source_plan[key]
            for key in (
                "kind",
                "target_id",
                "target_name",
                "target_selector",
                "target_type",
            )
            if key in raw_source_plan
        }
        source_plan["schema_version"] = 1
        source_plan["plan"] = _relative_manifest_path(
            plan_metadata["intro_targets"], sidecar_path.parent
        )
        source_plan["compile_commands"] = _relative_manifest_path(
            plan_metadata["compile_commands"], sidecar_path.parent
        )
        source_plan["plan_sha256"] = _sha256_file(plan_metadata["intro_targets"])
        source_plan["compile_commands_sha256"] = _sha256_file(
            plan_metadata["compile_commands"]
        )
        source_plan_identity = dict(source_plan)
        source_plan["digest"] = hashlib.sha256(
            json.dumps(
                source_plan_identity,
                sort_keys=True,
                separators=(",", ":"),
            ).encode("utf-8")
        ).hexdigest()
        manifest["source_plan"] = source_plan
        build = manifest.get("build")
        if isinstance(build, dict):
            build["source_plan_digest"] = source_plan["digest"]

    closure = manifest.get("object_closure")
    if not isinstance(closure, dict):
        raise SourceExtensionProducerError(
            f"built extension {produced.module} lost object-closure custody"
        )
    objects = closure.get("objects")
    if not isinstance(objects, list) or not objects:
        raise SourceExtensionProducerError(
            f"built extension {produced.module} has no final object closure"
        )
    manifest["sources"] = [
        str(item["source"])
        for item in objects
        if isinstance(item, Mapping) and isinstance(item.get("source"), str)
    ]
    raw_regenerations = manifest.get("cython_standalone")
    if isinstance(raw_regenerations, list):
        manifest["cython_standalone"] = [
            {
                key: regeneration[key]
                for key in (
                    "pyx",
                    "regenerated_c",
                    "standalone",
                    "cython_version",
                    "cython_argv",
                    "compile_profile",
                    "compile_args",
                    "cimport_packages",
                    "dependencies",
                    "working_directory",
                )
                if key in regeneration
            }
            for regeneration in raw_regenerations
            if isinstance(regeneration, Mapping)
        ]
    closure["closure_sha256"] = _object_closure_digest(
        closure,
        manifest_dir=sidecar_path.parent,
        manifest=manifest,
    )
    build = manifest.get("build")
    if isinstance(build, dict):
        build["object_closure_sha256"] = closure["closure_sha256"]
    manifest = _compact_source_extension_manifest(manifest)
    _validate_compact_source_extension_manifest(manifest)
    _require_location_neutral(
        manifest,
        authority=f"extension manifest for {produced.module}",
    )
    try:
        wheel_sha256, _embedded_manifest = _rewrite_staged_extension_wheel(
            produced.wheel_path,
            wheel_destination,
            canonical_embedded_manifest=canonical_embedded_manifest,
        )
    except ValueError as exc:
        raise SourceExtensionProducerError(
            f"failed to finalize canonical extension wheel: {exc}"
        ) from exc
    manifest["wheel_sha256"] = wheel_sha256
    _atomic_write_json(sidecar_path, manifest, sort_keys=True, indent=2)
    return replace(
        produced,
        artifact_path=destination,
        artifact_manifest_path=sidecar_path,
        wheel_path=wheel_destination,
        wheel_sha256=wheel_sha256,
        object_closure_sha256=str(closure["closure_sha256"]),
    )


def _stage_canonical_metadata_file(
    source: Path,
    destination: Path,
    *,
    location_roots: Sequence[tuple[Path, str]],
    normalize_meson_dependency_ids: bool = False,
) -> Path:
    try:
        text = source.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        raise SourceExtensionProducerError(
            f"cannot stage build metadata {source}: {exc}"
        ) from exc
    try:
        payload = json.loads(text)
    except json.JSONDecodeError:
        canonical = _canonicalize_location_string(text, location_roots)
        _require_location_neutral(
            canonical,
            authority=f"canonical build metadata {source}",
        )
        _atomic_write_text(destination, canonical)
    else:
        canonical_payload = (
            _canonicalize_meson_metadata(payload, location_roots)
            if normalize_meson_dependency_ids
            else _canonicalize_locations(payload, location_roots)
        )
        _require_location_neutral(
            canonical_payload,
            authority=f"canonical build metadata {source}",
        )
        _atomic_write_json(destination, canonical_payload, sort_keys=True, indent=2)
    return destination


def _stage_build_metadata(
    *,
    publish_root: Path,
    metadata_root: Path,
    intro_targets: Path,
    compile_commands: Path,
    intro_installed: Path,
    config_tool_cross: Path | None,
    target_metadata_payload: Mapping[str, Any],
    location_roots: Sequence[tuple[Path, str]],
) -> tuple[dict[str, Path], dict[str, Any]]:
    metadata_publish_root = publish_root / "provenance" / "metadata"
    staged = {
        "intro_targets": _stage_canonical_metadata_file(
            intro_targets,
            metadata_publish_root / "meson" / "intro-targets.json",
            location_roots=location_roots,
            normalize_meson_dependency_ids=True,
        ),
        "compile_commands": _stage_canonical_metadata_file(
            compile_commands,
            metadata_publish_root / "meson" / "compile-commands.json",
            location_roots=location_roots,
        ),
        "intro_installed": _stage_canonical_metadata_file(
            intro_installed,
            metadata_publish_root / "meson" / "intro-installed.json",
            location_roots=location_roots,
        ),
    }
    if config_tool_cross is not None:
        staged["config_tool_cross"] = _stage_canonical_metadata_file(
            config_tool_cross,
            metadata_publish_root / "meson" / "build-config-tools.cross",
            location_roots=location_roots,
        )
    for path in sorted(metadata_root.rglob("*")):
        if not path.is_file():
            continue
        relative = path.relative_to(metadata_root)
        if relative.as_posix() == "source-extension-target-metadata.json":
            continue
        staged[f"target/{relative.as_posix()}"] = _stage_canonical_metadata_file(
            path,
            metadata_publish_root / "target" / relative,
            location_roots=location_roots,
        )
    canonical_target_metadata = _canonicalize_locations(
        target_metadata_payload,
        location_roots,
    )
    if not isinstance(canonical_target_metadata, dict):
        raise SourceExtensionProducerError("canonical target metadata is not an object")
    python_pc = staged.get("target/pkgconfig/python3.pc")
    meson_cross = staged.get("target/meson.cross")
    if python_pc is None or meson_cross is None:
        raise SourceExtensionProducerError(
            "canonical target metadata lost python3.pc or meson.cross"
        )
    canonical_target_metadata["digests"] = {
        "python_pc_sha256": _sha256_file(python_pc),
        "meson_cross_sha256": _sha256_file(meson_cross),
    }
    canonical_target_metadata.pop("digest", None)
    encoded = json.dumps(
        canonical_target_metadata,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    canonical_target_metadata["digest"] = hashlib.sha256(encoded).hexdigest()
    _require_location_neutral(
        canonical_target_metadata,
        authority="canonical source-extension target metadata",
    )
    target_sidecar = (
        metadata_publish_root / "target" / "source-extension-target-metadata.json"
    )
    _atomic_write_json(
        target_sidecar,
        canonical_target_metadata,
        sort_keys=True,
        indent=2,
    )
    staged["target/source-extension-target-metadata.json"] = target_sidecar
    return staged, canonical_target_metadata


def _source_package_input_role(relative: Path) -> str:
    posix = relative.as_posix()
    if posix == "extension_set_manifest.json":
        return "set-manifest"
    if posix.endswith(".molt.wasm"):
        return "extension-artifact"
    if posix.endswith(".molt.wasm.extension_manifest.json"):
        return "extension-manifest"
    if posix.startswith("provenance/compiled-inputs/"):
        return "compiled-input"
    if posix.startswith("provenance/wheels/"):
        return "wheel"
    if posix.startswith("provenance/metadata/"):
        return "build-metadata"
    if relative.suffix in {".py", ".pyi"} or relative.name == "py.typed":
        return "python-source"
    return "package-data"


def _target_tool_commands(
    metadata_payload: Mapping[str, Any],
) -> dict[str, tuple[str, ...]]:
    toolchain = metadata_payload.get("toolchain")
    raw_commands = toolchain.get("commands") if isinstance(toolchain, Mapping) else None
    if not isinstance(raw_commands, Mapping):
        raise SourceExtensionProducerError(
            "target metadata has no materialized LLVM/WASI command family"
        )
    required_roles = ("ar", "c", "cpp", "ld", "nm", "ranlib", "strip")
    commands: dict[str, tuple[str, ...]] = {}
    for command_role in required_roles:
        raw_command = raw_commands.get(command_role)
        if (
            not isinstance(raw_command, list)
            or not raw_command
            or not all(isinstance(item, str) and item for item in raw_command)
        ):
            raise SourceExtensionProducerError(
                f"target metadata is missing materialized {command_role} command"
            )
        commands[command_role] = tuple(raw_command)
    return commands


def _producer_location_roots(
    *,
    source_root: Path,
    build_root: Path,
    transaction_root: Path,
    metadata_payload: Mapping[str, Any],
    config_tools: Sequence[_SourceBuildConfigTool],
) -> tuple[tuple[Path, str], ...]:
    roots: list[tuple[Path, str]] = [
        (source_root, "@source"),
        (build_root, "@build"),
        (transaction_root, "@transaction"),
        (_REPO_ROOT, "@molt"),
        (Path(sys.prefix), "@python-env"),
        (Path(sys.base_prefix), "@python-base"),
    ]
    for scheme, token in (
        ("include", "@python-include"),
        ("platinclude", "@python-platform-include"),
    ):
        raw_path = sysconfig.get_path(scheme)
        if raw_path:
            roots.append((Path(raw_path), token))
    abi = metadata_payload.get("abi")
    raw_include_dirs = abi.get("include_dirs") if isinstance(abi, Mapping) else None
    if isinstance(raw_include_dirs, list):
        roots.extend(
            (Path(raw_path), f"@molt-abi-include/{index}")
            for index, raw_path in enumerate(raw_include_dirs)
            if isinstance(raw_path, str) and raw_path
        )
    toolchain = metadata_payload.get("toolchain")
    if isinstance(toolchain, Mapping):
        wasi_sysroot = toolchain.get("wasi_sysroot")
        if isinstance(wasi_sysroot, str) and wasi_sysroot:
            roots.append((Path(wasi_sysroot), "@wasi-sysroot"))
        archives = toolchain.get("link_probe_archives")
        compiler_builtins = (
            archives.get("compiler_builtins") if isinstance(archives, Mapping) else None
        )
        compiler_builtins_path = (
            compiler_builtins.get("path")
            if isinstance(compiler_builtins, Mapping)
            else None
        )
        if isinstance(compiler_builtins_path, str) and compiler_builtins_path:
            builtins_path = Path(compiler_builtins_path)
            roots.append((builtins_path, "@compiler-builtins"))
            roots.append((builtins_path.parent, "@rust-target-libdir"))
        tools = toolchain.get("tools")
        if isinstance(tools, Mapping):
            tool_paths: dict[str, Path] = {}
            for role, tool in sorted(tools.items()):
                if not isinstance(tool, Mapping):
                    continue
                raw_path = tool.get("path")
                if isinstance(raw_path, str) and raw_path:
                    tool_paths[str(role)] = (
                        Path(raw_path).expanduser().resolve().parent
                    )
            tool_parents = set(tool_paths.values())
            if len(tool_parents) == 1:
                parent = next(iter(tool_parents))
                roots.extend(((parent, "@llvm-bin"), (parent.parent, "@llvm-prefix")))
            else:
                for role, parent in tool_paths.items():
                    roots.extend(
                        (
                            (parent, f"@llvm-{role}-bin"),
                            (parent.parent, f"@llvm-{role}-prefix"),
                        )
                    )
    config_by_parent: dict[Path, list[str]] = {}
    for tool in config_tools:
        config_by_parent.setdefault(tool.path.resolve().parent, []).append(tool.name)
    if len(config_by_parent) == 1:
        roots.append((next(iter(config_by_parent)), "@python-scripts"))
    else:
        for parent, names in sorted(config_by_parent.items()):
            role = "-".join(sorted(name.replace("_", "-") for name in names))
            roots.append((parent, f"@config-{role}-bin"))
    deduped: dict[Path, str] = {}
    for path, token in roots:
        deduped.setdefault(path.resolve(), token)
    return tuple(deduped.items())


def _validate_complete_publish_root(
    *,
    publish_root: Path,
    extension_set: ScientificExtensionSet,
    set_manifest: Mapping[str, Any],
) -> None:
    target_metadata = set_manifest.get("target_metadata")
    if not isinstance(target_metadata, Mapping):
        raise SourceExtensionProducerError(
            "extension-set manifest target_metadata is missing"
        )
    target_identity = dict(target_metadata)
    target_digest = target_identity.pop("digest", None)
    computed_target_digest = hashlib.sha256(
        json.dumps(
            target_identity,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    ).hexdigest()
    if target_digest != computed_target_digest:
        raise SourceExtensionProducerError(
            "extension-set target_metadata identity checksum is false"
        )
    target_sidecar_path = (
        publish_root
        / "provenance"
        / "metadata"
        / "target"
        / "source-extension-target-metadata.json"
    )
    try:
        target_sidecar = json.loads(target_sidecar_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise SourceExtensionProducerError(
            f"cannot read canonical target metadata sidecar: {exc}"
        ) from exc
    if target_sidecar != target_metadata:
        raise SourceExtensionProducerError(
            "extension-set target_metadata differs from its canonical sidecar"
        )
    target_digests = target_metadata.get("digests")
    if not isinstance(target_digests, Mapping):
        raise SourceExtensionProducerError(
            "extension-set target_metadata digests are missing"
        )
    target_files = {
        "python_pc_sha256": (
            publish_root / "provenance/metadata/target/pkgconfig/python3.pc"
        ),
        "meson_cross_sha256": (publish_root / "provenance/metadata/target/meson.cross"),
    }
    for digest_name, target_file in target_files.items():
        if not target_file.is_file() or target_digests.get(digest_name) != _sha256_file(
            target_file
        ):
            raise SourceExtensionProducerError(
                f"extension-set target_metadata {digest_name} is false"
            )
    toolchain = target_metadata.get("toolchain")
    tools = toolchain.get("tools") if isinstance(toolchain, Mapping) else None
    target_commands = (
        toolchain.get("commands") if isinstance(toolchain, Mapping) else None
    )
    tool_roles = {
        "ar": "ar",
        "c": "cc",
        "cpp": "cxx",
        "ld": "wasm_ld",
        "nm": "nm",
        "ranlib": "ranlib",
        "strip": "strip",
    }
    if not isinstance(tools, Mapping) or set(tools) != set(tool_roles.values()):
        raise SourceExtensionProducerError(
            "extension-set target metadata has an incomplete tool identity family"
        )
    if not isinstance(target_commands, Mapping) or set(target_commands) != set(
        tool_roles
    ):
        raise SourceExtensionProducerError(
            "extension-set target metadata has an incomplete command family"
        )
    for command_role, tool_role in tool_roles.items():
        identity = tools.get(tool_role)
        command = target_commands.get(command_role)
        if not (
            isinstance(identity, Mapping)
            and isinstance(identity.get("path"), str)
            and isinstance(identity.get("sha256"), str)
            and len(str(identity.get("sha256"))) == 64
            and isinstance(identity.get("command"), list)
            and identity.get("command")
            and isinstance(command, list)
            and command
            and command[0] == identity["command"][0]
        ):
            raise SourceExtensionProducerError(
                f"extension-set target metadata {command_role} identity is invalid"
            )

    installed_files = set_manifest.get("installed_package_files")
    if (
        not isinstance(installed_files, list)
        or not all(isinstance(item, str) and item for item in installed_files)
        or installed_files != sorted(set(installed_files))
    ):
        raise SourceExtensionProducerError(
            "extension-set installed package inventory is invalid"
        )
    missing_required = sorted(
        set(extension_set.required_installed_files) - set(installed_files)
    )
    if missing_required:
        raise SourceExtensionProducerError(
            "extension-set installed package inventory is missing configured files: "
            + ", ".join(missing_required)
        )
    missing_installed = [
        relative
        for relative in installed_files
        if not (
            publish_root
            / validate_source_package_relative_path(
                relative,
                field="extension-set installed_package_files entry",
            )
        ).is_file()
    ]
    if missing_installed:
        raise SourceExtensionProducerError(
            "extension-set installed package files are absent on disk: "
            + ", ".join(missing_installed)
        )

    configured_contracts = tuple(
        (
            spec.module,
            spec.target,
            spec.python_exports,
            spec.capabilities,
            spec.provided_capsules,
            spec.exclude_linked_static_libraries,
        )
        for spec in extension_set.extensions
    )
    raw_extensions = set_manifest.get("extensions")
    if not isinstance(raw_extensions, list) or not all(
        isinstance(item, Mapping)
        and isinstance(item.get("module"), str)
        and isinstance(item.get("target"), str)
        and isinstance(item.get("python_exports"), list)
        and isinstance(item.get("capabilities"), list)
        and isinstance(item.get("provided_capsules"), list)
        and isinstance(item.get("exclude_linked_static_libraries"), list)
        and all(isinstance(value, str) for value in item["python_exports"])
        and all(isinstance(value, str) for value in item["capabilities"])
        and all(isinstance(value, str) for value in item["provided_capsules"])
        and all(
            isinstance(value, str) for value in item["exclude_linked_static_libraries"]
        )
        for item in raw_extensions
    ):
        raise SourceExtensionProducerError(
            "extension-set manifest extensions must be module objects"
        )
    manifest_contracts = tuple(
        (
            str(item["module"]),
            str(item["target"]),
            tuple(item["python_exports"]),
            tuple(item["capabilities"]),
            tuple(item["provided_capsules"]),
            tuple(item["exclude_linked_static_libraries"]),
        )
        for item in raw_extensions
    )
    if manifest_contracts != configured_contracts:
        raise SourceExtensionProducerError(
            "extension-set manifest typed extension contracts differ from "
            f"configured complete set: expected {configured_contracts}, "
            f"got {manifest_contracts}"
        )

    expected_sidecars = {
        publish_root.joinpath(
            *spec.module.split(".")[:-1],
            f"{spec.target}.molt.wasm.extension_manifest.json",
        ).resolve()
        for spec in extension_set.extensions
    }
    actual_sidecars = {
        path.resolve()
        for path in publish_root.glob("**/*.molt.wasm.extension_manifest.json")
        if path.is_file()
    }
    if actual_sidecars != expected_sidecars:
        missing = sorted(str(path) for path in expected_sidecars - actual_sidecars)
        unexpected = sorted(str(path) for path in actual_sidecars - expected_sidecars)
        raise SourceExtensionProducerError(
            "published extension sidecars differ from configured complete set; "
            f"missing={missing}, unexpected={unexpected}"
        )
    missing_artifacts = [
        str(path).removesuffix(".extension_manifest.json")
        for path in sorted(expected_sidecars)
        if not Path(str(path).removesuffix(".extension_manifest.json")).is_file()
    ]
    if missing_artifacts:
        raise SourceExtensionProducerError(
            "published extension set is missing configured artifacts: "
            + ", ".join(missing_artifacts)
        )
    entries_by_module = {
        str(item["module"]): item
        for item in raw_extensions
        if isinstance(item, Mapping)
    }
    for spec in extension_set.extensions:
        sidecar_path = publish_root.joinpath(
            *spec.module.split(".")[:-1],
            f"{spec.target}.molt.wasm.extension_manifest.json",
        ).resolve()
        try:
            sidecar = json.loads(sidecar_path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError) as exc:
            raise SourceExtensionProducerError(
                f"failed to read published extension sidecar {sidecar_path}: {exc}"
            ) from exc
        if not isinstance(sidecar, Mapping) or sidecar.get("module") != spec.module:
            raise SourceExtensionProducerError(
                f"published extension sidecar has wrong module: {sidecar_path}"
            )
        try:
            _validate_compact_source_extension_manifest(sidecar)
            _require_location_neutral(
                sidecar,
                authority=f"published extension sidecar {sidecar_path}",
            )
        except ValueError as exc:
            raise SourceExtensionProducerError(str(exc)) from exc
        entry = entries_by_module[spec.module]
        closure = sidecar.get("object_closure")
        closure_sha256 = (
            closure.get("closure_sha256") if isinstance(closure, Mapping) else None
        )
        checksums = {
            "artifact_sha256": sidecar.get("extension_sha256"),
            "wheel_sha256": sidecar.get("wheel_sha256"),
            "object_closure_sha256": closure_sha256,
        }
        for field_name, sidecar_value in checksums.items():
            if entry.get(field_name) != sidecar_value:
                raise SourceExtensionProducerError(
                    f"extension-set manifest {field_name} differs from sidecar for "
                    f"{spec.module}"
                )
        closure_objects = (
            closure.get("objects") if isinstance(closure, Mapping) else None
        )
        if not isinstance(closure_objects, list):
            raise SourceExtensionProducerError(
                f"extension sidecar object closure is invalid for {spec.module}"
            )
        for object_index, closure_object in enumerate(closure_objects):
            if not isinstance(closure_object, Mapping):
                raise SourceExtensionProducerError(
                    f"extension sidecar object[{object_index}] is invalid"
                )
            closure_object = cast(Mapping[str, Any], closure_object)
            source = closure_object.get("source")
            try:
                compile_command = _manifest_sequence(
                    sidecar, closure_object, "compile_command"
                )
                symbol_command = _manifest_sequence(
                    sidecar, closure_object, "symbol_command"
                )
            except ValueError as exc:
                raise SourceExtensionProducerError(str(exc)) from exc
            compiler_role = (
                "cpp"
                if isinstance(source, str)
                and Path(source).suffix.lower() in {".cc", ".cpp", ".cxx", ".c++"}
                else "c"
            )
            expected_compiler = target_commands[compiler_role]
            expected_nm = target_commands["nm"]
            if not (
                isinstance(compile_command, list)
                and compile_command[: len(expected_compiler)] == expected_compiler
                and isinstance(symbol_command, list)
                and symbol_command == expected_nm
            ):
                raise SourceExtensionProducerError(
                    f"extension sidecar object[{object_index}] for {spec.module} "
                    "did not consume the canonical compiler/nm commands"
                )
            if closure_object.get("unit_sha256") != _object_unit_sha256(
                sidecar, closure_object
            ):
                raise SourceExtensionProducerError(
                    f"extension sidecar object[{object_index}] for {spec.module} "
                    "has false content-addressed unit identity"
                )
        artifact_path = Path(str(sidecar_path).removesuffix(".extension_manifest.json"))
        if _sha256_file(artifact_path) != entry.get("artifact_sha256"):
            raise SourceExtensionProducerError(
                f"extension-set manifest artifact checksum differs from bytes for "
                f"{spec.module}"
            )


def _missing_installed_generated_inputs(
    *,
    intro_installed: Path,
    source_root: Path,
    build_root: Path,
) -> set[Path]:
    try:
        payload = json.loads(intro_installed.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise SourceExtensionProducerError(
            f"failed to read Meson installed-file introspection {intro_installed}: {exc}"
        ) from exc
    if not isinstance(payload, Mapping):
        raise SourceExtensionProducerError(
            f"Meson installed-file introspection must be an object: {intro_installed}"
        )
    missing: set[Path] = set()
    for raw_source in payload:
        if not isinstance(raw_source, str):
            raise SourceExtensionProducerError(
                "Meson installed-file introspection source keys must be strings"
            )
        source = _installed_source_path(
            raw_source, source_root=source_root, build_root=build_root
        )
        if (
            not source.is_file()
            and source.suffix.lower() in _GENERATED_INPUT_SUFFIXES
            and source.is_relative_to(build_root.resolve())
        ):
            missing.add(source)
    return missing


def _missing_extension_generated_inputs(
    *,
    backend: _SourceNinjaDriver,
    build_root: Path,
    intro_targets: Path,
    extension_set: ScientificExtensionSet,
) -> set[Path]:
    try:
        payload = json.loads(intro_targets.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise SourceExtensionProducerError(
            f"failed to read Meson target introspection {intro_targets}: {exc}"
        ) from exc
    if not isinstance(payload, list):
        raise SourceExtensionProducerError(
            f"Meson target introspection must be an array: {intro_targets}"
        )
    missing: set[Path] = set()
    ninja_inputs = _load_ninja_build_all_inputs(build_root)

    def standalone_regenerable(path: Path) -> bool:
        if path.suffix.lower() not in {".c", ".cc", ".cpp", ".cxx"}:
            return False
        if not any(
            dependency.suffix.lower() == ".pyx" and dependency.is_file()
            for dependency in ninja_inputs.get(path.resolve(), ())
        ):
            return False
        pyx_input, error = _source_extension_cython.generated_c_pyx_from_ninja(
            generated_c=path,
            build_root=build_root,
            ninja_command=backend.command,
        )
        if error is not None:
            raise SourceExtensionProducerError(error)
        return pyx_input is not None

    def target_names(target: Mapping[str, Any]) -> set[str]:
        names = {
            str(target.get("id", "")),
            str(target.get("name", "")),
        }
        names.update(_meson_target_filename_names(target.get("filename")))
        return names

    def excluded_target(
        target: Mapping[str, Any], excluded_libraries: Sequence[str]
    ) -> bool:
        normalized_exclusions = {
            Path(name).name.lower().removeprefix("lib").removesuffix(".a")
            for name in excluded_libraries
        }
        normalized_names = {
            Path(name).name.lower().removeprefix("lib").removesuffix(".a")
            for name in target_names(target)
        }
        return bool(normalized_exclusions.intersection(normalized_names))

    for spec in extension_set.extensions:
        matches = [
            target
            for target in payload
            if isinstance(target, Mapping)
            and str(target.get("type", "")).strip() in _MESON_EXTENSION_TARGET_TYPES
            and spec.target in target_names(target)
        ]
        if len(matches) != 1:
            raise SourceExtensionProducerError(
                f"Meson target selector {spec.target!r} for {spec.module} matched "
                f"{len(matches)} extension targets"
            )
        primary = matches[0]
        linked = _meson_linked_static_library_targets(
            primary_target=primary,
            payload=payload,
            build_root=build_root,
        )
        excluded = tuple(
            target
            for target in linked
            if excluded_target(target, spec.exclude_linked_static_libraries)
        )
        targets = (primary, *(target for target in linked if target not in excluded))
        excluded_outputs = {
            output
            for target in excluded
            for output in _meson_target_output_paths(
                target.get("filename"), build_root=build_root
            )
        }
        for target in targets:
            groups = target.get("target_sources")
            if not isinstance(groups, list):
                continue
            for group in groups:
                generated = (
                    group.get("generated_sources")
                    if isinstance(group, Mapping)
                    else None
                )
                if not isinstance(generated, list):
                    continue
                for raw_path in generated:
                    if not isinstance(raw_path, str):
                        raise SourceExtensionProducerError(
                            f"Meson target {target.get('name')!r} has a non-string "
                            "generated source"
                        )
                    path = Path(raw_path).expanduser()
                    if not path.is_absolute():
                        path = build_root / path
                    path = path.resolve()
                    if not path.is_file() and not standalone_regenerable(path):
                        missing.add(path)
        queue = [
            output
            for target in targets
            for output in _meson_target_output_paths(
                target.get("filename"), build_root=build_root
            )
        ]
        seen: set[Path] = set()
        while queue:
            output = queue.pop()
            if output in seen or output in excluded_outputs:
                continue
            seen.add(output)
            for dependency in ninja_inputs.get(output, ()):
                if dependency in excluded_outputs:
                    continue
                if dependency in ninja_inputs:
                    queue.append(dependency)
                if (
                    not dependency.is_file()
                    and dependency.suffix.lower() in _GENERATED_INPUT_SUFFIXES
                    and dependency in ninja_inputs
                    and not standalone_regenerable(dependency)
                ):
                    missing.add(dependency)
    return {
        path.resolve()
        for path in missing
        if path.resolve().is_relative_to(build_root.resolve())
    }


def _materialize_generated_inputs(
    *,
    backend: _SourceNinjaDriver,
    source_root: Path,
    build_root: Path,
    intro_targets: Path,
    intro_installed: Path,
    extension_set: ScientificExtensionSet,
) -> tuple[Path, ...]:
    missing = _missing_installed_generated_inputs(
        intro_installed=intro_installed,
        source_root=source_root,
        build_root=build_root,
    )
    missing.update(
        _missing_extension_generated_inputs(
            backend=backend,
            build_root=build_root,
            intro_targets=intro_targets,
            extension_set=extension_set,
        )
    )
    if not missing:
        return ()
    relative_targets = tuple(
        sorted(path.relative_to(build_root.resolve()).as_posix() for path in missing)
    )
    result = _run_process(
        (*backend.command, "-C", str(build_root), *relative_targets),
        cwd=source_root,
    )
    if result.returncode != 0:
        detail = "\n".join(
            part.strip() for part in (result.stdout, result.stderr) if part.strip()
        )
        raise SourceExtensionProducerError(
            "upstream Meson generator materialization failed "
            f"({result.returncode}): {detail}"
        )
    still_missing = [path for path in sorted(missing) if not path.is_file()]
    if still_missing:
        raise SourceExtensionProducerError(
            "upstream Meson reported successful generator materialization but "
            "outputs remain absent: " + ", ".join(str(path) for path in still_missing)
        )
    return tuple(sorted(missing))


def _recover_and_prune_producer_transactions(
    destination: Path, *, publication_custody: SourceExtensionPublicationCustody
) -> None:
    """Recover durable publication commits, then remove abandoned build state."""

    for prior in sorted(destination.parent.glob(f".{destination.name}.produce-*")):
        recovered_publication = recover_source_extension_publication(
            prior, custody=publication_custody
        )
        if (
            recovered_publication is not None
            and recovered_publication.get("state") != "committed"
        ):
            raise SourceExtensionProducerError(
                f"identity publication recovery did not commit: {prior}"
            )
        recover_source_package_seal_commits(prior / "package-store")
        retired = prior / "retired-destination"
        if retired.exists():
            raise SourceExtensionProducerError(
                "legacy producer transaction contains a retired canonical "
                f"destination and requires manual custody review: {retired}"
            )
        _remove_file_or_tree(prior)


def produce_source_extension_set(
    *,
    package: str,
    module_set: str,
    source: str,
    build_root: str,
    target: str = "wasm",
    abi_tier: str = "cpython-abi",
    expected_identity_sha256: str | None = None,
    expected_candidate_identity_sha256: str | None = None,
    json_output: bool = False,
) -> int:
    source_root = Path(source).expanduser().resolve()
    resolved_build_root = Path(build_root).expanduser().resolve()
    transaction_root: Path | None = None
    producer_lock = None
    publication_custody = None
    published = False
    incumbent_identity: Mapping[str, Any] | None = None
    incumbent_seal = None
    try:
        if not source_root.is_dir():
            raise SourceExtensionProducerError(
                f"source checkout is not a directory: {source_root}"
            )
        extension_set = scientific_extension_set(package, module_set)
        locked_environment = source_build_environment(
            _REPO_ROOT, extension_set.build_dependency_group
        )
        if not locked_environment.active:
            locked_environment = provision_source_build_environment(
                _REPO_ROOT, extension_set.build_dependency_group
            )
            return _run_locked_source_extension_producer(
                locked_environment,
                package=package,
                module_set=module_set,
                source=str(source_root),
                build_root=str(resolved_build_root),
                target=target,
                abi_tier=abi_tier,
                expected_identity_sha256=expected_identity_sha256,
                expected_candidate_identity_sha256=(expected_candidate_identity_sha256),
                json_output=json_output,
            )
        destination = scientific_extension_set_root(extension_set)
        destination.parent.mkdir(parents=True, exist_ok=True)
        lock_path = destination.parent / f".{destination.name}.producer.lock"
        producer_lock = _acquire_file_lock(
            lock_path,
            timeout_s=300.0,
            timeout_message=(
                "timed out waiting for the canonical extension-set producer "
                f"lock {lock_path}; another producer owns {destination}"
            ),
        )
        publication_custody = _source_extension_publication_custody(
            destination, producer_lock
        )
        _recover_and_prune_producer_transactions(
            destination, publication_custody=publication_custody
        )
        if destination.exists():
            if (
                expected_identity_sha256 is None
                or expected_candidate_identity_sha256 is None
            ):
                raise SourceExtensionProducerError(
                    "canonical extension seal already exists; replacement requires "
                    "both --expected-identity-sha256 and "
                    "--expected-candidate-identity-sha256 so publication proves "
                    "the complete compare-and-swap transition before mutation"
                )
            incumbent_seal = verify_source_package_seal(destination)
            incumbent_identity = _require_expected_source_extension_set_identity(
                incumbent_seal.payload_root,
                expected_identity_sha256,
                inventory_sha256={
                    entry.relative_path: entry.sha256 for entry in incumbent_seal.files
                },
            )
        elif expected_identity_sha256 is not None:
            raise SourceExtensionProducerError(
                "--expected-identity-sha256 requires an incumbent canonical seal"
            )
        verify_source_checkout(package, source_root)
        _provision_recursive_submodules(source_root)
        submodules = _verify_recursive_submodules(source_root)
        verify_cpython_abi_headers(repo_root=_REPO_ROOT)
        build_environment = _ensure_source_build_environment(
            source_root, custody=locked_environment.custody
        )
        meson_driver = _source_meson_driver(source_root)
        ninja_driver = _source_ninja_driver(source_root)
        discovered_config_tools = _source_build_config_tools(build_environment)
        if extension_set.use_pkg_config:
            pkg_config_tool = _ensure_meson_pkg_config(source_root)
            if any(
                tool.name == pkg_config_tool.name for tool in discovered_config_tools
            ):
                raise SourceExtensionProducerError(
                    "upstream build requirements duplicate Molt's pkg-config "
                    "tool authority"
                )
            build_config_tools = tuple(
                sorted(
                    (*discovered_config_tools, pkg_config_tool),
                    key=lambda item: item.name,
                )
            )
        else:
            build_config_tools = discovered_config_tools
        _require_fresh_build_root(resolved_build_root)

        transaction_root = Path(
            tempfile.mkdtemp(
                prefix=f".{destination.name}.produce-", dir=destination.parent
            )
        )
        publish_root = transaction_root / "publish"
        publish_root.mkdir()
        metadata_root = transaction_root / "target-metadata"
        metadata, metadata_errors = _materialize_source_extension_target_metadata(
            molt_root=_REPO_ROOT,
            out_dir=metadata_root,
            target_triple=target,
            abi_tier=abi_tier,
        )
        if metadata is None or metadata_errors:
            raise SourceExtensionProducerError(
                "failed to materialize source-extension target metadata: "
                + "; ".join(metadata_errors)
            )
        tool_commands = _target_tool_commands(metadata.payload)
        config_tool_cross = _materialize_meson_config_tool_cross(
            metadata_root / "build-config-tools.cross",
            build_config_tools,
        )
        meson_cross_files = [metadata.meson_cross]
        if config_tool_cross is not None:
            meson_cross_files.append(config_tool_cross)
        _run_meson_setup(
            source_root=source_root,
            build_root=resolved_build_root,
            meson_cross_files=meson_cross_files,
            setup_args=extension_set.meson_setup_args,
            driver=meson_driver,
        )
        intro_targets, compile_commands, intro_installed = _require_real_meson_metadata(
            resolved_build_root
        )
        generated_inputs = _materialize_generated_inputs(
            backend=ninja_driver,
            source_root=source_root,
            build_root=resolved_build_root,
            intro_targets=intro_targets,
            intro_installed=intro_installed,
            extension_set=extension_set,
        )
        location_roots = _producer_location_roots(
            source_root=source_root,
            build_root=resolved_build_root,
            transaction_root=transaction_root,
            metadata_payload=metadata.payload,
            config_tools=build_config_tools,
        )
        staged_metadata, canonical_target_metadata = _stage_build_metadata(
            publish_root=publish_root,
            metadata_root=metadata_root,
            intro_targets=intro_targets,
            compile_commands=compile_commands,
            intro_installed=intro_installed,
            config_tool_cross=config_tool_cross,
            target_metadata_payload=metadata.payload,
            location_roots=location_roots,
        )
        installed_files = _stage_installed_package_files(
            intro_installed=intro_installed,
            source_root=source_root,
            build_root=resolved_build_root,
            package=extension_set.package,
            publish_root=publish_root,
            location_roots=location_roots,
            required_installed_files=extension_set.required_installed_files,
        )
        _preflight_extension_set_plans(
            source_root=source_root,
            build_root=resolved_build_root,
            intro_targets=intro_targets,
            compile_commands=compile_commands,
            extension_set=extension_set,
        )

        produced: list[_ProducedExtension] = []
        for index, spec in enumerate(extension_set.extensions):
            output_root = transaction_root / "builds" / f"{index:02d}-{spec.target}"
            result = _build_extension(
                source_root=source_root,
                build_root=resolved_build_root,
                intro_targets=intro_targets,
                compile_commands=compile_commands,
                output_root=output_root,
                module=spec.module,
                target_name=spec.target,
                python_exports=spec.python_exports,
                capabilities=spec.capabilities,
                provided_capsules=spec.provided_capsules,
                exclude_linked_static_libraries=(spec.exclude_linked_static_libraries),
                target=target,
                abi_tier=abi_tier,
                tool_commands=tool_commands,
                backend=ninja_driver,
            )
            result = _stage_extension(
                result,
                publish_root=publish_root,
                location_roots=location_roots,
                plan_metadata=staged_metadata,
            )
            produced.append(result)

        set_manifest = {
            "schema_version": SOURCE_EXTENSION_SET_SCHEMA_VERSION,
            "kind": "molt-source-extension-set",
            "package": extension_set.package,
            "name": extension_set.name,
            "seal_name": extension_set.seal_name,
            "source_head": _git_head(source_root),
            "submodules": [item.manifest_payload() for item in submodules],
            "target": target,
            "target_triple": metadata.target_triple,
            "abi_tier": abi_tier,
            "build_environment": build_environment.manifest_payload(),
            "meson": {
                "driver": meson_driver.manifest_payload(),
                "backend": ninja_driver.manifest_payload(),
                "build_root": "@build",
                "setup_args": list(extension_set.meson_setup_args),
                "intro_targets_sha256": _sha256_file(staged_metadata["intro_targets"]),
                "compile_commands_sha256": _sha256_file(
                    staged_metadata["compile_commands"]
                ),
                "intro_installed_sha256": _sha256_file(
                    staged_metadata["intro_installed"]
                ),
                "config_tool_cross_sha256": (
                    _sha256_file(staged_metadata["config_tool_cross"])
                    if "config_tool_cross" in staged_metadata
                    else None
                ),
                "config_tools": [
                    tool.manifest_payload() for tool in build_config_tools
                ],
                "pkg_config_requirement": (
                    MOLT_PKGCONF_REQUIREMENT if extension_set.use_pkg_config else None
                ),
                "generated_inputs": [
                    path.relative_to(resolved_build_root).as_posix()
                    for path in generated_inputs
                ],
            },
            "target_metadata": canonical_target_metadata,
            "installed_package_files": [
                relative
                for relative in sorted(
                    str(path.relative_to(publish_root)).replace("\\", "/")
                    for path in installed_files
                )
            ],
            "extensions": [
                {
                    "module": spec.module,
                    "target": spec.target,
                    "python_exports": list(spec.python_exports),
                    "capabilities": list(spec.capabilities),
                    "provided_capsules": list(spec.provided_capsules),
                    "exclude_linked_static_libraries": list(
                        spec.exclude_linked_static_libraries
                    ),
                    "artifact_sha256": item.artifact_sha256,
                    "wheel_sha256": item.wheel_sha256,
                    "object_closure_sha256": item.object_closure_sha256,
                }
                for spec, item in zip(extension_set.extensions, produced, strict=True)
            ],
        }
        _require_location_neutral(
            set_manifest,
            authority="source-extension set manifest",
        )
        _atomic_write_json(
            publish_root / "extension_set_manifest.json",
            set_manifest,
            sort_keys=True,
        )
        _validate_complete_publish_root(
            publish_root=publish_root,
            extension_set=extension_set,
            set_manifest=set_manifest,
        )
        package_store = transaction_root / "package-store"
        seal = stage_source_package_seal(
            package_store,
            [
                SourcePackageInput(
                    path,
                    path.relative_to(publish_root).as_posix(),
                    _source_package_input_role(path.relative_to(publish_root)),
                )
                for path in sorted(publish_root.rglob("*"))
                if path.is_file()
            ],
        )
        verify_source_package_seal(seal.root, expected_sha256=seal.seal_sha256)
        candidate_identity = _source_extension_set_identity(
            seal.payload_root,
            inventory_sha256={
                entry.relative_path: entry.sha256 for entry in seal.files
            },
        )
        if expected_candidate_identity_sha256 is not None and (
            candidate_identity["canonical_sha256"] != expected_candidate_identity_sha256
        ):
            evidence_path = transaction_root / "identity-comparison.json"
            comparison = {
                "schema_version": 1,
                "kind": "source-extension-candidate-identity",
                "expected_candidate_identity_sha256": (
                    expected_candidate_identity_sha256
                ),
                "candidate_seal_sha256": seal.seal_sha256,
                "candidate_identity": candidate_identity,
                "reproduced": False,
            }
            _atomic_write_json(evidence_path, comparison, sort_keys=True, indent=2)
            raise SourceExtensionProducerError(
                "candidate extension seal does not match declared canonical "
                f"identity {expected_candidate_identity_sha256}; incumbent "
                f"preserved and comparison evidence written to {evidence_path}"
            )
        if expected_identity_sha256 is not None:
            assert incumbent_identity is not None
            assert incumbent_seal is not None
            assert expected_candidate_identity_sha256 is not None
            assert publication_custody is not None
            comparison = _source_extension_reproduction_comparison(
                expected_incumbent_sha256=expected_identity_sha256,
                expected_candidate_sha256=expected_candidate_identity_sha256,
                incumbent_seal_sha256=incumbent_seal.seal_sha256,
                incumbent_identity=incumbent_identity,
                candidate_seal_sha256=seal.seal_sha256,
                candidate_identity=candidate_identity,
            )
            evidence_path = transaction_root / "identity-comparison.json"
            _atomic_write_json(evidence_path, comparison, sort_keys=True, indent=2)
            if not comparison["reproduced"]:
                raise SourceExtensionProducerError(
                    "candidate extension seal does not reproduce expected canonical "
                    f"identity {expected_candidate_identity_sha256}; incumbent preserved and "
                    f"comparison evidence written to {evidence_path}"
                )
            publication = publish_source_extension_candidate(
                custody=publication_custody,
                destination=destination,
                candidate_seal=seal,
                transaction_root=transaction_root,
                expected_incumbent_identity_sha256=expected_identity_sha256,
                expected_candidate_identity_sha256=(expected_candidate_identity_sha256),
            )
            published = True
            published_seal = verify_source_package_seal(destination)
            data = {
                "package": extension_set.package,
                "module_set": extension_set.name,
                "root": str(destination),
                "module_root": str(published_seal.payload_root),
                "seal_sha256": published_seal.seal_sha256,
                "candidate_seal_sha256": seal.seal_sha256,
                "identity_sha256": expected_candidate_identity_sha256,
                "reproduced": True,
                "no_op": publication["no_op"],
                "upgraded": publication["upgraded"],
                "modules": [item.module for item in produced],
                "target": metadata.target_triple,
                "abi_tier": abi_tier,
            }
            if json_output:
                _emit_json(
                    _json_payload("extension-produce-set", "ok", data=data),
                    json_output=True,
                )
            else:
                action = (
                    "without replacement" if publication["no_op"] else "by CAS upgrade"
                )
                print(f"Reproduced extension set {action}: {destination}")
                print(f"Canonical identity: {expected_candidate_identity_sha256}")
            return 0
        if destination.exists():
            raise SourceExtensionProducerError(
                "internal publication error: incumbent identity guard did not "
                "terminate existing-destination production as a verified no-op"
            )
        commit = prepare_source_package_seal_commit(
            package_store,
            seal,
            destination,
        )
        committed = commit_source_package_seal(commit)
        published_seal = verify_source_package_seal(
            committed.destination,
            expected_sha256=seal.seal_sha256,
        )
        published = True
        data = {
            "package": extension_set.package,
            "module_set": extension_set.name,
            "root": str(destination),
            "module_root": str(published_seal.payload_root),
            "seal_sha256": published_seal.seal_sha256,
            "modules": [item.module for item in produced],
            "installed_package_file_count": len(installed_files),
            "target": metadata.target_triple,
            "abi_tier": abi_tier,
        }
        if json_output:
            _emit_json(
                _json_payload("extension-produce-set", "ok", data=data),
                json_output=True,
            )
        else:
            print(f"Published extension set: {destination}")
            print("Modules: " + ", ".join(data["modules"]))
        return 0
    except (
        OSError,
        SourceExtensionProducerError,
        SourcePackageSealError,
        SourcePackageSealVerificationError,
        RuntimeError,
        ValueError,
    ) as exc:
        detail = str(exc)
        if transaction_root is not None and transaction_root.exists():
            detail += f"; preserved producer transaction: {transaction_root}"
        return _fail(detail, json_output, command="extension-produce-set")
    finally:
        if published and transaction_root is not None and transaction_root.exists():
            with contextlib.suppress(OSError):
                _remove_file_or_tree(transaction_root)
        if producer_lock is not None:
            _release_file_lock(producer_lock)
