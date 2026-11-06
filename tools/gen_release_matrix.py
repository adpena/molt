#!/usr/bin/env python3
"""Generate the packaged release-target and Tier 1 projection from its TOML authority.

`config/release_targets.toml` is the one authority for the release targets and
for the Tier 1 support cells: the cells the two flagship witnesses need, plus
Python 3.15 (owner decision 2026-11-05, docs/design/v0_1_support_matrix.md).
Every other cell is Tier 2. The generator derives each Tier 1 cell's id and
artifact target and refuses a cell that names an unknown host, an unpinned
Python, an excluded backend or a lane that the release acceptance matrix does
not declare.
"""

from __future__ import annotations

import json
import re
import sys
import tomllib
from collections.abc import Mapping
from pathlib import Path
from typing import Any

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

ROOT = Path(__file__).resolve().parents[1]


from tools.generator_io import generator_main  # noqa: E402
from molt.target_python_policy import SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS  # noqa: E402


SOURCE = ROOT / "config" / "release_targets.toml"
SCHEMA = "molt.release-targets.v2"
OUTPUT = ROOT / "src" / "molt" / "release_matrix.py"
ACCEPTANCE_SOURCE = ROOT / "config" / "release_acceptance_matrix.toml"
TARGET_KEYS = frozenset({"id", "runner", "platform", "arch", "rust_target", "archive"})
TIER1_KEYS = frozenset(
    {"witness", "host", "python", "backend", "runtime_profile", "execution"}
)
# Closed domains of the Tier 1 claim. A new witness or execution host is an
# owner decision and a change here; the generated Literal types follow.
TIER1_WITNESSES = ("django-offload", "falcon-ocr-workers")
TIER1_EXECUTIONS = ("host", "workerd")
TIER1_GUEST_PROFILE = "release"
WASM_ARTIFACT_TARGET = "wasm32-wasip1"
PLATFORM_ARCHITECTURES = {
    "linux": frozenset({"aarch64", "x86_64"}),
    "macos": frozenset({"arm64", "x86_64"}),
    "windows": frozenset({"arm64", "x86_64"}),
}


def _expected_rust_target(platform: str, architecture: str) -> str:
    rust_arch = "aarch64" if architecture == "arm64" else architecture
    suffix = {
        "linux": "unknown-linux-gnu",
        "macos": "apple-darwin",
        "windows": "pc-windows-msvc",
    }[platform]
    return f"{rust_arch}-{suffix}"


def _expected_archive(platform: str) -> str:
    """Release archives are zip on Windows and tar.gz elsewhere (packaging/install.sh)."""
    return "zip" if platform == "windows" else "tar.gz"


def _runner_matches_coordinate(
    runner: str, *, platform: str, architecture: str
) -> bool:
    if platform == "linux":
        return runner.startswith("ubuntu-") and (
            (architecture == "aarch64") == runner.endswith("-arm")
        )
    if platform == "macos":
        return runner.startswith("macos-") and (
            (architecture == "x86_64") == runner.endswith("-intel")
        )
    return runner.startswith("windows-") and (
        (architecture == "arm64") == runner.endswith("-arm")
    )


def _require_mapping(value: object, *, label: str) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise ValueError(f"{label} must be a table")
    return value


def load_release_target_authority(path: Path = SOURCE) -> dict[str, object]:
    with path.open("rb") as stream:
        document = tomllib.load(stream)
    if set(document) != {"schema", "target", "tier1_cell"}:
        raise ValueError("release target authority keys are not exact")
    if document["schema"] != SCHEMA:
        raise ValueError("release target authority schema is unsupported")
    raw_targets = document["target"]
    if not isinstance(raw_targets, list):
        raise ValueError("release targets must be an array of tables")
    targets: list[dict[str, str]] = []
    for index, raw in enumerate(raw_targets):
        target = _require_mapping(raw, label=f"release target {index}")
        if set(target) != TARGET_KEYS or not all(
            isinstance(target[key], str) and target[key] for key in TARGET_KEYS
        ):
            raise ValueError(f"release target {index} fields are not exact strings")
        targets.append({key: str(target[key]) for key in sorted(TARGET_KEYS)})
    ids = [target["id"] for target in targets]
    coordinates = [(target["platform"], target["arch"]) for target in targets]
    rust_targets = [target["rust_target"] for target in targets]
    expected_coordinates = {
        (platform, architecture)
        for platform, architectures in PLATFORM_ARCHITECTURES.items()
        for architecture in architectures
    }
    if (
        not targets
        or len(ids) != len(set(ids))
        or len(coordinates) != len(set(coordinates))
        or len(rust_targets) != len(set(rust_targets))
        or set(coordinates) != expected_coordinates
    ):
        raise ValueError("release target matrix must contain each supported cell once")
    for target in targets:
        expected_id = f"{target['platform']}-{target['arch']}"
        if (
            target["id"] != expected_id
            or target["archive"] != _expected_archive(target["platform"])
            or "latest" in target["runner"]
            or "self-hosted" in target["runner"]
            or re.fullmatch(r"[a-z0-9_]+(?:-[a-z0-9_]+)+", target["rust_target"])
            is None
            or target["rust_target"]
            != _expected_rust_target(target["platform"], target["arch"])
            or not _runner_matches_coordinate(
                target["runner"],
                platform=target["platform"],
                architecture=target["arch"],
            )
        ):
            raise ValueError(f"release target policy is invalid: {target['id']}")
    tier1 = _load_tier1_cells(document["tier1_cell"], targets)
    return {"targets": targets, "tier1_cells": tier1}


