"""Canonical release target, digest, and SBOM model."""

from __future__ import annotations

from dataclasses import dataclass
import datetime as dt
from email.parser import Parser
import json
from pathlib import Path
import re
import tomllib
from typing import Any, Iterable
import zipfile

from molt.release_matrix import RELEASE_TARGETS
from molt.portable_paths import portable_relative_path
from molt.toolchain_identity import stable_regular_file_identity
from packaging.utils import parse_wheel_filename
from tools.git_identity import require_git_object_id


ROOT = Path(__file__).resolve().parents[2]
CONFIG_PATH = ROOT / "config" / "release_supply_chain.toml"
VERSION_RE = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
MANIFEST_SCHEMA = "molt.release-manifest.v3"
SPDX_VERSION = "2.3"
SPDX_PREDICATE_TYPE = f"https://spdx.dev/Document/v{SPDX_VERSION}"
RELEASE_EXIT_ARCHIVE_KIND = "release-exit-evidence"
PHASE_EXIT_KIND = "phase-exit-evidence"
PHASE_ATTESTATION_KIND = "phase-exit-attestation"
FILE_FIELDS = frozenset({"kind", "filename", "sha256", "size"})
ATTESTATION_POLICY = {
    "provenance": "SLSA v1 signed by GitHub artifact attestations",
    "sbom": f"SPDX {SPDX_VERSION} signed by GitHub artifact attestations",
    "signature": "Sigstore keyless OIDC certificate",
}


def release_exit_archive_filename(source_sha: str) -> str:
    require_git_object_id(source_sha, label="release source SHA")
    return f"molt-release-exit-{source_sha}.zip"


def stable_release(version: str) -> bool:
    return int(normalized_version(version).split(".")[0]) >= 1


def phase_exit_filename(source_sha: str) -> str:
    require_git_object_id(source_sha, label="release source SHA")
    return f"molt-phase-exit-H0-{source_sha}.json"


def phase_exit_attestation_filename(source_sha: str) -> str:
    return phase_exit_filename(source_sha).removesuffix(".json") + ".sigstore.json"


@dataclass(frozen=True)
class ReleaseTarget:
    id: str
    runner: str
    platform: str
    arch: str
    archive: str

    @property
    def worker_filename(self) -> str:
        return "molt-worker.exe" if self.platform == "windows" else "molt-worker"

    def artifact_filename(self, name: str, version: str) -> str:
        return f"{name}-{version}-{self.platform}-{self.arch}.{self.archive}"


def load_config() -> dict[str, Any]:
    with CONFIG_PATH.open("rb") as handle:
        document = tomllib.load(handle)
    if document.get("schema") != "molt.release-supply-chain.v1":
        raise ValueError("release supply-chain manifest schema is not supported")
    return document


def release_targets() -> tuple[ReleaseTarget, ...]:
    """Release targets are the generated release matrix; there is no second table."""
    targets = tuple(
        ReleaseTarget(
            id=str(record["id"]),
            runner=str(record["runner"]),
            platform=str(record["platform"]),
            arch=str(record["arch"]),
            archive=str(record["archive"]),
        )
        for record in RELEASE_TARGETS
    )
    ids = [target.id for target in targets]
    coordinates = [(target.platform, target.arch) for target in targets]
    if not targets or len(ids) != len(set(ids)):
        raise ValueError("release target ids must be non-empty and unique")
    if len(coordinates) != len(set(coordinates)):
        raise ValueError("release platform/architecture coordinates must be unique")
    for target in targets:
        if target.platform not in {"linux", "macos", "windows"}:
            raise ValueError(f"unsupported release platform: {target.platform}")
        expected_archive = "zip" if target.platform == "windows" else "tar.gz"
        if target.archive != expected_archive:
            raise ValueError(
                f"{target.id}: expected {expected_archive}, got {target.archive}"
            )
    return targets


