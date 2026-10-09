"""Source-owned executable release lanes, shared by shipped and developer code.

The declaration is config/release_acceptance_matrix.toml. Logical code generators
are distinct even where their physical runtime cells are shared. No compiler
capability or execution qualification follows from declaring a required lane.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import tomllib
from typing import Any

from molt.backend_executable_names import DEFAULT_CODEGEN_BACKEND, CodegenBackend
from molt.portable_paths import portable_relative_path
from molt.source_root import compiler_source_root
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    capture_stable_regular_file,
    verify_stable_regular_file_identity,
)
from molt.verified_subset import VerifiedSubsetPolicy, capture_verified_subset_policy

SCHEMA = "molt.release-acceptance-matrix.v1"
_CONFIG_PATH = "config/release_acceptance_matrix.toml"


def lane_codegen_backend(backend: str) -> CodegenBackend:
    """The ``molt build --backend`` value that a lane's backend name selects."""
    return "llvm" if backend == "llvm" else DEFAULT_CODEGEN_BACKEND


@dataclass(frozen=True, slots=True)
class ReleaseLane:
    backend: str
    target: str
    guest_profile: str
    runtime_profile: str
    compiler_profile: str

    @property
    def id(self) -> str:
        return "-".join(self.as_record().values())

    @property
    def codegen_backend(self) -> CodegenBackend:
        return lane_codegen_backend(self.backend)

    @property
    def compiler_features(self) -> tuple[str, ...]:
        from molt.backend_executable_names import backend_features_for_target

        return backend_features_for_target(
            is_wasm=self.target == "wasm",
            is_luau_transpile=False,
            is_rust_transpile=False,
            codegen_backend=self.codegen_backend,
        )

    def build_args(self) -> tuple[str, ...]:
        """The ``molt build`` flags that select this lane's code generator.

        The CLI takes the code generator only as a flag; the environment never
        selects it, so an ambient ``MOLT_BACKEND`` cannot change the lane.
        """
        return ("--backend", self.codegen_backend)

    def as_record(self) -> dict[str, str]:
        return {
            "backend": self.backend,
            "target": self.target,
            "guest_profile": self.guest_profile,
            "runtime_profile": self.runtime_profile,
            "compiler_profile": self.compiler_profile,
        }

    def environment(self) -> dict[str, str]:
        """Pin the profile selectors; ``build_args`` selects the code generator."""
        return {
            "MOLT_BACKEND_PROFILE": "release",
            "MOLT_RELEASE_BACKEND_CARGO_PROFILE": self.compiler_profile,
            "MOLT_DEV_BACKEND_CARGO_PROFILE": self.compiler_profile,
            "MOLT_RELEASE_CARGO_PROFILE": self.runtime_profile,
            "MOLT_DEV_CARGO_PROFILE": self.runtime_profile,
            "MOLT_WASM_CARGO_PROFILE": self.runtime_profile,
            "MOLT_RUNTIME_BUILD_PROFILE": "",
            "MOLT_RUNTIME_WASM_INCREMENTAL": "0",
        }


@dataclass(frozen=True)
class ReleaseLaneInventory:
    lanes: tuple[ReleaseLane, ...]
    configuration: dict[str, Any]
    semantic_policy: VerifiedSubsetPolicy
    identities: tuple[StableRegularFileIdentity, ...]

    def select(self, *, backend: str, runtime_profile: str) -> ReleaseLane:
        matches = tuple(
            lane
            for lane in self.lanes
            if (lane.backend, lane.runtime_profile) == (backend, runtime_profile)
        )
        if len(matches) != 1:
            raise ValueError(
                f"unsupported release lane: backend={backend}, runtime_profile={runtime_profile}"
            )
        return matches[0]

    def verify(self) -> None:
        for identity in self.identities:
            verify_stable_regular_file_identity(
                identity, label="release lane authority"
            )


def capture_release_lanes(root: Path | None = None) -> ReleaseLaneInventory:
    """Move the acceptance reader's one validation into the shipped source owner."""
    root = (compiler_source_root() if root is None else root).resolve(strict=True)
    config_identity, raw = capture_stable_regular_file(
        root / _CONFIG_PATH, label="release matrix config"
    )
    config = tomllib.loads(raw.decode("utf-8"))
    if (
        set(config)
        != {
            "schema",
            "lane",
            "excluded_backend",
            "required_metric",
            "required_semantic_backends",
        }
        or config["schema"] != SCHEMA
    ):
        raise ValueError("release matrix config keys/schema are not exact")
    policy, policy_identity = capture_verified_subset_policy(
        root / "config/verified_subset.toml"
    )
    cargo_identity, raw = capture_stable_regular_file(
        root / "Cargo.toml", label="release matrix Cargo profiles"
    )
    profiles = tomllib.loads(raw.decode("utf-8")).get("profile")
    if not isinstance(profiles, dict):
        raise ValueError("release matrix Cargo profiles are missing")
    declarations = config["lane"]
    if not isinstance(declarations, list) or not declarations:
        raise ValueError("release matrix needs explicit applicable lanes")
    keys = {
        "backend",
        "guest_profile",
        "runtime_profile",
        "compiler_profile",
        "authority",
    }
    lanes: list[ReleaseLane] = []
    coordinates: set[tuple[str, str]] = set()
    authorities: set[str] = set()
    for declaration in declarations:
        if (
            not isinstance(declaration, dict)
            or set(declaration) != keys
            or any(
                not isinstance(v, str) or not v or v.strip() != v
                for v in declaration.values()
            )
        ):
            raise ValueError("release matrix lane is not exact")
        backend = declaration["backend"]
        if (
            backend not in policy.backends
            or declaration["guest_profile"] not in policy.build_profiles
        ):
            raise ValueError("release matrix lane is outside declared runtime support")
        for field in ("runtime_profile", "compiler_profile"):
            value = declaration[field]
            if value not in profiles:
                raise ValueError(f"release matrix unknown {field}")
        lane = ReleaseLane(
            backend,
            "wasm" if backend == "wasm" else "native",
            declaration["guest_profile"],
            declaration["runtime_profile"],
            declaration["compiler_profile"],
        )
        # One CLI profile coordinate cannot select two distinct logical plans.
        coordinate = (lane.backend, lane.runtime_profile)
        if coordinate in coordinates:
            raise ValueError("duplicate release matrix lane")
        coordinates.add(coordinate)
        lanes.append(lane)
        authorities.add(declaration["authority"])
    if len({lane.id for lane in lanes}) != len(lanes):
        raise ValueError("release lane output names collide")
    minimum = {(b, p) for b in policy.backends for p in policy.build_profiles}
    if not minimum <= {(lane.backend, lane.guest_profile) for lane in lanes} or not any(
        lane.backend == "llvm" and lane.runtime_profile == "release-fast"
        for lane in lanes
    ):
        raise ValueError("release matrix drops verified-subset or daily LLVM coverage")
    if config["required_semantic_backends"] != list(policy.backends):
        raise ValueError(
            "release matrix cannot narrow advertised semantic backend coverage"
        )
    identities = [config_identity, policy_identity, cargo_identity]
    for authority in sorted(authorities):
        relative = portable_relative_path(authority)
        path = root.joinpath(*relative.parts)
        if path.is_symlink() or path.resolve(strict=True) != path or not path.is_file():
            raise ValueError(
                "release matrix applicability authority is missing or noncanonical"
            )
        identity, _raw = capture_stable_regular_file(
            path, label="release lane applicability"
        )
        identities.append(identity)
    inventory = ReleaseLaneInventory(tuple(lanes), config, policy, tuple(identities))
    inventory.verify()
    return inventory
