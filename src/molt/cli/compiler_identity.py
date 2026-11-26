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
    from molt.toolchain_identity import (
        StableRegularFileObservation,
        StableRegularFileVersion,
    )


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
    binary_observation: StableRegularFileObservation
    force_fresh: bool = False

    def host_tool_observation(
        self, product: HostToolProduct
    ) -> StableRegularFileObservation:
        from molt.compiler_distribution import installed_compiler

        current = installed_compiler(self.compiler.source_root)
        if current != self.compiler:
            raise CompilerIdentityError(
                "Installed executable release selection changed"
            )
        assert current is not None
        if product is HostToolProduct.BACKEND:
            current.select_executable(product).verify(self.binary_observation)
            return self.binary_observation
        return _installed_executable_observation(
            current, self.fingerprint, product, fresh=self.force_fresh
        )


def _installed_executable_observation(
    compiler: InstalledCompiler,
    release: str,
    product: HostToolProduct,
    *,
    features: tuple[str, ...] | None = None,
    cargo_profile: str | None = None,
    fresh: bool = False,
) -> StableRegularFileObservation:
    from molt.cli.cache_fingerprints import _SOURCE_TREE_FINGERPRINT_TRANSACTION

    selection = compiler.select_executable(product, features, cargo_profile)
    key = (os.fspath(compiler.source_root.resolve()), release, product)
    transaction = _SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
    prior = (
        None
        if transaction is None or fresh
        else transaction.installed_executables.get(key)
    )
    if prior is not None:
        try:
            selection.verify(prior)
        except (OSError, ValueError):
            assert transaction is not None
            transaction.installed_executables.pop(key, None)
            raise
        return prior
    observed = selection.capture()
    if transaction is not None:
        transaction.installed_executables[key] = observed
    return observed


def installed_compiler_admission(
    root: Path,
    features: tuple[str, ...] | None = None,
    cargo_profile: str | None = None,
    *,
    fresh: bool = False,
) -> InstalledCompilerAdmission | None:
    from molt.compiler_distribution import (
        InstalledSourceAdmission,
        installed_compiler,
    )

    try:
        compiler = installed_compiler(
            root,
            sources=(
                InstalledSourceAdmission.FRESH
                if fresh
                else InstalledSourceAdmission.REUSE
            ),
        )
        if compiler is None:
            return None
        release = compiler.release_fingerprint()
        # Source admission is reusable within the immutable operation. Executable
        # selection and bytes remain live, including an alias replaced mid-build.
        observed = _installed_executable_observation(
            compiler,
            release,
            HostToolProduct.BACKEND,
            features=features,
            cargo_profile=cargo_profile,
            fresh=fresh,
        )
        return InstalledCompilerAdmission(compiler, release, observed, fresh)
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
    from molt.cargo_resource_selection import CargoResourceRoot
    from molt.cli.cargo_resource_custody import CargoResourceCustody
    from molt.llvm_toolchain import (
        sdk_cargo_resource_roots,
        verify_available_llvm_toolchain,
    )
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
    sdk_roots = sdk_cargo_resource_roots(
        verification.prefix,
        tuple(verification.prefix / fact.path for fact in verification.library_facts),
    )
    resources = CargoResourceCustody.capture(
        (
            *sdk_roots,
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
    sdk_file_count = sum(
        len(walk.entries) for walk in resources.walks[: len(sdk_roots)]
    )
    if sdk_file_count != len(verification.content_facts):
        raise CompilerIdentityError(
            "LLVM SDK file selection changed after verification"
        )
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
    from molt.rust_toolchain import rust_flag_remap_identity

    transaction = _SOURCE_TREE_FINGERPRINT_TRANSACTION.get()
    if not isinstance(product, HostToolProduct):
        raise CompilerIdentityError("Unknown host tool product")
    if product is HostToolProduct.WASM_FACTS and features != product.required_features:
        raise CompilerIdentityError(
            "The WASM facts tool requires its host-tool feature"
        )
    # The complete caller environment keys operation-local reuse; semantic output
    # identity below projects only admitted Cargo/profile/toolchain selectors.
    # The environment itself is the key: hashing it needs no text encoding, so
    # a value that is not valid UTF-8 cannot fail the build (HF-556).
    environment = ProcessEnvironment(env)
    key = (
        os.fspath(root.resolve()),
        product.value,
        cargo_profile,
        ",".join(sorted(features)),
        rust_cpu.value,
        environment,
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
        logical_roots = tuple(
            (label, os.fspath(path)) for label, path in plan.logical_paths
        )
        compile_command, link_arguments = plan.partition_command()
        # Cargo's delimiter starts final-crate rustc arguments. Linker
        # arguments have their own scope and must remain literal inputs.
        if "--" in compile_command:
            boundary = compile_command.index("--")
            cargo_arguments = compile_command[:boundary]
            rust_arguments = compile_command[boundary + 1 :]
        else:
            cargo_arguments, rust_arguments = compile_command, ()
        # Build-script and rustc ambient reads are captured from Cargo's actual
        # unit outputs after execution, then checked separately on every reuse.
        # The plan owns direct Cargo/profile selectors and typed tool/resources.
        fingerprint = canonical_json_sha256(
            {
                "schema": "molt.compiler-cargo-build.v2",
                "command": {
                    "cargo": cargo_arguments,
                    "rust": rust_flag_remap_identity(rust_arguments, logical_roots),
                    "link": link_arguments,
                },
                "features": sorted(features),
                "profile": cargo_profile,
                "rustflags": rust_flag_remap_identity(plan.rustflags, logical_roots),
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
        from molt.source_admission import admitted_source_files, source_directories
        from molt.file_publication import metadata_is_link_like

        files = admitted_source_files(roots)
        directories: dict[Path, tuple[int, ...]] = {}
        for path in source_directories(files, roots):
            metadata = path.lstat()
            if metadata_is_link_like(metadata):
                raise CompilerIdentityError(
                    f"Compiler source directory is not directly owned: {path}"
                )
            # Membership is the admitted set. Directory timestamps also change
            # for ignored locks/stages, so only replacement custody is fenced.
            directories[path] = (metadata.st_dev, metadata.st_ino, metadata.st_mode)
        return files, tuple(sorted(directories.items()))

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