def target_by_id(target_id: str) -> ReleaseTarget:
    for target in release_targets():
        if target.id == target_id:
            return target
    raise ValueError(f"unknown release target: {target_id}")


def normalized_version(raw: str) -> str:
    value = raw.strip()
    if value.startswith("v"):
        value = value[1:]
    if not VERSION_RE.fullmatch(value):
        raise ValueError(f"invalid release version: {raw!r}")
    return value


def _numeric_version_identity(value: str) -> tuple[int, int, int]:
    normalized = normalized_version(value)
    major, minor, patch = normalized.split(".")
    return int(major), int(minor), int(patch)


def sha256_file(path: Path) -> str:
    return stable_regular_file_identity(path, label="release file").sha256


def file_record(path: Path, *, kind: str) -> dict[str, object]:
    identity = stable_regular_file_identity(path, label="release artifact")
    return {
        "kind": kind,
        "filename": path.name,
        "sha256": identity.sha256,
        "size": identity.size,
    }


def validate_file_record(value: object) -> dict[str, Any]:
    if not isinstance(value, dict) or not FILE_FIELDS <= value.keys():
        raise ValueError("release file record is incomplete")
    name = value["filename"]
    if len(portable_relative_path(name).parts) != 1:
        raise ValueError("release file name must be one portable component")
    digest = value["sha256"]
    if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
        raise ValueError("release file SHA256 is invalid")
    if type(value["size"]) is not int or value["size"] <= 0:
        raise ValueError("release file size must be a positive integer")
    if not isinstance(value["kind"], str) or not value["kind"]:
        raise ValueError("release file kind is invalid")
    return value


def validate_artifact_record(
    value: object, *, version: str, published: bool = False
) -> dict[str, Any]:
    record = validate_file_record(value)
    keys = FILE_FIELDS | {"name", "version", "platform", "arch", "libc"}
    if published:
        keys |= {"url"}
    if set(record) != keys or record["version"] != version:
        raise ValueError("release artifact metadata is invalid")
    name, platform, arch = (record[key] for key in ("name", "platform", "arch"))
    if name == "molt-wheel":
        wheel_name, wheel_version, _, _ = parse_wheel_filename(record["filename"])
        if (
            wheel_name != "molt"
            or _numeric_version_identity(str(wheel_version))
            != _numeric_version_identity(version)
            or (record["kind"], platform, arch, record["libc"])
            != ("wheel", "any", "any", None)
        ):
            raise ValueError("release wheel metadata is invalid")
    else:
        target = next(
            (t for t in release_targets() if (t.platform, t.arch) == (platform, arch)),
            None,
        )
        if (
            target is None
            or name not in {"molt", "molt-worker"}
            or record["kind"] != name
            or record["filename"] != target.artifact_filename(name, version)
            or record["libc"] != ("gnu" if platform == "linux" else None)
        ):
            raise ValueError("release artifact target metadata is invalid")
    return record


