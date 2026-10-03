"""Compiler admission shared by build, wrapper caches and daemon selection.

Developer execution uses the existing Cargo plan authority. Installed execution
uses its admitted release, never a hypothetical developer build of shipped Rust.
Only an explicit source-fingerprint operation owns reuse of live admissions.
"""

from __future__ import annotations

import hashlib
import os
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Mapping, Sequence, cast

from molt.exact_json import canonical_json_sha256, string_keyed_mapping

if TYPE_CHECKING:
    from molt.cli.runtime_cargo_plan import CargoResourceCustody, RuntimeCargoPlan
    from molt.compiler_distribution import InstalledCompiler
    from molt.toolchain_identity import StableRegularFileVersion


class CompilerIdentityError(ValueError):
    """A compiler input could not be admitted or changed before publication."""


def compiler_cargo_profile(env: Mapping[str, str]) -> str:
    from molt.cli.cargo_profiles import (
        _resolve_backend_cargo_profile_name_cached,
        _resolve_backend_profile_cached,
    )

    profile, error = _resolve_backend_profile_cached(env.get("MOLT_BACKEND_PROFILE"))
    if error:
        raise CompilerIdentityError(error)
    name = (
        "MOLT_DEV_BACKEND_CARGO_PROFILE"
        if profile == "dev"
        else "MOLT_RELEASE_BACKEND_CARGO_PROFILE"
    )
    selected, error = _resolve_backend_cargo_profile_name_cached(
        profile, env.get(name, "")
    )
    if error:
        raise CompilerIdentityError(error)
    return selected


@dataclass(frozen=True)
class InstalledCompilerAdmission:
    compiler: InstalledCompiler
    fingerprint: str


def installed_compiler_admission(
    root: Path,
    features: tuple[str, ...] | None = None,
    cargo_profile: str | None = None,
    *,
    fresh: bool = False,
) -> InstalledCompilerAdmission | None:
    from molt.cli.cache_fingerprints import _SOURCE_TREE_FINGERPRINT_TRANSACTION
    from molt.compiler_distribution import installed_compiler

    try:
        compiler = installed_compiler(root)
        if compiler is None:
            return None
        release = canonical_json_sha256(
            {
                "schema": "molt.installed-compiler-identity.v1",
                "source": compiler.source_sha,
                "files": compiler.files,
                "compiler": compiler.record,
                "launcher": compiler.launcher,
                "runtime": compiler.runtime,
            }
        )
        key = (os.fspath(root.resolve()), release)
        transaction = _SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
        if fresh or transaction is None or key not in transaction.installed_sources:
            compiler.verify_sources()
            compiler.verify_executing_package()
            if transaction is not None:
                transaction.installed_sources.add(key)
        # Source admission is reusable within the immutable operation. Executable
        # selection and bytes remain live, including an alias replaced mid-build.
        compiler.verify_binary(
            tuple(compiler.record["features"]) if features is None else features,
            compiler.record["profile"] if cargo_profile is None else cargo_profile,
        )
        return InstalledCompilerAdmission(compiler, release)
    except (OSError, ValueError) as exc:
        raise CompilerIdentityError(f"Installed compiler identity: {exc}") from exc


@dataclass(frozen=True)
class BackendBuildAdmission:
    plan: RuntimeCargoPlan
    resources: CargoResourceCustody
    fingerprint: str

    def verify(self) -> None:
        try:
            self.plan.verify()
            self.resources.verify()
        except (OSError, ValueError) as exc:
            raise CompilerIdentityError(
                f"Compiler build inputs changed: {exc}"
            ) from exc


