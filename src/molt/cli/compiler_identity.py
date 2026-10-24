"""Compiler admission shared by build, wrapper caches and daemon selection.

Developer execution uses the existing Cargo plan authority. Installed execution
uses its admitted release, never a hypothetical developer build of shipped Rust.
Only an explicit source-fingerprint operation owns reuse of live admissions.
"""

from __future__ import annotations

from molt.process_environment import ProcessEnvironment

import os
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Mapping, Sequence, cast

from molt.exact_json import canonical_json_sha256
from molt.host_tool_products import HostToolProduct

if TYPE_CHECKING:
    from molt.cli.cargo_resource_custody import CargoResourceCustody
    from molt.cli.runtime_cargo_plan import RuntimeCargoPlan
    from molt.cli.rust_cpu_policy import RustCpuPolicy
    from molt.compiler_distribution import InstalledCompiler
    from molt.llvm_link_plan import LlvmNativeLinkPlan
    from molt.llvm_toolchain import LlvmPrefixVerification
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
                "wasm_facts": compiler.wasm_facts,
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
class LlvmCompilerInputs:
    """The one retained LLVM input admission used by source and release builds."""

    root: Path
    verification: LlvmPrefixVerification
    resources: CargoResourceCustody
    link_plan: LlvmNativeLinkPlan

    def environment(self, env: Mapping[str, str]) -> ProcessEnvironment:
        from molt.llvm_toolchain import project_llvm_toolchain_environment

        return project_llvm_toolchain_environment(
            self.root, self.verification, environ=env, native_link_plan=self.link_plan
        )

    def verify(self) -> None:
        self.resources.verify()


def admit_llvm_compiler_inputs(
    root: Path,
    env: Mapping[str, str],
    *,
    shared_capability: tuple[Mapping[str, object], frozenset[str]] | None = None,
) -> LlvmCompilerInputs:
    from molt.cli.cargo_resource_custody import CargoResourceCustody, CargoResourceRoot
    from molt.llvm_toolchain import sdk_content_paths, verify_available_llvm_toolchain
    from molt.llvm_link_plan import select_native_llvm_link_plan

    verification = verify_available_llvm_toolchain(root, environ=env)
    if verification is None:
        raise CompilerIdentityError(
            "Selected LLVM backend has no admitted LLVM toolchain"
        )
    link_plan = select_native_llvm_link_plan(
        root, verification, environ=env, shared_capability=shared_capability
    )
    # llvm-sys supplies the C wrappers; the native receiver consumes this exact
    # LLVM archive and system dependency plan, with all selected files in custody.
    # Keep the verifier's broader SDK content family; do not scan unrelated tools.
    resources = CargoResourceCustody.capture(
        (
            *(
                CargoResourceRoot(
                    "compiler/llvm-sdk/"
                    + path.relative_to(verification.prefix).as_posix(),
                    path,
                )
                for path in sdk_content_paths(verification)
            ),
            CargoResourceRoot("compiler/llvm-config", verification.llvm_config),
            CargoResourceRoot("compiler/llvm-driver", link_plan.driver),
            *(
                CargoResourceRoot(f"compiler/llvm-system/{index}", path)
                for index, path in enumerate(link_plan.system_paths)
            ),
        )
    )
    config = resources.file_identity(verification.llvm_config)
    facts = [fact for fact in verification.tool_versions if fact.role == "llvm-config"]
    if len(facts) != 1 or (facts[0].sha256, facts[0].size) != (
        config.sha256,
        config.size,
    ):
        raise CompilerIdentityError("llvm-config changed after SDK verification")
    driver = resources.file_identity(link_plan.driver)
    driver_facts = [
        fact for fact in verification.tool_versions if fact.role == "clang++"
    ]
    if len(driver_facts) != 1 or (driver_facts[0].sha256, driver_facts[0].size) != (
        driver.sha256,
        driver.size,
    ):
        raise CompilerIdentityError("LLVM dependency driver changed after verification")
    for fact in verification.content_facts:
        identity = resources.file_identity(verification.prefix / fact.path)
        if (
            identity.size != fact.size
            or identity._stat_identity[4] != fact.mtime_ns
            or (
                fact.change_ns is not None
                and identity._content_change_time_ns != fact.change_ns
            )
            or (fact.sha256 and identity.sha256 != fact.sha256)
        ):
            raise CompilerIdentityError("LLVM SDK content changed after verification")
    return LlvmCompilerInputs(root, verification, resources, link_plan)