def validate_release_manifest(value: object) -> dict[str, Any]:
    keys = {
        "schema",
        "version",
        "source_sha",
        "source_date_epoch",
        "repo",
        "evidence_archive",
        "phase_exit",
        "artifacts",
        "attestation",
    }
    if (
        not isinstance(value, dict)
        or set(value) != keys
        or value.get("schema") != MANIFEST_SCHEMA
    ):
        raise ValueError(f"release manifest must use exact {MANIFEST_SCHEMA} schema")
    version = value["version"]
    if not isinstance(version, str) or normalized_version(version) != version:
        raise ValueError("release manifest version is invalid")
    source_sha = require_git_object_id(
        value["source_sha"], label="release manifest source SHA"
    )
    if type(value["source_date_epoch"]) is not int or value["source_date_epoch"] <= 0:
        raise ValueError("release manifest source epoch is invalid")
    config = load_config()["repository"]
    repo = f"{config['owner']}/{config['name']}"
    if value["repo"] != repo or value["attestation"] != ATTESTATION_POLICY:
        raise ValueError("release manifest repository/attestation policy differs")
    evidence = validate_file_record(value["evidence_archive"])
    if (
        set(evidence) != FILE_FIELDS
        or evidence["kind"] != RELEASE_EXIT_ARCHIVE_KIND
        or evidence["filename"] != release_exit_archive_filename(source_sha)
    ):
        raise ValueError("release manifest evidence archive is invalid")
    phase = value["phase_exit"]
    if stable_release(version):
        if not isinstance(phase, dict) or set(phase) != {"manifest", "attestation"}:
            raise ValueError("release manifest H0 phase evidence is invalid")
        for key, kind, filename in (
            ("manifest", PHASE_EXIT_KIND, phase_exit_filename(source_sha)),
            (
                "attestation",
                PHASE_ATTESTATION_KIND,
                phase_exit_attestation_filename(source_sha),
            ),
        ):
            record = validate_file_record(phase[key])
            if set(record) != FILE_FIELDS or (record["kind"], record["filename"]) != (
                kind,
                filename,
            ):
                raise ValueError("release manifest H0 phase evidence is invalid")
    elif phase is not None:
        raise ValueError("pre-stable release manifest must not claim an H0 phase exit")
    artifacts = value["artifacts"]
    expected = {("molt-wheel", "any", "any")} | {
        (name, target.platform, target.arch)
        for target in release_targets()
        for name in ("molt", "molt-worker")
    }
    if not isinstance(artifacts, list) or len(artifacts) != len(expected):
        raise ValueError("release manifest artifact matrix is incomplete")
    names: list[str] = []
    coordinates: set[tuple[str, str, str]] = set()
    for raw in artifacts:
        record = validate_artifact_record(raw, version=version, published=True)
        names.append(record["filename"])
        coordinates.add((record["name"], record["platform"], record["arch"]))
        if (
            record["url"]
            != f"https://github.com/{repo}/releases/download/v{version}/{record['filename']}"
        ):
            raise ValueError("release artifact URL differs from its source")
    if names != sorted(set(names)) or coordinates != expected:
        raise ValueError("release manifest artifact matrix must be exact and sorted")
    return value


def release_subjects(manifest: dict[str, Any]) -> list[dict[str, Any]]:
    """One checksum/SBOM/publication subject projection, including semantic proof."""
    subjects = [*manifest["artifacts"], manifest["evidence_archive"]]
    if manifest["phase_exit"] is not None:
        subjects.extend(manifest["phase_exit"].values())
    return sorted(subjects, key=lambda record: record["filename"])