def backend_build_admission(
    root: Path,
    features: tuple[str, ...],
    cargo_profile: str,
    env: Mapping[str, str],
) -> BackendBuildAdmission:
    from molt.cli.cache_fingerprints import _SOURCE_TREE_FINGERPRINT_TRANSACTION
    from molt.cli.cargo_execution import _cargo_build_env, _maybe_enable_native_cpu
    from molt.cli.runtime_build_identity import (
        _runtime_build_environment_identity,
    )
    from molt.cli.runtime_cargo_plan import (
        CargoResourceCustody,
        CargoResourceRoot,
        resolve_runtime_cargo_plan,
    )
    from molt.cli.runtime_paths import _cargo_target_root_cached
    from molt.llvm_toolchain import (
        LlvmToolchainConfigError,
        project_llvm_toolchain_environment,
        verify_available_llvm_toolchain,
    )

    transaction = _SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
    # The complete caller environment keys operation-local reuse; semantic output
    # identity below projects only admitted Cargo/build-script input domains.
    key = (
        os.fspath(root.resolve()),
        cargo_profile,
        ",".join(sorted(features)),
        canonical_json_sha256(dict(env)),
    )
    if transaction is not None and key in transaction.compiler_plans:
        return cast(BackendBuildAdmission, transaction.compiler_plans[key])
    try:
        prepared = _cargo_build_env(env)
        prepared["CARGO_TARGET_DIR"] = os.fspath(
            _cargo_target_root_cached(
                os.fspath(root),
                prepared.get("CARGO_TARGET_DIR"),
                os.fspath(Path.cwd()),
                prepared.get("MOLT_SESSION_ID", ""),
            )
        )
        llvm_roots = ()

        def compiler_environment(environment: Mapping[str, str]) -> Mapping[str, str]:
            nonlocal llvm_roots
            effective = dict(environment)
            _maybe_enable_native_cpu(effective)
            if "llvm" not in features:
                return effective
            verification = verify_available_llvm_toolchain(root, environ=effective)
            if verification is None:
                raise CompilerIdentityError(
                    "Selected LLVM backend has no admitted LLVM toolchain"
                )
            effective = project_llvm_toolchain_environment(
                root, verification, environ=effective
            )
            llvm_roots = (
                CargoResourceRoot("compiler/llvm-sdk", verification.prefix),
                CargoResourceRoot("compiler/llvm-config", verification.llvm_config),
            )
            return effective

        command = [
            "cargo",
            "build",
            "--locked",
            "--package",
            "molt-backend",
            "--bin",
            "molt-backend",
            "--profile",
            cargo_profile,
        ]
        if features:
            command.extend(("--no-default-features", "--features", ",".join(features)))
        plan = resolve_runtime_cargo_plan(
            root,
            env=prepared,
            cargo_command=command,
            requested_target=None,
            environment_transform=compiler_environment,
        )
        # llvm-sys consumes both selectors and prefix contents. Its build script
        # can link archives, query llvm-config and consume headers under this root.
        resources = CargoResourceCustody.capture(llvm_roots)
        environment = _runtime_build_environment_identity(
            plan, cargo_profile=cargo_profile
        )
        # An ambient value can override a non-forced [env] default. Capturing
        # configuration bytes alone would miss that effective build-script input.
        configured_environment = set()
        for document in [
            *(item.document for item in plan.configuration),
            plan.cli_configuration,
        ]:
            configured_values = string_keyed_mapping(document.get("env", {}))
            if configured_values is None:
                raise CompilerIdentityError(
                    "Cargo environment configuration must be a table"
                )
            configured_environment.update(configured_values)
        for name in configured_environment:
            selected = plan.environment.get(name)
            if selected is not None:
                environment["config-env/" + name] = hashlib.sha256(
                    selected.encode("utf-8")
                ).hexdigest()
        for name, value in plan.environment.items():
            if (
                name.startswith("CARGO_BUILD_")
                or ("llvm" in features and name.startswith("LLVM_SYS_"))
            ) and name not in {
                "CARGO_BUILD_JOBS",
                "CARGO_BUILD_BUILD_DIR",
                "CARGO_BUILD_TARGET_DIR",
            }:
                environment[name] = hashlib.sha256(value.encode("utf-8")).hexdigest()
        fingerprint = canonical_json_sha256(
            {
                "schema": "molt.compiler-cargo-build.v1",
                "command": plan.partition_command(),
                "features": sorted(features),
                "profile": cargo_profile,
                "rustflags": plan.rustflags,
                "environment": environment,
                "c_environment": dict(plan.c_environment),
                "toolchain": plan.toolchain_identity(),
                "link_resources": plan.link_resources.content_identity(),
                "compiler_resources": resources.content_identity(),
            }
        )
        result = BackendBuildAdmission(plan, resources, fingerprint)
        if transaction is not None:
            transaction.compiler_plans[key] = result
        return result
    except CompilerIdentityError:
        raise
    except (
        OSError,
        ValueError,
        LlvmToolchainConfigError,
        subprocess.TimeoutExpired,
    ) as exc:
        raise CompilerIdentityError(f"Compiler build identity: {exc}") from exc


