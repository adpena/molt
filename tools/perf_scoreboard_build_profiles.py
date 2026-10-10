from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING, Mapping

from molt.release_lanes import ReleaseLane, capture_release_lanes

if TYPE_CHECKING:
    from perf_scoreboard_model import BackendSpec

_ROOT = Path(__file__).resolve().parents[1]


def profile_build_flags() -> dict[str, str]:
    """CLI choices are a projection, never another profile declaration."""
    flags: dict[str, str] = {}
    for lane in capture_release_lanes(_ROOT).lanes:
        previous = flags.setdefault(lane.runtime_profile, lane.guest_profile)
        if previous != lane.guest_profile:
            raise ValueError("release lanes disagree on the guest profile selector")
    return flags


def profile_selection(spec: BackendSpec, profile: str) -> ReleaseLane:
    lane = profile_selection_for_backend(spec.backend, profile)
    if lane.target != spec.build_target:
        raise ValueError("backend specification differs from the release lane target")
    return lane


def profile_selection_for_backend(backend: str, profile: str) -> ReleaseLane:
    return capture_release_lanes(_ROOT).select(backend=backend, runtime_profile=profile)


def profile_binding_problems(
    observation: object, *, backend: str, profile: str
) -> list[str]:
    """Require every selected lane fact; this is not loaded-byte attestation."""
    try:
        expected = profile_selection_for_backend(backend, profile)
    except ValueError as exc:
        return [str(exc)]
    facts = (
        observation.get("selected_profiles")
        if isinstance(observation, Mapping)
        else None
    )
    if not isinstance(facts, Mapping):
        return ["missing selected-profile observation; historical/unbound result"]
    return [
        f"selected {key}: expected {value!r}, observed {facts.get(key)!r}"
        for key, value in expected.as_record().items()
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
