"""Checkouts that a native toolchain build has populated, for transfer tests.

Each checkout gets its own Cargo target and build-state root, a real native
runtime generation published through the CLI, and a backend generation
published and selected the way ``_ensure_backend_binary`` admits one. Only the identity captures and Cargo
are substituted.
"""

from __future__ import annotations

from molt.process_environment import ProcessEnvironment

from tests.runtime_build_identity_helper import runtime_cargo_environment_inputs

from dataclasses import dataclass
import os
from pathlib import Path

import pytest

from molt.cli.rust_cpu_policy import RustCpuPolicy
from molt.backend_executable_names import backend_executable_name
from molt.cli import backend_binary
from molt.cli import native_toolchain_transfer as transfer
from molt.cli import runtime_native_build
from molt.cli import runtime_native_generation as generations
from molt.cli.product_store import ProductStore
from molt.cli.backend_compile import _BackendSelection
from molt.cli.runtime_identity_schema import RuntimeBuildIdentity
from molt.exact_json import canonical_json_sha256
from tests.cli.backend_generation_test_support import publish_backend
from tests.cli.native_link_test_support import write_test_static_archive
from tests.compiler_identity_helper import (
    compiler_environment_inputs,
    stub_compiler_admission,
    write_compiler_source,
)
from tests.runtime_build_identity_helper import (
    native_runtime_staticlib_identity,
    native_runtime_build_inputs,
)

RUNTIME_PROFILE = "dev-fast"
BACKEND_PROFILE = "release"
BACKEND_FEATURES = ("native-backend",)


def runtime_identity(seed: str = "runtime") -> RuntimeBuildIdentity:
    return native_runtime_staticlib_identity(
        cargo_profile=RUNTIME_PROFILE, family_seed=seed
    )


def backend_fingerprint(seed: str = "backend") -> dict[str, str]:
    return {
        "hash": canonical_json_sha256(f"{seed}-source"),
        "rustc": "rustc-1",
        "inputs_digest": canonical_json_sha256(f"{seed}-inputs"),
        "meta_digest": canonical_json_sha256("backend-meta"),
    }


@dataclass
class Checkout:
    """One source checkout and the identities its current inputs produce."""

    root: Path
    session_target: Path
    build_state: Path
    runtime_seed: str = "runtime"
    backend_seed: str = "backend"

    @property
    def selection(self) -> transfer.NativeToolchainSelection:
        name = backend_executable_name(os_name=os.name, features=BACKEND_FEATURES)
        return transfer.NativeToolchainSelection(
            project_root=self.root,
            runtime_lib=self.session_target
            / RUNTIME_PROFILE
            / "libmolt_runtime.stdlib_full.a",
            runtime_cargo_profile=RUNTIME_PROFILE,
            backend=_BackendSelection(
                cargo_profile=BACKEND_PROFILE,
                features=BACKEND_FEATURES,
                binary=self.session_target / BACKEND_PROFILE / name,
                rust_cpu=RustCpuPolicy.BASELINE,
            ),
        )

    def activate(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Make this checkout's environment and identities the current ones."""
        monkeypatch.setenv("CARGO_TARGET_DIR", str(self.session_target))
        monkeypatch.setenv("MOLT_BUILD_STATE_DIR", str(self.build_state))
        monkeypatch.setattr(
            transfer.NativeToolchainSelection,
            "runtime_build_inputs",
            lambda _selection: native_runtime_build_inputs(
                runtime_identity(self.runtime_seed)
            ),
        )
        monkeypatch.setattr(
            backend_binary,
            "_backend_fingerprint",
            lambda *_args, **_kwargs: backend_fingerprint(self.backend_seed),
        )

    @property
    def runtime_store(self) -> ProductStore:
        """The product store this checkout's builds use (after ``activate``)."""
        return runtime_native_build.native_runtime_store(self.root)

    @property
    def backend_store(self) -> ProductStore:
        """The host-tool store this checkout's builds use (after ``activate``)."""
        return backend_binary.host_tool_store(self.root)

    def stored_backend_variants(self) -> list[Path]:
        """Every published host-tool variant in this checkout's store."""
        root = self.backend_store.root
        return sorted(root.glob("[0-9a-f]" * 64 + "/" + "[0-9a-f]" * 64))

    def stored_runtimes(self, seed: str | None = None) -> tuple:
        """This environment's admitted store generation of the runtime identity."""
        generation = generations.native_runtime_store_generation(
            self.runtime_store,
            runtime_identity(self.runtime_seed if seed is None else seed),
            ProcessEnvironment(os.environ),
            runtime_name=self.selection.runtime_lib.name,
            cargo_profile=RUNTIME_PROFILE,
            target_triple=None,
        )
        return () if generation is None else (generation,)

    def build(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Leave what ``molt build`` admits: a stored runtime and a backend."""
        self.activate(monkeypatch)
        selection = self.selection
        scratch = selection.runtime_lib.with_name("cargo-output.a")
        scratch.parent.mkdir(parents=True, exist_ok=True)
        write_test_static_archive(scratch, payload=self.runtime_seed.encode())
        identity = runtime_identity(self.runtime_seed)
        environment = runtime_cargo_environment_inputs()
        assert generations.publish_native_runtime_generation(
            self.runtime_store.variant(identity.digest, environment)
            / selection.runtime_lib.name,
            source_archive=scratch,
            cargo_stdout="",
            cargo_stderr="note: native-static-libs: -lc\n",
            cargo_profile=RUNTIME_PROFILE,
            target_triple=None,
            build_identity=identity,
            inputs_are_current=lambda: True,
            cargo_environment=environment,
        )
        backend = selection.backend.binary
        publish_backend(
            backend,
            f"#!/bin/sh\nexit 0\n# {self.backend_seed}\n".encode(),
            project_root=self.root,
            fingerprint=backend_fingerprint(self.backend_seed),
            cargo_environment=compiler_environment_inputs(),
        )


def checkout(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, name: str, **seeds: str
) -> Checkout:
    root = tmp_path / name
    write_compiler_source(root)
    stub_compiler_admission(monkeypatch)
    return Checkout(
        root=root,
        session_target=tmp_path / f"{name}-session-target",
        build_state=tmp_path / f"{name}-state",
        **seeds,
    )