def _acceptance_lanes(
    path: Path = ACCEPTANCE_SOURCE,
) -> tuple[frozenset[str], frozenset[str], frozenset[tuple[str, str, str]]]:
    """The admitted backends, excluded backends and (backend, guest, runtime) lanes."""
    with path.open("rb") as stream:
        document = tomllib.load(stream)
    backends = frozenset(str(item) for item in document["required_semantic_backends"])
    excluded = frozenset(str(item["backend"]) for item in document["excluded_backend"])
    lanes = frozenset(
        (str(lane["backend"]), str(lane["guest_profile"]), str(lane["runtime_profile"]))
        for lane in document["lane"]
    )
    return backends, excluded, lanes


def _load_tier1_cells(
    raw_cells: object, targets: list[dict[str, str]]
) -> list[dict[str, str]]:
    # Imported here, not at module scope: the generator's import closure stays
    # the release policy alone (tests/test_generator_manifest.py).
    from molt._python_release_generated import CPYTHON_RELEASES

    if not isinstance(raw_cells, list) or not raw_cells:
        raise ValueError("tier1_cell must be a non-empty array of tables")
    targets_by_id = {target["id"]: target for target in targets}
    backends, excluded, lanes = _acceptance_lanes()
    cells: list[dict[str, str]] = []
    for index, raw in enumerate(raw_cells):
        row = _require_mapping(raw, label=f"tier1_cell {index}")
        if set(row) != TIER1_KEYS or not all(
            isinstance(row[key], str) and row[key] for key in TIER1_KEYS
        ):
            raise ValueError(f"tier1_cell {index} fields are not exact strings")
        witness, host, python = row["witness"], row["host"], row["python"]
        backend, runtime_profile = row["backend"], row["runtime_profile"]
        execution = row["execution"]
        label = f"tier1_cell {index} ({witness} {host} {python} {backend})"
        if witness not in TIER1_WITNESSES:
            raise ValueError(f"{label}: unknown witness {witness!r}")
        if execution not in TIER1_EXECUTIONS:
            raise ValueError(f"{label}: unknown execution host {execution!r}")
        target = targets_by_id.get(host)
        if target is None:
            raise ValueError(f"{label}: host is not a release target id")
        if python not in CPYTHON_RELEASES:
            raise ValueError(f"{label}: Python {python} has no pinned CPython source")
        if backend in excluded or backend not in backends:
            raise ValueError(f"{label}: backend is not an admitted release backend")
        if (backend, TIER1_GUEST_PROFILE, runtime_profile) not in lanes:
            raise ValueError(
                f"{label}: ({backend}, {TIER1_GUEST_PROFILE}, {runtime_profile}) is "
                "not a lane of config/release_acceptance_matrix.toml"
            )
        if (execution == "workerd") != (backend == "wasm"):
            raise ValueError(f"{label}: workerd runs exactly the wasm backend")
        cells.append(
            {
                "id": f"tier1-{host}-py{python.replace('.', '')}-{backend}",
                "witness": witness,
                "host": host,
                "platform": target["platform"],
                "arch": target["arch"],
                "python": python,
                "backend": backend,
                "artifact_target": (
                    WASM_ARTIFACT_TARGET if backend == "wasm" else target["rust_target"]
                ),
                "guest_profile": TIER1_GUEST_PROFILE,
                "runtime_profile": runtime_profile,
                "execution": execution,
            }
        )
    ids = [cell["id"] for cell in cells]
    if ids != sorted(set(ids)):
        raise ValueError("tier1_cell rows must be unique and sorted by derived id")
    return cells