@dataclass(frozen=True)
class CompilerSourceGeneration:
    """Live rebuild fence, never a portable cache key or retained receipt."""

    roots: tuple[Path, ...]
    files: tuple[StableRegularFileVersion, ...]
    directories: tuple[tuple[Path, tuple[int, ...]], ...]

    @staticmethod
    def _topology(
        roots: tuple[Path, ...],
    ) -> tuple[tuple[Path, ...], tuple[tuple[Path, tuple[int, ...]], ...]]:
        from molt.file_hashing import (
            _source_fingerprint_files,
            _source_fingerprint_should_skip,
            content_change_time_ns,
        )
        from molt.file_publication import metadata_is_link_like

        def scan_error(error: OSError) -> None:
            raise error

        files: set[Path] = set()
        directories: dict[Path, tuple[int, ...]] = {}
        for root in roots:
            if metadata_is_link_like(root.lstat()):
                raise CompilerIdentityError(
                    f"Compiler source root is not directly owned: {root}"
                )
            files.update(_source_fingerprint_files(root))
            if not root.is_dir():
                continue
            for directory, children, _names in os.walk(
                root, followlinks=False, onerror=scan_error
            ):
                path = Path(directory)
                children[:] = [
                    name
                    for name in children
                    if not _source_fingerprint_should_skip(path / name)
                ]
                # Cargo can follow a source directory alias even though the
                # source enumerator does not. Never admit that unowned subtree.
                for name in children:
                    child = path / name
                    if metadata_is_link_like(child.lstat()):
                        raise CompilerIdentityError(
                            f"Compiler source directory is not directly owned: {child}"
                        )
                metadata = path.lstat()
                change_time = content_change_time_ns(path, metadata)
                if metadata_is_link_like(metadata) or change_time is None:
                    raise CompilerIdentityError(
                        f"Compiler directory has no direct generation custody: {path}"
                    )
                directories[path] = (
                    metadata.st_dev,
                    metadata.st_ino,
                    metadata.st_mode,
                    metadata.st_mtime_ns,
                    change_time,
                )
        return tuple(sorted(files)), tuple(sorted(directories.items()))

    @classmethod
    def capture(cls, roots: Sequence[Path]) -> CompilerSourceGeneration:
        from molt.toolchain_identity import stable_regular_file_version

        selected = tuple(sorted(set(roots)))
        files, directories = cls._topology(selected)
        result = cls(
            selected,
            tuple(
                stable_regular_file_version(path, label="compiler build source")
                for path in files
            ),
            directories,
        )
        result.verify()
        return result

    def verify(self) -> None:
        from molt.toolchain_identity import verify_stable_regular_file_identity

        files, directories = self._topology(self.roots)
        if (
            files != tuple(item.path for item in self.files)
            or directories != self.directories
        ):
            raise CompilerIdentityError(
                "Compiler source membership/generation changed during build"
            )
        for item in self.files:
            verify_stable_regular_file_identity(item, label="compiler build source")
