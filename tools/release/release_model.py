"""Canonical release target, digest, and SBOM model."""

from __future__ import annotations

from dataclasses import dataclass
import datetime as dt
from email.parser import Parser
import hashlib
import json
from pathlib import Path
import re
import tomllib
from typing import Any, Iterable
import zipfile


ROOT = Path(__file__).resolve().parents[2]
CONFIG_PATH = ROOT / "config" / "release_supply_chain.toml"
VERSION_RE = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")


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
    document = load_config()
    targets = tuple(ReleaseTarget(**raw) for raw in document.get("target", []))
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
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def file_record(path: Path, *, kind: str) -> dict[str, object]:
    if not path.is_file():
        raise ValueError(f"release artifact is missing: {path}")
    return {
        "kind": kind,
        "filename": path.name,
        "sha256": sha256_file(path),
        "size": path.stat().st_size,
    }


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
        "spdxVersion": "SPDX-2.3",
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