def render_release_matrix(authority: Mapping[str, object]) -> str:
    targets = authority["targets"]
    assert isinstance(targets, list)
    versions = ", ".join(
        json.dumps(version) for version in SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS
    )
    if len(SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS) == 1:
        versions += ","
    lines = [
        '"""Generated release OS, architecture, and Rust-target policy."""',
        "",
        "from __future__ import annotations",
        "",
        "from dataclasses import dataclass",
        "from types import MappingProxyType",
        "from typing import Literal",
        "",
        "# Generated by tools/gen_release_matrix.py from config/release_targets.toml.",
        'PYTHON_IMPLEMENTATION = "cpython"',
        f"SUPPORTED_CPYTHON_VERSIONS = ({versions})",
        "MINIMUM_CPYTHON_VERSION = tuple(",
        '    int(part) for part in SUPPORTED_CPYTHON_VERSIONS[0].split(".")',
        ")",
        "RELEASE_TARGETS = (",
    ]
    for target in targets:
        assert isinstance(target, dict)
        lines.append("    MappingProxyType(")
        lines.append("        {")
        for key in ("id", "runner", "platform", "arch", "rust_target", "archive"):
            lines.append(f"            {json.dumps(key)}: {json.dumps(target[key])},")
        lines.extend(("        }", "    ),"))
    lines.extend(
        (
            ")",
            "RUST_TARGET_BY_COORDINATE = MappingProxyType(",
            "    {",
        )
    )
    for target in targets:
        assert isinstance(target, dict)
        lines.append(
            f"        ({json.dumps(target['platform'])}, "
            f"{json.dumps(target['arch'])}): {json.dumps(target['rust_target'])},"
        )
    lines.extend(
        (
            "    }",
            ")",
            "",
            "",
            "def wasi_sdk_host_id(platform: str, architecture: str) -> str:",
            '    """Project one shipped release coordinate into wasi-sdk asset spelling."""',
            "",
            '    normalized_architecture = "aarch64" if architecture == "arm64" else architecture',
            '    return f"{platform}-{normalized_architecture}"',
            "",
            "",
            "REQUIRED_WASI_SDK_HOST_IDS = frozenset(",
            "    wasi_sdk_host_id(platform, architecture)",
            "    for platform, architecture in RUST_TARGET_BY_COORDINATE",
            ")",
            "",
            "",
        )
    )
    lines.extend(_render_tier1(authority["tier1_cells"]))
    return "\n".join(lines)


def _literal(values: tuple[str, ...]) -> str:
    return "Literal[" + ", ".join(json.dumps(value) for value in values) + "]"


def _render_tier1(cells: object) -> list[str]:
    assert isinstance(cells, list)
    fields = (
        "id",
        "witness",
        "host",
        "platform",
        "arch",
        "python",
        "backend",
        "artifact_target",
        "guest_profile",
        "runtime_profile",
        "execution",
    )
    lines = [
        f"Tier1Witness = {_literal(TIER1_WITNESSES)}",
        f"Tier1Execution = {_literal(TIER1_EXECUTIONS)}",
        "",
        "",
        "@dataclass(frozen=True, slots=True)",
        "class Tier1Cell:",
        '    """One release-blocking support cell; every other cell is Tier 2."""',
        "",
    ]
    for field in fields:
        annotation = {"witness": "Tier1Witness", "execution": "Tier1Execution"}.get(
            field, "str"
        )
        lines.append(f"    {field}: {annotation}")
    lines.extend(("", "", "TIER1_CELLS: tuple[Tier1Cell, ...] = ("))
    for cell in cells:
        assert isinstance(cell, dict)
        lines.append("    Tier1Cell(")
        lines.extend(f"        {field}={json.dumps(cell[field])}," for field in fields)
        lines.append("    ),")
    lines.extend((")", ""))
    return lines


def generated_outputs() -> dict[Path, str]:
    """Each output path mapped to its exact generated text."""
    return {OUTPUT: render_release_matrix(load_release_target_authority())}


def main(argv: list[str] | None = None) -> int:
    return generator_main(generated_outputs, argv, description=__doc__)


if __name__ == "__main__":
    raise SystemExit(main())
