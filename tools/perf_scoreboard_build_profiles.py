from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING, Mapping

if TYPE_CHECKING:
    from perf_scoreboard_model import BackendSpec

PROFILE_BUILD_FLAG = {
    "release-fast": "release",
    "release-output": "release",
    "dev-fast": "dev",
    "release-size": "release",
    "wasm-release": "release",
}


@dataclass(frozen=True)
class ProfileSelection:
    coordinate_profile: str
    cli_build_profile: str
    guest_cargo_profile: str
    host_cargo_profile: str

    def environment(self) -> dict[str, str]:
        # Pin each producer explicitly. The host compiler is independent from
        # the measured guest; development overrides cannot change either.
        return {
            "MOLT_BACKEND_PROFILE": "release",
            "MOLT_RELEASE_BACKEND_CARGO_PROFILE": self.host_cargo_profile,
            "MOLT_DEV_BACKEND_CARGO_PROFILE": "dev-fast",
            "MOLT_RELEASE_CARGO_PROFILE": self.guest_cargo_profile,
            "MOLT_DEV_CARGO_PROFILE": self.guest_cargo_profile,
            "MOLT_WASM_CARGO_PROFILE": self.guest_cargo_profile,
            "MOLT_RUNTIME_BUILD_PROFILE": "",
            "MOLT_RUNTIME_WASM_INCREMENTAL": "0",
        }


def profile_selection(spec: BackendSpec, profile: str) -> ProfileSelection:
    return profile_selection_for_target(spec.build_target, profile)


def profile_selection_for_target(build_target: str, profile: str) -> ProfileSelection:
    if profile not in PROFILE_BUILD_FLAG:
        raise ValueError(f"unknown performance profile: {profile!r}")
    if profile == "wasm-release" and build_target != "wasm":
        raise ValueError("wasm-release is a WASM artifact coordinate")
    # Preserve both declared WASM release-output and wasm-release coordinates.
    # The CLI maps only generic Cargo release to wasm-release; an explicit
    # artifact profile must never be renamed to a different measured lane.
    return ProfileSelection(profile, PROFILE_BUILD_FLAG[profile], profile, "release")


def profile_binding_problems(
    observation: object, *, build_target: str, profile: str
) -> list[str]:
    """Require selected profile facts; labels and legacy paths are not binding.

    This checks publication observations, not loaded-daemon attestation. The
    compiled_with_verified field retains its separate, stronger meaning.
    """
    expected = profile_selection_for_target(build_target, profile)
    facts = (
        observation.get("selected_profiles")
        if isinstance(observation, Mapping)
        else None
    )
    if not isinstance(facts, Mapping):
        return ["missing selected-profile observation; historical/unbound result"]
    coordinates = {
        "guest_profile": expected.cli_build_profile,
        "compiler_profile": expected.host_cargo_profile,
        "runtime_profile": expected.guest_cargo_profile,
        "target": build_target,
    }
    return [
        f"selected {key}: expected {value!r}, observed {facts.get(key)!r}"
        for key, value in coordinates.items()
        if facts.get(key) != value
    ]


def record_measured_backend_identity(
    provenance: dict, *, observation: object, backend: str, profile: str
) -> None:
    """Bind lane provenance to selected compiler file publication observations.

    A pre-build alias probe may be absent or obsolete. A later probe cannot
    retrospectively establish which compiler was selected during measurement.
    """
    import re
    from molt.exact_json import canonical_json_sha256

    compiler = observation.get("compiler") if isinstance(observation, Mapping) else None
    identity = compiler.get("identity") if isinstance(compiler, Mapping) else None
    if (
        not isinstance(identity, Mapping)
        or set(identity) != {"entrypoint", "content_filename", "size", "sha256"}
        or not isinstance(identity.get("size"), int)
        or isinstance(identity.get("size"), bool)
        or identity["size"] <= 0
        or not all(
            isinstance(identity.get(k), str) and identity[k]
            for k in ("entrypoint", "content_filename")
        )
        or not isinstance(identity.get("sha256"), str)
        or re.fullmatch(r"[0-9a-f]{64}", identity["sha256"]) is None
    ):
        raise ValueError("measured build lacks a canonical selected compiler identity")
    key = f"{backend}/{profile}"
    digest = canonical_json_sha256({"schema": 2, "binary": dict(identity)})
    measured = provenance.setdefault("measured_backend_binary_identity", {})
    previous = measured.get(key)
    if previous is not None and previous != digest:
        raise ValueError(f"selected compiler changed within measured lane {key}")
    provenance.setdefault(
        "backend_binary_identity_before_build",
        dict(provenance.get("backend_binary_identity", {})),
    )
    measured[key] = digest
    provenance.setdefault("backend_binary_identity", {})[key] = digest
