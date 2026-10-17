from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING, Mapping

from molt.release_lanes import ReleaseLane, capture_release_lanes

if TYPE_CHECKING:
    from tools.perf_scoreboard_model import BackendSpec

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


def canonical_content_identity(identity: object, fields: frozenset[str]) -> bool:
    import re

    return (
        isinstance(identity, Mapping)
        and set(identity) == fields
        and isinstance(identity.get("size"), int)
        and not isinstance(identity.get("size"), bool)
        and identity["size"] > 0
        and all(
            isinstance(identity.get(k), str) and identity[k]
            for k in fields - {"size", "sha256"}
        )
        and isinstance(identity.get("sha256"), str)
        and re.fullmatch(r"[0-9a-f]{64}", identity["sha256"]) is not None
    )


def _measured_runtime_digest(observation: Mapping[str, object]) -> str | None:
    """The runtime library a native link bound, or None for a lane without one."""
    from molt.exact_json import canonical_json_sha256

    if "runtime" not in observation:
        raise ValueError("measured build observation lacks its runtime fact")
    runtime = observation["runtime"]
    if runtime is None:
        return None
    identity = runtime.get("identity") if isinstance(runtime, Mapping) else None
    if not canonical_content_identity(
        identity, frozenset({"filename", "size", "sha256"})
    ):
        error = runtime.get("error") if isinstance(runtime, Mapping) else None
        raise ValueError(
            "measured build lacks a canonical runtime identity"
            + (f": {error}" if isinstance(error, str) and error else "")
        )
    assert isinstance(identity, Mapping)
    return canonical_json_sha256({"schema": 1, "runtime": dict(identity)})


def record_measured_build_identity(
    provenance: dict, *, observation: object, backend: str, profile: str
) -> None:
    """Bind lane provenance to the compiler and runtime the build observed.

    A pre-build alias probe may be absent or obsolete. A later probe cannot
    retrospectively establish which compiler was selected during measurement.
    """
    from molt.backend_binary_identity import backend_binary_digest

    compiler = observation.get("compiler") if isinstance(observation, Mapping) else None
    identity = compiler.get("identity") if isinstance(compiler, Mapping) else None
    if not canonical_content_identity(
        identity, frozenset({"entrypoint", "content_filename", "size", "sha256"})
    ):
        raise ValueError("measured build lacks a canonical selected compiler identity")
    assert isinstance(identity, Mapping) and isinstance(observation, Mapping)
    key = f"{backend}/{profile}"
    digest = backend_binary_digest(identity)
    measured = provenance.setdefault("measured_backend_binary_identity", {})
    previous = measured.get(key)
    if previous is not None and previous != digest:
        raise ValueError(f"selected compiler changed within measured lane {key}")
    runtime_digest = _measured_runtime_digest(observation)
    runtimes = provenance.setdefault("runtime_identity", {})
    if key in runtimes and runtimes[key] != runtime_digest:
        raise ValueError(f"runtime library changed within measured lane {key}")
    provenance.setdefault(
        "backend_binary_identity_before_build",
        dict(provenance.get("backend_binary_identity", {})),
    )
    measured[key] = digest
    runtimes[key] = runtime_digest
    provenance.setdefault("backend_binary_identity", {})[key] = digest
