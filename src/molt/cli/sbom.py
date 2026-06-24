from __future__ import annotations

import json
from pathlib import Path
import re
from typing import Any

from molt.cli.compiler_metadata import _compiler_metadata, _rustc_version
from molt.cli.deps import _classify_tier, _dep_allowlists, _load_toml, _normalize_name
from molt.cli.file_hashing import _normalize_sha256


_SPDX_SAFE_RE = re.compile(r"[^A-Za-z0-9._-]+")


def _sbom_component_hashes(pkg: dict[str, Any]) -> list[dict[str, str]]:
    digests: set[str] = set()
    sdist = pkg.get("sdist")
    if isinstance(sdist, dict):
        digest = sdist.get("hash", "")
        if isinstance(digest, str) and digest:
            digests.add(digest)
    for wheel in pkg.get("wheels", []):
        if not isinstance(wheel, dict):
            continue
        digest = wheel.get("hash", "")
        if isinstance(digest, str) and digest:
            digests.add(digest)
    hashes: list[dict[str, str]] = []
    for entry in sorted(digests):
        if ":" in entry:
            algo, digest = entry.split(":", 1)
        else:
            algo, digest = "sha256", entry
        if digest:
            hashes.append({"alg": algo.upper(), "content": digest})
    return hashes


def _sbom_component_for_lock_pkg(
    pkg: dict[str, Any],
    allow: dict[str, set[str]],
) -> dict[str, Any] | None:
    name = pkg.get("name")
    if not isinstance(name, str) or not name.strip():
        return None
    source = pkg.get("source", {})
    if isinstance(source, dict) and source.get("virtual") == ".":
        return None
    version = pkg.get("version")
    if not isinstance(version, str):
        version = None
    norm = _normalize_name(name)
    purl = f"pkg:pypi/{norm}"
    if version:
        purl = f"{purl}@{version}"
    tier, reason = _classify_tier(name, pkg, allow)
    component: dict[str, Any] = {
        "type": "library",
        "name": name,
        "bom-ref": purl,
        "purl": purl,
    }
    if version:
        component["version"] = version
    hashes = _sbom_component_hashes(pkg)
    if hashes:
        component["hashes"] = hashes
    properties = [
        {"name": "molt.tier", "value": tier},
        {"name": "molt.tier_reason", "value": reason},
    ]
    if isinstance(source, dict):
        if source.get("git"):
            properties.append({"name": "molt.source", "value": "git"})
            if isinstance(source.get("git"), str):
                properties.append({"name": "molt.source_git", "value": source["git"]})
        elif source.get("path"):
            properties.append({"name": "molt.source", "value": "path"})
    component["properties"] = properties
    return component


def _sbom_dependencies(
    project_root: Path,
) -> tuple[list[dict[str, Any]], list[str], list[str]]:
    warnings: list[str] = []
    lock_path = project_root / "uv.lock"
    if not lock_path.exists():
        warnings.append("uv.lock not found; SBOM excludes Python dependencies.")
        return [], [], warnings
    lock = _load_toml(lock_path)
    pyproject = _load_toml(project_root / "pyproject.toml")
    allow = _dep_allowlists(pyproject)
    components: list[dict[str, Any]] = []
    refs: list[str] = []
    packages = lock.get("package", [])
    if not packages:
        warnings.append("uv.lock contains no package entries.")
        return [], [], warnings
    for pkg in packages:
        if not isinstance(pkg, dict):
            continue
        component = _sbom_component_for_lock_pkg(pkg, allow)
        if component is None:
            continue
        components.append(component)
    components.sort(key=lambda entry: (entry.get("name", ""), entry.get("version", "")))
    for component in components:
        ref = component.get("bom-ref")
        if isinstance(ref, str):
            refs.append(ref)
    return components, refs, warnings


def _build_sbom(
    *,
    manifest: dict[str, Any],
    artifact_path: Path,
    checksum: str,
    project_root: Path,
    format_name: str = "cyclonedx",
) -> tuple[dict[str, Any], list[str]]:
    if format_name == "cyclonedx":
        return _build_cyclonedx_sbom(
            manifest=manifest,
            artifact_path=artifact_path,
            checksum=checksum,
            project_root=project_root,
        )
    if format_name == "spdx":
        return _build_spdx_sbom(
            manifest=manifest,
            artifact_path=artifact_path,
            checksum=checksum,
            project_root=project_root,
        )
    raise ValueError(f"Unsupported SBOM format: {format_name}")