@dataclass(frozen=True)
class BackendBuildAdmission:
    plan: RuntimeCargoPlan
    fingerprint: str

    def verify(self) -> None:
        try:
            self.plan.verify()
        except (OSError, ValueError) as exc:
            raise CompilerIdentityError(
                f"Compiler build inputs changed: {exc}"
            ) from exc


def backend_build_admission(
    root: Path,
    features: tuple[str, ...],
    cargo_profile: str,
    env: Mapping[str, str],
    *,
    rust_cpu: RustCpuPolicy,
    product: HostToolProduct = HostToolProduct.BACKEND,
) -> BackendBuildAdmission:
    """Admit one compiler/host-tool product through the shared Cargo authority.

    ``rust_cpu`` is the build's Rust CPU policy. The Cargo plan applies it to
    the Rust flags Cargo selects, and the fingerprint binds those flags.
    """
    from molt.cli.cache_fingerprints import _SOURCE_TREE_FINGERPRINT_TRANSACTION
    from molt.cli.cargo_execution import _cargo_build_env
    from molt.cli.runtime_build_identity import (
        _runtime_build_environment_identity,
    )
    from molt.cli.runtime_cargo_plan import resolve_runtime_cargo_plan
    from molt.dx import project_cargo_target_dir
    from molt.llvm_toolchain import LlvmToolchainConfigError

    transaction = _SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
    if not isinstance(product, HostToolProduct):
        raise CompilerIdentityError("Unknown host tool product")
    if product is HostToolProduct.WASM_FACTS and features != product.required_features:
        raise CompilerIdentityError(
            "The WASM facts tool requires its host-tool feature"
        )
    # The complete caller environment keys operation-local reuse; semantic output
    # identity below projects only admitted Cargo/profile/toolchain selectors.
    environment = ProcessEnvironment(env)
    key = (
        os.fspath(root.resolve()),
        product.value,
        cargo_profile,
        ",".join(sorted(features)),
        rust_cpu.value,
        canonical_json_sha256(
            {"rule": environment.rule.value, "values": dict(environment)}
        ),
    )
    if transaction is not None and key in transaction.compiler_plans:
        return cast(BackendBuildAdmission, transaction.compiler_plans[key])
    try:
        prepared = _cargo_build_env(environment)
        prepared["CARGO_TARGET_DIR"] = os.fspath(
            project_cargo_target_dir(root, prepared)
        )
        llvm_inputs: LlvmCompilerInputs | None = None

        def compiler_environment(environment: ProcessEnvironment) -> Mapping[str, str]:
            nonlocal llvm_inputs
            effective = environment
            if "llvm" not in features:
                return effective
            llvm_inputs = admit_llvm_compiler_inputs(root, effective)
            return llvm_inputs.environment(effective)

        command = [
            "cargo",
            "build",
            "--locked",
            "--message-format=json-render-diagnostics",
            "--package",
            product.value,
            "--bin",
            product.value,
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
            rustflags_transform=rust_cpu.rustflags,
        )
        # llvm-sys consumes both selectors and prefix contents. Its build script
        # can link archives, query llvm-config and consume headers under this root.
        if llvm_inputs is not None:
            plan = plan.with_build_resources(llvm_inputs.resources)
        environment = _runtime_build_environment_identity(
            plan, cargo_profile=cargo_profile
        )
        # Build-script and rustc ambient reads are captured from Cargo's actual
        # unit outputs after execution, then checked separately on every reuse.
        # The plan owns direct Cargo/profile selectors and typed tool/resources.
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
            }
        )
        result = BackendBuildAdmission(plan, fingerprint)
        result.verify()
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