def write_json(path: Path, payload: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def _spdx_id(value: str) -> str:
    return "SPDXRef-" + re.sub(r"[^A-Za-z0-9.-]", "-", value).strip("-")


def _wheel_requirements(wheel: Path) -> tuple[str, tuple[str, ...]]:
    with zipfile.ZipFile(wheel) as archive:
        metadata_names = [
            name for name in archive.namelist() if name.endswith(".dist-info/METADATA")
        ]
        if len(metadata_names) != 1:
            raise ValueError(f"wheel must contain exactly one METADATA file: {wheel}")
        metadata = Parser().parsestr(archive.read(metadata_names[0]).decode("utf-8"))
    version = str(metadata.get("Version", ""))
    requirements = tuple(sorted(metadata.get_all("Requires-Dist", [])))
    return version, requirements


def spdx_document(
    *,
    version: str,
    source_sha: str,
    source_date_epoch: int,
    subjects: Iterable[dict[str, object]],
    wheel: Path,
) -> dict[str, object]:
    config = load_config()
    owner = str(config["repository"]["owner"])
    repository = str(config["repository"]["name"])
    wheel_version, python_requirements = _wheel_requirements(wheel)
    if _numeric_version_identity(wheel_version) != _numeric_version_identity(version):
        raise ValueError(f"wheel version {wheel_version} does not match {version}")

    cargo = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    cargo_packages = sorted(
        cargo.get("package", []),
        key=lambda item: (str(item["name"]), str(item["version"])),
    )
    packages: list[dict[str, object]] = [
        {
            "SPDXID": "SPDXRef-Package-Molt",
            "name": "molt",
            "versionInfo": wheel_version,
            "downloadLocation": "NOASSERTION",
            "filesAnalyzed": False,
            "licenseConcluded": "Apache-2.0",
            "licenseDeclared": "Apache-2.0",
            "supplier": "Organization: Molt contributors",
        }
    ]
    relationships: list[dict[str, str]] = []
    for index, requirement in enumerate(python_requirements):
        package_id = _spdx_id(f"Python-{index}-{requirement}")
        packages.append(
            {
                "SPDXID": package_id,
                "name": requirement,
                "downloadLocation": "NOASSERTION",
                "filesAnalyzed": False,
                "licenseConcluded": "NOASSERTION",
                "licenseDeclared": "NOASSERTION",
            }
        )
        relationships.append(
            {
                "spdxElementId": "SPDXRef-Package-Molt",
                "relationshipType": "DEPENDS_ON",
                "relatedSpdxElement": package_id,
            }
        )
    for index, package in enumerate(cargo_packages):
        name = str(package["name"])
        package_version = str(package["version"])
        package_id = _spdx_id(f"Cargo-{index}-{name}-{package_version}")
        entry: dict[str, object] = {
            "SPDXID": package_id,
            "name": name,
            "versionInfo": package_version,
            "downloadLocation": str(package.get("source", "NOASSERTION")),
            "filesAnalyzed": False,
            "licenseConcluded": "NOASSERTION",
            "licenseDeclared": "NOASSERTION",
            "externalRefs": [
                {
                    "referenceCategory": "PACKAGE-MANAGER",
                    "referenceType": "purl",
                    "referenceLocator": f"pkg:cargo/{name}@{package_version}",
                }
            ],
        }
        if checksum := package.get("checksum"):
            entry["checksums"] = [
                {"algorithm": "SHA256", "checksumValue": str(checksum)}
            ]
        packages.append(entry)
        relationships.append(
            {
                "spdxElementId": "SPDXRef-Package-Molt",
                "relationshipType": "DEPENDS_ON",
                "relatedSpdxElement": package_id,
            }
        )

    files: list[dict[str, object]] = []
    for subject in sorted(subjects, key=lambda item: str(item["filename"])):
        filename = str(subject["filename"])
        file_id = _spdx_id(f"File-{filename}")
        files.append(
            {
                "SPDXID": file_id,
                "fileName": f"./{filename}",
                "checksums": [
                    {"algorithm": "SHA256", "checksumValue": subject["sha256"]}
                ],
            }
        )
        relationships.append(
            {
                "spdxElementId": "SPDXRef-Package-Molt",
                "relationshipType": "CONTAINS",
                "relatedSpdxElement": file_id,
            }
        )
    relationships.insert(
        0,
        {
            "spdxElementId": "SPDXRef-DOCUMENT",
            "relationshipType": "DESCRIBES",
            "relatedSpdxElement": "SPDXRef-Package-Molt",
        },
    )
    created = (
        dt.datetime.fromtimestamp(source_date_epoch, tz=dt.UTC)
        .isoformat()
        .replace("+00:00", "Z")
    )
    return {
        "spdxVersion": f"SPDX-{SPDX_VERSION}",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": f"molt-{version}-release",
        "documentNamespace": (
            f"https://github.com/{owner}/{repository}/releases/download/"
            f"v{version}/spdx/{source_sha}"
        ),
        "creationInfo": {"created": created, "creators": ["Tool: molt-release/1"]},
        "documentDescribes": ["SPDXRef-Package-Molt"],
        "packages": packages,
        "files": files,
        "relationships": relationships,
    }