def _build_cyclonedx_sbom(
    *,
    manifest: dict[str, Any],
    artifact_path: Path,
    checksum: str,
    project_root: Path,
) -> tuple[dict[str, Any], list[str]]:
    warnings: list[str] = []
    compiler_version, compiler_rev = _compiler_metadata()
    rustc_version = _rustc_version()
    if rustc_version:
        rustc_version = rustc_version.splitlines()[0].strip() or rustc_version
    name = manifest.get("name", "molt_pkg")
    version = manifest.get("version", "0.0.0")
    target = manifest.get("target", "unknown")
    abi_version = manifest.get("abi_version")
    deterministic = manifest.get("deterministic")
    effects = manifest.get("effects")
    capabilities = manifest.get("capabilities")
    component_ref = f"pkg:molt/{_normalize_name(str(name))}@{version}"
    component = {
        "type": "library",
        "name": name,
        "version": version,
        "bom-ref": component_ref,
        "purl": component_ref,
        "hashes": [{"alg": "SHA-256", "content": checksum}],
        "properties": [
            {"name": "molt.target", "value": str(target)},
            {"name": "molt.abi_version", "value": str(abi_version)},
            {"name": "molt.deterministic", "value": str(deterministic)},
        ],
    }
    properties = component["properties"]
    if effects is not None:
        properties.append({"name": "molt.effects", "value": json.dumps(effects)})
    if capabilities is not None:
        properties.append(
            {"name": "molt.capabilities", "value": json.dumps(capabilities)}
        )
    properties.append({"name": "molt.artifact", "value": str(artifact_path)})
    meta_properties: list[dict[str, str]] = []
    if compiler_version:
        meta_properties.append(
            {"name": "molt.compiler.version", "value": compiler_version}
        )
    if compiler_rev:
        meta_properties.append({"name": "molt.compiler.git_rev", "value": compiler_rev})
    if rustc_version:
        meta_properties.append({"name": "molt.rustc.version", "value": rustc_version})
    components, dependency_refs, dep_warnings = _sbom_dependencies(project_root)
    warnings.extend(dep_warnings)
    sbom: dict[str, Any] = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "tools": [
                {
                    "vendor": "molt",
                    "name": "molt",
                    "version": compiler_version or "unknown",
                }
            ],
            "component": component,
        },
    }
    if meta_properties:
        sbom["metadata"]["properties"] = meta_properties
    if components:
        sbom["components"] = components
    if dependency_refs:
        sbom["dependencies"] = [{"ref": component_ref, "dependsOn": dependency_refs}]
    return sbom, warnings


def _spdx_id(base: str) -> str:
    cleaned = _SPDX_SAFE_RE.sub("-", base).strip(".-")
    if not cleaned:
        cleaned = "package"
    return f"SPDXRef-{cleaned}"


def _spdx_checksum(value: str | None) -> list[dict[str, str]] | None:
    digest = _normalize_sha256(value)
    if not digest:
        return None
    return [{"algorithm": "SHA256", "checksumValue": digest}]


def _spdx_package_entry(
    *,
    name: str,
    version: str | None,
    checksum: str | None,
    purl: str | None,
    spdx_id: str,
) -> dict[str, Any]:
    package: dict[str, Any] = {
        "SPDXID": spdx_id,
        "name": name,
        "downloadLocation": "NOASSERTION",
        "licenseConcluded": "NOASSERTION",
        "licenseDeclared": "NOASSERTION",
        "filesAnalyzed": False,
    }
    if version:
        package["versionInfo"] = version
    checksums = _spdx_checksum(checksum)
    if checksums:
        package["checksums"] = checksums
    if purl:
        package["externalRefs"] = [
            {
                "referenceCategory": "PACKAGE-MANAGER",
                "referenceType": "purl",
                "referenceLocator": purl,
            }
        ]
    return package


def _build_spdx_sbom(
    *,
    manifest: dict[str, Any],
    artifact_path: Path,
    checksum: str,
    project_root: Path,
) -> tuple[dict[str, Any], list[str]]:
    warnings: list[str] = []
    compiler_version, _compiler_rev = _compiler_metadata()
    name = manifest.get("name", "molt_pkg")
    version = manifest.get("version", "0.0.0")
    target = manifest.get("target", "unknown")
    namespace_seed = f"{name}-{version}-{target}-{checksum}"
    namespace_token = _SPDX_SAFE_RE.sub("-", namespace_seed).strip(".-")
    if not namespace_token:
        namespace_token = "molt"
    document_namespace = f"https://molt.dev/spdx/{namespace_token}"
    created = "1970-01-01T00:00:00Z"
    tool_version = compiler_version or "unknown"
    root_purl = f"pkg:molt/{_normalize_name(str(name))}@{version}"
    root_id = _spdx_id(f"{name}-{version}")

    packages: list[dict[str, Any]] = []
    packages.append(
        _spdx_package_entry(
            name=str(name),
            version=str(version),
            checksum=checksum,
            purl=root_purl,
            spdx_id=root_id,
        )
    )
    components, dependency_refs, dep_warnings = _sbom_dependencies(project_root)
    warnings.extend(dep_warnings)
    relationships: list[dict[str, str]] = []
    if components:
        for component in components:
            dep_name = str(component.get("name") or "dependency")
            dep_version = component.get("version")
            dep_id = _spdx_id(f"{dep_name}-{dep_version or 'unknown'}")
            dep_checksum = None
            hashes = component.get("hashes")
            if isinstance(hashes, list):
                for entry in hashes:
                    if (
                        isinstance(entry, dict)
                        and entry.get("alg") == "SHA-256"
                        and isinstance(entry.get("content"), str)
                    ):
                        dep_checksum = entry.get("content")
                        break
            dep_purl = component.get("purl") if isinstance(component, dict) else None
            packages.append(
                _spdx_package_entry(
                    name=dep_name,
                    version=str(dep_version) if dep_version else None,
                    checksum=dep_checksum,
                    purl=dep_purl if isinstance(dep_purl, str) else None,
                    spdx_id=dep_id,
                )
            )
            relationships.append(
                {
                    "spdxElementId": root_id,
                    "relationshipType": "DEPENDS_ON",
                    "relatedSpdxElement": dep_id,
                }
            )

    sbom: dict[str, Any] = {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": f"molt-{name}-{version}",
        "documentNamespace": document_namespace,
        "creationInfo": {
            "created": created,
            "creators": [f"Tool: molt {tool_version}"],
        },
        "documentDescribes": [root_id],
        "packages": packages,
    }
    if relationships:
        sbom["relationships"] = relationships
    return sbom, warnings
