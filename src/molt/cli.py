import argparse
import ast
import base64
import errno
import datetime as dt
import hashlib
import http.client
import tempfile
import json
import keyword
import os
import platform
import posixpath
import re
import shlex
import shutil
import subprocess
import sys
import tomllib
import time
import tokenize
import urllib.parse
import urllib.request
import uuid
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Literal, NamedTuple

from packaging.markers import InvalidMarker, Marker
from packaging.requirements import InvalidRequirement, Requirement
from molt.compat import CompatibilityError
from molt.frontend import SimpleTIRGenerator
from molt.type_facts import (
    collect_type_facts_from_paths,
    load_type_facts,
    write_type_facts,
)

Target = str
ParseCodec = Literal["msgpack", "cbor", "json"]
TypeHintPolicy = Literal["ignore", "trust", "check"]
FallbackPolicy = Literal["error", "bridge"]
BuildProfile = Literal["dev", "release"]
EmitMode = Literal["bin", "obj", "wasm"]
STUB_MODULES = {"molt_buffer", "molt_cbor", "molt_json", "molt_msgpack"}
STUB_PARENT_MODULES = {"molt"}
ENTRY_OVERRIDE_ENV = "MOLT_ENTRY_MODULE"
ENTRY_OVERRIDE_SPAWN = "multiprocessing.spawn"
IMPORTER_MODULE_NAME = "_molt_importer"
JSON_SCHEMA_VERSION = "1.0"
REMOTE_REGISTRY_SCHEMES = {"http", "https"}
CAPABILITY_PROFILES: dict[str, list[str]] = {
    "core": [],
    "fs": ["fs.read", "fs.write"],
    "env": ["env.read", "env.write"],
    "net": ["net", "websocket.connect", "websocket.listen"],
    "db": ["db.read", "db.write"],
    "time": ["time"],
    "random": ["random"],
}
CAPABILITY_TOKEN_RE = re.compile(r"^[a-z0-9][a-z0-9._-]*$")
_OUTPUT_BASE_SAFE_RE = re.compile(r"[^A-Za-z0-9._-]+")
_ABI_VERSION_RE = re.compile(r"^(\d+)\.(\d+)(?:\.(\d+))?$")
_SUPPORTED_PKG_ABI_MAJOR = 0
_SUPPORTED_PKG_ABI_MINOR = 1
_SUPPORTED_PKG_ABI = f"{_SUPPORTED_PKG_ABI_MAJOR}.{_SUPPORTED_PKG_ABI_MINOR}"
CapabilityInput = str | list[str] | dict[str, Any]


def _dedupe_preserve_order(items: Iterable[str]) -> list[str]:
    seen: set[str] = set()
    deduped: list[str] = []
    for item in items:
        if item in seen:
            continue
        seen.add(item)
        deduped.append(item)
    return deduped


def _split_tokens(value: str) -> list[str]:
    return [token for token in re.split(r"[,\s]+", value) if token]


@dataclass(frozen=True)
class CapabilityGrant:
    allow: list[str] | None
    deny: list[str]
    effects: list[str] | None

    def merged(self, other: "CapabilityGrant") -> "CapabilityGrant":
        allow = _merge_optional_list(self.allow, other.allow)
        deny = _dedupe_preserve_order([*self.deny, *other.deny])
        effects = _merge_optional_list(self.effects, other.effects)
        return CapabilityGrant(allow=allow, deny=deny, effects=effects)


@dataclass(frozen=True)
class CapabilityManifest:
    allow: list[str] | None
    deny: list[str]
    effects: list[str] | None
    packages: dict[str, CapabilityGrant]


@dataclass(frozen=True)
class CapabilitySpec:
    capabilities: list[str] | None
    profiles: list[str]
    source: str | None
    errors: list[str]
    manifest: CapabilityManifest | None


@dataclass(frozen=True)
class PgoProfileSummary:
    version: str
    hash: str
    hot_functions: list[str]


def _emit_json(payload: dict[str, Any], json_output: bool) -> None:
    if json_output:
        print(json.dumps(payload))


def _json_payload(
    command: str,
    status: str,
    *,
    data: dict[str, Any] | None = None,
    warnings: list[str] | None = None,
    errors: list[str] | None = None,
) -> dict[str, Any]:
    payload = {
        "schema_version": JSON_SCHEMA_VERSION,
        "command": command,
        "status": status,
        "data": data or {},
        "warnings": warnings or [],
        "errors": errors or [],
    }
    return payload


def _fail(
    message: str,
    json_output: bool,
    code: int = 2,
    command: str = "molt",
) -> int:
    if json_output:
        payload = _json_payload(
            command,
            "error",
            data={"returncode": code},
            errors=[message],
        )
        _emit_json(payload, json_output=True)
    else:
        print(message, file=sys.stderr)
    return code


def _write_importer_module(module_names: list[str], output_dir: Path) -> Path:
    filtered_names = [name for name in module_names if name]

    def needs_importlib(name: str) -> bool:
        return any(
            not part.isidentifier() or keyword.iskeyword(part)
            for part in name.split(".")
        )

    importlib_needed = any(needs_importlib(name) for name in filtered_names)
    lines = [
        '"""Auto-generated import dispatcher for Molt-compiled modules."""',
        "",
        "from __future__ import annotations",
        "",
    ]
    if importlib_needed:
        lines.append("import importlib as _importlib")
        lines.append("")
    lines.extend(
        [
            "import sys as _sys",
            "",
            "def _resolve_name(name: str, package: str | None, level: int) -> str:",
            "    if level <= 0:",
            "        return name",
            "    if not package:",
            '        raise ImportError("relative import requires package")',
            '    parts = package.split(".")',
            "    if level > len(parts):",
            '        raise ImportError(\"attempted relative import beyond top-level package\")',
            "    cut = len(parts) - level + 1",
            "    base = \".\".join(parts[:cut])",
            "    if name:",
            "        return f\"{base}.{name}\" if base else name",
            "    return base",
            "",
            "def _molt_import(name, globals=None, locals=None, fromlist=(), level=0):",
            "    if not name:",
            '        raise ImportError("Empty module name")',
            "    package = None",
            "    if isinstance(globals, dict):",
            '        package = globals.get(\"__package__\")',
            "        if not package and globals.get(\"__path__\") and globals.get(\"__name__\"):",
            '            package = globals.get(\"__name__\")',
            "    resolved = _resolve_name(name, package, level) if level else name",
            '    modules = getattr(_sys, "modules", {})',
            "    if resolved in modules:",
            "        mod = modules[resolved]",
            "        if mod is None:",
            '            raise ImportError(f\"import of {resolved} halted; None in sys.modules\")',
            "        if fromlist:",
            "            return mod",
            '        top = resolved.split(\".\", 1)[0]',
            "        return modules.get(top, mod)",
        ]
    )
    for module_name in filtered_names:
        lines.append(f"    if resolved == {module_name!r}:")
        if needs_importlib(module_name):
            lines.append("        _importlib.import_module(resolved)")
        else:
            lines.append(f"        import {module_name}")
        lines.append("        mod = modules.get(resolved)")
        lines.append("        if mod is None:")
        lines.append("            raise ImportError(f\"No module named '{resolved}'\")")
        lines.append("        if fromlist:")
        lines.append("            return mod")
        lines.append('        top = resolved.split(".", 1)[0]')
        lines.append("        return modules.get(top, mod)")
    lines.append("    raise ImportError(f\"No module named '{resolved}'\")")
    path = output_dir / f"{IMPORTER_MODULE_NAME}.py"
    path.write_text("\n".join(lines) + "\n")
    return path


def _collect_env_overrides(file_path: str) -> dict[str, str]:
    overrides: dict[str, str] = {}
    try:
        text = Path(file_path).read_text()
    except OSError:
        return overrides
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith("# MOLT_ENV:"):
            continue
        payload = stripped[len("# MOLT_ENV:") :].strip()
        for token in payload.split():
            if "=" not in token:
                continue
            key, value = token.split("=", 1)
            overrides[key] = value
    return overrides


def _resolve_python_exe(python_exe: str | None) -> str:
    if not python_exe:
        return sys.executable
    if python_exe[0].isdigit() and os.sep not in python_exe:
        python_exe = f"python{python_exe}"
    if os.sep in python_exe or Path(python_exe).is_absolute():
        candidate = Path(python_exe)
        if candidate.exists():
            return python_exe
        base_exe = getattr(sys, "_base_executable", "")
        if base_exe and Path(base_exe).exists():
            return base_exe
    return python_exe


def _vendor_roots(project_root: Path) -> list[Path]:
    vendor_root = project_root / "vendor"
    roots: list[Path] = []
    for name in ("packages", "local"):
        candidate = vendor_root / name
        if candidate.exists():
            roots.append(candidate)
    return roots


def _base_env(
    root: Path,
    script_path: Path | None = None,
    *,
    molt_root: Path | None = None,
) -> dict[str, str]:
    env = os.environ.copy()
    paths = [env.get("PYTHONPATH", "")]
    if script_path is not None:
        paths.append(str(script_path.parent))
    roots: list[Path] = []
    if molt_root is not None and molt_root != root:
        roots.append(molt_root)
    roots.append(root)
    for base in roots:
        paths.extend([str(base / "src"), str(base)])
        paths.extend(str(path) for path in _vendor_roots(base))
    env["PYTHONPATH"] = os.pathsep.join(p for p in paths if p)
    if molt_root is not None:
        env.setdefault("MOLT_PROJECT_ROOT", str(molt_root))
    return env


def _run_command(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
    json_output: bool = False,
    verbose: bool = False,
    label: str | None = None,
    warnings: list[str] | None = None,
) -> int:
    cmd = [str(part) for part in cmd]
    if verbose and not json_output:
        print(f"Running: {shlex.join(cmd)}")
    capture = json_output
    result = subprocess.run(
        cmd,
        env=env,
        cwd=cwd,
        capture_output=capture,
        text=True,
    )
    if json_output:
        data: dict[str, Any] = {"returncode": result.returncode}
        if result.stdout:
            data["stdout"] = result.stdout
        if result.stderr:
            data["stderr"] = result.stderr
        payload = _json_payload(
            label or cmd[0],
            "ok" if result.returncode == 0 else "error",
            data=data,
            warnings=warnings,
        )
        _emit_json(payload, json_output=True)
    return result.returncode


class _TimedResult(NamedTuple):
    returncode: int
    stdout: str
    stderr: str
    duration_s: float


def _run_command_timed(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
    verbose: bool = False,
    capture_output: bool = False,
) -> _TimedResult:
    cmd = [str(part) for part in cmd]
    if verbose:
        print(f"Running: {shlex.join(cmd)}")
    start = time.perf_counter()
    result = subprocess.run(
        cmd,
        env=env,
        cwd=cwd,
        capture_output=capture_output,
        text=True,
    )
    duration = time.perf_counter() - start
    return _TimedResult(
        result.returncode,
        result.stdout or "",
        result.stderr or "",
        duration,
    )


def _format_duration(seconds: float) -> str:
    if seconds < 0:
        seconds = 0.0
    if seconds < 0.001:
        return f"{seconds * 1_000_000:.0f} µs"
    if seconds < 1:
        return f"{seconds * 1000:.1f} ms"
    if seconds < 60:
        return f"{seconds:.3f} s"
    return f"{seconds / 60:.2f} min"


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _git_rev(root: Path) -> str | None:
    try:
        result = subprocess.run(
            ["git", "-C", str(root), "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    value = result.stdout.strip()
    return value or None


def _abi_version_error(value: str) -> str | None:
    cleaned = value.strip()
    match = _ABI_VERSION_RE.match(cleaned)
    if match is None:
        return "abi_version must be MAJOR.MINOR[.PATCH] (e.g., 0.1)"
    major = int(match.group(1))
    minor = int(match.group(2))
    if major != _SUPPORTED_PKG_ABI_MAJOR or minor != _SUPPORTED_PKG_ABI_MINOR:
        return f"unsupported abi_version {cleaned} (supported: {_SUPPORTED_PKG_ABI})"
    return None


def _manifest_errors(manifest: dict[str, Any]) -> list[str]:
    required = [
        "name",
        "version",
        "abi_version",
        "target",
        "capabilities",
        "deterministic",
        "effects",
    ]
    errors: list[str] = []
    for key in required:
        if key not in manifest:
            errors.append(f"missing {key}")
    name = manifest.get("name")
    version = manifest.get("version")
    abi_version = manifest.get("abi_version")
    target = manifest.get("target")
    capabilities = manifest.get("capabilities")
    deterministic = manifest.get("deterministic")
    effects = manifest.get("effects")
    exports = manifest.get("exports")
    if name is not None and not isinstance(name, str):
        errors.append("name must be a string")
    if version is not None and not isinstance(version, str):
        errors.append("version must be a string")
    if abi_version is not None and not isinstance(abi_version, str):
        errors.append("abi_version must be a string")
    if isinstance(abi_version, str):
        abi_error = _abi_version_error(abi_version)
        if abi_error:
            errors.append(abi_error)
    if target is not None and not isinstance(target, str):
        errors.append("target must be a string")
    if capabilities is not None:
        if not isinstance(capabilities, list) or not all(
            isinstance(item, str) for item in capabilities
        ):
            errors.append("capabilities must be a list of strings")
    if deterministic is not None and not isinstance(deterministic, bool):
        errors.append("deterministic must be a boolean")
    if effects is not None and not isinstance(effects, (list, str)):
        errors.append("effects must be a list or string")
    if exports is not None:
        if not isinstance(exports, list) or not all(
            isinstance(item, str) for item in exports
        ):
            errors.append("exports must be a list of strings")
    return errors


def _normalize_effects(value: Any) -> list[str]:
    if value is None:
        return []
    if isinstance(value, list):
        normalized: list[str] = []
        for entry in value:
            if isinstance(entry, str):
                stripped = entry.strip()
                if stripped:
                    normalized.append(stripped)
        return normalized
    if isinstance(value, str):
        return _split_tokens(value)
    return []


def _load_manifest(path: Path) -> dict[str, Any] | None:
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return None


def _write_zip_member(zf: zipfile.ZipFile, name: str, data: bytes) -> None:
    info = zipfile.ZipInfo(name)
    info.date_time = (1980, 1, 1, 0, 0, 0)
    info.compress_type = zipfile.ZIP_DEFLATED
    zf.writestr(info, data)


def _compiler_metadata() -> tuple[str | None, str | None]:
    compiler_root = Path(__file__).resolve().parents[2]
    pyproject = _load_toml(compiler_root / "pyproject.toml")
    version = pyproject.get("project", {}).get("version")
    git_rev = _git_rev(compiler_root)
    return version if isinstance(version, str) else None, git_rev


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
    if effects is not None:
        component["properties"].append(
            {"name": "molt.effects", "value": json.dumps(effects)}
        )
    if capabilities is not None:
        component["properties"].append(
            {"name": "molt.capabilities", "value": json.dumps(capabilities)}
        )
    component["properties"].append(
        {"name": "molt.artifact", "value": str(artifact_path)}
    )
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
    cleaned = _OUTPUT_BASE_SAFE_RE.sub("-", base).strip(".-")
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
    namespace_token = _OUTPUT_BASE_SAFE_RE.sub("-", namespace_seed).strip(".-")
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


def _is_macho(path: Path) -> bool:
    try:
        data = path.read_bytes()[:4]
    except OSError:
        return False
    if len(data) < 4:
        return False
    be = int.from_bytes(data, "big")
    le = int.from_bytes(data, "little")
    magic_values = {
        0xFEEDFACE,
        0xCEFAEDFE,
        0xFEEDFACF,
        0xCFFAEDFE,
        0xCAFEBABE,
        0xBEBAFECA,
    }
    return be in magic_values or le in magic_values


def _cosign_key_hash(key_path: Path) -> str | None:
    try:
        return _sha256_file(key_path)
    except OSError:
        return None


def _cosign_sign_blob(
    artifact_path: Path,
    key: str,
    *,
    tlog_upload: bool = False,
) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="molt_cosign_") as tmpdir:
        sig_path = Path(tmpdir) / "artifact.sig"
        cert_path = Path(tmpdir) / "artifact.pem"
        cmd = [
            "cosign",
            "sign-blob",
            "--yes",
            "--key",
            key,
            "--output-signature",
            str(sig_path),
            "--output-certificate",
            str(cert_path),
        ]
        if not tlog_upload:
            cmd.append("--tlog-upload=false")
        cmd.append(str(artifact_path))
        result = subprocess.run(cmd, capture_output=True, text=True, check=False)
        if result.returncode != 0:
            detail = (result.stderr or result.stdout).strip() or "unknown error"
            raise RuntimeError(f"cosign sign-blob failed: {detail}")
        signature = sig_path.read_text().strip()
        certificate = cert_path.read_text().strip()
    metadata: dict[str, Any] = {
        "tool": {"name": "cosign"},
        "signature": {"format": "cosign-blob", "value": signature},
    }
    if certificate:
        metadata["signature"]["certificate"] = certificate
    key_path = Path(key).expanduser()
    if key_path.exists():
        key_hash = _cosign_key_hash(key_path)
        if key_hash:
            metadata["key"] = {"sha256": key_hash}
    return metadata


def _codesign_identity_info(artifact_path: Path) -> dict[str, Any]:
    result = subprocess.run(
        ["codesign", "--display", "--verbose=4", str(artifact_path)],
        capture_output=True,
        text=True,
        check=False,
    )
    output = (result.stderr or "") + (result.stdout or "")
    info: dict[str, Any] = {"tool": {"name": "codesign"}}
    authorities: list[str] = []
    for line in output.splitlines():
        if line.startswith("Authority="):
            authorities.append(line.split("=", 1)[1].strip())
        elif line.startswith("TeamIdentifier="):
            info["team_id"] = line.split("=", 1)[1].strip()
        elif line.startswith("Identifier="):
            info["identifier"] = line.split("=", 1)[1].strip()
        elif line.startswith("Format="):
            info["format"] = line.split("=", 1)[1].strip()
    if authorities:
        info["authorities"] = authorities
    return info


def _codesign_sign(artifact_path: Path, identity: str) -> dict[str, Any]:
    cmd = [
        "codesign",
        "--force",
        "--sign",
        identity,
        "--timestamp=none",
        str(artifact_path),
    ]
    result = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip() or "unknown error"
        raise RuntimeError(f"codesign failed: {detail}")
    info = _codesign_identity_info(artifact_path)
    metadata: dict[str, Any] = {"tool": {"name": "codesign"}}
    metadata.update(info)
    return metadata


def _select_signer(preferred: str | None, *, artifact_path: Path | None) -> str | None:
    selected = preferred
    if selected in {"auto", "", None}:
        if (
            sys.platform == "darwin"
            and shutil.which("codesign")
            and (artifact_path is None or _is_macho(artifact_path))
        ):
            return "codesign"
        if shutil.which("cosign"):
            return "cosign"
        if sys.platform == "darwin" and shutil.which("codesign"):
            return "codesign"
        return None
    return selected


def _sign_artifact(
    *,
    artifact_path: Path,
    sign: bool,
    signer: str | None,
    signing_key: str | None,
    signing_identity: str | None,
    tlog_upload: bool,
) -> tuple[dict[str, Any] | None, str | None]:
    if not sign:
        return None, None
    selected = _select_signer(signer, artifact_path=artifact_path)
    if selected is None:
        raise RuntimeError("No signing tool available (cosign/codesign not found)")
    if selected == "cosign":
        key = signing_key or os.environ.get("COSIGN_KEY")
        if not key:
            raise RuntimeError("cosign signing requires --signing-key or COSIGN_KEY")
        cosign_meta = _cosign_sign_blob(artifact_path, key, tlog_upload=tlog_upload)
        return cosign_meta, selected
    if selected == "codesign":
        if sys.platform != "darwin":
            raise RuntimeError("codesign signing is only available on macOS")
        if not _is_macho(artifact_path):
            raise RuntimeError("codesign requires a Mach-O artifact")
        identity = signing_identity or os.environ.get("MOLT_CODESIGN_IDENTITY")
        if not identity:
            raise RuntimeError(
                "codesign signing requires --signing-identity or MOLT_CODESIGN_IDENTITY"
            )
        codesign_meta = _codesign_sign(artifact_path, identity)
        return codesign_meta, selected
    raise RuntimeError(f"Unsupported signer: {selected}")


def _signature_metadata(
    *,
    artifact_path: Path,
    checksum: str,
    signer_meta: dict[str, Any] | None,
    signer: str | None,
    signature_name: str | None,
    signature_checksum: str | None,
) -> dict[str, Any]:
    metadata: dict[str, Any] = {
        "schema_version": 1,
        "artifact": {"path": str(artifact_path), "sha256": checksum},
    }
    signed = signer_meta is not None or signature_name is not None
    metadata["status"] = "signed" if signed else "unsigned"
    if not signed:
        metadata["reason"] = "signing disabled"
    signature_info: dict[str, Any] = {
        "status": "signed" if signature_name or signer_meta is not None else "unsigned",
        "algorithm": "sha256",
    }
    if signature_name:
        signature_info["file"] = signature_name
    if signature_checksum:
        signature_info["checksum"] = signature_checksum
    metadata["signature"] = signature_info
    if signature_name:
        metadata["signature_file"] = {
            "name": signature_name,
            "sha256": signature_checksum,
        }
    if signer_meta is not None:
        metadata["signer"] = signer_meta
        if signer:
            metadata["signer"]["selected"] = signer
    return metadata


@dataclass(frozen=True)
class TrustPolicy:
    cosign_keys: set[str]
    cosign_cert_substrings: list[str]
    codesign_team_ids: set[str]
    codesign_identifiers: set[str]
    codesign_authorities: set[str]


def _normalize_sha256(value: str | None) -> str | None:
    if not value:
        return None
    cleaned = value.strip().lower()
    if cleaned.startswith("sha256:"):
        cleaned = cleaned[len("sha256:") :]
    return cleaned


def _load_trust_policy(path: Path) -> TrustPolicy:
    if not path.exists():
        raise FileNotFoundError(f"Trust policy not found: {path}")
    if path.suffix == ".json":
        data = json.loads(path.read_text())
    else:
        data = tomllib.loads(path.read_text())
    cosign = data.get("cosign", {})
    codesign = data.get("codesign", {})
    cosign_keys = {
        _normalize_sha256(key)
        for key in cosign.get("keys", [])
        if isinstance(key, str) and _normalize_sha256(key)
    }
    cosign_cert_substrings = [
        value
        for value in cosign.get("certificates", [])
        if isinstance(value, str) and value
    ]
    codesign_team_ids = {
        value
        for value in codesign.get("team_ids", [])
        if isinstance(value, str) and value
    }
    codesign_identifiers = {
        value
        for value in codesign.get("identifiers", [])
        if isinstance(value, str) and value
    }
    codesign_authorities = {
        value
        for value in codesign.get("authorities", [])
        if isinstance(value, str) and value
    }
    return TrustPolicy(
        cosign_keys=cosign_keys,
        cosign_cert_substrings=cosign_cert_substrings,
        codesign_team_ids=codesign_team_ids,
        codesign_identifiers=codesign_identifiers,
        codesign_authorities=codesign_authorities,
    )


def _trust_policy_allows(
    signer: str | None, signer_meta: dict[str, Any] | None, policy: TrustPolicy
) -> tuple[bool, str]:
    if signer is None:
        return False, "missing signer metadata"
    if signer == "cosign":
        if signer_meta is None:
            return False, "missing cosign metadata"
        key_meta = signer_meta.get("key", {}) if isinstance(signer_meta, dict) else {}
        key_hash = _normalize_sha256(
            key_meta.get("sha256") if isinstance(key_meta, dict) else None
        )
        if policy.cosign_keys and key_hash and key_hash in policy.cosign_keys:
            return True, "cosign key trusted"
        if policy.cosign_cert_substrings:
            cert = None
            signature = signer_meta.get("signature")
            if isinstance(signature, dict):
                cert = signature.get("certificate")
            if isinstance(cert, str):
                for token in policy.cosign_cert_substrings:
                    if token in cert:
                        return True, "cosign certificate trusted"
        return False, "cosign signer not in trusted policy"
    if signer == "codesign":
        if signer_meta is None:
            return False, "missing codesign metadata"
        team_id = signer_meta.get("team_id") if isinstance(signer_meta, dict) else None
        if policy.codesign_team_ids and isinstance(team_id, str):
            if team_id in policy.codesign_team_ids:
                return True, "codesign team trusted"
        identifier = (
            signer_meta.get("identifier") if isinstance(signer_meta, dict) else None
        )
        if policy.codesign_identifiers and isinstance(identifier, str):
            if identifier in policy.codesign_identifiers:
                return True, "codesign identifier trusted"
        authorities = (
            signer_meta.get("authorities") if isinstance(signer_meta, dict) else None
        )
        if policy.codesign_authorities and isinstance(authorities, list):
            for authority in authorities:
                if (
                    isinstance(authority, str)
                    and authority in policy.codesign_authorities
                ):
                    return True, "codesign authority trusted"
        return False, "codesign signer not in trusted policy"
    return False, f"unsupported signer {signer}"


def _resolve_signature_tool(
    signer: str | None,
    signer_meta: dict[str, Any] | None,
    artifact_path: Path,
    signature_bytes: bytes | None,
) -> str | None:
    if signer and signer != "auto":
        return signer
    if isinstance(signer_meta, dict):
        selected = signer_meta.get("selected")
        if isinstance(selected, str) and selected:
            return selected
        tool = signer_meta.get("tool")
        if isinstance(tool, dict):
            name = tool.get("name")
            if isinstance(name, str) and name:
                return name
    if _is_macho(artifact_path):
        return "codesign"
    if signature_bytes is not None:
        return "cosign"
    return None


def _verify_cosign_signature(
    artifact_path: Path, signature_bytes: bytes, signing_key: str
) -> None:
    with tempfile.TemporaryDirectory(prefix="molt_cosign_verify_") as tmpdir:
        sig_path = Path(tmpdir) / "artifact.sig"
        sig_path.write_bytes(signature_bytes)
        cmd = [
            "cosign",
            "verify-blob",
            "--key",
            signing_key,
            "--signature",
            str(sig_path),
            "--insecure-ignore-tlog",
            str(artifact_path),
        ]
        result = subprocess.run(cmd, capture_output=True, text=True, check=False)
        if result.returncode != 0:
            detail = (result.stderr or result.stdout).strip() or "unknown error"
            raise RuntimeError(f"cosign verify-blob failed: {detail}")


def _verify_codesign_signature(artifact_path: Path) -> None:
    result = subprocess.run(
        ["codesign", "--verify", "--verbose=4", str(artifact_path)],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip() or "unknown error"
        raise RuntimeError(f"codesign verify failed: {detail}")


def _module_name_from_path(path: Path, roots: list[Path], stdlib_root: Path) -> str:
    resolved = path.resolve()
    cpython_test_root: Path | None = None
    cpython_dir = os.environ.get("MOLT_REGRTEST_CPYTHON_DIR")
    if cpython_dir:
        cpython_test_root = (Path(cpython_dir) / "Lib" / "test").resolve()
    rel = None
    try:
        rel = resolved.relative_to(stdlib_root.resolve())
    except ValueError:
        rel = None
    if rel is not None:
        if rel.name == "__init__.py":
            rel = rel.parent
            if not rel.parts:
                return resolved.parent.name
        else:
            rel = rel.with_suffix("")
        if rel.parts:
            return ".".join(rel.parts)
        rel = None
    if rel is None:
        best_rel = None
        best_len = -1
        for root in roots:
            try:
                root_resolved = root.resolve()
                if cpython_test_root is not None and root_resolved == cpython_test_root:
                    continue
                candidate = resolved.relative_to(root_resolved)
            except ValueError:
                continue
            root_len = len(root_resolved.parts)
            if root_len > best_len:
                best_len = root_len
                best_rel = candidate
        rel = best_rel
    if rel is None:
        rel = resolved.with_suffix("")
    if rel.name == "__init__.py":
        rel = rel.parent
    else:
        rel = rel.with_suffix("")
    if not rel.parts:
        return resolved.parent.name
    return ".".join(rel.parts)


def _expand_module_chain(name: str) -> list[str]:
    parts = name.split(".")
    return [".".join(parts[:idx]) for idx in range(1, len(parts) + 1)]


def _resolve_root_override(var: str) -> Path | None:
    override = os.environ.get(var)
    if not override:
        return None
    path = Path(override).expanduser()
    if not path.is_absolute():
        path = (Path.cwd() / path).absolute()
    if path.exists():
        return path
    return None


def _has_molt_repo_markers(path: Path) -> bool:
    return (path / "runtime/molt-runtime/Cargo.toml").exists() and (
        path / "src/molt/cli.py"
    ).exists()


def _find_project_root(start: Path) -> Path:
    override = _resolve_root_override("MOLT_PROJECT_ROOT")
    if override:
        return override
    for parent in [start] + list(start.parents):
        if _has_project_markers(parent):
            return parent
    return start.parent


def _has_project_markers(path: Path) -> bool:
    return (
        (path / "pyproject.toml").exists()
        or (path / ".git").exists()
        or _has_molt_repo_markers(path)
    )


def _find_molt_root(*candidates: Path) -> Path:
    override = _resolve_root_override("MOLT_PROJECT_ROOT")
    if override:
        return override
    for candidate in candidates:
        for parent in [candidate] + list(candidate.parents):
            if _has_molt_repo_markers(parent):
                return parent
    module_path = Path(__file__).resolve()
    for parent in [module_path] + list(module_path.parents):
        if _has_molt_repo_markers(parent):
            return parent
    if candidates:
        return candidates[0]
    return Path.cwd()


def _require_molt_root(
    molt_root: Path,
    json_output: bool,
    command: str,
) -> int | None:
    runtime_toml = molt_root / "runtime/molt-runtime/Cargo.toml"
    backend_toml = molt_root / "runtime/molt-backend/Cargo.toml"
    if runtime_toml.exists() and backend_toml.exists():
        return None
    message = (
        f"Molt runtime sources not found under {molt_root}. "
        "Set MOLT_PROJECT_ROOT to the Molt repo root or run from within the Molt repo."
    )
    return _fail(message, json_output, command=command)


def _stdlib_root_path() -> Path:
    override = os.environ.get("MOLT_PROJECT_ROOT")
    if override:
        root = Path(override).expanduser()
        if not root.is_absolute():
            root = (Path.cwd() / root).absolute()
        candidate = root / "src/molt/stdlib"
        if candidate.exists():
            return candidate.resolve()
    candidate = Path(__file__).resolve().parent / "stdlib"
    if candidate.exists():
        return candidate.resolve()
    return Path("src/molt/stdlib").resolve()


def _resolve_module_path(module_name: str, roots: list[Path]) -> Path | None:
    parts = module_name.split(".")
    rel = Path(*parts)
    for root in roots:
        mod_path = root / f"{rel}.py"
        if mod_path.exists():
            return mod_path
        pkg_path = root / rel / "__init__.py"
        if pkg_path.exists():
            return pkg_path
    return None


def _resolve_entry_module(
    module_name: str, roots: list[Path]
) -> tuple[str, Path] | None:
    stripped = module_name.strip()
    if not stripped:
        return None
    main_name = f"{stripped}.__main__"
    main_path = _resolve_module_path(main_name, roots)
    if main_path is not None:
        return main_name, main_path
    mod_path = _resolve_module_path(stripped, roots)
    if mod_path is not None:
        return stripped, mod_path
    return None


def _output_base_for_entry(entry_module: str, source_path: Path) -> str:
    base = entry_module.rsplit(".", 1)[-1] or source_path.stem
    if base == "__main__" and "." in entry_module:
        base = entry_module.rsplit(".", 2)[-2]
    return base


def _resolve_module_roots(
    project_root: Path,
    cwd_root: Path,
    *,
    respect_pythonpath: bool,
) -> list[Path]:
    module_roots: list[Path] = []
    extra_roots = os.environ.get("MOLT_MODULE_ROOTS", "")
    if extra_roots:
        for entry in extra_roots.split(os.pathsep):
            if not entry:
                continue
            entry_path = Path(entry).expanduser()
            if entry_path.exists():
                module_roots.append(entry_path)
    for root in (project_root, cwd_root):
        if root.exists():
            module_roots.append(root)
        src_root = root / "src"
        if src_root.exists():
            module_roots.append(src_root)
        module_roots.extend(_vendor_roots(root))
    if respect_pythonpath:
        pythonpath = os.environ.get("PYTHONPATH", "")
        if pythonpath:
            for entry in pythonpath.split(os.pathsep):
                if not entry:
                    continue
                entry_path = Path(entry).expanduser()
                if entry_path.exists():
                    module_roots.append(entry_path)
    return list(dict.fromkeys(root.resolve() for root in module_roots))


def _build_args_respect_pythonpath(args: list[str]) -> bool:
    if any(arg == "--no-respect-pythonpath" for arg in args):
        return False
    return any(arg == "--respect-pythonpath" for arg in args)


def _has_namespace_dir(module_name: str, roots: list[Path]) -> bool:
    rel = Path(*module_name.split("."))
    for root in roots:
        candidate = root / rel
        if candidate.exists() and candidate.is_dir():
            return True
    return False


def _collect_namespace_parents(
    module_graph: dict[str, Path],
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    explicit_imports: set[str] | None = None,
) -> set[str]:
    namespace_parents: set[str] = set()

    def maybe_add(name: str) -> None:
        if name in module_graph:
            return
        candidate_roots = _roots_for_module(name, roots, stdlib_root, stdlib_allowlist)
        if _resolve_module_path(name, candidate_roots) is not None:
            return
        if _has_namespace_dir(name, candidate_roots):
            namespace_parents.add(name)

    for module_name in module_graph:
        parts = module_name.split(".")
        for idx in range(1, len(parts)):
            maybe_add(".".join(parts[:idx]))

    if explicit_imports:
        for name in explicit_imports:
            for candidate in _expand_module_chain(name):
                maybe_add(candidate)
    return namespace_parents


def _namespace_paths(name: str, roots: list[Path]) -> list[str]:
    rel = Path(*name.split("."))
    paths: list[str] = []
    for root in roots:
        candidate = root / rel
        if candidate.exists() and candidate.is_dir():
            paths.append(str(candidate))
    return list(dict.fromkeys(paths))


def _spec_parent(spec_name: str, is_package: bool) -> str:
    if is_package:
        return spec_name
    return spec_name.rpartition(".")[0]


def _is_modulespec_ctor(node: ast.AST) -> bool:
    if isinstance(node, ast.Name):
        return node.id == "ModuleSpec"
    if isinstance(node, ast.Attribute):
        return node.attr == "ModuleSpec"
    return False


def _parse_modulespec_override(
    value: ast.AST,
) -> tuple[str, bool | None] | None:
    if not isinstance(value, ast.Call):
        return None
    if not _is_modulespec_ctor(value.func):
        return None
    spec_name = None
    if value.args:
        first = value.args[0]
        if isinstance(first, ast.Constant) and isinstance(first.value, str):
            spec_name = first.value
    for kw in value.keywords:
        if (
            kw.arg == "name"
            and spec_name is None
            and isinstance(kw.value, ast.Constant)
            and isinstance(kw.value.value, str)
        ):
            spec_name = kw.value.value
    if spec_name is None:
        return None
    is_package = None
    if len(value.args) >= 4:
        arg = value.args[3]
        if isinstance(arg, ast.Constant) and isinstance(arg.value, bool):
            is_package = arg.value
    for kw in value.keywords:
        if (
            kw.arg == "is_package"
            and isinstance(kw.value, ast.Constant)
            and isinstance(kw.value.value, bool)
        ):
            is_package = kw.value.value
    return spec_name, is_package


def _infer_module_overrides(
    tree: ast.AST,
) -> tuple[bool, str | None, bool, str | None, bool | None]:
    package_override_set = False
    package_override: str | None = None
    spec_override_set = False
    spec_override: str | None = None
    spec_override_is_package: bool | None = None
    for stmt in getattr(tree, "body", []):
        if isinstance(stmt, ast.Assign):
            targets = stmt.targets
            value = stmt.value
        elif isinstance(stmt, ast.AnnAssign) and stmt.value is not None:
            targets = [stmt.target]
            value = stmt.value
        else:
            continue
        for target in targets:
            if not isinstance(target, ast.Name):
                continue
            if target.id == "__package__":
                package_override_set = True
                if isinstance(value, ast.Constant) and isinstance(value.value, str):
                    package_override = value.value
                elif isinstance(value, ast.Constant) and value.value is None:
                    package_override = None
                else:
                    package_override = None
            elif target.id == "__spec__":
                if isinstance(value, ast.Constant) and value.value is None:
                    spec_override_set = False
                    spec_override = None
                    spec_override_is_package = None
                else:
                    parsed = _parse_modulespec_override(value)
                    if parsed is None:
                        continue
                    spec_override_set = True
                    spec_override, spec_override_is_package = parsed
    return (
        package_override_set,
        package_override,
        spec_override_set,
        spec_override,
        spec_override_is_package,
    )


def _package_root_for_override(source_path: Path, package_name: str) -> Path | None:
    parts = [part for part in package_name.split(".") if part]
    if not parts:
        return None
    package_dir = source_path.parent
    if len(parts) > len(package_dir.parts):
        return None
    if tuple(package_dir.parts[-len(parts) :]) != tuple(parts):
        return None
    root = package_dir
    for _ in parts:
        root = root.parent
    return root


def _write_namespace_module(name: str, paths: list[str], output_dir: Path) -> Path:
    safe = name.replace(".", "_")
    stub_path = output_dir / f"namespace_{safe}.py"
    lines = [
        '"""Auto-generated namespace package stub for Molt."""',
        "",
        f"__package__ = {name!r}",
        f"__path__ = {paths!r}",
        "try:",
        "    spec = __spec__",
        "except NameError:",
        "    spec = None",
        "if spec is not None:",
        "    try:",
        "        spec.submodule_search_locations = list(__path__)",
        "    except Exception:",
        "        pass",
        "",
    ]
    stub_path.write_text("\n".join(lines))
    return stub_path


def _collect_package_parents(
    module_graph: dict[str, Path],
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
) -> None:
    changed = True
    while changed:
        changed = False
        for module_name in list(module_graph):
            parts = module_name.split(".")
            for idx in range(1, len(parts)):
                parent = ".".join(parts[:idx])
                if parent in module_graph:
                    continue
                resolved = _resolve_module_path(
                    parent,
                    _roots_for_module(parent, roots, stdlib_root, stdlib_allowlist),
                )
                if resolved is None:
                    continue
                if resolved.name != "__init__.py":
                    continue
                module_graph[parent] = resolved
                changed = True


def _resolve_relative_import(
    module_name: str,
    *,
    is_package: bool,
    level: int,
    module: str | None,
    package_override: str | None = None,
    package_override_set: bool = False,
    spec_override: str | None = None,
    spec_override_set: bool = False,
    spec_override_is_package: bool | None = None,
) -> str | None:
    if level <= 0:
        return module
    package = ""
    if package_override_set:
        package = package_override or ""
    else:
        if spec_override_set and spec_override:
            override_is_package = (
                spec_override_is_package
                if spec_override_is_package is not None
                else is_package
            )
            package = _spec_parent(spec_override, override_is_package)
        else:
            if is_package:
                package = module_name
            elif "." in module_name:
                package = module_name.rsplit(".", 1)[0]
            else:
                package = ""
    if not package:
        return None
    parts = package.split(".")
    if level > len(parts):
        return None
    base_parts = parts[: len(parts) - (level - 1)]
    base_name = ".".join(base_parts)
    if module:
        if base_name:
            return f"{base_name}.{module}"
        return module
    return base_name or None


def _collect_imports(
    tree: ast.AST,
    module_name: str | None = None,
    is_package: bool = False,
) -> list[str]:
    imports: list[str] = []
    needs_typing = False
    type_alias_cls = getattr(ast, "TypeAlias", None)
    (
        package_override_set,
        package_override,
        spec_override_set,
        spec_override,
        spec_override_is_package,
    ) = _infer_module_overrides(tree)

    def _importlib_target(func: ast.expr) -> str | None:
        if isinstance(func, ast.Attribute):
            parts: list[str] = []
            current: ast.expr | None = func
            while isinstance(current, ast.Attribute):
                parts.append(current.attr)
                current = current.value
            if isinstance(current, ast.Name):
                parts.append(current.id)
                return ".".join(reversed(parts))
        return None

    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                imports.append(alias.name)
            continue
        if isinstance(node, ast.ImportFrom):
            if node.level:
                if module_name:
                    resolved = _resolve_relative_import(
                        module_name,
                        is_package=is_package,
                        level=node.level,
                        module=node.module,
                        package_override=package_override,
                        package_override_set=package_override_set,
                        spec_override=spec_override,
                        spec_override_set=spec_override_set,
                        spec_override_is_package=spec_override_is_package,
                    )
                    if resolved:
                        imports.append(resolved)
                        for alias in node.names:
                            if alias.name != "*":
                                imports.append(f"{resolved}.{alias.name}")
                continue
            if node.module:
                imports.append(node.module)
                for alias in node.names:
                    if alias.name != "*":
                        imports.append(f"{node.module}.{alias.name}")
            continue
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            if getattr(node, "type_params", None):
                needs_typing = True
            continue
        if type_alias_cls is not None and isinstance(node, type_alias_cls):
            needs_typing = True
            continue
        if isinstance(node, ast.Call) and node.args:
            target = _importlib_target(node.func)
            if target in {"importlib.import_module", "importlib.util.find_spec"}:
                first = node.args[0]
                if isinstance(first, ast.Constant) and isinstance(first.value, str):
                    imports.append(first.value)
            continue
    if needs_typing:
        imports.append("typing")
    return imports


def _module_dependencies(
    tree: ast.AST, module_name: str, module_graph: dict[str, Path]
) -> set[str]:
    deps: set[str] = set()
    path = module_graph.get(module_name)
    is_package = path is not None and path.name == "__init__.py"
    for name in _collect_imports(tree, module_name, is_package):
        for candidate in _expand_module_chain(name):
            if candidate == "molt" and module_name.startswith("molt."):
                continue
            if candidate in module_graph and candidate != module_name:
                deps.add(candidate)
            if candidate.startswith("molt.stdlib."):
                stdlib_candidate = candidate[len("molt.stdlib.") :]
                if stdlib_candidate in module_graph and stdlib_candidate != module_name:
                    deps.add(stdlib_candidate)
    return deps


@dataclass(frozen=True)
class ModuleSyntaxErrorInfo:
    message: str
    filename: str
    lineno: int | None
    offset: int | None
    text: str | None


def _read_module_source(path: Path) -> str:
    with tokenize.open(path) as handle:
        return handle.read()


def _syntax_error_info_from_exception(
    exc: Exception, *, path: Path
) -> ModuleSyntaxErrorInfo:
    if isinstance(exc, SyntaxError):
        message = exc.msg or str(exc)
        lineno = exc.lineno
        offset = exc.offset
        text = exc.text
        filename = exc.filename or str(path)
    elif isinstance(exc, UnicodeDecodeError):
        message = str(exc)
        lineno = 1
        offset = exc.start + 1 if exc.start is not None else None
        text = None
        filename = str(path)
    else:
        message = str(exc)
        lineno = None
        offset = None
        text = None
        filename = str(path)
    if isinstance(text, str):
        text = text.rstrip("\n")
    return ModuleSyntaxErrorInfo(
        message=message,
        filename=filename,
        lineno=lineno,
        offset=offset,
        text=text,
    )


def _format_syntax_error_message(info: ModuleSyntaxErrorInfo) -> str:
    if info.lineno is None:
        return info.message
    filename = Path(info.filename).name if info.filename else "<unknown>"
    return f"{info.message} ({filename}, line {info.lineno})"


def _syntax_error_stub_ast(info: ModuleSyntaxErrorInfo) -> ast.Module:
    msg = _format_syntax_error_message(info)
    err_name = ast.Name(id="err", ctx=ast.Store())
    err_value = ast.Name(id="err", ctx=ast.Load())
    stmts: list[ast.stmt] = [
        ast.Assign(
            targets=[err_name],
            value=ast.Call(
                func=ast.Name(id="SyntaxError", ctx=ast.Load()),
                args=[ast.Constant(msg)],
                keywords=[],
            ),
        )
    ]
    attr_values = [
        ("lineno", info.lineno),
        ("offset", info.offset),
        ("filename", Path(info.filename).name if info.filename else None),
        ("text", info.text),
    ]
    for attr_name, value in attr_values:
        if value is None:
            continue
        stmts.append(
            ast.Assign(
                targets=[
                    ast.Attribute(
                        value=err_value,
                        attr=attr_name,
                        ctx=ast.Store(),
                    )
                ],
                value=ast.Constant(value),
            )
        )
    stmts.append(ast.Raise(exc=err_value, cause=None))
    module = ast.Module(body=stmts, type_ignores=[])
    return ast.fix_missing_locations(module)


def _default_spec_for_expr(expr: ast.expr) -> dict[str, Any]:
    if isinstance(expr, ast.Constant):
        return {"const": True, "value": expr.value}
    return {"const": False}


def _default_specs_from_args(args: ast.arguments) -> list[dict[str, Any]]:
    default_specs = [_default_spec_for_expr(expr) for expr in args.defaults]
    if not args.kwonlyargs or not args.kw_defaults:
        return default_specs
    kwonly_names = [arg.arg for arg in args.kwonlyargs]
    kwonly_pairs = list(zip(kwonly_names, args.kw_defaults))
    suffix: list[tuple[str, ast.expr]] = []
    for name, expr in reversed(kwonly_pairs):
        if expr is None:
            break
        suffix.append((name, expr))
    for name, expr in reversed(suffix):
        spec = _default_spec_for_expr(expr)
        spec["kwonly"] = True
        spec["name"] = name
        default_specs.append(spec)
    return default_specs


def _collect_func_defaults(tree: ast.AST) -> dict[str, dict[str, Any]]:
    defaults: dict[str, dict[str, Any]] = {}
    if not isinstance(tree, ast.Module):
        return defaults
    for stmt in tree.body:
        if not isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        if stmt.args.vararg or stmt.args.kwarg:
            continue
        params = [
            arg.arg
            for arg in (stmt.args.posonlyargs + stmt.args.args + stmt.args.kwonlyargs)
        ]
        default_specs = _default_specs_from_args(stmt.args)
        defaults[stmt.name] = {"params": len(params), "defaults": default_specs}
    return defaults


def _topo_sort_modules(
    module_graph: dict[str, Path], module_deps: dict[str, set[str]]
) -> list[str]:
    in_degree = {name: 0 for name in module_graph}
    dependents: dict[str, set[str]] = {name: set() for name in module_graph}
    for name, deps in module_deps.items():
        for dep in deps:
            dependents[dep].add(name)
            in_degree[name] += 1
    ready = sorted(name for name, degree in in_degree.items() if degree == 0)
    order: list[str] = []
    while ready:
        name = ready.pop(0)
        order.append(name)
        for child in sorted(dependents[name]):
            in_degree[child] -= 1
            if in_degree[child] == 0:
                ready.append(child)
    if len(order) != len(module_graph):
        remaining = sorted(name for name in module_graph if name not in order)
        order.extend(remaining)
    return order


def _stdlib_allowlist() -> set[str]:
    allowlist: set[str] = set()
    spec_path = Path("docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md")
    if not spec_path.exists():
        project_root = os.environ.get("MOLT_PROJECT_ROOT")
        if project_root:
            spec_path = (
                Path(project_root)
                / "docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md"
            )
        else:
            spec_path = (
                Path(__file__).resolve().parents[2]
                / "docs/spec/areas/compat/0015_STDLIB_COMPATIBILITY_MATRIX.md"
            )
    if not spec_path.exists():
        return allowlist
    for line in spec_path.read_text().splitlines():
        if not line.startswith("|"):
            continue
        if line.startswith("| ---"):
            continue
        parts = [part.strip() for part in line.strip().strip("|").split("|")]
        if not parts:
            continue
        module_name = parts[0]
        if not module_name or module_name == "Module":
            continue
        for entry in module_name.split("/"):
            entry = entry.strip()
            if entry:
                allowlist.add(entry)
    return allowlist


def _stdlib_module_uses_intrinsics(path: Path) -> bool:
    try:
        source = path.read_text(encoding="utf-8")
    except Exception:
        return False
    return "require_intrinsic" in source or "_molt_intrinsics" in source


def _enforce_intrinsic_stdlib(
    module_graph: dict[str, Path],
    stdlib_root: Path,
    json_output: bool,
) -> int | None:
    missing: list[str] = []
    stdlib_root = stdlib_root.resolve()
    for name, path in module_graph.items():
        if not path or not path.suffix == ".py":
            continue
        try:
            path.resolve().relative_to(stdlib_root)
        except ValueError:
            continue
        if not _stdlib_module_uses_intrinsics(path):
            missing.append(name)
    if not missing:
        return None
    missing.sort()
    message = (
        "Intrinsic-only stdlib enforcement failed. These modules are Python-only "
        "and must be lowered to Rust intrinsics (or become thin intrinsic wrappers):\n"
        + "\n".join(f"  - {name}" for name in missing)
    )
    return _fail(message, json_output, command="build")


def _is_stdlib_module(name: str, stdlib_allowlist: set[str]) -> bool:
    if name.startswith("molt."):
        return False
    if name in stdlib_allowlist:
        return True
    top = name.split(".", 1)[0]
    return top in stdlib_allowlist


def _roots_for_module(
    module_name: str,
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
) -> list[Path]:
    if _is_stdlib_module(module_name, stdlib_allowlist):
        if module_name == "test.tokenizedata" or module_name.startswith(
            "test.tokenizedata."
        ):
            return [stdlib_root] + [root for root in roots if root != stdlib_root]
        if module_name == "test" or module_name.startswith("test."):
            if os.environ.get("MOLT_REGRTEST_CPYTHON_DIR"):
                return roots
        return [stdlib_root]
    return roots


def _ensure_core_stdlib_modules(
    module_graph: dict[str, Path], stdlib_root: Path
) -> None:
    for name in ("builtins", "sys", "types"):
        path = _resolve_module_path(name, [stdlib_root])
        if path is not None:
            module_graph.setdefault(name, path)


def _discover_module_graph(
    entry_path: Path,
    roots: list[Path],
    module_roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    skip_modules: set[str] | None = None,
    stub_parents: set[str] | None = None,
) -> tuple[dict[str, Path], set[str]]:
    graph: dict[str, Path] = {}
    skip_modules = skip_modules or set()
    stub_parents = stub_parents or set()
    explicit_imports: set[str] = set()
    queue = [entry_path]
    while queue:
        path = queue.pop()
        module_name = _module_name_from_path(path, module_roots, stdlib_root)
        if module_name in graph:
            continue
        graph[module_name] = path
        try:
            source = _read_module_source(path)
        except (OSError, SyntaxError, UnicodeDecodeError):
            continue
        try:
            tree = ast.parse(source, filename=str(path))
        except SyntaxError:
            continue
        is_package = path.name == "__init__.py"
        for name in _collect_imports(tree, module_name, is_package):
            explicit_imports.add(name)
            for candidate in _expand_module_chain(name):
                if candidate in stub_parents:
                    continue
                if candidate.split(".", 1)[0] in skip_modules:
                    continue
                resolved = None
                if candidate.startswith("molt.stdlib."):
                    stdlib_candidate = candidate[len("molt.stdlib.") :]
                    resolved = _resolve_module_path(stdlib_candidate, [stdlib_root])
                if resolved is None:
                    resolved = _resolve_module_path(
                        candidate,
                        _roots_for_module(
                            candidate, roots, stdlib_root, stdlib_allowlist
                        ),
                    )
                if resolved is not None:
                    queue.append(resolved)
    return graph, explicit_imports


def _latest_mtime(paths: list[Path]) -> float:
    latest = 0.0
    for path in paths:
        if path.is_dir():
            for item in path.rglob("*"):
                if item.is_file():
                    latest = max(latest, item.stat().st_mtime)
        elif path.exists():
            latest = max(latest, path.stat().st_mtime)
    return latest


def _rustc_version() -> str | None:
    try:
        result = subprocess.run(
            ["rustc", "-Vv"], capture_output=True, text=True, check=False
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    return result.stdout.strip()


def _runtime_fingerprint_path(
    project_root: Path,
    artifact: Path,
    profile: BuildProfile,
    target_triple: str | None,
) -> Path:
    target = (target_triple or "native").replace(os.sep, "_").replace(":", "_")
    root = project_root / "target" / "runtime_fingerprints"
    return root / f"{artifact.name}.{profile}.{target}.fingerprint"


def _hash_runtime_file(path: Path, root: Path, hasher: Any) -> None:
    try:
        rel_path = path.relative_to(root)
        rel_bytes = str(rel_path).encode("utf-8")
    except ValueError:
        rel_bytes = str(path).encode("utf-8")
    hasher.update(rel_bytes)
    hasher.update(b"\0")
    with path.open("rb") as handle:
        while True:
            chunk = handle.read(65536)
            if not chunk:
                break
            hasher.update(chunk)
    hasher.update(b"\0")


def _runtime_fingerprint(
    project_root: Path,
    *,
    profile: BuildProfile,
    target_triple: str | None,
    rustflags: str,
) -> dict[str, str | None] | None:
    hasher = hashlib.sha256()
    meta = f"profile:{profile}\ntarget:{target_triple or 'native'}\n"
    meta += f"rustflags:{rustflags}\n"
    hasher.update(meta.encode("utf-8"))
    rustc_info = _rustc_version()
    try:
        for path in sorted(_runtime_source_paths(project_root), key=lambda p: str(p)):
            if path.is_dir():
                for item in sorted(path.rglob("*"), key=lambda p: str(p)):
                    if item.is_file():
                        _hash_runtime_file(item, project_root, hasher)
            elif path.exists():
                _hash_runtime_file(path, project_root, hasher)
    except OSError:
        return None
    return {"hash": hasher.hexdigest(), "rustc": rustc_info}


def _read_runtime_fingerprint(path: Path) -> dict[str, str | None] | None:
    try:
        text = path.read_text().strip()
    except OSError:
        return None
    if not text:
        return None
    try:
        data = json.loads(text)
    except json.JSONDecodeError:
        return {"hash": text, "rustc": None}
    if not isinstance(data, dict):
        return None
    hash_value = data.get("hash")
    if not isinstance(hash_value, str) or not hash_value:
        return None
    rustc_value = data.get("rustc")
    if rustc_value is not None and not isinstance(rustc_value, str):
        rustc_value = None
    return {"hash": hash_value, "rustc": rustc_value}


def _write_runtime_fingerprint(path: Path, fingerprint: dict[str, str | None]) -> None:
    payload = {
        "version": 1,
        "hash": fingerprint.get("hash"),
        "rustc": fingerprint.get("rustc"),
    }
    path.write_text(json.dumps(payload, indent=2) + "\n")


def _check_lockfiles(
    project_root: Path,
    json_output: bool,
    warnings: list[str],
    deterministic: bool,
    deterministic_warn: bool,
    command: str,
) -> int | None:
    pyproject = project_root / "pyproject.toml"
    if not pyproject.exists():
        return None
    lock_path = project_root / "uv.lock"
    cargo_lock = project_root / "Cargo.lock"
    missing = []
    if not lock_path.exists():
        missing.append("uv.lock")
    if not cargo_lock.exists():
        missing.append("Cargo.lock")
    if missing and deterministic:
        missing_text = ", ".join(missing)
        message = (
            f"Missing lockfiles ({missing_text}); run `uv lock` and ensure Cargo.lock."
        )
        if deterministic_warn:
            warnings.append(message)
        else:
            return _fail(message, json_output, command=command)
    if missing:
        warnings.append(f"Missing lockfiles: {', '.join(missing)}")
        return None
    if deterministic:
        skip_uv_lock = os.environ.get("UV_NO_SYNC") == "1"
        if skip_uv_lock:
            warnings.append("Skipping uv.lock check because UV_NO_SYNC=1.")
        else:
            uv_error = _verify_uv_lock(project_root)
            if uv_error is not None:
                if deterministic_warn:
                    warnings.append(uv_error)
                else:
                    return _fail(uv_error, json_output, command=command)
        skip_cargo_lock = os.environ.get("MOLT_SKIP_CARGO_LOCK") == "1"
        if skip_cargo_lock:
            warnings.append("Skipping Cargo.lock check because MOLT_SKIP_CARGO_LOCK=1.")
        else:
            cargo_error = _verify_cargo_lock(project_root)
            if cargo_error is not None:
                if deterministic_warn:
                    warnings.append(cargo_error)
                else:
                    return _fail(cargo_error, json_output, command=command)
    return None


def _verify_uv_lock(project_root: Path) -> str | None:
    if shutil.which("uv") is None:
        return "Deterministic builds require uv; install uv to validate uv.lock."
    try:
        result = subprocess.run(
            ["uv", "lock", "--check"],
            cwd=project_root,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        return f"Failed to run `uv lock --check`: {exc}"
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip() or "uv lock check failed"
        return f"uv.lock is out of date or invalid: {detail}"
    return None


def _verify_cargo_lock(project_root: Path) -> str | None:
    if shutil.which("cargo") is None:
        return "Deterministic builds require cargo; install Rust toolchain to validate Cargo.lock."
    try:
        result = subprocess.run(
            ["cargo", "metadata", "--locked", "--format-version", "1"],
            cwd=project_root,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        return f"Failed to run `cargo metadata --locked`: {exc}"
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip() or "cargo metadata failed"
        return f"Cargo.lock is out of date or invalid: {detail}"
    return None


def _load_molt_config(project_root: Path) -> dict[str, Any]:
    config: dict[str, Any] = {}
    molt_toml = project_root / "molt.toml"
    if molt_toml.exists():
        try:
            config.update(tomllib.loads(molt_toml.read_text()))
        except (OSError, tomllib.TOMLDecodeError):
            pass
    pyproject = project_root / "pyproject.toml"
    if pyproject.exists():
        try:
            data = tomllib.loads(pyproject.read_text())
        except (OSError, tomllib.TOMLDecodeError):
            data = {}
        tool_cfg = data.get("tool", {}).get("molt", {})
        if tool_cfg:
            config.setdefault("tool", {})
            config["tool"].setdefault("molt", {})
            config["tool"]["molt"].update(tool_cfg)
    return config


def _config_value(config: dict[str, Any], path: list[str]) -> Any | None:
    current: Any = config
    for key in path:
        if not isinstance(current, dict) or key not in current:
            return None
        current = current[key]
    return current


def _resolve_command_config(config: dict[str, Any], command: str) -> dict[str, Any]:
    cmd_cfg: dict[str, Any] = {}
    direct = _config_value(config, [command])
    if isinstance(direct, dict):
        cmd_cfg.update(direct)
    tool_cfg = _config_value(config, ["tool", "molt", command])
    if isinstance(tool_cfg, dict):
        cmd_cfg.update(tool_cfg)
    return cmd_cfg


def _resolve_build_config(config: dict[str, Any]) -> dict[str, Any]:
    return _resolve_command_config(config, "build")


def _resolve_capabilities_config(config: dict[str, Any]) -> CapabilityInput | None:
    for path in (["capabilities"], ["tool", "molt", "capabilities"]):
        caps = _config_value(config, path)
        if isinstance(caps, (list, str, dict)):
            return caps
    return None


def _coerce_bool(value: Any, default: bool) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        return value.strip().lower() in {"1", "true", "yes", "on"}
    return default


def _merge_optional_list(
    left: list[str] | None, right: list[str] | None
) -> list[str] | None:
    if left is None:
        return right
    if right is None:
        return left
    return _dedupe_preserve_order([*left, *right])


def _coerce_token_list(
    value: Any, field: str, errors: list[str]
) -> tuple[list[str], bool]:
    if value is None:
        return [], False
    if isinstance(value, list):
        tokens: list[str] = []
        for entry in value:
            if isinstance(entry, str):
                stripped = entry.strip()
                if stripped:
                    tokens.append(stripped)
            else:
                errors.append(f"{field} entries must be strings")
        return tokens, True
    if isinstance(value, str):
        return _split_tokens(value), True
    errors.append(f"{field} must be a list or string")
    return [], True


def _coerce_effects_list(
    value: Any, field: str, errors: list[str]
) -> tuple[list[str], bool]:
    if value is None:
        return [], False
    if isinstance(value, list):
        effects: list[str] = []
        for entry in value:
            if isinstance(entry, str):
                stripped = entry.strip()
                if stripped:
                    effects.append(stripped)
            else:
                errors.append(f"{field} entries must be strings")
        return effects, True
    if isinstance(value, str):
        return _split_tokens(value), True
    errors.append(f"{field} must be a list or string")
    return [], True


def _fs_entry_enabled(value: Any, field: str, errors: list[str]) -> tuple[bool, bool]:
    if value is None:
        return False, False
    if isinstance(value, bool):
        return True, value
    if isinstance(value, str):
        return True, bool(value.strip())
    if isinstance(value, list):
        for entry in value:
            if not isinstance(entry, str):
                errors.append(f"{field} entries must be strings")
        return True, bool(value)
    errors.append(f"{field} must be a list, string, or bool")
    return True, False


def _parse_fs_block(
    value: Any, field: str, errors: list[str]
) -> tuple[list[str], bool]:
    if value is None:
        return [], False
    if not isinstance(value, dict):
        errors.append(f"{field} must be a table")
        return [], True
    allow: list[str] = []
    for key, capability in (("read", "fs.read"), ("write", "fs.write")):
        present, enabled = _fs_entry_enabled(value.get(key), f"{field}.{key}", errors)
        if present and enabled:
            allow.append(capability)
    return allow, True


def _parse_package_grant(value: Any, field: str, errors: list[str]) -> CapabilityGrant:
    if value is None:
        return CapabilityGrant(allow=None, deny=[], effects=None)
    if isinstance(value, (list, str)):
        allow, _present = _coerce_token_list(value, f"{field}.allow", errors)
        return CapabilityGrant(
            allow=_dedupe_preserve_order(allow), deny=[], effects=None
        )
    if not isinstance(value, dict):
        errors.append(f"{field} must be a list, string, or table")
        return CapabilityGrant(allow=None, deny=[], effects=None)
    allow_tokens, allow_present = _coerce_token_list(
        value.get("allow"), f"{field}.allow", errors
    )
    caps_value = value.get("capabilities")
    caps_tokens: list[str] = []
    caps_present = False
    if isinstance(caps_value, dict):
        nested = _parse_package_grant(caps_value, f"{field}.capabilities", errors)
        allow_tokens = _dedupe_preserve_order(allow_tokens + (nested.allow or []))
        allow_present = True
        if nested.deny:
            errors.append(f"{field}.capabilities must not include deny entries")
        if nested.effects is not None:
            errors.append(f"{field}.capabilities must not include effects entries")
    else:
        caps_tokens, caps_present = _coerce_token_list(
            caps_value, f"{field}.capabilities", errors
        )
    deny_tokens, _deny_present = _coerce_token_list(
        value.get("deny"), f"{field}.deny", errors
    )
    effects_tokens, effects_present = _coerce_effects_list(
        value.get("effects"), f"{field}.effects", errors
    )
    fs_tokens, fs_present = _parse_fs_block(value.get("fs"), f"{field}.fs", errors)
    combined_allow: list[str] = []
    if allow_present:
        combined_allow.extend(allow_tokens)
    if caps_present:
        combined_allow.extend(caps_tokens)
    if fs_present:
        combined_allow.extend(fs_tokens)
    allow = (
        _dedupe_preserve_order(combined_allow)
        if allow_present or caps_present or fs_present
        else None
    )
    effects = _dedupe_preserve_order(effects_tokens) if effects_present else None
    return CapabilityGrant(
        allow=allow,
        deny=_dedupe_preserve_order(deny_tokens),
        effects=effects,
    )


def _parse_package_grants(
    value: Any, field: str, errors: list[str]
) -> dict[str, CapabilityGrant]:
    packages: dict[str, CapabilityGrant] = {}
    if value is None:
        return packages
    if isinstance(value, dict):
        for name, entry in value.items():
            if not isinstance(name, str) or not name:
                errors.append(f"{field} entries must be keyed by package name")
                continue
            grant = _parse_package_grant(entry, f"{field}.{name}", errors)
            if name in packages:
                packages[name] = packages[name].merged(grant)
            else:
                packages[name] = grant
        return packages
    if isinstance(value, list):
        for idx, entry in enumerate(value):
            if not isinstance(entry, dict):
                errors.append(f"{field}[{idx}] must be a table")
                continue
            name = entry.get("name") or entry.get("package")
            if not isinstance(name, str) or not name:
                errors.append(f"{field}[{idx}].name must be a non-empty string")
                continue
            grant = _parse_package_grant(entry, f"{field}.{name}", errors)
            if name in packages:
                packages[name] = packages[name].merged(grant)
            else:
                packages[name] = grant
        return packages
    errors.append(f"{field} must be a table or list")
    return packages


def _parse_capability_manifest_dict(
    data: Any, field: str, errors: list[str]
) -> CapabilityManifest | None:
    if not isinstance(data, dict):
        errors.append(f"{field} must be a table")
        return None
    allow: list[str] | None = None
    deny: list[str] = []
    effects: list[str] | None = None
    packages: dict[str, CapabilityGrant] = {}

    def apply_section(section: Any, ctx: str) -> None:
        nonlocal allow, deny, effects, packages
        if not isinstance(section, dict):
            errors.append(f"{ctx} must be a table")
            return
        caps_value = section.get("capabilities")
        if isinstance(caps_value, dict):
            apply_section(caps_value, f"{ctx}.capabilities")
            caps_value = None
        allow_tokens, allow_present = _coerce_token_list(
            section.get("allow"), f"{ctx}.allow", errors
        )
        caps_tokens: list[str] = []
        caps_present = False
        if caps_value is not None:
            caps_tokens, caps_present = _coerce_token_list(
                caps_value, f"{ctx}.capabilities", errors
            )
        fs_tokens, fs_present = _parse_fs_block(section.get("fs"), f"{ctx}.fs", errors)
        combined_allow: list[str] = []
        if allow_present:
            combined_allow.extend(allow_tokens)
        if caps_present:
            combined_allow.extend(caps_tokens)
        if fs_present:
            combined_allow.extend(fs_tokens)
        if allow_present or caps_present or fs_present:
            if allow is None:
                allow = _dedupe_preserve_order(combined_allow)
            else:
                allow = _dedupe_preserve_order([*allow, *combined_allow])
        deny_tokens, deny_present = _coerce_token_list(
            section.get("deny"), f"{ctx}.deny", errors
        )
        if deny_present:
            deny = _dedupe_preserve_order([*deny, *deny_tokens])
        effects_tokens, effects_present = _coerce_effects_list(
            section.get("effects"), f"{ctx}.effects", errors
        )
        if effects_present:
            if effects is None:
                effects = _dedupe_preserve_order(effects_tokens)
            else:
                effects = _dedupe_preserve_order([*effects, *effects_tokens])
        pkg_entries = _parse_package_grants(
            section.get("packages"), f"{ctx}.packages", errors
        )
        if pkg_entries:
            for name, grant in pkg_entries.items():
                if name in packages:
                    packages[name] = packages[name].merged(grant)
                else:
                    packages[name] = grant

    apply_section(data, field)
    molt_section = data.get("molt")
    if isinstance(molt_section, dict):
        apply_section(molt_section, f"{field}.molt")
    tool_section = data.get("tool")
    if isinstance(tool_section, dict):
        tool_molt = tool_section.get("molt")
        if isinstance(tool_molt, dict):
            apply_section(tool_molt, f"{field}.tool.molt")

    return CapabilityManifest(
        allow=allow,
        deny=deny,
        effects=effects,
        packages=packages,
    )


def _validate_capability_tokens(
    tokens: Iterable[str], field: str, errors: list[str]
) -> None:
    for cap in tokens:
        if not CAPABILITY_TOKEN_RE.match(cap):
            errors.append(f"invalid capability token in {field}: {cap}")


def _validate_effect_tokens(
    tokens: Iterable[str], field: str, errors: list[str]
) -> None:
    for effect in tokens:
        if not CAPABILITY_TOKEN_RE.match(effect):
            errors.append(f"invalid effect token in {field}: {effect}")


def _resolve_capability_manifest(
    manifest: CapabilityManifest,
) -> tuple[list[str], list[str], list[str]]:
    errors: list[str] = []
    allow_tokens = manifest.allow or []
    allow_expanded, allow_profiles = _expand_capabilities(allow_tokens)
    deny_expanded, deny_profiles = _expand_capabilities(manifest.deny)
    profiles = _dedupe_preserve_order([*allow_profiles, *deny_profiles])
    _validate_capability_tokens(allow_expanded, "allow", errors)
    _validate_capability_tokens(deny_expanded, "deny", errors)
    deny_set = set(deny_expanded)
    resolved = _dedupe_preserve_order(
        cap for cap in allow_expanded if cap not in deny_set
    )
    manifest_effects_set: set[str] | None = None
    if manifest.effects is not None:
        _validate_effect_tokens(manifest.effects, "effects", errors)
        manifest_effects_set = set(manifest.effects)
    global_allow = set(resolved)
    for name, grant in manifest.packages.items():
        pkg_allow_tokens = grant.allow or []
        pkg_allow_expanded, pkg_allow_profiles = _expand_capabilities(pkg_allow_tokens)
        pkg_deny_expanded, pkg_deny_profiles = _expand_capabilities(grant.deny)
        profiles = _dedupe_preserve_order(
            [*profiles, *pkg_allow_profiles, *pkg_deny_profiles]
        )
        _validate_capability_tokens(
            pkg_allow_expanded, f"packages.{name}.allow", errors
        )
        _validate_capability_tokens(pkg_deny_expanded, f"packages.{name}.deny", errors)
        if grant.allow is not None:
            extras = [
                cap
                for cap in _dedupe_preserve_order(pkg_allow_expanded)
                if cap not in global_allow
            ]
            if extras:
                errors.append(
                    "packages."
                    + name
                    + ".allow includes capabilities not in global allowlist: "
                    + ", ".join(extras)
                )
        if grant.effects is not None:
            _validate_effect_tokens(grant.effects, f"packages.{name}.effects", errors)
            if manifest_effects_set is not None:
                effect_extras = [
                    effect
                    for effect in _dedupe_preserve_order(grant.effects)
                    if effect not in manifest_effects_set
                ]
                if effect_extras:
                    errors.append(
                        "packages."
                        + name
                        + ".effects includes effects not in global effects allowlist: "
                        + ", ".join(effect_extras)
                    )
    return resolved, profiles, errors


def _parse_capabilities_spec(
    capabilities: CapabilityInput | None,
) -> CapabilitySpec:
    if capabilities is None:
        return CapabilitySpec(
            capabilities=None,
            profiles=[],
            source=None,
            errors=[],
            manifest=None,
        )
    errors: list[str] = []
    profiles: list[str] = []
    source: str | None = None
    manifest: CapabilityManifest | None = None
    if isinstance(capabilities, dict):
        source = "config"
        manifest = _parse_capability_manifest_dict(capabilities, "capabilities", errors)
    elif isinstance(capabilities, list):
        source = "config"
        tokens, _present = _coerce_token_list(capabilities, "capabilities", errors)
        manifest = CapabilityManifest(
            allow=_dedupe_preserve_order(tokens),
            deny=[],
            effects=None,
            packages={},
        )
    else:
        if isinstance(capabilities, str) and not capabilities.strip():
            source = "inline"
            manifest = CapabilityManifest(
                allow=[],
                deny=[],
                effects=None,
                packages={},
            )
            resolved, profiles, resolve_errors = _resolve_capability_manifest(manifest)
            if resolve_errors:
                return CapabilitySpec(
                    capabilities=None,
                    profiles=profiles,
                    source=None,
                    errors=resolve_errors,
                    manifest=manifest,
                )
            return CapabilitySpec(
                capabilities=resolved,
                profiles=profiles,
                source=source,
                errors=[],
                manifest=manifest,
            )
        path = Path(capabilities)
        if path.exists():
            source = str(path)
            try:
                if path.suffix == ".json":
                    data = json.loads(path.read_text())
                else:
                    data = tomllib.loads(path.read_text())
            except (OSError, json.JSONDecodeError, tomllib.TOMLDecodeError):
                return CapabilitySpec(
                    capabilities=None,
                    profiles=[],
                    source=source,
                    errors=["failed to load capabilities file"],
                    manifest=None,
                )
            manifest = _parse_capability_manifest_dict(data, "capabilities", errors)
        else:
            source = "inline"
            tokens = _split_tokens(capabilities)
            manifest = CapabilityManifest(
                allow=_dedupe_preserve_order(tokens),
                deny=[],
                effects=None,
                packages={},
            )
    if manifest is None:
        return CapabilitySpec(
            capabilities=None,
            profiles=profiles,
            source=source,
            errors=errors,
            manifest=None,
        )
    resolved, profiles, resolve_errors = _resolve_capability_manifest(manifest)
    errors.extend(resolve_errors)
    if errors:
        return CapabilitySpec(
            capabilities=None,
            profiles=profiles,
            source=source,
            errors=errors,
            manifest=manifest,
        )
    return CapabilitySpec(
        capabilities=resolved,
        profiles=profiles,
        source=source,
        errors=[],
        manifest=manifest,
    )


def _parse_capabilities(
    capabilities: CapabilityInput | None,
) -> tuple[list[str] | None, list[str], str | None, list[str]]:
    spec = _parse_capabilities_spec(capabilities)
    return spec.capabilities, spec.profiles, spec.source, spec.errors


def _format_capabilities_input(value: CapabilityInput | None) -> str:
    if value is None:
        return "none"
    if isinstance(value, list):
        return ", ".join(value) if value else "(empty)"
    if isinstance(value, str):
        return value if value else "(empty)"
    return json.dumps(value, sort_keys=True)


def _allowed_capabilities_for_package(
    global_allow: list[str],
    manifest: CapabilityManifest | None,
    package_name: str | None,
) -> set[str]:
    allowed = set(global_allow)
    if manifest is None or not package_name:
        return allowed
    grant = manifest.packages.get(package_name)
    if grant is None:
        return allowed
    if grant.allow is not None:
        grant_allow, _profiles = _expand_capabilities(grant.allow)
        allowed &= set(grant_allow)
    if grant.deny:
        grant_deny, _profiles = _expand_capabilities(grant.deny)
        allowed -= set(grant_deny)
    return allowed


def _allowed_effects_for_package(
    manifest: CapabilityManifest | None,
    package_name: str | None,
) -> set[str] | None:
    if manifest is None:
        return None
    allowed: set[str] | None = None
    if manifest.effects is not None:
        allowed = set(manifest.effects)
    grant = manifest.packages.get(package_name) if package_name else None
    if grant is None or grant.effects is None:
        return allowed
    grant_effects = set(grant.effects)
    if allowed is None:
        return grant_effects
    return allowed & grant_effects


def _materialize_capabilities_arg(
    capabilities: CapabilityInput,
) -> tuple[str, Path | None]:
    if isinstance(capabilities, list):
        return ",".join(capabilities), None
    if isinstance(capabilities, str):
        return capabilities, None
    handle = tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        suffix=".json",
        prefix="molt_capabilities_",
        delete=False,
    )
    try:
        json.dump(capabilities, handle, sort_keys=True, indent=2)
        handle.write("\n")
        path = Path(handle.name)
    finally:
        handle.close()
    return str(path), path


def _expand_capabilities(items: list[str]) -> tuple[list[str], list[str]]:
    expanded: list[str] = []
    profiles: list[str] = []
    for item in items:
        key = item.strip()
        if not key:
            continue
        profile = CAPABILITY_PROFILES.get(key)
        if profile is not None:
            profiles.append(key)
            expanded.extend(profile)
        else:
            expanded.append(key)
    # Preserve order while de-duplicating.
    seen: set[str] = set()
    deduped: list[str] = []
    for cap in expanded:
        if cap in seen:
            continue
        seen.add(cap)
        deduped.append(cap)
    return deduped, profiles


def _runtime_source_paths(project_root: Path) -> list[Path]:
    return [
        project_root / "runtime/molt-runtime/src",
        project_root / "runtime/molt-runtime/Cargo.toml",
        project_root / "runtime/molt-runtime/build.rs",
        project_root / "runtime/molt-obj-model/src",
        project_root / "runtime/molt-obj-model/Cargo.toml",
        project_root / "runtime/molt-obj-model/build.rs",
        project_root / "Cargo.toml",
        project_root / "Cargo.lock",
    ]


def _backend_source_paths(project_root: Path) -> list[Path]:
    return [
        project_root / "runtime/molt-backend/src",
        project_root / "runtime/molt-backend/Cargo.toml",
        project_root / "runtime/molt-backend/build.rs",
        project_root / "Cargo.toml",
        project_root / "Cargo.lock",
    ]


def _backend_bin_path(project_root: Path, profile: BuildProfile) -> Path:
    profile_dir = _cargo_profile_dir(profile)
    target_root = _cargo_target_root(project_root)
    exe_suffix = ".exe" if os.name == "nt" else ""
    return target_root / profile_dir / f"molt-backend{exe_suffix}"


def _resolve_backend_profile() -> tuple[BuildProfile, str | None]:
    raw = os.environ.get("MOLT_BACKEND_PROFILE")
    if not raw:
        return "release", None
    value = raw.strip().lower()
    if value not in {"dev", "release"}:
        return "release", f"Invalid MOLT_BACKEND_PROFILE value: {raw}"
    return value, None


def _backend_fingerprint_path(
    project_root: Path,
    artifact: Path,
    profile: BuildProfile,
) -> Path:
    root = project_root / "target" / "backend_fingerprints"
    return root / f"{artifact.name}.{profile}.fingerprint"


def _backend_fingerprint(
    project_root: Path,
    *,
    profile: BuildProfile,
    rustflags: str,
) -> dict[str, str | None] | None:
    hasher = hashlib.sha256()
    meta = f"profile:{profile}\n"
    meta += f"rustflags:{rustflags}\n"
    hasher.update(meta.encode("utf-8"))
    rustc_info = _rustc_version()
    try:
        for path in sorted(_backend_source_paths(project_root), key=lambda p: str(p)):
            if path.is_dir():
                for item in sorted(path.rglob("*"), key=lambda p: str(p)):
                    if item.is_file():
                        _hash_runtime_file(item, project_root, hasher)
            elif path.exists():
                _hash_runtime_file(path, project_root, hasher)
    except OSError:
        return None
    return {"hash": hasher.hexdigest(), "rustc": rustc_info}


def _ensure_backend_binary(
    backend_bin: Path,
    *,
    cargo_timeout: float | None,
    json_output: bool,
    profile: BuildProfile,
    project_root: Path,
) -> bool:
    rustflags = os.environ.get("RUSTFLAGS", "")
    fingerprint = _backend_fingerprint(
        project_root,
        profile=profile,
        rustflags=rustflags,
    )
    fingerprint_path = _backend_fingerprint_path(project_root, backend_bin, profile)
    stored_fingerprint = (
        _read_runtime_fingerprint(fingerprint_path)
        if fingerprint_path.exists()
        else None
    )
    needs_build = not backend_bin.exists()
    if not needs_build:
        if fingerprint is None or stored_fingerprint is None:
            needs_build = True
        elif stored_fingerprint.get("hash") != fingerprint.get("hash"):
            needs_build = True
        elif fingerprint.get("rustc"):
            stored_rustc = stored_fingerprint.get("rustc")
            needs_build = stored_rustc is None or stored_rustc != fingerprint["rustc"]
    if not needs_build:
        return True
    if not json_output:
        print("Backend sources changed; rebuilding backend...")
    cmd = ["cargo", "build", "--package", "molt-backend"]
    if profile == "release":
        cmd.append("--release")
    try:
        build = subprocess.run(
            cmd,
            cwd=project_root,
            capture_output=True,
            text=True,
            timeout=cargo_timeout,
        )
    except subprocess.TimeoutExpired:
        if not json_output:
            timeout_note = (
                f"Backend build timed out after {cargo_timeout:.1f}s."
                if cargo_timeout is not None
                else "Backend build timed out."
            )
            print(timeout_note, file=sys.stderr)
        return False
    if build.returncode != 0:
        if not json_output:
            err = build.stderr.strip() or build.stdout.strip()
            if err:
                print(err, file=sys.stderr)
        return False
    if fingerprint is not None:
        try:
            fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
            _write_runtime_fingerprint(fingerprint_path, fingerprint)
        except OSError:
            if not json_output:
                print(
                    "Warning: failed to write backend fingerprint metadata.",
                    file=sys.stderr,
                )
    return True


def _ensure_runtime_lib(
    runtime_lib: Path,
    target_triple: str | None,
    json_output: bool,
    profile: BuildProfile,
    project_root: Path,
    cargo_timeout: float | None,
) -> bool:
    rustflags = os.environ.get("RUSTFLAGS", "")
    fingerprint = _runtime_fingerprint(
        project_root,
        profile=profile,
        target_triple=target_triple,
        rustflags=rustflags,
    )
    fingerprint_path = _runtime_fingerprint_path(
        project_root, runtime_lib, profile, target_triple
    )
    stored_fingerprint = (
        _read_runtime_fingerprint(fingerprint_path)
        if fingerprint_path.exists()
        else None
    )
    needs_build = not runtime_lib.exists()
    if not needs_build:
        if fingerprint is None or stored_fingerprint is None:
            needs_build = True
        elif stored_fingerprint.get("hash") != fingerprint.get("hash"):
            needs_build = True
        elif fingerprint.get("rustc"):
            stored_rustc = stored_fingerprint.get("rustc")
            needs_build = stored_rustc is None or stored_rustc != fingerprint["rustc"]
    if not needs_build:
        return True
    if not json_output:
        print("Runtime sources changed; rebuilding runtime...")
    cmd = ["cargo", "build", "-p", "molt-runtime"]
    if profile == "release":
        cmd.append("--release")
    if target_triple:
        cmd.extend(["--target", target_triple])
    try:
        build = subprocess.run(
            cmd,
            cwd=project_root,
            capture_output=json_output,
            text=json_output,
            timeout=cargo_timeout,
        )
    except subprocess.TimeoutExpired:
        if not json_output:
            timeout_note = (
                f"Runtime build timed out after {cargo_timeout:.1f}s."
                if cargo_timeout is not None
                else "Runtime build timed out."
            )
            print(timeout_note, file=sys.stderr)
        return False
    if build.returncode != 0:
        if json_output:
            err = build.stderr.strip() or build.stdout.strip()
            if err:
                print(err, file=sys.stderr)
        return False
    if fingerprint is not None:
        try:
            fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
            _write_runtime_fingerprint(fingerprint_path, fingerprint)
        except OSError:
            if not json_output:
                print(
                    "Warning: failed to write runtime fingerprint metadata.",
                    file=sys.stderr,
                )
    return True


def _append_rustflags(env: dict[str, str], flags: str) -> None:
    existing = env.get("RUSTFLAGS", "")
    joined = f"{existing} {flags}".strip()
    env["RUSTFLAGS"] = joined


def _ensure_runtime_wasm(
    runtime_wasm: Path,
    *,
    reloc: bool,
    json_output: bool,
    profile: BuildProfile,
    cargo_timeout: float | None,
    project_root: Path | None = None,
) -> bool:
    root = project_root or Path(__file__).resolve().parents[2]
    env = os.environ.copy()
    if reloc:
        flags = (
            "-C link-arg=--relocatable -C link-arg=--no-gc-sections"
            " -C relocation-model=pic"
        )
    else:
        flags = (
            "-C link-arg=--import-memory -C link-arg=--import-table"
            " -C link-arg=--growable-table"
        )
    rustflags = f"{env.get('RUSTFLAGS', '')} {flags}".strip()
    fingerprint = _runtime_fingerprint(
        root,
        profile=profile,
        target_triple="wasm32-wasip1",
        rustflags=rustflags,
    )
    fingerprint_path = _runtime_fingerprint_path(
        root, runtime_wasm, profile, "wasm32-wasip1"
    )
    stored_fingerprint = (
        _read_runtime_fingerprint(fingerprint_path)
        if fingerprint_path.exists()
        else None
    )
    needs_build = not runtime_wasm.exists()
    if not needs_build:
        if fingerprint is None or stored_fingerprint is None:
            needs_build = True
        elif stored_fingerprint.get("hash") != fingerprint.get("hash"):
            needs_build = True
        elif fingerprint.get("rustc"):
            stored_rustc = stored_fingerprint.get("rustc")
            needs_build = stored_rustc is None or stored_rustc != fingerprint["rustc"]
    if not needs_build:
        return True
    if not json_output:
        print("Runtime sources changed; rebuilding runtime...")
    _append_rustflags(env, flags)
    cmd = [
        "cargo",
        "build",
        "--package",
        "molt-runtime",
        "--target",
        "wasm32-wasip1",
    ]
    if profile == "release":
        cmd.append("--release")
    try:
        build = subprocess.run(
            cmd,
            cwd=root,
            env=env,
            capture_output=True,
            text=True,
            timeout=cargo_timeout,
        )
    except subprocess.TimeoutExpired:
        if not json_output:
            timeout_note = (
                f"Runtime wasm build timed out after {cargo_timeout:.1f}s."
                if cargo_timeout is not None
                else "Runtime wasm build timed out."
            )
            print(timeout_note, file=sys.stderr)
        return False
    if build.returncode != 0:
        if not json_output:
            err = build.stderr.strip() or build.stdout.strip()
            if err:
                print(err, file=sys.stderr)
            print("Runtime wasm build failed", file=sys.stderr)
        return False
    profile_dir = _cargo_profile_dir(profile)
    src = root / "target" / "wasm32-wasip1" / profile_dir / "molt_runtime.wasm"
    if not src.exists():
        if not json_output:
            print(
                "Runtime wasm build succeeded but artifact is missing.", file=sys.stderr
            )
        return False
    runtime_wasm.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, runtime_wasm)
    if fingerprint is not None:
        try:
            fingerprint_path.parent.mkdir(parents=True, exist_ok=True)
            _write_runtime_fingerprint(fingerprint_path, fingerprint)
        except OSError:
            if not json_output:
                print(
                    "Warning: failed to write runtime fingerprint metadata.",
                    file=sys.stderr,
                )
    return True


def _read_wasm_varuint(data: bytes, offset: int) -> tuple[int, int]:
    result = 0
    shift = 0
    while True:
        if offset >= len(data):
            raise ValueError("Unexpected EOF while reading varuint")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if byte & 0x80 == 0:
            return result, offset
        shift += 7
        if shift > 35:
            raise ValueError("varuint too large")


def _read_wasm_string(data: bytes, offset: int) -> tuple[str, int]:
    length, offset = _read_wasm_varuint(data, offset)
    end = offset + length
    if end > len(data):
        raise ValueError("Unexpected EOF while reading string")
    return data[offset:end].decode("utf-8"), end


def _read_wasm_varint(data: bytes, offset: int, bits: int) -> tuple[int, int]:
    result = 0
    shift = 0
    byte = 0
    while True:
        if offset >= len(data):
            raise ValueError("Unexpected EOF while reading varint")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        shift += 7
        if byte & 0x80 == 0:
            break
        if shift > bits + 7:
            raise ValueError("varint too large")
    if shift < bits and (byte & 0x40):
        result |= -1 << shift
    return result, offset


def _read_wasm_const_expr_i32(data: bytes, offset: int) -> tuple[int, int]:
    if offset >= len(data):
        raise ValueError("Unexpected EOF while reading const expr")
    opcode = data[offset]
    offset += 1
    if opcode == 0x41:  # i32.const
        value, offset = _read_wasm_varint(data, offset, 32)
    elif opcode == 0x42:  # i64.const
        value, offset = _read_wasm_varint(data, offset, 64)
    else:
        raise ValueError("Unsupported const expr opcode")
    if offset >= len(data) or data[offset] != 0x0B:
        raise ValueError("Invalid const expr terminator")
    offset += 1
    return value, offset


def _read_wasm_table_min(path: Path) -> int | None:
    try:
        data = path.read_bytes()
    except OSError:
        return None
    if len(data) < 8 or data[:4] != b"\0asm" or data[4:8] != b"\x01\x00\x00\x00":
        return None
    offset = 8
    try:
        while offset < len(data):
            section_id = data[offset]
            offset += 1
            size, offset = _read_wasm_varuint(data, offset)
            end = offset + size
            if end > len(data):
                raise ValueError("Unexpected EOF while reading section")
            if section_id != 2:
                offset = end
                continue
            payload = data[offset:end]
            offset = end
            cursor = 0
            count, cursor = _read_wasm_varuint(payload, cursor)
            for _ in range(count):
                module, cursor = _read_wasm_string(payload, cursor)
                name, cursor = _read_wasm_string(payload, cursor)
                if cursor >= len(payload):
                    raise ValueError("Unexpected EOF while reading import")
                kind = payload[cursor]
                cursor += 1
                if kind == 0:
                    _, cursor = _read_wasm_varuint(payload, cursor)
                elif kind == 1:
                    if cursor >= len(payload):
                        raise ValueError("Unexpected EOF while reading table type")
                    cursor += 1
                    flags, cursor = _read_wasm_varuint(payload, cursor)
                    minimum, cursor = _read_wasm_varuint(payload, cursor)
                    if flags & 0x1:
                        _, cursor = _read_wasm_varuint(payload, cursor)
                    if module == "env" and name == "__indirect_function_table":
                        return minimum
                elif kind == 2:
                    flags, cursor = _read_wasm_varuint(payload, cursor)
                    _, cursor = _read_wasm_varuint(payload, cursor)
                    if flags & 0x1:
                        _, cursor = _read_wasm_varuint(payload, cursor)
                elif kind == 3:
                    if cursor + 2 > len(payload):
                        raise ValueError("Unexpected EOF while reading global type")
                    cursor += 2
                else:
                    raise ValueError("Unknown import kind")
    except ValueError:
        return None
    return None


def _read_wasm_data_end(path: Path) -> int | None:
    try:
        data = path.read_bytes()
    except OSError:
        return None
    if len(data) < 8 or data[:4] != b"\0asm" or data[4:8] != b"\x01\x00\x00\x00":
        return None
    offset = 8
    max_end = None
    try:
        while offset < len(data):
            section_id = data[offset]
            offset += 1
            size, offset = _read_wasm_varuint(data, offset)
            end = offset + size
            if end > len(data):
                raise ValueError("Unexpected EOF while reading section")
            if section_id != 11:
                offset = end
                continue
            payload = data[offset:end]
            offset = end
            cursor = 0
            count, cursor = _read_wasm_varuint(payload, cursor)
            for _ in range(count):
                if cursor >= len(payload):
                    raise ValueError("Unexpected EOF while reading data segment")
                flags = payload[cursor]
                cursor += 1
                is_passive = flags & 0x1
                has_memidx = flags & 0x2
                if has_memidx:
                    _, cursor = _read_wasm_varuint(payload, cursor)
                if is_passive:
                    size_bytes, cursor = _read_wasm_varuint(payload, cursor)
                    cursor += size_bytes
                    continue
                offset_val, cursor = _read_wasm_const_expr_i32(payload, cursor)
                size_bytes, cursor = _read_wasm_varuint(payload, cursor)
                cursor += size_bytes
                if offset_val < 0:
                    continue
                end_val = offset_val + size_bytes
                if max_end is None or end_val > max_end:
                    max_end = end_val
    except ValueError:
        return None
    return max_end


def _cargo_profile_dir(profile: BuildProfile) -> str:
    return "release" if profile == "release" else "debug"


def _resolve_env_path(var: str, default: Path) -> Path:
    value = os.environ.get(var)
    if not value:
        return default
    path = Path(value).expanduser()
    if not path.is_absolute():
        path = (Path.cwd() / path).absolute()
    return path


def _safe_output_base(name: str) -> str:
    cleaned = _OUTPUT_BASE_SAFE_RE.sub("_", name)
    return cleaned or "molt"


def _default_molt_home() -> Path:
    return _resolve_env_path("MOLT_HOME", Path.home() / ".molt")


def _default_molt_bin() -> Path:
    return _resolve_env_path("MOLT_BIN", _default_molt_home() / "bin")


def _default_molt_cache() -> Path:
    cache_override = os.environ.get("MOLT_CACHE")
    if cache_override:
        return _resolve_env_path("MOLT_CACHE", Path())
    if sys.platform == "darwin":
        base = Path.home() / "Library" / "Caches"
    else:
        xdg = os.environ.get("XDG_CACHE_HOME")
        if xdg:
            base = Path(xdg).expanduser()
            if not base.is_absolute():
                base = (Path.cwd() / base).absolute()
        else:
            base = Path.home() / ".cache"
    return base / "molt"


def _cargo_target_root(project_root: Path) -> Path:
    return _resolve_env_path("CARGO_TARGET_DIR", project_root / "target")


def _default_build_root(output_base: str) -> Path:
    safe_base = _safe_output_base(output_base)
    return _default_molt_home() / "build" / safe_base


def _resolve_cache_root(project_root: Path, cache_dir: str | None) -> Path:
    if not cache_dir:
        return _default_molt_cache()
    path = Path(cache_dir).expanduser()
    if not path.is_absolute():
        path = (project_root / path).absolute()
    return path


def _resolve_out_dir(project_root: Path, out_dir: str | Path | None) -> Path | None:
    if not out_dir:
        return None
    path = Path(out_dir).expanduser()
    if not path.is_absolute():
        path = (project_root / path).absolute()
    path.mkdir(parents=True, exist_ok=True)
    return path


def _resolve_sysroot(project_root: Path, sysroot: str | None) -> Path | None:
    raw = (
        sysroot
        or os.environ.get("MOLT_SYSROOT")
        or os.environ.get("MOLT_CROSS_SYSROOT")
    )
    if not raw:
        return None
    path = Path(raw).expanduser()
    if not path.is_absolute():
        path = (project_root / path).absolute()
    return path


def _pgo_hotspot_entries(
    hotspots: Any, warnings: list[str]
) -> list[tuple[str, float | None]]:
    entries: list[tuple[str, float | None]] = []
    if hotspots is None:
        return entries
    if isinstance(hotspots, dict):
        for name, score in hotspots.items():
            if not isinstance(name, str) or not name:
                continue
            score_val = score if isinstance(score, (int, float)) else None
            entries.append((name, float(score_val) if score_val is not None else None))
        return entries
    if isinstance(hotspots, list):
        for entry in hotspots:
            if isinstance(entry, str) and entry:
                entries.append((entry, None))
                continue
            if isinstance(entry, (list, tuple)) and entry:
                name = entry[0]
                score = entry[1] if len(entry) > 1 else None
                if isinstance(name, str) and name:
                    score_val = score if isinstance(score, (int, float)) else None
                    entries.append(
                        (name, float(score_val) if score_val is not None else None)
                    )
                continue
            if isinstance(entry, dict):
                name = (
                    entry.get("symbol")
                    or entry.get("name")
                    or entry.get("func")
                    or entry.get("function")
                )
                if not isinstance(name, str) or not name:
                    continue
                score = entry.get("score")
                if score is None:
                    score = entry.get("time_ms")
                if score is None:
                    score = entry.get("time_us")
                if score is None:
                    score = entry.get("count")
                score_val = score if isinstance(score, (int, float)) else None
                entries.append(
                    (name, float(score_val) if score_val is not None else None)
                )
                continue
        return entries
    warnings.append("PGO profile hotspots must be a list or object; ignoring.")
    return entries


def _extract_hot_functions(profile: dict[str, Any], warnings: list[str]) -> list[str]:
    entries = _pgo_hotspot_entries(profile.get("hotspots"), warnings)
    if not entries:
        return []
    has_score = any(score is not None for _, score in entries)
    if has_score:
        entries = sorted(
            entries,
            key=lambda item: (-(item[1] or 0.0), item[0]),
        )
    else:
        entries = sorted(entries, key=lambda item: item[0])
    seen: set[str] = set()
    hot: list[str] = []
    for name, _score in entries:
        if name in seen:
            continue
        seen.add(name)
        hot.append(name)
    return hot


def _load_pgo_profile(
    project_root: Path,
    profile_path: str,
    warnings: list[str],
    json_output: bool,
    command: str,
) -> tuple[PgoProfileSummary | None, Path | None, int | None]:
    path = Path(profile_path).expanduser()
    if not path.is_absolute():
        path = (project_root / path).absolute()
    if not path.exists():
        return (
            None,
            None,
            _fail(f"PGO profile not found: {path}", json_output, command=command),
        )
    try:
        raw = path.read_bytes()
    except OSError as exc:
        return (
            None,
            None,
            _fail(
                f"Failed to read PGO profile {path}: {exc}",
                json_output,
                command=command,
            ),
        )
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as exc:
        return (
            None,
            None,
            _fail(
                f"Invalid PGO profile JSON at {path}:{exc.lineno}:{exc.colno}: {exc.msg}",
                json_output,
                command=command,
            ),
        )
    if not isinstance(payload, dict):
        return (
            None,
            None,
            _fail(
                f"Invalid PGO profile {path}: expected a JSON object.",
                json_output,
                command=command,
            ),
        )
    errors: list[str] = []
    version = payload.get("molt_profile_version")
    if not isinstance(version, str):
        errors.append("missing molt_profile_version")
    elif version != "0.1":
        errors.append(f"unsupported molt_profile_version {version}")
    python_impl = payload.get("python_implementation")
    if not isinstance(python_impl, str) or not python_impl:
        errors.append("missing python_implementation")
    python_version = payload.get("python_version")
    if not isinstance(python_version, str) or not python_version:
        errors.append("missing python_version")
    platform_meta = payload.get("platform")
    if not isinstance(platform_meta, dict):
        errors.append("missing platform")
    else:
        if not isinstance(platform_meta.get("os"), str):
            errors.append("platform.os must be a string")
        if not isinstance(platform_meta.get("arch"), str):
            errors.append("platform.arch must be a string")
    run_meta = payload.get("run_metadata")
    if not isinstance(run_meta, dict):
        errors.append("missing run_metadata")
    else:
        if not isinstance(run_meta.get("entrypoint"), str):
            errors.append("run_metadata.entrypoint must be a string")
        argv = run_meta.get("argv")
        if not isinstance(argv, list) or not all(isinstance(arg, str) for arg in argv):
            errors.append("run_metadata.argv must be a list of strings")
        if not isinstance(run_meta.get("env_fingerprint"), str):
            errors.append("run_metadata.env_fingerprint must be a string")
        if not isinstance(run_meta.get("inputs_fingerprint"), str):
            errors.append("run_metadata.inputs_fingerprint must be a string")
        duration_ms = run_meta.get("duration_ms")
        if not isinstance(duration_ms, (int, float)) or duration_ms < 0:
            errors.append("run_metadata.duration_ms must be a non-negative number")
    if errors:
        return (
            None,
            None,
            _fail(
                f"Invalid PGO profile {path}: " + "; ".join(errors),
                json_output,
                command=command,
            ),
        )
    hot_functions = _extract_hot_functions(payload, warnings)
    digest = hashlib.sha256(raw).hexdigest()
    summary = PgoProfileSummary(
        version=version, hash=digest, hot_functions=hot_functions
    )
    return summary, path, None


def _resolve_timeout_env(env_name: str) -> tuple[float | None, str | None]:
    raw = os.environ.get(env_name)
    if raw is None:
        return None, None
    try:
        timeout = float(raw)
    except ValueError:
        return None, f"Invalid {env_name} value: {raw}"
    if timeout <= 0:
        return None, f"{env_name} must be greater than zero."
    return timeout, None


def _resolve_output_roots(
    project_root: Path, out_dir: Path | None, output_base: str
) -> tuple[Path, Path, Path]:
    artifacts_root = _default_build_root(output_base)
    bin_root = out_dir if out_dir is not None else _default_molt_bin()
    output_root = out_dir if out_dir is not None else project_root
    artifacts_root.mkdir(parents=True, exist_ok=True)
    bin_root.mkdir(parents=True, exist_ok=True)
    if output_root != bin_root:
        output_root.mkdir(parents=True, exist_ok=True)
    return artifacts_root, bin_root, output_root


def _resolve_output_path(
    output: str | None,
    default: Path,
    *,
    out_dir: Path | None,
    project_root: Path,
) -> Path:
    if not output:
        return default
    path = Path(output).expanduser()
    if not path.is_absolute():
        base = out_dir if out_dir is not None else project_root
        path = base / path
    if output.endswith(os.sep) or (os.altsep and output.endswith(os.altsep)):
        return path / default.name
    try:
        if path.exists() and path.is_dir():
            return path / default.name
    except OSError:
        pass
    return path


_CACHE_FINGERPRINT: str | None = None


def _cache_fingerprint() -> str:
    global _CACHE_FINGERPRINT
    if _CACHE_FINGERPRINT is not None:
        return _CACHE_FINGERPRINT
    root = Path(__file__).resolve().parents[2]
    hasher = hashlib.sha256()
    rustc_info = _rustc_version() or ""
    rustflags = os.environ.get("RUSTFLAGS", "")
    hasher.update(f"rustc:{rustc_info}\n".encode("utf-8"))
    hasher.update(f"rustflags:{rustflags}\n".encode("utf-8"))
    seen: set[Path] = set()
    for path in sorted(
        _backend_source_paths(root) + _runtime_source_paths(root),
        key=lambda p: str(p),
    ):
        if path in seen:
            continue
        seen.add(path)
        if path.is_dir():
            for item in sorted(path.rglob("*"), key=lambda p: str(p)):
                if item.is_file():
                    _hash_runtime_file(item, root, hasher)
        elif path.exists():
            _hash_runtime_file(path, root, hasher)
    _CACHE_FINGERPRINT = hasher.hexdigest()
    return _CACHE_FINGERPRINT


def _json_ir_default(value: Any) -> Any:
    if isinstance(value, complex):
        return {"__complex__": [value.real, value.imag]}
    raise TypeError(f"Object of type {type(value).__name__} is not JSON serializable")


def _cache_ir_payload(ir: dict[str, Any]) -> bytes:
    normalized: dict[str, Any] = ir
    funcs = ir.get("functions")
    if isinstance(funcs, list) and funcs:

        def _func_sort_key(entry: Any) -> str:
            if isinstance(entry, dict):
                name = entry.get("name")
                if isinstance(name, str):
                    return name
            return ""

        sorted_funcs = sorted(funcs, key=_func_sort_key)
        if sorted_funcs != funcs:
            normalized = dict(ir)
            normalized["functions"] = sorted_funcs
    return json.dumps(
        normalized, sort_keys=True, separators=(",", ":"), default=_json_ir_default
    ).encode("utf-8")


def _cache_key(
    ir: dict[str, Any],
    target: str,
    target_triple: str | None,
    variant: str = "",
) -> str:
    payload = _cache_ir_payload(ir)
    suffix = target_triple or target
    if variant:
        suffix = f"{suffix}:{variant}"
    fingerprint = _cache_fingerprint().encode("utf-8")
    digest = hashlib.sha256(
        payload + b"|" + suffix.encode("utf-8") + b"|" + fingerprint
    ).hexdigest()
    return digest


def _ensure_rustup_target(target_triple: str, warnings: list[str]) -> bool:
    rustup_path = shutil.which("rustup")
    if not rustup_path:
        warnings.append(f"rustup not found; cannot ensure target {target_triple}")
        return False
    try:
        result = subprocess.run(
            ["rustup", "target", "list", "--installed"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        warnings.append(f"Failed to query rustup targets: {exc}")
        return False
    installed = result.stdout.split()
    if target_triple in installed:
        return True
    try:
        add = subprocess.run(
            ["rustup", "target", "add", target_triple],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        warnings.append(f"Failed to add rustup target {target_triple}: {exc}")
        return False
    if add.returncode != 0:
        detail = (add.stderr or add.stdout).strip() or "unknown error"
        warnings.append(f"rustup target add failed for {target_triple}: {detail}")
        return False
    return True


def _strip_arch_flags(args: list[str]) -> list[str]:
    cleaned: list[str] = []
    skip_next = False
    for arg in args:
        if skip_next:
            skip_next = False
            continue
        if arg == "-arch":
            skip_next = True
            continue
        if arg.startswith("-arch="):
            continue
        cleaned.append(arg)
    return cleaned


def _zig_target_query(target_triple: str) -> str:
    triple = target_triple.strip()
    if not triple:
        return target_triple
    parts = [part for part in triple.split("-") if part]
    if len(parts) < 2:
        return target_triple

    arch_aliases = {
        "amd64": "x86_64",
        "x64": "x86_64",
        "arm64": "aarch64",
        "armv7l": "armv7",
        "i386": "x86",
        "i486": "x86",
        "i586": "x86",
        "i686": "x86",
    }
    os_aliases = {
        "darwin": "macos",
        "macosx": "macos",
        "win32": "windows",
        "mingw32": "windows",
        "mingw64": "windows",
        "cygwin": "windows",
    }
    abi_aliases = {
        "sim": "simulator",
        "androideabi": "android",
    }
    abi_tokens = {
        "gnu",
        "gnueabi",
        "gnueabihf",
        "gnuabi64",
        "gnux32",
        "musl",
        "musleabi",
        "musleabihf",
        "msvc",
        "eabi",
        "eabihf",
        "android",
        "simulator",
        "sim",
        "ilp32",
        "uclibc",
        "ohos",
        "macabi",
        "androideabi",
    }
    os_tokens = {
        "linux",
        "windows",
        "darwin",
        "macos",
        "macosx",
        "ios",
        "tvos",
        "watchos",
        "freebsd",
        "netbsd",
        "openbsd",
        "dragonfly",
        "solaris",
        "haiku",
        "hurd",
        "android",
        "wasi",
        "emscripten",
        "fuchsia",
        "uefi",
        "mingw32",
        "mingw64",
        "cygwin",
        "illumos",
        "aix",
    }

    def is_os_token(token: str) -> bool:
        lowered = token.lower()
        return lowered in os_tokens or lowered in os_aliases

    arch = arch_aliases.get(parts[0].lower(), parts[0].lower())
    remainder = [part.lower() for part in parts[1:]]
    abi = None
    if remainder:
        last = remainder[-1]
        if len(remainder) >= 2 and last in abi_tokens and is_os_token(remainder[-2]):
            abi = abi_aliases.get(last, last)
            remainder = remainder[:-1]
        elif last in abi_tokens and last not in os_tokens:
            abi = abi_aliases.get(last, last)
            remainder = remainder[:-1]
    os_part = remainder[-1] if remainder else None
    vendor_parts = remainder[:-1] if len(remainder) > 1 else []
    if os_part is None:
        return f"{arch}-{abi}" if abi else arch
    os_token = os_part.lower()
    match = re.match(r"^(darwin|macosx|macos|ios|tvos|watchos)([0-9].*)$", os_token)
    if match:
        os_token = match.group(1)
    os_name = os_aliases.get(os_token, os_token)
    if os_name in {"unknown", "none"}:
        os_name = "freestanding"
    if os_name == "windows" and abi is None:
        if any(token in {"w64", "mingw32", "mingw64"} for token in vendor_parts):
            abi = "gnu"
    if os_name in {"mingw32", "mingw64"}:
        os_name = "windows"
        if abi is None:
            abi = "gnu"
    if os_name in {"macos", "ios", "tvos", "watchos"}:
        if abi == "sim":
            abi = "simulator"
        elif os_name == "macos":
            abi = None
        elif abi in {
            "gnu",
            "gnueabi",
            "gnueabihf",
            "gnuabi64",
            "gnux32",
            "musl",
            "musleabi",
            "musleabihf",
            "msvc",
            "android",
            "eabi",
            "eabihf",
            "uclibc",
        }:
            abi = None

    if abi:
        return f"{arch}-{os_name}-{abi}"
    return f"{arch}-{os_name}"


def _detect_macos_arch(obj_path: Path) -> str | None:
    try:
        result = subprocess.run(
            ["lipo", "-archs", str(obj_path)],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    archs = result.stdout.strip().split()
    return archs[0] if archs else None


def _detect_macos_deployment_target() -> str | None:
    env_target = os.environ.get("MOLT_MACOSX_DEPLOYMENT_TARGET")
    if env_target:
        return env_target
    env_target = os.environ.get("MACOSX_DEPLOYMENT_TARGET")
    if env_target:
        return env_target
    try:
        result = subprocess.run(
            ["xcrun", "--show-sdk-version"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    version = result.stdout.strip()
    return version or None


def build(
    file_path: str | None,
    target: Target = "native",
    parse_codec: ParseCodec = "msgpack",
    type_hint_policy: TypeHintPolicy = "ignore",
    fallback_policy: FallbackPolicy = "error",
    type_facts_path: str | None = None,
    pgo_profile: str | None = None,
    output: str | None = None,
    json_output: bool = False,
    verbose: bool = False,
    deterministic: bool = True,
    deterministic_warn: bool = False,
    trusted: bool = False,
    capabilities: CapabilityInput | None = None,
    cache: bool = True,
    cache_dir: str | None = None,
    cache_report: bool = False,
    sysroot: str | None = None,
    emit_ir: str | None = None,
    emit: EmitMode | None = None,
    out_dir: str | None = None,
    profile: BuildProfile = "release",
    linked: bool = False,
    linked_output: str | None = None,
    require_linked: bool = False,
    respect_pythonpath: bool = False,
    module: str | None = None,
) -> int:
    if isinstance(profile, bool):
        profile = "release"
    if profile not in {"dev", "release"}:
        return _fail(f"Invalid build profile: {profile}", json_output, command="build")
    if file_path and module:
        return _fail(
            "Use a file path or --module, not both.", json_output, command="build"
        )
    if not file_path and not module:
        return _fail("Missing entry file or module.", json_output, command="build")

    stdlib_root = _stdlib_root_path()
    warnings: list[str] = []
    cwd_root = _find_project_root(Path.cwd())
    project_root = (
        _find_project_root(Path(file_path).resolve()) if file_path else cwd_root
    )
    if not _has_project_markers(project_root) and _has_project_markers(cwd_root):
        project_root = cwd_root
    molt_root = _find_molt_root(project_root, cwd_root)
    root_error = _require_molt_root(molt_root, json_output, "build")
    if root_error is not None:
        return root_error
    lock_error = _check_lockfiles(
        molt_root,
        json_output,
        warnings,
        deterministic,
        deterministic_warn,
        "build",
    )
    if lock_error is not None:
        return lock_error
    sysroot_path = _resolve_sysroot(project_root, sysroot)
    if sysroot_path is not None and not sysroot_path.exists():
        return _fail(
            f"Sysroot not found: {sysroot_path}",
            json_output,
            command="build",
        )
    pgo_profile_summary: PgoProfileSummary | None = None
    pgo_profile_path: Path | None = None
    if pgo_profile:
        summary, resolved, err = _load_pgo_profile(
            project_root,
            pgo_profile,
            warnings,
            json_output,
            command="build",
        )
        if err is not None:
            return err
        pgo_profile_summary = summary
        pgo_profile_path = resolved
    pgo_profile_payload: dict[str, Any] | None = None
    if pgo_profile_summary is not None and pgo_profile_path is not None:
        pgo_profile_payload = {
            "path": str(pgo_profile_path),
            "version": pgo_profile_summary.version,
            "hash": pgo_profile_summary.hash,
            "hot_functions": pgo_profile_summary.hot_functions,
        }
    cargo_timeout, timeout_err = _resolve_timeout_env("MOLT_CARGO_TIMEOUT")
    if timeout_err:
        return _fail(timeout_err, json_output, command="build")
    backend_timeout, timeout_err = _resolve_timeout_env("MOLT_BACKEND_TIMEOUT")
    if timeout_err:
        return _fail(timeout_err, json_output, command="build")
    link_timeout, timeout_err = _resolve_timeout_env("MOLT_LINK_TIMEOUT")
    if timeout_err:
        return _fail(timeout_err, json_output, command="build")
    backend_profile, profile_err = _resolve_backend_profile()
    if profile_err:
        return _fail(profile_err, json_output, command="build")
    capabilities_list: list[str] | None = None
    capabilities_source = None
    capability_profiles: list[str] = []
    if capabilities is not None:
        parsed, profiles, source, errors = _parse_capabilities(capabilities)
        if errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(errors),
                json_output,
                command="build",
            )
        capabilities_list = parsed
        capability_profiles = profiles
        capabilities_source = source
    cwd_root = _find_project_root(Path.cwd())
    module_roots: list[Path] = []
    extra_roots = os.environ.get("MOLT_MODULE_ROOTS", "")
    if extra_roots:
        for entry in extra_roots.split(os.pathsep):
            if not entry:
                continue
            entry_path = Path(entry).expanduser()
            if entry_path.exists():
                module_roots.append(entry_path)
    for root in (project_root, cwd_root):
        if root.exists():
            module_roots.append(root)
        src_root = root / "src"
        if src_root.exists():
            module_roots.append(src_root)
        module_roots.extend(_vendor_roots(root))
    source_path: Path | None = None
    entry_module: str | None = None
    if file_path:
        source_path = Path(file_path)
        if not source_path.exists():
            return _fail(f"File not found: {source_path}", json_output, command="build")
        module_roots.append(source_path.parent)
    if respect_pythonpath:
        pythonpath = os.environ.get("PYTHONPATH", "")
        if pythonpath:
            for entry in pythonpath.split(os.pathsep):
                if not entry:
                    continue
                entry_path = Path(entry).expanduser()
                if entry_path.exists():
                    module_roots.append(entry_path)
    module_roots = list(dict.fromkeys(root.resolve() for root in module_roots))
    if module:
        resolved = _resolve_entry_module(module, module_roots)
        if resolved is None:
            return _fail(
                f"Entry module not found: {module}",
                json_output,
                command="build",
            )
        entry_module, source_path = resolved
        module_roots.append(source_path.parent.resolve())
        module_roots = list(dict.fromkeys(module_roots))
    elif source_path is not None:
        entry_module = _module_name_from_path(source_path, module_roots, stdlib_root)
    if source_path is None or entry_module is None:
        return _fail("Failed to resolve entry module.", json_output, command="build")
    try:
        entry_source = _read_module_source(source_path)
    except (SyntaxError, UnicodeDecodeError) as exc:
        return _fail(
            f"Syntax error in {source_path}: {exc}",
            json_output,
            command="build",
        )
    except OSError as exc:
        return _fail(
            f"Failed to read entry module {source_path}: {exc}",
            json_output,
            command="build",
        )
    try:
        entry_tree = ast.parse(entry_source, filename=str(source_path))
    except SyntaxError as exc:
        return _fail(
            f"Syntax error in {source_path}: {exc}",
            json_output,
            command="build",
        )
    (
        entry_pkg_override_set,
        entry_pkg_override,
        entry_spec_override_set,
        entry_spec_override,
        entry_spec_override_is_package,
    ) = _infer_module_overrides(entry_tree)
    if entry_pkg_override_set and entry_pkg_override:
        root = _package_root_for_override(source_path, entry_pkg_override)
        if root is not None:
            source_parent = source_path.parent.resolve()
            module_roots = [
                candidate
                for candidate in module_roots
                if candidate.resolve() != source_parent
            ]
            module_roots.append(root)
            entry_module = _module_name_from_path(source_path, [root], stdlib_root)
    elif entry_spec_override_set and entry_spec_override:
        override_is_package = (
            entry_spec_override_is_package
            if entry_spec_override_is_package is not None
            else source_path.name == "__init__.py"
        )
        package_name = _spec_parent(entry_spec_override, override_is_package)
        if package_name:
            root = _package_root_for_override(source_path, package_name)
            if root is not None:
                source_parent = source_path.parent.resolve()
                module_roots = [
                    candidate
                    for candidate in module_roots
                    if candidate.resolve() != source_parent
                ]
                module_roots.append(root)
                entry_module = _module_name_from_path(source_path, [root], stdlib_root)
    module_roots = list(dict.fromkeys(root.resolve() for root in module_roots))
    entry_imports = set(
        _collect_imports(entry_tree, entry_module, source_path.name == "__init__.py")
    )
    stub_parents = STUB_PARENT_MODULES - entry_imports
    stdlib_allowlist = _stdlib_allowlist()
    roots = module_roots + [stdlib_root]
    module_graph, explicit_imports = _discover_module_graph(
        source_path,
        roots,
        module_roots,
        stdlib_root,
        stdlib_allowlist,
        skip_modules=STUB_MODULES,
        stub_parents=stub_parents,
    )
    _collect_package_parents(module_graph, roots, stdlib_root, stdlib_allowlist)
    _ensure_core_stdlib_modules(module_graph, stdlib_root)
    intrinsic_enforced = _enforce_intrinsic_stdlib(
        module_graph, stdlib_root, json_output
    )
    if intrinsic_enforced is not None:
        return intrinsic_enforced
    core_paths = [
        path
        for name in ("builtins", "sys")
        if (path := module_graph.get(name)) is not None
    ]
    for core_path in core_paths:
        core_graph, _ = _discover_module_graph(
            core_path,
            roots,
            module_roots,
            stdlib_root,
            stdlib_allowlist,
            skip_modules=STUB_MODULES,
            stub_parents=stub_parents,
        )
        for name, path in core_graph.items():
            module_graph.setdefault(name, path)
    spawn_enabled = False
    spawn_path = _resolve_module_path(ENTRY_OVERRIDE_SPAWN, [stdlib_root])
    if spawn_path is not None:
        spawn_enabled = True
        spawn_graph, _ = _discover_module_graph(
            spawn_path,
            roots,
            module_roots,
            stdlib_root,
            stdlib_allowlist,
            skip_modules=STUB_MODULES,
            stub_parents=stub_parents,
        )
        for name, path in spawn_graph.items():
            module_graph.setdefault(name, path)
    namespace_parents = _collect_namespace_parents(
        module_graph, roots, stdlib_root, stdlib_allowlist, explicit_imports
    )
    if verbose and not json_output:
        print(f"Project root: {project_root}")
        print(f"Module roots: {', '.join(str(root) for root in module_roots)}")
        print(f"Modules discovered: {len(module_graph)}")
    output_base = _output_base_for_entry(entry_module, source_path)
    out_dir_path = _resolve_out_dir(project_root, out_dir)
    artifacts_root, bin_root, output_root = _resolve_output_roots(
        project_root, out_dir_path, output_base
    )
    namespace_modules: dict[str, Path] = {}
    if namespace_parents:
        for name in sorted(namespace_parents):
            paths = _namespace_paths(
                name,
                _roots_for_module(name, roots, stdlib_root, stdlib_allowlist),
            )
            if not paths:
                continue
            stub_path = _write_namespace_module(name, paths, artifacts_root)
            namespace_modules[name] = stub_path
        if namespace_modules:
            module_graph.update(namespace_modules)
    namespace_module_names = set(namespace_modules)
    is_wasm = target == "wasm"
    if trusted and is_wasm:
        return _fail(
            "Trusted mode is not supported for wasm targets",
            json_output,
            command="build",
        )
    if require_linked and not is_wasm:
        return _fail(
            "--require-linked is only supported for wasm targets",
            json_output,
            command="build",
        )
    if linked_output and not linked and not require_linked:
        return _fail(
            "--linked-output requires --linked",
            json_output,
            command="build",
        )
    if linked and not is_wasm:
        return _fail(
            "Linked output is only supported for wasm targets",
            json_output,
            command="build",
        )
    if require_linked and not linked:
        linked = True
    target_triple = None if target in {"native", "wasm"} else target
    emit_mode = emit or ("wasm" if is_wasm else "bin")
    if emit_mode not in {"bin", "obj", "wasm"}:
        return _fail(
            f"Invalid emit mode: {emit_mode}",
            json_output,
            command="build",
        )
    if is_wasm and emit_mode != "wasm":
        return _fail(
            f"Invalid emit mode for wasm target: {emit_mode}",
            json_output,
            command="build",
        )
    if not is_wasm and emit_mode == "wasm":
        return _fail(
            "emit=wasm requires --target wasm",
            json_output,
            command="build",
        )
    output_binary: Path | None = None
    linked_output_path: Path | None = None
    if is_wasm:
        output_wasm = _resolve_output_path(
            output,
            output_root / "output.wasm",
            out_dir=out_dir_path,
            project_root=project_root,
        )
        output_artifact = output_wasm
        if linked:
            stem = output_wasm.stem
            if stem.endswith("_linked"):
                stem = stem[: -len("_linked")]
            linked_output_path = output_wasm.with_name(
                f"{stem}_linked{output_wasm.suffix}"
            )
            if linked_output is not None:
                linked_output_path = _resolve_output_path(
                    linked_output,
                    linked_output_path,
                    out_dir=out_dir_path,
                    project_root=project_root,
                )
    else:
        output_obj = artifacts_root / "output.o"
        if emit_mode == "obj":
            output_obj = _resolve_output_path(
                output,
                output_root / "output.o",
                out_dir=out_dir_path,
                project_root=project_root,
            )
        output_artifact = output_obj
        if emit_mode == "bin":
            output_binary = _resolve_output_path(
                output,
                bin_root / f"{output_base}_molt",
                out_dir=out_dir_path,
                project_root=project_root,
            )
    for path in (output_artifact, output_binary):
        if path is not None and path.parent != Path("."):
            path.parent.mkdir(parents=True, exist_ok=True)
    emit_ir_path: Path | None = None
    if emit_ir:
        emit_ir_path = Path(emit_ir)
        if not emit_ir_path.is_absolute():
            emit_ir_path = artifacts_root / emit_ir_path
        if emit_ir_path.parent != Path("."):
            emit_ir_path.parent.mkdir(parents=True, exist_ok=True)
    for stub in stub_parents:
        if stub != entry_module:
            module_graph.pop(stub, None)
    if IMPORTER_MODULE_NAME not in module_graph:
        importer_names = sorted(
            {
                name
                for name in module_graph
                if name not in {IMPORTER_MODULE_NAME, "builtins"}
            }.union(stub_parents)
        )
        importer_path = _write_importer_module(importer_names, artifacts_root)
        module_graph[IMPORTER_MODULE_NAME] = importer_path
    machinery_path = _resolve_module_path("importlib.machinery", [stdlib_root])
    if machinery_path is not None:
        module_graph.setdefault("importlib.machinery", machinery_path)
    known_modules = set(module_graph.keys())
    stdlib_allowlist.update(STUB_MODULES)
    stdlib_allowlist.update(stub_parents)
    stdlib_allowlist.add("molt.stdlib")
    module_deps: dict[str, set[str]] = {}
    known_func_defaults: dict[str, dict[str, dict[str, Any]]] = {}
    module_trees: dict[str, ast.AST] = {}
    syntax_error_modules: dict[str, ModuleSyntaxErrorInfo] = {}
    for module_name, module_path in module_graph.items():
        try:
            source = _read_module_source(module_path)
        except (SyntaxError, UnicodeDecodeError) as exc:
            if module_name == entry_module:
                return _fail(
                    f"Syntax error in {module_path}: {exc}",
                    json_output,
                    command="build",
                )
            syntax_error_modules[module_name] = _syntax_error_info_from_exception(
                exc, path=module_path
            )
            module_deps[module_name] = set()
            known_func_defaults[module_name] = {}
            continue
        except OSError as exc:
            return _fail(
                f"Failed to read module {module_path}: {exc}",
                json_output,
                command="build",
            )
        try:
            tree = ast.parse(source, filename=str(module_path))
        except SyntaxError as exc:
            if module_name == entry_module:
                return _fail(
                    f"Syntax error in {module_path}: {exc}",
                    json_output,
                    command="build",
                )
            syntax_error_modules[module_name] = _syntax_error_info_from_exception(
                exc, path=module_path
            )
            module_deps[module_name] = set()
            known_func_defaults[module_name] = {}
            continue
        module_trees[module_name] = tree
        module_deps[module_name] = _module_dependencies(tree, module_name, module_graph)
        known_func_defaults[module_name] = _collect_func_defaults(tree)
    module_order = _topo_sort_modules(module_graph, module_deps)
    type_facts = None
    if type_facts_path is None and type_hint_policy in {"trust", "check"}:
        type_facts, ty_ok = _collect_type_facts_for_build(
            list(module_graph.values()), type_hint_policy, source_path
        )
        if type_facts is None and type_hint_policy == "trust":
            return _fail(
                "Type facts unavailable; refusing trusted build.",
                json_output,
                command="build",
            )
        if type_hint_policy == "trust" and not ty_ok:
            return _fail(
                "ty check failed; refusing trusted build.",
                json_output,
                command="build",
            )
        if type_hint_policy == "check" and not ty_ok:
            warning = "ty check failed; continuing with guarded hints only."
            warnings.append(warning)
            if not json_output:
                print(warning, file=sys.stderr)
    if type_facts_path is not None:
        facts_path = Path(type_facts_path)
        if not facts_path.exists():
            return _fail(
                f"Type facts not found: {facts_path}",
                json_output,
                command="build",
            )
        try:
            type_facts = load_type_facts(facts_path)
        except (OSError, json.JSONDecodeError, ValueError) as exc:
            return _fail(
                f"Failed to load type facts: {exc}",
                json_output,
                command="build",
            )

    functions: list[dict[str, Any]] = []
    # Normalize code-slot IDs across modules to keep tracebacks consistent.
    global_code_ids: dict[str, int] = {}
    global_code_id_counter = 0

    def _register_global_code_id(symbol: str) -> int:
        nonlocal global_code_id_counter
        code_id = global_code_ids.get(symbol)
        if code_id is None:
            code_id = global_code_id_counter
            global_code_ids[symbol] = code_id
            global_code_id_counter += 1
        return code_id

    def _remap_module_code_ops(
        module_name: str,
        funcs: list[dict[str, Any]],
        local_id_to_symbol: dict[int, str],
    ) -> None:
        for func in funcs:
            ops = func.get("ops", [])
            remapped_ops: list[dict[str, Any]] = []
            for op in ops:
                kind = op.get("kind")
                if kind == "code_slots_init":
                    continue
                if kind == "call":
                    symbol = op.get("s_value")
                    if symbol:
                        op["value"] = _register_global_code_id(symbol)
                elif kind == "code_slot_set":
                    local_id = op.get("value")
                    symbol = local_id_to_symbol.get(local_id)
                    if symbol is None:
                        raise ValueError(
                            "Missing code symbol for id "
                            f"{local_id} in module {module_name}"
                        )
                    op["value"] = _register_global_code_id(symbol)
                remapped_ops.append(op)
            func["ops"] = remapped_ops

    enable_phi = not is_wasm
    if target_triple:
        _ensure_rustup_target(target_triple, warnings)
    known_classes: dict[str, Any] = {}
    for module_name in module_order:
        module_path = module_graph[module_name]
        if module_name in syntax_error_modules:
            tree = _syntax_error_stub_ast(syntax_error_modules[module_name])
        else:
            tree = module_trees.get(module_name)
            if tree is None:
                try:
                    source = _read_module_source(module_path)
                except (SyntaxError, UnicodeDecodeError) as exc:
                    return _fail(
                        f"Syntax error in {module_path}: {exc}",
                        json_output,
                        command="build",
                    )
                except OSError as exc:
                    return _fail(
                        f"Failed to read module {module_path}: {exc}",
                        json_output,
                        command="build",
                    )
                try:
                    tree = ast.parse(source, filename=str(module_path))
                except SyntaxError as exc:
                    return _fail(
                        f"Syntax error in {module_path}: {exc}",
                        json_output,
                        command="build",
                    )
        entry_override = entry_module
        if module_name == entry_module and entry_module != "__main__":
            entry_override = None
        gen = SimpleTIRGenerator(
            parse_codec=parse_codec,
            type_hint_policy=type_hint_policy,
            fallback_policy=fallback_policy,
            source_path=str(module_path),
            type_facts=type_facts,
            module_name=module_name,
            module_is_namespace=module_name in namespace_module_names,
            entry_module=entry_override,
            enable_phi=enable_phi,
            known_modules=known_modules,
            known_classes=known_classes,
            stdlib_allowlist=stdlib_allowlist,
            known_func_defaults=known_func_defaults,
        )
        try:
            gen.visit(tree)
        except CompatibilityError as exc:
            return _fail(str(exc), json_output, command="build")
        ir = gen.to_json()
        init_symbol = SimpleTIRGenerator.module_init_symbol(module_name)
        local_code_ids = dict(gen.func_code_ids)
        if "molt_main" in local_code_ids:
            local_code_ids[init_symbol] = local_code_ids.pop("molt_main")
        local_id_to_symbol = {
            code_id: symbol for symbol, code_id in local_code_ids.items()
        }
        try:
            _remap_module_code_ops(module_name, ir["functions"], local_id_to_symbol)
        except ValueError as exc:
            return _fail(str(exc), json_output, command="build")
        for func in ir["functions"]:
            if func["name"] == "molt_main":
                func["name"] = init_symbol
        functions.extend(ir["functions"])
        for class_name in gen.local_class_names:
            known_classes[class_name] = gen.classes[class_name]

    entry_path: Path | None = None
    if entry_module != "__main__":
        entry_path = module_graph.get(entry_module)
        if entry_path is None:
            return _fail(
                f"Entry module not found: {entry_module}",
                json_output,
                command="build",
            )
        try:
            source = _read_module_source(entry_path)
        except (SyntaxError, UnicodeDecodeError) as exc:
            return _fail(
                f"Syntax error in {entry_path}: {exc}",
                json_output,
                command="build",
            )
        except OSError as exc:
            return _fail(
                f"Failed to read module {entry_path}: {exc}",
                json_output,
                command="build",
            )
        try:
            tree = ast.parse(source, filename=str(entry_path))
        except SyntaxError as exc:
            return _fail(
                f"Syntax error in {entry_path}: {exc}",
                json_output,
                command="build",
            )
        main_gen = SimpleTIRGenerator(
            parse_codec=parse_codec,
            type_hint_policy=type_hint_policy,
            fallback_policy=fallback_policy,
            source_path=str(entry_path),
            type_facts=type_facts,
            type_facts_module=entry_module,
            module_name="__main__",
            module_spec_name=entry_module,
            entry_module=None,
            enable_phi=enable_phi,
            known_modules=known_modules,
            known_classes=known_classes,
            stdlib_allowlist=stdlib_allowlist,
            known_func_defaults=known_func_defaults,
        )
        try:
            main_gen.visit(tree)
        except CompatibilityError as exc:
            return _fail(str(exc), json_output, command="build")
        main_ir = main_gen.to_json()
        main_init = SimpleTIRGenerator.module_init_symbol("__main__")
        local_code_ids = dict(main_gen.func_code_ids)
        if "molt_main" in local_code_ids:
            local_code_ids[main_init] = local_code_ids.pop("molt_main")
        local_id_to_symbol = {
            code_id: symbol for symbol, code_id in local_code_ids.items()
        }
        try:
            _remap_module_code_ops("__main__", main_ir["functions"], local_id_to_symbol)
        except ValueError as exc:
            return _fail(str(exc), json_output, command="build")
        for func in main_ir["functions"]:
            if func["name"] == "molt_main":
                func["name"] = main_init
        functions.extend(main_ir["functions"])

    entry_init_name = "__main__" if entry_module != "__main__" else entry_module
    entry_init = SimpleTIRGenerator.module_init_symbol(entry_init_name)
    py_version = sys.version_info
    version_release = py_version.releaselevel
    version_serial = py_version.serial
    version_suffix = ""
    if version_release == "alpha":
        version_suffix = f"a{version_serial}"
    elif version_release == "beta":
        version_suffix = f"b{version_serial}"
    elif version_release == "candidate":
        version_suffix = f"rc{version_serial}"
    elif version_release != "final":
        version_suffix = f"{version_release}{version_serial}"
    version_str = (
        f"{py_version.major}.{py_version.minor}.{py_version.micro}"
        f"{version_suffix} (molt)"
    )
    entry_ops = [
        {
            "kind": "call",
            "s_value": "molt_runtime_init",
            "args": [],
            "out": "v0",
            "value": _register_global_code_id("molt_runtime_init"),
        },
        {
            "kind": "call",
            "s_value": entry_init,
            "args": [],
            "out": "v1",
            "value": _register_global_code_id(entry_init),
        },
        {
            "kind": "call",
            "s_value": "molt_runtime_shutdown",
            "args": [],
            "out": "v2",
            "value": _register_global_code_id("molt_runtime_shutdown"),
        },
        {"kind": "ret_void"},
    ]
    version_ops = [
        {"kind": "const", "value": py_version.major, "out": "v3"},
        {"kind": "const", "value": py_version.minor, "out": "v4"},
        {"kind": "const", "value": py_version.micro, "out": "v5"},
        {"kind": "const_str", "s_value": version_release, "out": "v6"},
        {"kind": "const", "value": version_serial, "out": "v7"},
        {"kind": "const_str", "s_value": version_str, "out": "v8"},
        {
            "kind": "call",
            "s_value": "molt_sys_set_version_info",
            "args": ["v3", "v4", "v5", "v6", "v7", "v8"],
            "out": "v9",
            "value": _register_global_code_id("molt_sys_set_version_info"),
        },
    ]
    entry_ops[1:1] = version_ops
    entry_call_idx = next(
        idx
        for idx, op in enumerate(entry_ops)
        if op.get("kind") == "call" and op.get("s_value") == entry_init
    )
    used_vars: set[int] = set()
    for op in entry_ops:
        out = op.get("out")
        if isinstance(out, str) and out.startswith("v"):
            try:
                used_vars.add(int(out[1:]))
            except ValueError:
                continue
    next_var = max(used_vars, default=-1) + 1
    if "sys" in module_graph:
        sys_init = SimpleTIRGenerator.module_init_symbol("sys")
        sys_out_var = f"v{next_var}"
        next_var += 1
        sys_init_op = {
            "kind": "call",
            "s_value": sys_init,
            "args": [],
            "out": sys_out_var,
            "value": _register_global_code_id(sys_init),
        }
        entry_call_idx = next(
            idx
            for idx, op in enumerate(entry_ops)
            if op.get("kind") == "call" and op.get("s_value") == entry_init
        )
        entry_ops[entry_call_idx:entry_call_idx] = [sys_init_op]

    module_code_ops: list[dict[str, Any]] = []
    for module_name in module_order:
        module_path = module_graph[module_name]
        init_symbol = SimpleTIRGenerator.module_init_symbol(module_name)
        code_id = _register_global_code_id(init_symbol)
        file_var = f"v{next_var}"
        next_var += 1
        name_var = f"v{next_var}"
        next_var += 1
        line_var = f"v{next_var}"
        next_var += 1
        linetable_var = f"v{next_var}"
        next_var += 1
        varnames_var = f"v{next_var}"
        next_var += 1
        argcount_var = f"v{next_var}"
        next_var += 1
        posonly_var = f"v{next_var}"
        next_var += 1
        kwonly_var = f"v{next_var}"
        next_var += 1
        code_var = f"v{next_var}"
        next_var += 1
        module_code_ops.extend(
            [
                {
                    "kind": "const_str",
                    "s_value": module_path.as_posix(),
                    "out": file_var,
                },
                {"kind": "const_str", "s_value": "<module>", "out": name_var},
                {"kind": "const", "value": 1, "out": line_var},
                {"kind": "const_none", "out": linetable_var},
                {"kind": "tuple_new", "args": [], "out": varnames_var},
                {"kind": "const", "value": 0, "out": argcount_var},
                {"kind": "const", "value": 0, "out": posonly_var},
                {"kind": "const", "value": 0, "out": kwonly_var},
                {
                    "kind": "code_new",
                    "args": [
                        file_var,
                        name_var,
                        line_var,
                        linetable_var,
                        varnames_var,
                        argcount_var,
                        posonly_var,
                        kwonly_var,
                    ],
                    "out": code_var,
                },
                {
                    "kind": "code_slot_set",
                    "value": code_id,
                    "args": [code_var],
                },
            ]
        )
    if entry_module != "__main__" and entry_path is not None:
        init_symbol = SimpleTIRGenerator.module_init_symbol("__main__")
        code_id = _register_global_code_id(init_symbol)
        file_var = f"v{next_var}"
        next_var += 1
        name_var = f"v{next_var}"
        next_var += 1
        line_var = f"v{next_var}"
        next_var += 1
        linetable_var = f"v{next_var}"
        next_var += 1
        varnames_var = f"v{next_var}"
        next_var += 1
        argcount_var = f"v{next_var}"
        next_var += 1
        posonly_var = f"v{next_var}"
        next_var += 1
        kwonly_var = f"v{next_var}"
        next_var += 1
        code_var = f"v{next_var}"
        next_var += 1
        module_code_ops.extend(
            [
                {
                    "kind": "const_str",
                    "s_value": entry_path.as_posix(),
                    "out": file_var,
                },
                {"kind": "const_str", "s_value": "<module>", "out": name_var},
                {"kind": "const", "value": 1, "out": line_var},
                {"kind": "const_none", "out": linetable_var},
                {"kind": "tuple_new", "args": [], "out": varnames_var},
                {"kind": "const", "value": 0, "out": argcount_var},
                {"kind": "const", "value": 0, "out": posonly_var},
                {"kind": "const", "value": 0, "out": kwonly_var},
                {
                    "kind": "code_new",
                    "args": [
                        file_var,
                        name_var,
                        line_var,
                        linetable_var,
                        varnames_var,
                        argcount_var,
                        posonly_var,
                        kwonly_var,
                    ],
                    "out": code_var,
                },
                {
                    "kind": "code_slot_set",
                    "value": code_id,
                    "args": [code_var],
                },
            ]
        )
    entry_ops[entry_call_idx:entry_call_idx] = module_code_ops
    if spawn_enabled:
        spawn_init = SimpleTIRGenerator.module_init_symbol(ENTRY_OVERRIDE_SPAWN)
        spawn_code_id = _register_global_code_id(spawn_init)
        entry_call_idx = next(
            idx
            for idx, op in enumerate(entry_ops)
            if op.get("kind") == "call" and op.get("s_value") == entry_init
        )
        entry_code_id = _register_global_code_id(entry_init)
        env_key_var = f"v{next_var}"
        next_var += 1
        env_default_var = f"v{next_var}"
        next_var += 1
        env_value_var = f"v{next_var}"
        next_var += 1
        spawn_name_var = f"v{next_var}"
        next_var += 1
        spawn_eq_var = f"v{next_var}"
        next_var += 1
        spawn_out_var = f"v{next_var}"
        next_var += 1
        entry_out_var = f"v{next_var}"
        next_var += 1
        entry_ops[entry_call_idx : entry_call_idx + 1] = [
            {"kind": "const_str", "s_value": ENTRY_OVERRIDE_ENV, "out": env_key_var},
            {"kind": "const_str", "s_value": "", "out": env_default_var},
            {
                "kind": "env_get",
                "args": [env_key_var, env_default_var],
                "out": env_value_var,
            },
            {
                "kind": "const_str",
                "s_value": ENTRY_OVERRIDE_SPAWN,
                "out": spawn_name_var,
            },
            {
                "kind": "string_eq",
                "args": [env_value_var, spawn_name_var],
                "out": spawn_eq_var,
            },
            {"kind": "if", "args": [spawn_eq_var]},
            {
                "kind": "call",
                "s_value": spawn_init,
                "args": [],
                "out": spawn_out_var,
                "value": spawn_code_id,
            },
            {"kind": "else"},
            {
                "kind": "call",
                "s_value": entry_init,
                "args": [],
                "out": entry_out_var,
                "value": entry_code_id,
            },
            {"kind": "end_if"},
        ]
    entry_ops.insert(1, {"kind": "code_slots_init", "value": len(global_code_ids)})
    functions.append({"name": "molt_main", "params": [], "ops": entry_ops})
    isolate_bootstrap_ops = [
        {"kind": "code_slots_init", "value": len(global_code_ids)},
        *version_ops,
        *module_code_ops,
        {"kind": "ret_void"},
    ]
    functions.append(
        {"name": "molt_isolate_bootstrap", "params": [], "ops": isolate_bootstrap_ops}
    )
    import_ops: list[dict[str, Any]] = []
    import_var_idx = 0

    def _import_var() -> str:
        nonlocal import_var_idx
        name = f"v{import_var_idx}"
        import_var_idx += 1
        return name

    name_var = "p0"
    module_var = _import_var()
    import_ops.append(
        {"kind": "module_cache_get", "args": [name_var], "out": module_var}
    )
    none_var = _import_var()
    import_ops.append({"kind": "const_none", "out": none_var})
    is_none_var = _import_var()
    import_ops.append(
        {"kind": "is", "args": [module_var, none_var], "out": is_none_var}
    )
    import_ops.append({"kind": "if", "args": [is_none_var]})
    if module_order:
        for idx, module_name in enumerate(module_order):
            match_name_var = _import_var()
            import_ops.append(
                {"kind": "const_str", "s_value": module_name, "out": match_name_var}
            )
            match_var = _import_var()
            import_ops.append(
                {
                    "kind": "string_eq",
                    "args": [name_var, match_name_var],
                    "out": match_var,
                }
            )
            import_ops.append({"kind": "if", "args": [match_var]})
            init_symbol = SimpleTIRGenerator.module_init_symbol(module_name)
            init_out = _import_var()
            import_ops.append(
                {
                    "kind": "call",
                    "s_value": init_symbol,
                    "args": [],
                    "out": init_out,
                    "value": _register_global_code_id(init_symbol),
                }
            )
            if idx < len(module_order) - 1:
                import_ops.append({"kind": "else"})
        import_ops.extend({"kind": "end_if"} for _ in module_order)
    import_ops.append({"kind": "end_if"})
    loaded_var = _import_var()
    import_ops.append(
        {"kind": "module_cache_get", "args": [name_var], "out": loaded_var}
    )
    import_ops.append({"kind": "ret", "args": [loaded_var]})
    functions.append(
        {"name": "molt_isolate_import", "params": ["p0"], "ops": import_ops}
    )
    ir = {"functions": functions}
    if pgo_profile_summary is not None:
        ir["profile"] = {
            "version": pgo_profile_summary.version,
            "hash": pgo_profile_summary.hash,
            "hot_functions": pgo_profile_summary.hot_functions,
        }
    if emit_ir_path is not None:
        try:
            emit_ir_path.write_text(
                json.dumps(ir, indent=2, default=_json_ir_default) + "\n"
            )
        except OSError as exc:
            return _fail(f"Failed to write IR: {exc}", json_output, command="build")
    runtime_lib: Path | None = None
    if is_wasm:
        runtime_wasm = molt_root / "wasm" / "molt_runtime.wasm"
        if not _ensure_runtime_wasm(
            runtime_wasm,
            reloc=False,
            json_output=json_output,
            profile=profile,
            cargo_timeout=cargo_timeout,
            project_root=molt_root,
        ):
            return _fail("Runtime wasm build failed", json_output, command="build")
    elif emit_mode == "bin":
        profile_dir = _cargo_profile_dir(profile)
        target_root = _cargo_target_root(molt_root)
        if target_triple:
            runtime_lib = (
                target_root / target_triple / profile_dir / "libmolt_runtime.a"
            )
        else:
            runtime_lib = target_root / profile_dir / "libmolt_runtime.a"
        if not _ensure_runtime_lib(
            runtime_lib,
            target_triple,
            json_output,
            profile,
            molt_root,
            cargo_timeout,
        ):
            return _fail("Runtime build failed", json_output, command="build")
    cache_hit = False
    cache_key = None
    cache_path: Path | None = None
    if cache:
        cache_variant = "linked" if linked else ""
        cache_key = _cache_key(ir, target, target_triple, cache_variant)
        cache_root = _resolve_cache_root(project_root, cache_dir)
        try:
            cache_root.mkdir(parents=True, exist_ok=True)
        except OSError as exc:
            warnings.append(f"Cache disabled: {exc}")
            cache = False
        else:
            ext = "wasm" if is_wasm else "o"
            cache_path = cache_root / f"{cache_key}.{ext}"
            if cache_path.exists():
                try:
                    shutil.copy2(cache_path, output_artifact)
                    cache_hit = True
                except OSError as exc:
                    warnings.append(f"Cache copy failed: {exc}")
                    cache_hit = False
    if (verbose or cache_report) and not json_output:
        if not cache:
            print("Cache: disabled")
        elif cache_key:
            cache_state = "hit" if cache_hit else "miss"
            cache_detail = f" ({cache_key})" if cache_key else ""
            print(f"Cache: {cache_state}{cache_detail}")

    # 2. Backend: JSON IR -> output.o / output.wasm
    if not cache_hit:
        backend_env = os.environ.copy() if is_wasm else None
        reloc_requested = is_wasm and (
            linked or os.environ.get("MOLT_WASM_LINK") == "1"
        )
        if is_wasm and backend_env is not None:
            if "MOLT_WASM_DATA_BASE" not in backend_env:
                runtime_wasm = molt_root / "wasm" / "molt_runtime.wasm"
                if not _ensure_runtime_wasm(
                    runtime_wasm,
                    reloc=False,
                    json_output=json_output,
                    profile=profile,
                    cargo_timeout=cargo_timeout,
                    project_root=molt_root,
                ):
                    return _fail(
                        "Runtime wasm build failed",
                        json_output,
                        command="build",
                    )
            if runtime_wasm.exists():
                data_end = _read_wasm_data_end(runtime_wasm)
                if data_end is not None:
                    aligned = (data_end + 7) & ~7
                    backend_env["MOLT_WASM_DATA_BASE"] = str(aligned)
                else:
                    warnings.append(
                        "Failed to read runtime data size; using default data base."
                    )
        if reloc_requested and backend_env is not None:
            backend_env["MOLT_WASM_LINK"] = "1"
            if "MOLT_WASM_TABLE_BASE" not in backend_env:
                runtime_reloc = molt_root / "wasm" / "molt_runtime_reloc.wasm"
                if linked and not _ensure_runtime_wasm(
                    runtime_reloc,
                    reloc=True,
                    json_output=json_output,
                    profile=profile,
                    cargo_timeout=cargo_timeout,
                    project_root=molt_root,
                ):
                    return _fail(
                        "Runtime wasm build failed",
                        json_output,
                        command="build",
                    )
                if runtime_reloc.exists():
                    table_base = _read_wasm_table_min(runtime_reloc)
                    if table_base is not None:
                        backend_env["MOLT_WASM_TABLE_BASE"] = str(table_base)
                    else:
                        warnings.append(
                            "Failed to read runtime table size; using default table base."
                        )
        backend_bin = _backend_bin_path(molt_root, backend_profile)
        if not _ensure_backend_binary(
            backend_bin,
            cargo_timeout=cargo_timeout,
            json_output=json_output,
            profile=backend_profile,
            project_root=molt_root,
        ):
            return _fail("Backend build failed", json_output, command="build")
        if not backend_bin.exists():
            return _fail("Backend binary missing", json_output, command="build")
        cmd = [str(backend_bin)]
        if is_wasm:
            cmd.extend(["--target", "wasm"])
        elif target_triple:
            cmd.extend(["--target-triple", target_triple])

        with tempfile.TemporaryDirectory(
            dir=artifacts_root, prefix="backend_"
        ) as backend_dir:
            backend_dir_path = Path(backend_dir)
            backend_output = backend_dir_path / (
                "output.wasm" if is_wasm else "output.o"
            )
            cmd_with_output = cmd + ["--output", str(backend_output)]
            try:
                backend_process = subprocess.run(
                    cmd_with_output,
                    input=json.dumps(ir, default=_json_ir_default),
                    text=True,
                    capture_output=True,
                    env=backend_env,
                    timeout=backend_timeout,
                )
            except subprocess.TimeoutExpired:
                return _fail(
                    "Backend compilation timed out",
                    json_output,
                    command="build",
                )
            if backend_process.returncode != 0:
                if not json_output:
                    if backend_process.stderr:
                        print(backend_process.stderr, end="", file=sys.stderr)
                    if backend_process.stdout:
                        print(backend_process.stdout, end="")
                return _fail(
                    "Backend compilation failed",
                    json_output,
                    backend_process.returncode or 1,
                    command="build",
                )
            if verbose and not json_output:
                if backend_process.stdout:
                    print(backend_process.stdout, end="")
                if backend_process.stderr:
                    print(backend_process.stderr, end="", file=sys.stderr)
            if not backend_output.exists():
                return _fail("Backend output missing", json_output, command="build")
            try:
                if output_artifact.parent != Path("."):
                    output_artifact.parent.mkdir(parents=True, exist_ok=True)
                backend_output.replace(output_artifact)
            except OSError as exc:
                if exc.errno != errno.EXDEV:
                    return _fail(
                        f"Failed to move backend output: {exc}",
                        json_output,
                        command="build",
                    )
                try:
                    shutil.copy2(backend_output, output_artifact)
                    backend_output.unlink()
                except OSError as copy_exc:
                    return _fail(
                        f"Failed to move backend output: {copy_exc}",
                        json_output,
                        command="build",
                    )
        if cache and cache_path is not None:
            try:
                shutil.copy2(output_artifact, cache_path)
            except OSError as exc:
                warnings.append(f"Cache write failed: {exc}")

    if is_wasm:
        output_wasm = output_artifact
        if linked:
            runtime_reloc = molt_root / "wasm" / "molt_runtime_reloc.wasm"
            if not _ensure_runtime_wasm(
                runtime_reloc,
                reloc=True,
                json_output=json_output,
                profile=profile,
                cargo_timeout=cargo_timeout,
                project_root=molt_root,
            ):
                return _fail(
                    "Runtime wasm build failed",
                    json_output,
                    command="build",
                )
            if linked_output_path is None:
                linked_output_path = output_wasm.with_name("output_linked.wasm")
            if linked_output_path.parent != Path("."):
                linked_output_path.parent.mkdir(parents=True, exist_ok=True)
            tool = molt_root / "tools" / "wasm_link.py"
            link_process = subprocess.run(
                [
                    sys.executable,
                    str(tool),
                    "--runtime",
                    str(runtime_reloc),
                    "--input",
                    str(output_wasm),
                    "--output",
                    str(linked_output_path),
                ],
                cwd=molt_root,
                capture_output=True,
                text=True,
            )
            if link_process.returncode != 0:
                err = link_process.stderr.strip() or link_process.stdout.strip()
                msg = "Wasm link failed"
                if err:
                    msg = f"{msg}: {err}"
                return _fail(msg, json_output, command="build")
            if require_linked and linked_output_path is not None:
                if output_wasm != linked_output_path and output_wasm.exists():
                    try:
                        output_wasm.unlink()
                    except OSError as exc:
                        return _fail(
                            f"Failed to remove unlinked wasm: {exc}",
                            json_output,
                            command="build",
                        )
        primary_output = output_wasm
        if require_linked and linked_output_path is not None:
            primary_output = linked_output_path
        if json_output:
            cache_info: dict[str, Any] = {"enabled": cache, "hit": cache_hit}
            if cache_key:
                cache_info["key"] = cache_key
            if cache_path is not None:
                cache_info["path"] = str(cache_path)
            data = {
                "target": target,
                "target_triple": target_triple,
                "entry": str(source_path),
                "output": str(primary_output),
                "deterministic": deterministic,
                "trusted": trusted,
                "capabilities": capabilities_list,
                "capability_profiles": capability_profiles,
                "capabilities_source": capabilities_source,
                "sysroot": str(sysroot_path) if sysroot_path is not None else None,
                "cache": cache_info,
                "emit": emit_mode,
                "profile": profile,
                "linked": linked,
                "require_linked": require_linked,
            }
            if pgo_profile_payload is not None:
                data["pgo_profile"] = pgo_profile_payload
            if linked_output_path is not None:
                data["linked_output"] = str(linked_output_path)
            if emit_ir_path is not None:
                data["emit_ir"] = str(emit_ir_path)
            payload = _json_payload(
                "build",
                "ok",
                data=data,
                warnings=warnings,
            )
            _emit_json(payload, json_output)
        else:
            if require_linked:
                print(f"Successfully built {primary_output}")
            else:
                print(f"Successfully built {output_wasm}")
            if linked_output_path is not None and not require_linked:
                print(f"Successfully linked {linked_output_path}")
        return 0

    output_obj = output_artifact
    if emit_mode == "obj":
        if json_output:
            cache_info = {"enabled": cache, "hit": cache_hit}
            if cache_key:
                cache_info["key"] = cache_key
            if cache_path is not None:
                cache_info["path"] = str(cache_path)
            data = {
                "target": target,
                "target_triple": target_triple,
                "entry": str(source_path),
                "output": str(output_obj),
                "deterministic": deterministic,
                "trusted": trusted,
                "capabilities": capabilities_list,
                "capability_profiles": capability_profiles,
                "capabilities_source": capabilities_source,
                "sysroot": str(sysroot_path) if sysroot_path is not None else None,
                "cache": cache_info,
                "emit": emit_mode,
                "profile": profile,
                "artifacts": {"object": str(output_obj)},
            }
            if pgo_profile_payload is not None:
                data["pgo_profile"] = pgo_profile_payload
            if emit_ir_path is not None:
                data["emit_ir"] = str(emit_ir_path)
            payload = _json_payload(
                "build",
                "ok",
                data=data,
                warnings=warnings,
            )
            _emit_json(payload, json_output)
        else:
            print(f"Successfully built {output_obj}")
        return 0

    # 3. Linking: output.o + main.c -> binary
    trusted_snippet = ""
    trusted_call = ""
    if trusted:
        trusted_snippet = """
static void molt_set_trusted() {
#ifdef _WIN32
    _putenv_s("MOLT_TRUSTED", "1");
#else
    setenv("MOLT_TRUSTED", "1", 1);
#endif
}
"""
        trusted_call = "    molt_set_trusted();\n"
    capabilities_snippet = ""
    capabilities_call = ""
    if capabilities_list is not None:
        caps_literal = json.dumps(",".join(capabilities_list))
        capabilities_snippet = f"""
static void molt_set_capabilities() {{
#ifdef _WIN32
    _putenv_s("MOLT_CAPABILITIES", {caps_literal});
#else
    setenv("MOLT_CAPABILITIES", {caps_literal}, 1);
#endif
}}
"""
        capabilities_call = "    molt_set_capabilities();\n"
    main_c_content = """
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifdef _WIN32
#include <wchar.h>
#endif
extern unsigned long long molt_runtime_init();
extern void molt_runtime_ensure_gil();
extern unsigned long long molt_runtime_shutdown();
extern void molt_set_argv(int argc, const char** argv);
#ifdef _WIN32
extern void molt_set_argv_utf16(int argc, const wchar_t** argv);
#endif
extern void molt_main();
extern unsigned long long molt_exception_pending();
extern unsigned long long molt_exception_last();
extern unsigned long long molt_raise(unsigned long long exc_bits);
extern void molt_dec_ref(unsigned long long bits);
extern int molt_json_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_msgpack_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern int molt_cbor_parse_scalar(const char* ptr, long len, unsigned long long* out);
extern long molt_get_attr_generic(void* obj, const char* attr, long len);
extern unsigned long long molt_alloc(long size);
extern long molt_block_on(void* task);
extern long molt_async_sleep(void* obj);
extern void molt_spawn(void* task);
extern void* molt_chan_new(unsigned long long capacity);
extern long molt_chan_send(void* chan, long val);
extern long molt_chan_recv(void* chan);
extern long molt_chan_try_send(void* chan, long val);
extern long molt_chan_try_recv(void* chan);
extern long molt_chan_send_blocking(void* chan, long val);
extern long molt_chan_recv_blocking(void* chan);
extern void molt_print_obj(unsigned long long val);
extern void molt_profile_dump();
/* MOLT_TRUSTED_SNIPPET */
/* MOLT_CAPABILITIES_SNIPPET */

static int molt_finish() {
    unsigned long long pending = molt_exception_pending();
    const char* debug_exc = getenv("MOLT_DEBUG_MAIN_EXCEPTION");
    if (debug_exc != NULL && debug_exc[0] != '\\0' && strcmp(debug_exc, "0") != 0) {
        fprintf(stderr, "molt main finish pending=%d\\n", pending != 0);
    }
    if (pending != 0) {
        unsigned long long exc = molt_exception_last();
        molt_raise(exc);
        molt_dec_ref(exc);
        molt_runtime_shutdown();
        return 1;
    }
    const char* profile = getenv("MOLT_PROFILE");
    if (profile != NULL && profile[0] != '\\0' && strcmp(profile, "0") != 0) {
        molt_profile_dump();
    }
    molt_runtime_shutdown();
    return 0;
}

#ifdef _WIN32
int wmain(int argc, wchar_t** argv) {
    /* MOLT_TRUSTED_CALL */
    /* MOLT_CAPABILITIES_CALL */
    molt_runtime_init();
    molt_runtime_ensure_gil();
    molt_set_argv_utf16(argc, (const wchar_t**)argv);
    molt_main();
    return molt_finish();
}
#else
int main(int argc, char** argv) {
    /* MOLT_TRUSTED_CALL */
    /* MOLT_CAPABILITIES_CALL */
    molt_runtime_init();
    molt_runtime_ensure_gil();
    molt_set_argv(argc, (const char**)argv);
    molt_main();
    return molt_finish();
}
#endif
"""
    main_c_content = main_c_content.replace(
        "/* MOLT_TRUSTED_SNIPPET */", trusted_snippet
    )
    main_c_content = main_c_content.replace(
        "/* MOLT_CAPABILITIES_SNIPPET */", capabilities_snippet
    )
    main_c_content = main_c_content.replace("/* MOLT_TRUSTED_CALL */", trusted_call)
    main_c_content = main_c_content.replace(
        "/* MOLT_CAPABILITIES_CALL */", capabilities_call
    )
    stub_path = artifacts_root / "main_stub.c"
    stub_path.write_text(main_c_content)

    if output_binary is None:
        return _fail("Binary output unavailable", json_output, command="build")
    if output_binary.parent != Path("."):
        output_binary.parent.mkdir(parents=True, exist_ok=True)
    if runtime_lib is None:
        profile_dir = _cargo_profile_dir(profile)
        target_root = _cargo_target_root(molt_root)
        if target_triple:
            runtime_lib = (
                target_root / target_triple / profile_dir / "libmolt_runtime.a"
            )
        else:
            runtime_lib = target_root / profile_dir / "libmolt_runtime.a"

    cc = os.environ.get("CC", "clang")
    link_cmd = shlex.split(cc)
    if target_triple:
        cross_cc = os.environ.get("MOLT_CROSS_CC")
        target_arg = target_triple
        if cross_cc:
            link_cmd = shlex.split(cross_cc)
        elif shutil.which("zig"):
            link_cmd = ["zig", "cc"]
            target_arg = _zig_target_query(target_triple)
            if target_arg != target_triple:
                warnings.append(
                    f"Zig target normalized to {target_arg} from {target_triple}."
                )
        else:
            return _fail(
                f"Cross-target build requires zig or MOLT_CROSS_CC (missing for {target_triple}).",
                json_output,
                command="build",
            )
        link_cmd.extend(["-target", target_arg])
    if sysroot_path is not None:
        sysroot_flag = "--sysroot"
        if link_cmd and Path(link_cmd[0]).name.startswith("zig"):
            sysroot_flag = "--sysroot"
        elif (
            target_triple and ("apple" in target_triple or "darwin" in target_triple)
        ) or (not target_triple and sys.platform == "darwin"):
            sysroot_flag = "-isysroot"
        link_cmd.extend([sysroot_flag, str(sysroot_path)])
    cflags = os.environ.get("CFLAGS", "")
    if cflags:
        link_cmd.extend(shlex.split(cflags))
    if sys.platform == "darwin" and not target_triple:
        link_cmd = _strip_arch_flags(link_cmd)
        arch = (
            os.environ.get("MOLT_ARCH")
            or _detect_macos_arch(output_obj)
            or platform.machine()
        )
        link_cmd.extend(["-arch", arch])
        deployment_target = _detect_macos_deployment_target()
        if deployment_target:
            link_cmd.append(f"-mmacosx-version-min={deployment_target}")
    link_cmd.extend(
        [str(stub_path), str(output_obj), str(runtime_lib), "-o", str(output_binary)]
    )
    if target_triple:
        if "apple" in target_triple or "darwin" in target_triple:
            link_cmd.append("-lc++")
        elif "linux" in target_triple:
            link_cmd.append("-lstdc++")
            link_cmd.append("-lm")
    else:
        if sys.platform == "darwin":
            link_cmd.append("-lc++")
        elif sys.platform.startswith("linux"):
            link_cmd.append("-lstdc++")
            link_cmd.append("-lm")

    try:
        link_process = subprocess.run(
            link_cmd,
            capture_output=json_output,
            text=True,
            timeout=link_timeout,
        )
    except subprocess.TimeoutExpired:
        return _fail("Linker timed out", json_output, command="build")

    if link_process.returncode == 0:
        if json_output:
            cache_info = {"enabled": cache, "hit": cache_hit}
            if cache_key:
                cache_info["key"] = cache_key
            if cache_path is not None:
                cache_info["path"] = str(cache_path)
            data: dict[str, Any] = {
                "target": target,
                "target_triple": target_triple,
                "entry": str(source_path),
                "output": str(output_binary),
                "artifacts": {
                    "object": str(output_obj),
                    "stub": str(stub_path),
                    "runtime": str(runtime_lib),
                },
                "deterministic": deterministic,
                "trusted": trusted,
                "capabilities": capabilities_list,
                "capability_profiles": capability_profiles,
                "capabilities_source": capabilities_source,
                "sysroot": str(sysroot_path) if sysroot_path is not None else None,
                "cache": cache_info,
                "emit": emit_mode,
                "profile": profile,
            }
            if pgo_profile_payload is not None:
                data["pgo_profile"] = pgo_profile_payload
            if emit_ir_path is not None:
                data["emit_ir"] = str(emit_ir_path)
            if link_process.stdout:
                data["stdout"] = link_process.stdout
            if link_process.stderr:
                data["stderr"] = link_process.stderr
            payload = _json_payload(
                "build",
                "ok",
                data=data,
                warnings=warnings,
            )
            _emit_json(payload, json_output)
        else:
            print(f"Successfully built {output_binary}")
    else:
        if json_output:
            data: dict[str, Any] = {
                "target": target,
                "entry": str(source_path),
                "returncode": link_process.returncode,
                "emit": emit_mode,
                "profile": profile,
                "trusted": trusted,
            }
            if pgo_profile_payload is not None:
                data["pgo_profile"] = pgo_profile_payload
            data["cache"] = {
                "enabled": cache,
                "hit": cache_hit,
                "key": cache_key,
            }
            if cache_path is not None:
                data["cache"]["path"] = str(cache_path)
            if link_process.stdout:
                data["stdout"] = link_process.stdout
            if link_process.stderr:
                data["stderr"] = link_process.stderr
            payload = _json_payload(
                "build",
                "error",
                data=data,
                errors=["Linking failed"],
            )
            _emit_json(payload, json_output)
        else:
            print("Linking failed", file=sys.stderr)

    return link_process.returncode


def run_script(
    file_path: str | None,
    module: str | None,
    script_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
    timing: bool = False,
    trusted: bool = False,
    capabilities: CapabilityInput | None = None,
    build_args: list[str] | None = None,
) -> int:
    if file_path and module:
        return _fail(
            "Use a file path or --module, not both.", json_output, command="run"
        )
    if not file_path and not module:
        return _fail("Missing entry file or module.", json_output, command="run")
    project_root = (
        _find_project_root(Path(file_path).resolve())
        if file_path
        else _find_project_root(Path.cwd())
    )
    molt_root = _find_molt_root(project_root, Path.cwd())
    source_path: Path | None = None
    entry_module_name: str | None = None
    if file_path:
        source_path = Path(file_path)
        if not source_path.exists():
            return _fail(f"File not found: {source_path}", json_output, command="run")
    elif module:
        cwd_root = _find_project_root(Path.cwd())
        module_roots = _resolve_module_roots(
            project_root,
            cwd_root,
            respect_pythonpath=_build_args_respect_pythonpath(build_args or []),
        )
        resolved = _resolve_entry_module(module, module_roots)
        if resolved is None:
            return _fail(
                f"Entry module not found: {module}",
                json_output,
                command="run",
            )
        entry_module_name, source_path = resolved
    env = _base_env(project_root, source_path, molt_root=molt_root)
    if file_path:
        env.update(_collect_env_overrides(file_path))
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    if capabilities is not None:
        parsed, _profiles, _source, errors = _parse_capabilities(capabilities)
        if errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(errors),
                json_output,
                command="run",
            )
        if parsed is not None:
            env["MOLT_CAPABILITIES"] = ",".join(parsed)

    build_args = list(build_args or [])
    capabilities_tmp: Path | None = None
    if trusted and not _build_args_has_trusted_flag(build_args):
        build_args.append("--trusted")
    if capabilities is not None and not _build_args_has_capabilities_flag(build_args):
        cap_arg, capabilities_tmp = _materialize_capabilities_arg(capabilities)
        build_args.extend(["--capabilities", cap_arg])
    build_cmd = [sys.executable, "-m", "molt.cli", "build", *build_args]
    if module:
        build_cmd.extend(["--module", module])
    else:
        build_cmd.append(file_path)
    try:
        if timing:
            build_res = _run_command_timed(
                build_cmd,
                env=env,
                cwd=project_root,
                verbose=verbose,
                capture_output=json_output,
            )
            if build_res.returncode != 0:
                if json_output:
                    data: dict[str, Any] = {
                        "returncode": build_res.returncode,
                        "timing": {"build_s": build_res.duration_s},
                    }
                    if build_res.stdout:
                        data["build_stdout"] = build_res.stdout
                    if build_res.stderr:
                        data["build_stderr"] = build_res.stderr
                    payload = _json_payload(
                        "run",
                        "error",
                        data=data,
                        errors=["build failed"],
                    )
                    _emit_json(payload, json_output=True)
                return build_res.returncode
        else:
            build_res = subprocess.run(
                build_cmd,
                env=env,
                cwd=project_root,
                capture_output=json_output,
                text=json_output,
            )
            if build_res.returncode != 0:
                if json_output:
                    data = {"returncode": build_res.returncode}
                    if build_res.stdout:
                        data["build_stdout"] = build_res.stdout
                    if build_res.stderr:
                        data["build_stderr"] = build_res.stderr
                    payload = _json_payload(
                        "run",
                        "error",
                        data=data,
                        errors=["build failed"],
                    )
                    _emit_json(payload, json_output=True)
                elif build_res.stdout:
                    print(build_res.stdout, end="")
                    if build_res.stderr:
                        print(build_res.stderr, end="", file=sys.stderr)
                return build_res.returncode
    finally:
        if capabilities_tmp is not None:
            try:
                capabilities_tmp.unlink()
            except OSError:
                pass
    emit_arg = _extract_emit_arg(build_args)
    if emit_arg and emit_arg != "bin":
        return _fail(
            f"Compiled run requires emit=bin (got {emit_arg})",
            json_output,
            command="run",
        )
    output_binary = _extract_output_arg(build_args)
    out_dir = _extract_out_dir_arg(build_args)
    out_dir_path = _resolve_out_dir(project_root, out_dir)
    if entry_module_name is None:
        cwd_root = _find_project_root(Path.cwd())
        module_roots = _resolve_module_roots(
            project_root,
            cwd_root,
            respect_pythonpath=_build_args_respect_pythonpath(build_args),
        )
        if source_path is not None:
            module_roots.append(source_path.parent.resolve())
            module_roots = list(dict.fromkeys(module_roots))
            entry_module_name = _module_name_from_path(
                source_path, module_roots, _stdlib_root_path()
            )
    if entry_module_name is None or source_path is None:
        return _fail("Failed to resolve entry module.", json_output, command="run")
    output_base = _output_base_for_entry(entry_module_name, source_path)
    _artifacts_root, bin_root, _output_root = _resolve_output_roots(
        project_root, out_dir_path, output_base
    )
    output_binary = _resolve_output_path(
        str(output_binary) if output_binary is not None else None,
        bin_root / f"{output_base}_molt",
        out_dir=out_dir_path,
        project_root=project_root,
    )
    if timing:
        run_res = _run_command_timed(
            [str(output_binary), *script_args],
            env=env,
            cwd=project_root,
            verbose=verbose,
            capture_output=json_output,
        )
        if json_output:
            data = {
                "returncode": run_res.returncode,
                "timing": {
                    "build_s": build_res.duration_s,
                    "run_s": run_res.duration_s,
                    "total_s": build_res.duration_s + run_res.duration_s,
                },
            }
            if run_res.stdout:
                data["stdout"] = run_res.stdout
            if run_res.stderr:
                data["stderr"] = run_res.stderr
            payload = _json_payload(
                "run",
                "ok" if run_res.returncode == 0 else "error",
                data=data,
            )
            _emit_json(payload, json_output=True)
        else:
            print("Timing (compiled):", file=sys.stderr)
            print(
                f"- build: {_format_duration(build_res.duration_s)}",
                file=sys.stderr,
            )
            print(
                f"- run: {_format_duration(run_res.duration_s)}",
                file=sys.stderr,
            )
            total = build_res.duration_s + run_res.duration_s
            print(f"- total: {_format_duration(total)}", file=sys.stderr)
        return run_res.returncode
    return _run_command(
        [str(output_binary), *script_args],
        env=env,
        cwd=project_root,
        json_output=json_output,
        verbose=verbose,
        label="run",
    )


def compare(
    file_path: str | None,
    module: str | None,
    python_exe: str | None,
    script_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
    trusted: bool = False,
    capabilities: CapabilityInput | None = None,
    build_args: list[str] | None = None,
    rebuild: bool = False,
) -> int:
    if file_path and module:
        return _fail(
            "Use a file path or --module, not both.",
            json_output,
            command="compare",
        )
    if not file_path and not module:
        return _fail("Missing entry file or module.", json_output, command="compare")
    source_path: Path | None = None
    if file_path:
        source_path = Path(file_path)
        if not source_path.exists():
            return _fail(
                f"File not found: {source_path}", json_output, command="compare"
            )
    project_root = (
        _find_project_root(Path(file_path).resolve())
        if file_path
        else _find_project_root(Path.cwd())
    )
    molt_root = _find_molt_root(project_root, Path.cwd())
    env = _base_env(project_root, source_path, molt_root=molt_root)
    if file_path:
        env.update(_collect_env_overrides(file_path))
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    if capabilities is not None:
        parsed, _profiles, _source, errors = _parse_capabilities(capabilities)
        if errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(errors),
                json_output,
                command="compare",
            )
        if parsed is not None:
            env["MOLT_CAPABILITIES"] = ",".join(parsed)

    python_exe = _resolve_python_exe(python_exe)
    if module:
        cpy_cmd = [python_exe, "-m", module, *script_args]
    else:
        cpy_cmd = [python_exe, str(source_path), *script_args]
    cpy_res = _run_command_timed(
        cpy_cmd,
        env=env,
        cwd=project_root,
        verbose=verbose,
        capture_output=True,
    )

    build_args = list(build_args or [])
    capabilities_tmp: Path | None = None
    if rebuild and not _build_args_has_cache_flag(build_args):
        build_args.append("--no-cache")
    if trusted and not _build_args_has_trusted_flag(build_args):
        build_args.append("--trusted")
    if capabilities is not None and not _build_args_has_capabilities_flag(build_args):
        cap_arg, capabilities_tmp = _materialize_capabilities_arg(capabilities)
        build_args.extend(["--capabilities", cap_arg])
    emit_arg = _extract_emit_arg(build_args)
    if emit_arg and emit_arg != "bin":
        return _fail(
            f"Compare requires emit=bin (got {emit_arg})",
            json_output,
            command="compare",
        )
    build_cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--json",
        *build_args,
    ]
    if module:
        build_cmd.extend(["--module", module])
    else:
        build_cmd.append(file_path)
    try:
        build_res = _run_command_timed(
            build_cmd,
            env=env,
            cwd=project_root,
            verbose=verbose,
            capture_output=True,
        )
    finally:
        if capabilities_tmp is not None:
            try:
                capabilities_tmp.unlink()
            except OSError:
                pass
    if build_res.returncode != 0:
        if json_output:
            data: dict[str, Any] = {
                "returncode": build_res.returncode,
                "timing": {"build_s": build_res.duration_s},
            }
            if build_res.stdout:
                data["build_stdout"] = build_res.stdout
            if build_res.stderr:
                data["build_stderr"] = build_res.stderr
            payload = _json_payload(
                "compare",
                "error",
                data=data,
                errors=["build failed"],
            )
            _emit_json(payload, json_output=True)
        else:
            err = build_res.stderr or build_res.stdout
            if err:
                print(err, end="", file=sys.stderr)
        return build_res.returncode

    try:
        build_payload = json.loads(build_res.stdout.strip() or "{}")
    except json.JSONDecodeError:
        return _fail(
            "Failed to parse build JSON output.", json_output, command="compare"
        )
    output_str = build_payload.get("data", {}).get("output") or build_payload.get(
        "output"
    )
    if not output_str:
        return _fail(
            "Build output missing in JSON payload.", json_output, command="compare"
        )
    output_path = _resolve_binary_output(output_str)
    if output_path is None:
        return _fail(
            f"Compiled binary not found at {output_str}.",
            json_output,
            command="compare",
        )

    molt_res = _run_command_timed(
        [str(output_path), *script_args],
        env=env,
        cwd=project_root,
        verbose=verbose,
        capture_output=True,
    )

    stdout_match = cpy_res.stdout == molt_res.stdout
    stderr_match = cpy_res.stderr == molt_res.stderr
    exit_match = cpy_res.returncode == molt_res.returncode
    compare_ok = stdout_match and stderr_match and exit_match

    if json_output:
        data = {
            "entry": str(source_path),
            "python": python_exe,
            "output": str(output_path),
            "returncodes": {
                "cpython": cpy_res.returncode,
                "molt": molt_res.returncode,
                "build": build_res.returncode,
            },
            "match": {
                "stdout": stdout_match,
                "stderr": stderr_match,
                "exitcode": exit_match,
            },
            "timing": {
                "cpython_run_s": cpy_res.duration_s,
                "molt_build_s": build_res.duration_s,
                "molt_run_s": molt_res.duration_s,
                "molt_total_s": build_res.duration_s + molt_res.duration_s,
            },
            "cpython_stdout": cpy_res.stdout,
            "cpython_stderr": cpy_res.stderr,
            "molt_stdout": molt_res.stdout,
            "molt_stderr": molt_res.stderr,
        }
        payload = _json_payload(
            "compare",
            "ok" if compare_ok else "error",
            data=data,
        )
        _emit_json(payload, json_output=True)
        return 0 if compare_ok else 1

    print("Compare (CPython vs Molt):")
    print(
        f"- CPython run: {_format_duration(cpy_res.duration_s)} "
        f"(rc={cpy_res.returncode})"
    )
    print(f"- Molt build: {_format_duration(build_res.duration_s)}")
    print(
        f"- Molt run: {_format_duration(molt_res.duration_s)} "
        f"(rc={molt_res.returncode})"
    )
    total = build_res.duration_s + molt_res.duration_s
    print(f"- Molt total: {_format_duration(total)}")
    if cpy_res.duration_s > 0 and molt_res.duration_s > 0:
        speedup = cpy_res.duration_s / molt_res.duration_s
        print(f"- Molt speedup (run): {speedup:.2f}x")
    print(
        "- Output match: "
        f"stdout={'yes' if stdout_match else 'no'}, "
        f"stderr={'yes' if stderr_match else 'no'}, "
        f"exitcode={'yes' if exit_match else 'no'}"
    )
    if not compare_ok:
        if not stdout_match:
            print(
                f"- Stdout mismatch: CPython={len(cpy_res.stdout)} bytes, "
                f"Molt={len(molt_res.stdout)} bytes"
            )
        if not stderr_match:
            print(
                f"- Stderr mismatch: CPython={len(cpy_res.stderr)} bytes, "
                f"Molt={len(molt_res.stderr)} bytes"
            )
        if not exit_match:
            print(
                f"- Exitcode mismatch: CPython={cpy_res.returncode}, "
                f"Molt={molt_res.returncode}"
            )
        if verbose:
            print("CPython stdout:")
            print(cpy_res.stdout, end="" if cpy_res.stdout.endswith("\n") else "\n")
            print("Molt stdout:")
            print(molt_res.stdout, end="" if molt_res.stdout.endswith("\n") else "\n")
            print("CPython stderr:", file=sys.stderr)
            print(
                cpy_res.stderr,
                end="" if cpy_res.stderr.endswith("\n") else "\n",
                file=sys.stderr,
            )
            print("Molt stderr:", file=sys.stderr)
            print(
                molt_res.stderr,
                end="" if molt_res.stderr.endswith("\n") else "\n",
                file=sys.stderr,
            )
    return 0 if compare_ok else 1


def diff(
    file_path: str | None,
    python_version: str | None,
    trusted: bool = False,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "diff")
    if root_error is not None:
        return root_error
    env = _base_env(root, molt_root=root)
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    cmd = [sys.executable, "tests/molt_diff.py"]
    if python_version:
        cmd.extend(["--python-version", python_version])
    if file_path:
        cmd.append(file_path)
    return _run_command(
        cmd,
        env=env,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="diff",
    )


def lint(json_output: bool = False, verbose: bool = False) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "lint")
    if root_error is not None:
        return root_error
    cmd = [sys.executable, "tools/dev.py", "lint"]
    return _run_command(
        cmd,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="lint",
    )


def test(
    suite: str,
    file_path: str | None,
    python_version: str | None,
    pytest_args: list[str],
    trusted: bool = False,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "test")
    if root_error is not None:
        return root_error
    env = _base_env(root, molt_root=root)
    if trusted:
        env["MOLT_TRUSTED"] = "1"
    if suite == "dev":
        cmd = [sys.executable, "tools/dev.py", "test"]
    elif suite == "diff":
        cmd = [sys.executable, "tests/molt_diff.py"]
        if python_version:
            cmd.extend(["--python-version", python_version])
        if file_path:
            cmd.append(file_path)
    else:
        cmd = ["uv", "run", "--python", "3.12", "pytest", "-q"]
        if file_path:
            cmd.append(file_path)
        cmd.extend(pytest_args)
    return _run_command(
        cmd,
        env=env,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="test",
    )


def bench(
    wasm: bool,
    bench_args: list[str],
    bench_script: list[str] | None = None,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "bench")
    if root_error is not None:
        return root_error
    tool = "tools/bench_wasm.py" if wasm else "tools/bench.py"
    cmd = [sys.executable, tool]
    for script in bench_script or []:
        cmd.extend(["--script", script])
    cmd.extend(bench_args)
    return _run_command(
        cmd,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="bench",
    )


def profile(
    profile_args: list[str],
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "profile")
    if root_error is not None:
        return root_error
    cmd = [sys.executable, "tools/profile.py", *profile_args]
    return _run_command(
        cmd,
        cwd=root,
        json_output=json_output,
        verbose=verbose,
        label="profile",
    )


def doctor(
    json_output: bool = False,
    verbose: bool = False,
    strict: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "doctor")
    if root_error is not None:
        return root_error
    checks: list[dict[str, Any]] = []
    warnings: list[str] = []
    errors: list[str] = []
    system = platform.system()

    def record(
        name: str,
        ok: bool,
        detail: str,
        *,
        level: Literal["warning", "error"] = "error",
        advice: list[str] | None = None,
    ) -> None:
        entry: dict[str, Any] = {"name": name, "ok": ok, "detail": detail}
        if not ok:
            entry["level"] = level
            if advice:
                entry["advice"] = advice
            message = f"{name}: {detail}"
            if advice:
                message = f"{message}. See advice."
            if level == "error":
                errors.append(message)
            else:
                warnings.append(message)
        checks.append(entry)

    def _python_advice() -> list[str]:
        if system == "Darwin":
            return ["brew install python@3.12", "Ensure python3 is on PATH"]
        if system == "Windows":
            return ["winget install Python.Python.3.12", "Reopen your terminal"]
        return ["Install Python 3.12+ via your package manager"]

    def _uv_advice() -> list[str]:
        if system == "Darwin":
            return ["brew install uv"]
        if system == "Windows":
            return ["winget install Astral.Uv", "or: scoop install uv"]
        return ["curl -LsSf https://astral.sh/uv/install.sh | sh"]

    def _rustup_advice() -> list[str]:
        if system == "Windows":
            return ["winget install Rustlang.Rustup", "Reopen your terminal"]
        return ["curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"]

    def _cargo_advice() -> list[str]:
        return _rustup_advice() + ["source $HOME/.cargo/env (Unix)"]

    def _clang_advice() -> list[str]:
        if system == "Darwin":
            return ["xcode-select --install"]
        if system == "Windows":
            return ["winget install LLVM.LLVM", "set CC=clang"]
        return ["sudo apt-get update", "sudo apt-get install -y clang lld"]

    python_ok = sys.version_info >= (3, 12)
    record(
        "python",
        python_ok,
        f"{sys.version.split()[0]} (requires >=3.12)",
        level="error",
        advice=_python_advice() if not python_ok else None,
    )

    uv_path = shutil.which("uv")
    record(
        "uv",
        bool(uv_path),
        uv_path or "not found",
        level="warning",
        advice=_uv_advice() if not uv_path else None,
    )

    cargo_path = shutil.which("cargo")
    record(
        "cargo",
        bool(cargo_path),
        cargo_path or "not found",
        level="error",
        advice=_cargo_advice() if not cargo_path else None,
    )

    rustup_path = shutil.which("rustup")
    record(
        "rustup",
        bool(rustup_path),
        rustup_path or "not found",
        level="warning",
        advice=_rustup_advice() if not rustup_path else None,
    )

    cc = os.environ.get("CC", "clang")
    cc_path = shutil.which(cc) or shutil.which("clang")
    record(
        "clang",
        bool(cc_path),
        cc_path or "not found",
        level="error",
        advice=_clang_advice() if not cc_path else None,
    )

    zig_path = shutil.which("zig")
    record(
        "zig",
        bool(zig_path),
        zig_path or "not found",
        level="warning",
        advice=["Install zig if you need wasm linking"] if not zig_path else None,
    )

    pyproject = root / "pyproject.toml"
    lock_path = root / "uv.lock"
    if pyproject.exists():
        record(
            "uv.lock",
            lock_path.exists(),
            str(lock_path),
            level="warning",
            advice=["uv sync", "or: uv lock"] if not lock_path.exists() else None,
        )
        if lock_path.exists():
            try:
                if lock_path.stat().st_mtime < pyproject.stat().st_mtime:
                    record(
                        "uv.lock_fresh",
                        False,
                        "uv.lock older than pyproject.toml",
                        level="warning",
                        advice=["uv lock", "or: uv sync"],
                    )
            except OSError:
                record(
                    "uv.lock_fresh",
                    False,
                    "unable to stat uv.lock",
                    level="warning",
                    advice=["Ensure uv.lock exists and is readable"],
                )

    runtime_lib = _cargo_target_root(root) / "release" / "libmolt_runtime.a"
    record(
        "molt-runtime",
        runtime_lib.exists(),
        str(runtime_lib),
        level="warning",
        advice=["cargo build --release --package molt-runtime"]
        if not runtime_lib.exists()
        else None,
    )

    if rustup_path:
        try:
            result = subprocess.run(
                ["rustup", "target", "list", "--installed"],
                capture_output=True,
                text=True,
                check=False,
            )
        except OSError as exc:
            record("rustup-targets", False, f"failed to query: {exc}")
        else:
            targets = result.stdout.split()
            wasm_ok = any(
                target in targets
                for target in ("wasm32-wasip1", "wasm32-unknown-unknown")
            )
            record(
                "wasm-target",
                wasm_ok,
                "wasm32-wasip1 or wasm32-unknown-unknown",
                level="warning",
                advice=["rustup target add wasm32-wasip1"] if not wasm_ok else None,
            )

    failures = [
        check
        for check in checks
        if not check["ok"] and check.get("level", "error") == "error"
    ]
    status = "ok" if not failures else "error"
    if json_output:
        payload = _json_payload(
            "doctor",
            status,
            data={"checks": checks},
            warnings=warnings,
            errors=errors,
        )
        _emit_json(payload, json_output=True)
    else:
        for check in checks:
            if check["ok"]:
                print(f"OK: {check['name']} ({check['detail']})")
                continue
            level = check.get("level", "error").upper()
            print(f"{level}: {check['name']} ({check['detail']})")
            for hint in check.get("advice", []):
                print(f"  -> {hint}")
    if strict and any(not check["ok"] for check in checks):
        return 1
    return 0


def _resolve_sidecar_path(output_path: Path, override: str | None, suffix: str) -> Path:
    if override:
        path = Path(override).expanduser()
        if not path.is_absolute():
            path = (output_path.parent / path).absolute()
        return path
    return output_path.with_name(output_path.stem + suffix)


def _is_remote_registry(registry: str) -> bool:
    scheme = urllib.parse.urlparse(registry).scheme.lower()
    return scheme in REMOTE_REGISTRY_SCHEMES


def _validate_registry_url(registry: str) -> str | None:
    parsed = urllib.parse.urlparse(registry)
    if parsed.scheme.lower() not in REMOTE_REGISTRY_SCHEMES:
        return f"Unsupported registry scheme: {parsed.scheme or 'none'}"
    if not parsed.netloc:
        return "Registry URL is missing a host"
    if parsed.username or parsed.password:
        return (
            "Registry URL must not include credentials "
            "(use --registry-token or --registry-user/--registry-password)"
        )
    return None


def _read_secret_value(
    value: str | None, *, env_name: str, label: str, use_env: bool = True
) -> tuple[str | None, str | None]:
    source = None
    if value is None and use_env:
        env_val = os.environ.get(env_name)
        if env_val is not None:
            value = env_val
            source = "env"
    else:
        source = "arg"
    if value is None:
        return None, None
    if value.startswith("@"):
        secret_path = Path(value[1:]).expanduser()
        if not secret_path.exists():
            raise RuntimeError(f"{label} file not found: {secret_path}")
        value = secret_path.read_text()
        source = "file"
    value = value.strip()
    if not value:
        raise RuntimeError(f"{label} is empty")
    return value, source


def _resolve_registry_auth(
    registry_token: str | None,
    registry_user: str | None,
    registry_password: str | None,
) -> tuple[dict[str, str], dict[str, str]]:
    explicit_token = registry_token is not None
    explicit_user = registry_user is not None or registry_password is not None
    if explicit_token and explicit_user:
        raise RuntimeError(
            "Use --registry-token or --registry-user/--registry-password, not both."
        )
    token: str | None = None
    token_source: str | None = None
    if explicit_token:
        token, token_source = _read_secret_value(
            registry_token,
            env_name="MOLT_REGISTRY_TOKEN",
            label="Registry token",
            use_env=False,
        )
    elif not explicit_user:
        token, token_source = _read_secret_value(
            None,
            env_name="MOLT_REGISTRY_TOKEN",
            label="Registry token",
            use_env=True,
        )
    user = None
    user_source = None
    password = None
    password_source = None
    if token is None:
        user = registry_user
        user_source = "arg" if registry_user is not None else None
        if user is None:
            env_user = os.environ.get("MOLT_REGISTRY_USER")
            if env_user is not None:
                user = env_user
                user_source = "env"
        password, password_source = _read_secret_value(
            registry_password,
            env_name="MOLT_REGISTRY_PASSWORD",
            label="Registry password",
            use_env=registry_password is None,
        )
    if user and not password:
        raise RuntimeError("Registry password is required when using --registry-user.")
    if password and not user:
        raise RuntimeError("Registry user is required when using --registry-password.")
    headers: dict[str, str] = {}
    auth_info = {"mode": "none", "source": "none"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
        auth_info["mode"] = "bearer"
        auth_info["source"] = token_source or "unknown"
    elif user:
        credential = f"{user}:{password}"
        encoded = base64.b64encode(credential.encode("utf-8")).decode("ascii")
        headers["Authorization"] = f"Basic {encoded}"
        auth_info["mode"] = "basic"
        sources = {
            source for source in (user_source, password_source) if source is not None
        }
        if len(sources) == 1:
            auth_info["source"] = sources.pop()
        elif len(sources) > 1:
            auth_info["source"] = "mixed"
        else:
            auth_info["source"] = "unknown"
    return headers, auth_info


def _resolve_registry_timeout(value: float | None) -> float:
    timeout = value
    if timeout is None:
        env_val = os.environ.get("MOLT_REGISTRY_TIMEOUT")
        if env_val:
            try:
                timeout = float(env_val)
            except ValueError as exc:
                raise RuntimeError(
                    f"Invalid MOLT_REGISTRY_TIMEOUT value: {env_val}"
                ) from exc
    if timeout is None:
        timeout = 30.0
    if timeout <= 0:
        raise RuntimeError("Registry timeout must be greater than zero.")
    return timeout


def _remote_registry_destination(registry_url: str, filename: str) -> str:
    parsed = urllib.parse.urlparse(registry_url)
    path = parsed.path or ""
    if not path or path.endswith("/"):
        base_path = path or "/"
        if not base_path.endswith("/"):
            base_path += "/"
        dest_path = posixpath.join(base_path, filename)
    else:
        dest_path = path
    return urllib.parse.urlunparse(parsed._replace(path=dest_path))


def _remote_sidecar_url(dest_url: str, suffix: str) -> str:
    parsed = urllib.parse.urlparse(dest_url)
    path = parsed.path
    if not path:
        raise RuntimeError("Remote destination URL is missing a path")
    dir_name, file_name = posixpath.split(path)
    stem = Path(file_name).stem
    sidecar_name = f"{stem}{suffix}"
    if dir_name and not dir_name.endswith("/"):
        sidecar_path = posixpath.join(dir_name, sidecar_name)
    elif dir_name:
        sidecar_path = f"{dir_name}{sidecar_name}"
    else:
        sidecar_path = f"/{sidecar_name}"
    return urllib.parse.urlunparse(parsed._replace(path=sidecar_path))


def _registry_content_type(path: Path) -> str:
    if path.suffix == ".moltpkg":
        return "application/zip"
    if path.suffix == ".json":
        return "application/json"
    return "application/octet-stream"


def _upload_registry_file(
    source: Path,
    dest_url: str,
    headers: dict[str, str],
    timeout: float,
) -> dict[str, Any]:
    parsed = urllib.parse.urlparse(dest_url)
    scheme = parsed.scheme.lower()
    host = parsed.hostname
    if not host:
        raise RuntimeError(f"Invalid registry URL: {dest_url}")
    if scheme not in REMOTE_REGISTRY_SCHEMES:
        raise RuntimeError(f"Unsupported registry scheme: {scheme}")
    port = parsed.port
    path = parsed.path or "/"
    if parsed.params:
        path = f"{path};{parsed.params}"
    if parsed.query:
        path = f"{path}?{parsed.query}"
    conn_cls: type[http.client.HTTPConnection]
    if scheme == "https":
        conn_cls = http.client.HTTPSConnection
    else:
        conn_cls = http.client.HTTPConnection
    content_length = source.stat().st_size
    upload_headers = {
        "Content-Type": _registry_content_type(source),
        "Content-Length": str(content_length),
        "User-Agent": f"molt/{_compiler_metadata()[0] or 'unknown'}",
        "X-Molt-Upload-Id": str(uuid.uuid4()),
    }
    upload_headers.update(headers)
    conn = conn_cls(host, port, timeout=timeout)
    try:
        conn.putrequest("PUT", path)
        for key, value in upload_headers.items():
            conn.putheader(key, value)
        conn.endheaders()
        with source.open("rb") as handle:
            while True:
                chunk = handle.read(1024 * 64)
                if not chunk:
                    break
                conn.send(chunk)
        response = conn.getresponse()
        body = response.read()
    finally:
        conn.close()
    status = response.status
    if status < 200 or status >= 300:
        detail = body.decode("utf-8", errors="replace").strip()
        if detail:
            detail = f" {detail}"
        raise RuntimeError(
            f"Registry upload failed ({status} {response.reason}).{detail}"
        )
    return {
        "status": status,
        "reason": response.reason,
        "bytes": content_length,
        "etag": response.getheader("ETag"),
    }


def package(
    artifact: str,
    manifest_path: str,
    output: str | None,
    json_output: bool = False,
    verbose: bool = False,
    deterministic: bool = True,
    deterministic_warn: bool = False,
    capabilities: CapabilityInput | None = None,
    sbom: bool = True,
    sbom_output: str | None = None,
    sbom_format: str = "cyclonedx",
    signature: str | None = None,
    signature_output: str | None = None,
    sign: bool = False,
    signer: str | None = None,
    signing_key: str | None = None,
    signing_identity: str | None = None,
) -> int:
    artifact_path = Path(artifact)
    if not artifact_path.exists():
        return _fail(
            f"Artifact not found: {artifact_path}",
            json_output,
            command="package",
        )
    manifest_file = Path(manifest_path)
    manifest = _load_manifest(manifest_file)
    if manifest is None:
        return _fail(
            f"Failed to load manifest: {manifest_file}",
            json_output,
            command="package",
        )
    errors = _manifest_errors(manifest)
    if errors:
        return _fail(
            "Manifest errors: " + ", ".join(errors),
            json_output,
            command="package",
        )
    if deterministic and manifest.get("deterministic") is not True:
        return _fail(
            "Manifest is not deterministic.",
            json_output,
            command="package",
        )

    warnings: list[str] = []
    project_root = _find_project_root(manifest_file.resolve())
    lock_error = _check_lockfiles(
        project_root,
        json_output,
        warnings,
        deterministic,
        deterministic_warn,
        "package",
    )
    if lock_error is not None:
        return lock_error
    capabilities_list = None
    capability_profiles: list[str] = []
    capability_manifest: CapabilityManifest | None = None
    if capabilities is not None:
        spec = _parse_capabilities_spec(capabilities)
        if spec.errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(spec.errors),
                json_output,
                command="package",
            )
        capabilities_list = spec.capabilities
        capability_profiles = spec.profiles
        capability_manifest = spec.manifest
    if capabilities_list is not None:
        required = manifest.get("capabilities", [])
        pkg_name = manifest.get("name")
        allowlist = _allowed_capabilities_for_package(
            capabilities_list, capability_manifest, pkg_name
        )
        missing = [cap for cap in required if cap not in allowlist]
        if missing:
            return _fail(
                "Capabilities missing from allowlist: " + ", ".join(missing),
                json_output,
                command="package",
            )
        required_effects = _normalize_effects(manifest.get("effects"))
        allowed_effects = _allowed_effects_for_package(capability_manifest, pkg_name)
        if allowed_effects is not None:
            missing_effects = [
                effect for effect in required_effects if effect not in allowed_effects
            ]
            if missing_effects:
                return _fail(
                    "Effects missing from allowlist: " + ", ".join(missing_effects),
                    json_output,
                    command="package",
                )

    if signature and sign:
        return _fail(
            "Use --signature or --sign, not both.",
            json_output,
            command="package",
        )
    if sign and manifest.get("deterministic") is True:
        warnings.append("Signing may introduce non-determinism in packaged outputs.")

    tlog_upload = os.environ.get("MOLT_COSIGN_TLOG", "").lower() in {"1", "true", "yes"}
    signer_meta: dict[str, Any] | None = None
    signer_selected: str | None = None
    if sign:
        try:
            signer_meta, signer_selected = _sign_artifact(
                artifact_path=artifact_path,
                sign=sign,
                signer=signer,
                signing_key=signing_key,
                signing_identity=signing_identity,
                tlog_upload=tlog_upload,
            )
        except RuntimeError as exc:
            return _fail(str(exc), json_output, command="package")

    checksum = _sha256_file(artifact_path)
    manifest = dict(manifest)
    manifest["checksum"] = checksum
    name = manifest.get("name", "molt_pkg")
    version = manifest.get("version", "0.0.0")
    target = manifest.get("target", "unknown")

    if output:
        output_path = Path(output)
    else:
        output_path = Path("dist") / f"{name}-{version}-{target}.moltpkg"
    output_path.parent.mkdir(parents=True, exist_ok=True)

    signature_source = Path(signature).expanduser() if signature else None
    signature_bytes: bytes | None = None
    signature_checksum: str | None = None
    signature_path: Path | None = None
    if signature_source is not None:
        if not signature_source.exists():
            return _fail(
                f"Signature not found: {signature_source}",
                json_output,
                command="package",
            )
        signature_bytes = signature_source.read_bytes()
        signature_checksum = hashlib.sha256(signature_bytes).hexdigest()
        signature_path = _resolve_sidecar_path(output_path, signature_output, ".sig")
    elif signer_meta is not None:
        sig_value = (
            signer_meta.get("signature", {}).get("value")
            if isinstance(signer_meta.get("signature"), dict)
            else None
        )
        if isinstance(sig_value, str) and sig_value:
            signature_bytes = sig_value.encode("utf-8")
            signature_checksum = hashlib.sha256(signature_bytes).hexdigest()
            signature_path = _resolve_sidecar_path(
                output_path, signature_output, ".sig"
            )

    sbom_bytes: bytes | None = None
    sbom_path: Path | None = None
    if sbom:
        project_root = _find_project_root(manifest_file.resolve())
        sbom_path = _resolve_sidecar_path(output_path, sbom_output, ".sbom.json")
        sbom_data, sbom_warnings = _build_sbom(
            manifest=manifest,
            artifact_path=artifact_path,
            checksum=checksum,
            project_root=project_root,
            format_name=sbom_format,
        )
        warnings.extend(sbom_warnings)
        sbom_bytes = (
            json.dumps(sbom_data, sort_keys=True, indent=2).encode("utf-8") + b"\n"
        )

    signature_meta_path = _resolve_sidecar_path(output_path, None, ".sig.json")
    signature_meta = _signature_metadata(
        artifact_path=artifact_path,
        checksum=checksum,
        signer_meta=signer_meta,
        signer=signer_selected,
        signature_name=signature_path.name if signature_path is not None else None,
        signature_checksum=signature_checksum,
    )
    signature_meta_bytes = (
        json.dumps(signature_meta, sort_keys=True, indent=2).encode("utf-8") + b"\n"
    )

    artifact_bytes = artifact_path.read_bytes()
    manifest_bytes = (
        json.dumps(manifest, sort_keys=True, indent=2).encode("utf-8") + b"\n"
    )
    with zipfile.ZipFile(output_path, "w") as zf:
        _write_zip_member(zf, "manifest.json", manifest_bytes)
        _write_zip_member(zf, f"artifact/{artifact_path.name}", artifact_bytes)
        if sbom_bytes is not None:
            _write_zip_member(zf, "sbom.json", sbom_bytes)
        _write_zip_member(zf, "signature.json", signature_meta_bytes)
        if signature_bytes is not None and signature_path is not None:
            _write_zip_member(zf, f"signature/{signature_path.name}", signature_bytes)

    if sbom_bytes is not None and sbom_path is not None:
        sbom_path.parent.mkdir(parents=True, exist_ok=True)
        sbom_path.write_bytes(sbom_bytes)
    signature_meta_path.parent.mkdir(parents=True, exist_ok=True)
    signature_meta_path.write_bytes(signature_meta_bytes)
    if signature_bytes is not None and signature_path is not None:
        signature_path.parent.mkdir(parents=True, exist_ok=True)
        signature_path.write_bytes(signature_bytes)

    if json_output:
        payload = _json_payload(
            "package",
            "ok",
            data={
                "output": str(output_path),
                "checksum": checksum,
                "deterministic": deterministic,
                "capabilities": capabilities_list,
                "capability_profiles": capability_profiles,
                "sbom": str(sbom_path) if sbom_path is not None else None,
                "sbom_format": sbom_format if sbom else None,
                "signature_metadata": str(signature_meta_path),
                "signature": str(signature_path)
                if signature_path is not None
                else None,
                "signed": signer_meta is not None or signature_path is not None,
                "signer": signer_selected,
            },
            warnings=warnings,
        )
        _emit_json(payload, json_output=True)
    else:
        print(f"Packaged {output_path}")
        if verbose:
            print(f"Checksum: {checksum}")
            if sbom_path is not None:
                print(f"SBOM: {sbom_path}")
            print(f"Signature metadata: {signature_meta_path}")
            if signature_path is not None:
                print(f"Signature: {signature_path}")
            if signer_meta is not None:
                print(f"Signed with: {signer_selected}")
            for warning in warnings:
                print(f"WARN: {warning}")
    return 0


def publish(
    package_path: str,
    registry: str,
    dry_run: bool,
    json_output: bool = False,
    verbose: bool = False,
    deterministic: bool = True,
    deterministic_warn: bool = False,
    capabilities: CapabilityInput | None = None,
    require_signature: bool = False,
    verify_signature: bool = False,
    trusted_signers: str | None = None,
    signer: str | None = None,
    signing_key: str | None = None,
    registry_token: str | None = None,
    registry_user: str | None = None,
    registry_password: str | None = None,
    registry_timeout: float | None = None,
) -> int:
    source = Path(package_path)
    if not source.exists():
        return _fail(
            f"Package not found: {source}",
            json_output,
            command="publish",
        )
    warnings: list[str] = []
    project_root = _find_project_root(source.resolve())
    lock_error = _check_lockfiles(
        project_root,
        json_output,
        warnings,
        deterministic,
        deterministic_warn,
        "publish",
    )
    if lock_error is not None:
        return lock_error
    if verify_signature:
        require_signature = True
    should_verify = (
        deterministic
        or require_signature
        or verify_signature
        or trusted_signers is not None
    )
    if should_verify:
        verify_code = verify(
            package_path,
            None,
            None,
            True,
            False,
            verbose,
            deterministic,
            capabilities,
            require_signature,
            verify_signature,
            trusted_signers,
            signer,
            signing_key,
        )
        if verify_code != 0:
            return verify_code
    is_remote = _is_remote_registry(registry)
    sidecars: list[dict[str, str]] = []
    uploads: list[dict[str, Any]] = []
    auth_info = {"mode": "none", "source": "none"}
    if is_remote:
        url_error = _validate_registry_url(registry)
        if url_error:
            return _fail(url_error, json_output, command="publish")
        try:
            headers, auth_info = _resolve_registry_auth(
                registry_token, registry_user, registry_password
            )
            timeout = _resolve_registry_timeout(registry_timeout)
        except RuntimeError as exc:
            return _fail(str(exc), json_output, command="publish")
        dest = _remote_registry_destination(registry, source.name)
        upload_plan: list[tuple[Path, str]] = [(source, dest)]
        for suffix in (".sbom.json", ".sig.json", ".sig"):
            sidecar_src = source.with_name(source.stem + suffix)
            if not sidecar_src.exists():
                continue
            sidecar_dest = _remote_sidecar_url(dest, suffix)
            sidecars.append({"source": str(sidecar_src), "dest": sidecar_dest})
            upload_plan.append((sidecar_src, sidecar_dest))
        if not dry_run:
            for upload_src, upload_dest in upload_plan:
                try:
                    result = _upload_registry_file(
                        upload_src, upload_dest, headers, timeout
                    )
                except RuntimeError as exc:
                    return _fail(str(exc), json_output, command="publish")
                uploads.append(
                    {
                        "source": str(upload_src),
                        "dest": upload_dest,
                        **result,
                    }
                )
    else:
        registry_path = Path(registry)
        if registry_path.exists() and registry_path.is_dir():
            dest = registry_path / source.name
        elif registry.endswith(os.sep):
            dest = registry_path / source.name
        else:
            dest = registry_path
        if not dry_run:
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, dest)
        for suffix in (".sbom.json", ".sig.json", ".sig"):
            sidecar_src = source.with_name(source.stem + suffix)
            if not sidecar_src.exists():
                continue
            sidecar_dest = dest.with_name(dest.stem + suffix)
            sidecars.append({"source": str(sidecar_src), "dest": str(sidecar_dest)})
            if not dry_run:
                sidecar_dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(sidecar_src, sidecar_dest)
    if json_output:
        payload = _json_payload(
            "publish",
            "ok",
            data={
                "source": str(source),
                "dest": str(dest),
                "dry_run": dry_run,
                "deterministic": deterministic,
                "sidecars": sidecars,
                "remote": is_remote,
                "auth": auth_info,
                "uploads": uploads,
            },
            warnings=warnings,
        )
        _emit_json(payload, json_output=True)
    else:
        action = "Would publish" if dry_run else "Published"
        print(f"{action} {source} -> {dest}")
        if sidecars and verbose:
            for entry in sidecars:
                print(f"{action} {entry['source']} -> {entry['dest']}")
        if verbose:
            registry_label = registry
            if is_remote:
                print(f"Registry: {registry_label} (remote)")
                print(f"Auth: {auth_info['mode']}")
            else:
                print(f"Registry: {registry_label}")
    return 0


def verify(
    package_path: str | None,
    manifest_path: str | None,
    artifact_path: str | None,
    require_checksum: bool,
    json_output: bool = False,
    verbose: bool = False,
    require_deterministic: bool = False,
    capabilities: CapabilityInput | None = None,
    require_signature: bool = False,
    verify_signature: bool = False,
    trusted_signers: str | None = None,
    signer: str | None = None,
    signing_key: str | None = None,
) -> int:
    errors: list[str] = []
    warnings: list[str] = []
    manifest: dict[str, Any] | None = None
    artifact_name = None
    artifact_bytes = None
    artifact_file: Path | None = None
    checksum: str | None = None
    capabilities_list = None
    capability_profiles: list[str] = []
    capability_manifest: CapabilityManifest | None = None
    signature_meta: dict[str, Any] | None = None
    signature_bytes: bytes | None = None
    signature_name: str | None = None
    trust_policy: TrustPolicy | None = None

    if capabilities is not None:
        spec = _parse_capabilities_spec(capabilities)
        if spec.errors:
            return _fail(
                "Invalid capabilities: " + ", ".join(spec.errors),
                json_output,
                command="verify",
            )
        capabilities_list = spec.capabilities
        capability_profiles = spec.profiles
        capability_manifest = spec.manifest
    if trusted_signers:
        try:
            trust_policy = _load_trust_policy(Path(trusted_signers))
        except (OSError, json.JSONDecodeError, tomllib.TOMLDecodeError) as exc:
            return _fail(
                f"Failed to load trust policy: {exc}",
                json_output,
                command="verify",
            )

    if package_path:
        pkg_path = Path(package_path)
        if not pkg_path.exists():
            return _fail(
                f"Package not found: {pkg_path}",
                json_output,
                command="verify",
            )
        try:
            with zipfile.ZipFile(pkg_path) as zf:
                try:
                    manifest_bytes = zf.read("manifest.json")
                except KeyError:
                    errors.append("manifest.json not found in package")
                else:
                    manifest = json.loads(manifest_bytes.decode("utf-8"))
                try:
                    sig_meta_bytes = zf.read("signature.json")
                except KeyError:
                    signature_meta = None
                else:
                    signature_meta = json.loads(sig_meta_bytes.decode("utf-8"))
                artifact_entries = [
                    name for name in zf.namelist() if name.startswith("artifact/")
                ]
                if len(artifact_entries) == 1:
                    artifact_name = artifact_entries[0]
                    artifact_bytes = zf.read(artifact_name)
                elif not artifact_entries:
                    errors.append("artifact/* not found in package")
                else:
                    errors.append("multiple artifact entries in package")
                signature_entries = [
                    name for name in zf.namelist() if name.startswith("signature/")
                ]
                if len(signature_entries) == 1:
                    signature_name = signature_entries[0].split("/", 1)[1]
                    signature_bytes = zf.read(signature_entries[0])
                elif len(signature_entries) > 1:
                    errors.append("multiple signature entries in package")
        except (OSError, zipfile.BadZipFile) as exc:
            return _fail(
                f"Failed to read package: {exc}",
                json_output,
                command="verify",
            )
        if signature_meta is None:
            sidecar = pkg_path.with_name(pkg_path.stem + ".sig.json")
            if sidecar.exists():
                signature_meta = json.loads(sidecar.read_text())
        if signature_bytes is None:
            sidecar_sig = pkg_path.with_name(pkg_path.stem + ".sig")
            if sidecar_sig.exists():
                signature_bytes = sidecar_sig.read_bytes()
                signature_name = sidecar_sig.name
    else:
        if not manifest_path or not artifact_path:
            return _fail(
                "Provide --package or both --manifest and --artifact.",
                json_output,
                command="verify",
            )
        manifest = _load_manifest(Path(manifest_path))
        if manifest is None:
            errors.append("failed to load manifest")
        artifact_file = Path(artifact_path)
        if not artifact_file.exists():
            errors.append("artifact not found")
        else:
            artifact_name = artifact_file.name
            artifact_bytes = artifact_file.read_bytes()
        sidecar = artifact_file.with_name(artifact_file.stem + ".sig.json")
        if sidecar.exists():
            signature_meta = json.loads(sidecar.read_text())
        sidecar_sig = artifact_file.with_name(artifact_file.stem + ".sig")
        if sidecar_sig.exists():
            signature_bytes = sidecar_sig.read_bytes()
            signature_name = sidecar_sig.name

    if manifest is not None:
        errors.extend(_manifest_errors(manifest))
        checksum = manifest.get("checksum")
        if checksum and artifact_bytes is not None:
            actual = hashlib.sha256(artifact_bytes).hexdigest()
            if actual != checksum:
                errors.append("checksum mismatch")
        elif require_checksum:
            errors.append("checksum missing")
        elif not checksum:
            warnings.append("checksum missing")
        if require_deterministic and manifest.get("deterministic") is not True:
            errors.append("manifest is not deterministic")
        required_caps = manifest.get("capabilities", [])
        if not isinstance(required_caps, list):
            required_caps = []
        required_effects = _normalize_effects(manifest.get("effects"))
        if capabilities_list is None and (required_caps or required_effects):
            errors.append(
                "capabilities allowlist required; pass --capabilities or set "
                "tool.molt.capabilities in config"
            )
        if capabilities_list is not None:
            pkg_name = manifest.get("name")
            allowlist = _allowed_capabilities_for_package(
                capabilities_list, capability_manifest, pkg_name
            )
            missing = [cap for cap in required_caps if cap not in allowlist]
            if missing:
                errors.append(
                    "capabilities missing from allowlist: " + ", ".join(missing)
                )
            allowed_effects = _allowed_effects_for_package(
                capability_manifest, pkg_name
            )
            if allowed_effects is not None:
                missing_effects = [
                    effect
                    for effect in required_effects
                    if effect not in allowed_effects
                ]
                if missing_effects:
                    errors.append(
                        "effects missing from allowlist: " + ", ".join(missing_effects)
                    )

    signature_status = None
    signer_meta: dict[str, Any] | None = None
    if signature_meta and isinstance(signature_meta, dict):
        signature_status = signature_meta.get("status")
        signer_meta_val = signature_meta.get("signer")
        if isinstance(signer_meta_val, dict):
            signer_meta = signer_meta_val
        artifact_meta = signature_meta.get("artifact")
        if isinstance(artifact_meta, dict):
            meta_sha = _normalize_sha256(artifact_meta.get("sha256"))
            if meta_sha and checksum:
                if _normalize_sha256(checksum) != meta_sha:
                    errors.append("signature metadata artifact checksum mismatch")
        signature_file = signature_meta.get("signature_file")
        if isinstance(signature_file, dict) and signature_bytes is not None:
            expected_sig = _normalize_sha256(signature_file.get("sha256"))
            actual_sig = hashlib.sha256(signature_bytes).hexdigest()
            if expected_sig and _normalize_sha256(actual_sig) != expected_sig:
                errors.append("signature file checksum mismatch")

    if verify_signature:
        require_signature = True

    signed = False
    if signature_status == "signed":
        signed = True
    elif signature_status == "unsigned":
        signed = False
    elif signature_name or signature_bytes or signer_meta is not None:
        signed = True

    if require_signature or trust_policy is not None:
        if not signed:
            errors.append("signature required but not present")

    trust_status = None
    if trust_policy is not None and signed:
        signer_name = None
        if signer_meta is not None:
            selected = signer_meta.get("selected")
            if isinstance(selected, str) and selected:
                signer_name = selected
            else:
                tool = signer_meta.get("tool")
                if isinstance(tool, dict):
                    name = tool.get("name")
                    if isinstance(name, str) and name:
                        signer_name = name
        allowed, reason = _trust_policy_allows(signer_name, signer_meta, trust_policy)
        trust_status = "trusted" if allowed else "untrusted"
        if not allowed:
            errors.append(f"signature trust policy failed: {reason}")

    signature_verified = None
    if verify_signature and signed and artifact_bytes is not None:
        key = signing_key or os.environ.get("COSIGN_KEY")
        with tempfile.TemporaryDirectory(prefix="molt_verify_") as tmpdir:
            temp_dir = Path(tmpdir)
            artifact_path = artifact_file
            if artifact_path is None:
                filename = Path(artifact_name).name if artifact_name else "artifact.bin"
                artifact_path = temp_dir / filename
                artifact_path.write_bytes(artifact_bytes)
            tool = _resolve_signature_tool(
                signer, signer_meta, artifact_path, signature_bytes
            )
            try:
                if tool == "cosign":
                    if signature_bytes is None:
                        raise RuntimeError("cosign signature file is missing")
                    if not key:
                        raise RuntimeError(
                            "cosign verification requires --signing-key or COSIGN_KEY"
                        )
                    _verify_cosign_signature(artifact_path, signature_bytes, key)
                elif tool == "codesign":
                    if not _is_macho(artifact_path):
                        raise RuntimeError(
                            "codesign verification requires a Mach-O artifact"
                        )
                    _verify_codesign_signature(artifact_path)
                else:
                    raise RuntimeError(
                        "unable to resolve signing tool for verification"
                    )
            except RuntimeError as exc:
                signature_verified = False
                errors.append(str(exc))
            else:
                signature_verified = True

    status = "ok" if not errors else "error"
    if json_output:
        payload = _json_payload(
            "verify",
            status,
            data={
                "artifact": artifact_name,
                "deterministic": require_deterministic,
                "capability_profiles": capability_profiles,
                "signature_status": signature_status
                or ("signed" if signed else "unsigned"),
                "signature_verified": signature_verified,
                "trust_status": trust_status,
            },
            warnings=warnings,
            errors=errors,
        )
        _emit_json(payload, json_output=True)
    else:
        for err in errors:
            print(f"ERROR: {err}")
        for warn in warnings:
            print(f"WARN: {warn}")
        if not errors and verbose:
            print("Verification passed")
    return 0 if not errors else 1


def _summarize_tiers(rows: list[dict[str, Any]]) -> dict[str, int]:
    summary: dict[str, int] = {"Tier A": 0, "Tier B": 0, "Tier C": 0}
    for row in rows:
        tier = row.get("tier")
        if tier in summary:
            summary[tier] += 1
    return summary


def _git_ref_from_source(source: dict[str, Any]) -> tuple[str | None, str | None]:
    for key in ("rev", "revision", "commit", "reference"):
        value = source.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip(), key
    for key in ("tag", "branch"):
        value = source.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip(), key
    return None, None


def _resolve_git_ref(url: str, ref: str) -> tuple[str | None, str | None]:
    try:
        result = subprocess.run(
            ["git", "ls-remote", url, ref],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        return None, f"Failed to resolve git ref {ref}: {exc}"
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip() or "unknown error"
        return None, f"Failed to resolve git ref {ref}: {detail}"
    line = result.stdout.strip().splitlines()[0] if result.stdout.strip() else ""
    if not line:
        return None, f"Failed to resolve git ref {ref}: empty response"
    commit = line.split()[0]
    if not commit:
        return None, f"Failed to resolve git ref {ref}: empty commit"
    return commit, None


def _clone_git_source(
    url: str,
    ref: str,
    dest: Path,
    *,
    subdirectory: str | None = None,
) -> tuple[str, str]:
    tmp_root = dest.parent
    with tempfile.TemporaryDirectory(dir=tmp_root, prefix="git_vendor_") as tmpdir:
        repo_dir = Path(tmpdir) / "repo"
        try:
            clone = subprocess.run(
                [
                    "git",
                    "clone",
                    "--filter=blob:none",
                    "--no-checkout",
                    url,
                    str(repo_dir),
                ],
                capture_output=True,
                text=True,
                check=False,
            )
        except OSError as exc:
            raise RuntimeError(f"Failed to clone git repo {url}: {exc}") from exc
        if clone.returncode != 0:
            detail = (clone.stderr or clone.stdout).strip() or "unknown error"
            raise RuntimeError(f"Failed to clone git repo {url}: {detail}")
        fetch = subprocess.run(
            ["git", "-C", str(repo_dir), "fetch", "--depth", "1", "origin", ref],
            capture_output=True,
            text=True,
            check=False,
        )
        if fetch.returncode != 0:
            detail = (fetch.stderr or fetch.stdout).strip() or "unknown error"
            raise RuntimeError(f"Failed to fetch git ref {ref}: {detail}")
        checkout = subprocess.run(
            ["git", "-C", str(repo_dir), "checkout", "--detach", ref],
            capture_output=True,
            text=True,
            check=False,
        )
        if checkout.returncode != 0:
            detail = (checkout.stderr or checkout.stdout).strip() or "unknown error"
            raise RuntimeError(f"Failed to checkout git ref {ref}: {detail}")
        rev = subprocess.run(
            ["git", "-C", str(repo_dir), "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            check=False,
        )
        if rev.returncode != 0 or not rev.stdout.strip():
            detail = (rev.stderr or rev.stdout).strip() or "unknown error"
            raise RuntimeError(f"Failed to resolve git revision for {ref}: {detail}")
        resolved_commit = rev.stdout.strip()
        tree = subprocess.run(
            ["git", "-C", str(repo_dir), "rev-parse", "HEAD^{tree}"],
            capture_output=True,
            text=True,
            check=False,
        )
        if tree.returncode != 0 or not tree.stdout.strip():
            detail = (tree.stderr or tree.stdout).strip() or "unknown error"
            raise RuntimeError(f"Failed to resolve git tree hash: {detail}")
        tree_hash = tree.stdout.strip()
        source_dir = repo_dir
        if subdirectory:
            source_dir = repo_dir / subdirectory
            if not source_dir.exists():
                raise RuntimeError(f"Git subdirectory not found: {subdirectory}")
        if dest.exists():
            shutil.rmtree(dest)
        if source_dir.is_dir():
            shutil.copytree(source_dir, dest, ignore=shutil.ignore_patterns(".git"))
        else:
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source_dir, dest)
        return resolved_commit, tree_hash


def deps(include_dev: bool, json_output: bool = False, verbose: bool = False) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "deps")
    if root_error is not None:
        return root_error
    pyproject = _load_toml(root / "pyproject.toml")
    lock = _load_toml(root / "uv.lock")
    deps = _collect_deps(pyproject, include_dev=include_dev)
    packages = _lock_packages(lock)
    allow = _dep_allowlists(pyproject)

    rows: list[dict[str, Any]] = []
    for dep in deps:
        key = _normalize_name(dep)
        pkg = packages.get(key)
        version = pkg.get("version") if pkg else None
        tier, reason = _classify_tier(dep, pkg, allow)
        rows.append({"name": dep, "version": version, "tier": tier, "reason": reason})

    if json_output:
        data: dict[str, Any] = {"dependencies": rows}
        if verbose:
            data["summary"] = _summarize_tiers(rows)
        payload = _json_payload("deps", "ok", data=data)
        _emit_json(payload, json_output)
        return 0

    for row in rows:
        version = row["version"] or "missing"
        print(f"{row['name']} {version} {row['tier']} {row['reason']}")
    if verbose:
        summary = _summarize_tiers(rows)
        print(
            "Summary: "
            + ", ".join(f"{tier}={count}" for tier, count in summary.items())
        )
    return 0


def vendor(
    include_dev: bool,
    json_output: bool = False,
    verbose: bool = False,
    output: str | None = None,
    dry_run: bool = False,
    allow_non_tier_a: bool = False,
    extras: list[str] | None = None,
    deterministic: bool = True,
    deterministic_warn: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "vendor")
    if root_error is not None:
        return root_error
    warnings: list[str] = []
    lock_error = _check_lockfiles(
        root,
        json_output,
        warnings,
        deterministic,
        deterministic_warn,
        "vendor",
    )
    if lock_error is not None:
        return lock_error
    pyproject = _load_toml(root / "pyproject.toml")
    lock = _load_toml(root / "uv.lock")
    extras_set: set[str] = set()
    for extra in extras or []:
        for token in re.split(r"[,\s]+", extra):
            if token:
                extras_set.add(token)
    deps, root_extras, skipped_root = _collect_dep_specs(
        pyproject,
        include_dev=include_dev,
        extras=extras_set,
    )
    env = _marker_environment()
    packages, deps_graph, skipped = _lock_package_graph(
        lock,
        env=env,
        selected_extras=root_extras,
    )
    allow = _dep_allowlists(pyproject)

    root_names = deps
    closure, missing = _resolve_dependency_closure(root_names, deps_graph)
    vendor_list: list[dict[str, Any]] = []
    blockers: list[dict[str, Any]] = []
    for name in closure:
        pkg = packages.get(name)
        display = pkg.get("name", name) if pkg else name
        tier, reason = _classify_tier(display, pkg, allow)
        version = pkg.get("version") if pkg else None
        entry = {
            "name": display,
            "version": version,
            "tier": tier,
            "reason": reason,
        }
        if tier == "Tier A":
            vendor_list.append(entry)
        else:
            blockers.append(entry)

    if missing:
        blockers.append(
            {
                "name": ",".join(missing),
                "version": None,
                "tier": "Unknown",
                "reason": "missing from uv.lock",
            }
        )

    if blockers and not allow_non_tier_a:
        if json_output:
            payload = _json_payload(
                "vendor",
                "error",
                data={
                    "vendor": vendor_list,
                    "blockers": blockers,
                    "missing": missing,
                    "extras": sorted(extras_set),
                    "skipped": skipped,
                    "skipped_root": skipped_root,
                },
                errors=["vendoring blocked by non-Tier A dependencies"],
                warnings=warnings,
            )
            _emit_json(payload, json_output=True)
            return 2
        print("Vendoring blocked by non-Tier A dependencies:")
        for entry in blockers:
            version = entry["version"] or "missing"
            print(f"- {entry['name']} {version} {entry['tier']} {entry['reason']}")
        return 2

    output_dir = Path(output) if output else Path("vendor")
    package_dir = output_dir / "packages"
    local_dir = output_dir / "local"
    manifest: dict[str, Any] = {
        "created_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "root": str(root),
        "include_dev": include_dev,
        "extras": sorted(extras_set),
        "packages": [],
        "blockers": blockers,
        "missing": missing,
        "skipped": skipped,
        "skipped_root": skipped_root,
    }

    if not dry_run:
        package_dir.mkdir(parents=True, exist_ok=True)
        local_dir.mkdir(parents=True, exist_ok=True)

    for entry in vendor_list:
        pkg = packages.get(_normalize_name(entry["name"]))
        if not pkg:
            continue
        source = pkg.get("source", {})
        if source.get("path"):
            src_path = Path(source["path"])
            if not src_path.is_absolute():
                src_path = (root / src_path).resolve()
            dest = local_dir / entry["name"]
            if not dry_run:
                if dest.exists():
                    shutil.rmtree(dest)
                if src_path.is_dir():
                    shutil.copytree(src_path, dest)
                else:
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(src_path, dest)
            manifest["packages"].append(
                {
                    **entry,
                    "source": "path",
                    "path": str(src_path),
                }
            )
            continue
        if source.get("git"):
            url = source.get("git")
            if not isinstance(url, str) or not url.strip():
                blockers.append(
                    {**entry, "tier": "Tier A", "reason": "git source missing url"}
                )
                continue
            if shutil.which("git") is None:
                return _fail(
                    "git is required to vendor git sources",
                    json_output,
                    command="vendor",
                )
            ref, ref_kind = _git_ref_from_source(source)
            if ref is None:
                blockers.append(
                    {
                        **entry,
                        "tier": "Tier A",
                        "reason": "git source missing pinned revision",
                    }
                )
                continue
            resolved_ref = ref
            resolved_error = None
            if ref_kind in {"tag", "branch"}:
                resolved_ref, resolved_error = _resolve_git_ref(url, ref)
            if resolved_error:
                return _fail(
                    resolved_error,
                    json_output,
                    command="vendor",
                )
            subdir = source.get("subdirectory") or source.get("subdir")
            if subdir is not None and not isinstance(subdir, str):
                blockers.append(
                    {
                        **entry,
                        "tier": "Tier A",
                        "reason": "git source subdirectory must be a string",
                    }
                )
                continue
            dest = local_dir / entry["name"]
            resolved_commit = resolved_ref
            tree_hash = None
            if not dry_run:
                try:
                    resolved_commit, tree_hash = _clone_git_source(
                        url, resolved_ref, dest, subdirectory=subdir
                    )
                except RuntimeError as exc:
                    return _fail(
                        str(exc),
                        json_output,
                        command="vendor",
                    )
            manifest["packages"].append(
                {
                    **entry,
                    "source": "git",
                    "git": url,
                    "ref": ref,
                    "ref_kind": ref_kind,
                    "resolved": resolved_commit,
                    "tree": tree_hash,
                    "subdirectory": subdir,
                    "path": str(dest),
                }
            )
            continue
        picked = _pick_vendor_artifact(pkg)
        if picked is None:
            blockers.append(
                {**entry, "tier": "Tier A", "reason": "no artifact in uv.lock"}
            )
            continue
        kind, artifact = picked
        url = artifact.get("url", "")
        hash_value = artifact.get("hash", "")
        filename = Path(url).name if url else f"{entry['name']}-{entry['version']}"
        dest = package_dir / filename
        if not dry_run:
            try:
                data = _download_artifact(url, hash_value)
            except Exception as exc:
                return _fail(
                    f"Failed to download {url}: {exc}",
                    json_output,
                    command="vendor",
                )
            dest.write_bytes(data)
        manifest["packages"].append(
            {
                **entry,
                "source": kind,
                "url": url,
                "hash": hash_value,
                "file": str(dest),
            }
        )

    if not dry_run:
        manifest_path = output_dir / "manifest.json"
        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")

    if json_output:
        data: dict[str, Any] = {
            "vendor": vendor_list,
            "blockers": blockers,
            "missing": missing,
            "output": str(output_dir),
            "dry_run": dry_run,
            "extras": sorted(extras_set),
            "skipped": skipped,
            "skipped_root": skipped_root,
            "deterministic": deterministic,
        }
        if verbose:
            data["count"] = len(vendor_list)
        payload = _json_payload("vendor", "ok", data=data, warnings=warnings)
        _emit_json(payload, json_output=True)
        return 0

    banner = "Vendoring plan (Tier A)" if dry_run else "Vendoring Tier A packages"
    print(f"{banner}:")
    for entry in vendor_list:
        version = entry["version"] or "missing"
        print(f"- {entry['name']} {version}")
    if blockers:
        print("Blockers:")
        for entry in blockers:
            version = entry["version"] or "missing"
            print(f"- {entry['name']} {version} {entry['tier']} {entry['reason']}")
    if verbose:
        print(f"Total Tier A packages: {len(vendor_list)}")
        print(f"Output: {output_dir}")
    return 0


def clean(
    json_output: bool = False,
    verbose: bool = False,
    cache: bool = True,
    artifacts: bool = True,
    bins: bool = False,
    repo_artifacts: bool = False,
    cargo_target: bool = False,
    clean_all: bool = False,
    include_venvs: bool = False,
) -> int:
    root = _find_molt_root(Path.cwd())
    root_error = _require_molt_root(root, json_output, "clean")
    if root_error is not None:
        return root_error
    removed: list[str] = []
    missing: list[str] = []
    failures: list[str] = []

    if clean_all:
        cache = True
        artifacts = True
        bins = True
        repo_artifacts = True
        cargo_target = True
        include_venvs = True

    def _remove_path(path: Path) -> None:
        try:
            if path.is_symlink():
                path.unlink()
                removed.append(str(path))
                return
            if path.exists():
                if path.is_dir():
                    shutil.rmtree(path)
                else:
                    path.unlink()
                removed.append(str(path))
            else:
                missing.append(str(path))
        except OSError as exc:
            failures.append(f"{path}: {exc}")

    def _is_virtualenv_path(path: Path) -> bool:
        for part in path.parts:
            if part in {"venv", ".env", "env"}:
                return True
            if part.startswith(".venv"):
                return True
        return False

    def _iter_pycache_dirs(root_dir: Path) -> list[Path]:
        pycache_dirs: list[Path] = []
        for dirpath, dirnames, _filenames in os.walk(root_dir, followlinks=False):
            current = Path(dirpath)
            if not include_venvs and _is_virtualenv_path(current):
                dirnames[:] = []
                continue
            pruned: list[str] = []
            for name in dirnames:
                candidate = Path(dirpath, name)
                if candidate.is_symlink():
                    continue
                pruned.append(name)
            dirnames[:] = pruned
            if current.name == "__pycache__":
                pycache_dirs.append(current)
                dirnames[:] = []
        return pycache_dirs

    if cache:
        cache_root = _default_molt_cache()
        _remove_path(cache_root)
    if artifacts:
        build_root = _default_molt_home() / "build"
        _remove_path(build_root)
    if bins:
        bin_root = _default_molt_bin()
        _remove_path(bin_root)
    if repo_artifacts:
        repo_dirs = [
            root / "vendor",
            root / "logs",
            root / "dist",
            root / "build",
            root / ".pytest_cache",
            root / ".ruff_cache",
            root / ".mypy_cache",
            root / "__pycache__",
        ]
        for path in repo_dirs:
            _remove_path(path)
        for path in _iter_pycache_dirs(root):
            _remove_path(path)
        repo_files = [
            root / "output.wasm",
            root / "output_linked.wasm",
            root / "output.o",
            root / "main_stub.c",
        ]
        for path in repo_files:
            _remove_path(path)
    if cargo_target:
        cargo_root = root / "target"
        _remove_path(cargo_root)
    if json_output:
        data: dict[str, Any] = {"removed": removed}
        if verbose:
            data["missing"] = missing
        status = "error" if failures else "ok"
        payload = _json_payload(
            "clean",
            status,
            data=data,
            errors=failures if failures else None,
        )
        _emit_json(payload, json_output=True)
    else:
        if removed:
            print("Removed:")
            for path in removed:
                print(f"- {path}")
        if failures:
            print("Failed:")
            for entry in failures:
                print(f"- {entry}")
        if verbose and missing:
            print("Missing:")
            for path in missing:
                print(f"- {path}")
    return 1 if failures else 0


def show_config(
    config_root: Path,
    config: dict[str, Any],
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    molt_toml = config_root / "molt.toml"
    pyproject = config_root / "pyproject.toml"
    build_cfg = _resolve_build_config(config)
    run_cfg = _resolve_command_config(config, "run")
    compare_cfg = _resolve_command_config(config, "compare")
    test_cfg = _resolve_command_config(config, "test")
    diff_cfg = _resolve_command_config(config, "diff")
    publish_cfg = _resolve_command_config(config, "publish")
    publish_cfg = _resolve_command_config(config, "publish")
    caps_cfg = _resolve_capabilities_config(config)
    data: dict[str, Any] = {
        "root": str(config_root),
        "sources": {
            "molt_toml": str(molt_toml) if molt_toml.exists() else None,
            "pyproject": str(pyproject) if pyproject.exists() else None,
        },
        "build": build_cfg,
        "run": run_cfg,
        "compare": compare_cfg,
        "test": test_cfg,
        "diff": diff_cfg,
        "publish": publish_cfg,
        "capabilities": caps_cfg,
        "paths": {
            "molt_home": str(_default_molt_home()),
            "molt_bin": str(_default_molt_bin()),
            "molt_cache": str(_default_molt_cache()),
            "build_root": str(_default_molt_home() / "build"),
        },
    }
    if json_output:
        data["config"] = config
        payload = _json_payload("config", "ok", data=data)
        _emit_json(payload, json_output=True)
        return 0
    print(f"Config root: {config_root}")
    if data["sources"]["molt_toml"] or data["sources"]["pyproject"]:
        print("Sources:")
        if data["sources"]["molt_toml"]:
            print(f"- {data['sources']['molt_toml']}")
        if data["sources"]["pyproject"]:
            print(f"- {data['sources']['pyproject']}")
    print("Paths:")
    for key, value in data["paths"].items():
        print(f"- {key}: {value}")
    if build_cfg:
        print("Build defaults:")
        for key in sorted(build_cfg):
            print(f"- {key}: {build_cfg[key]}")
    else:
        print("Build defaults: none")
    if run_cfg:
        print("Run defaults:")
        for key in sorted(run_cfg):
            print(f"- {key}: {run_cfg[key]}")
    else:
        print("Run defaults: none")
    if compare_cfg:
        print("Compare defaults:")
        for key in sorted(compare_cfg):
            print(f"- {key}: {compare_cfg[key]}")
    else:
        print("Compare defaults: none")
    if test_cfg:
        print("Test defaults:")
        for key in sorted(test_cfg):
            print(f"- {key}: {test_cfg[key]}")
    else:
        print("Test defaults: none")
    if diff_cfg:
        print("Diff defaults:")
        for key in sorted(diff_cfg):
            print(f"- {key}: {diff_cfg[key]}")
    else:
        print("Diff defaults: none")
    if publish_cfg:
        print("Publish defaults:")
        for key in sorted(publish_cfg):
            print(f"- {key}: {publish_cfg[key]}")
    else:
        print("Publish defaults: none")
    if caps_cfg is not None:
        print(f"Capabilities: {_format_capabilities_input(caps_cfg)}")
    else:
        print("Capabilities: none")
    if verbose:
        print("Merged config:")
        print(json.dumps(config, indent=2))
    return 0


def _completion_script(shell: str) -> str:
    commands = [
        "build",
        "check",
        "run",
        "compare",
        "test",
        "diff",
        "bench",
        "profile",
        "lint",
        "doctor",
        "package",
        "publish",
        "verify",
        "deps",
        "vendor",
        "clean",
        "config",
        "completion",
    ]
    options = {
        "build": [
            "--module",
            "--target",
            "--codec",
            "--type-hints",
            "--fallback",
            "--type-facts",
            "--pgo-profile",
            "--output",
            "--out-dir",
            "--sysroot",
            "--emit",
            "--emit-ir",
            "--profile",
            "--deterministic",
            "--no-deterministic",
            "--deterministic-warn",
            "--no-deterministic-warn",
            "--trusted",
            "--no-trusted",
            "--capabilities",
            "--cache",
            "--no-cache",
            "--cache-dir",
            "--cache-report",
            "--rebuild",
            "--respect-pythonpath",
            "--no-respect-pythonpath",
            "--json",
            "--verbose",
        ],
        "check": [
            "--output",
            "--strict",
            "--deterministic",
            "--no-deterministic",
            "--deterministic-warn",
            "--no-deterministic-warn",
            "--json",
            "--verbose",
        ],
        "run": [
            "--module",
            "--build-arg",
            "--rebuild",
            "--timing",
            "--capabilities",
            "--trusted",
            "--no-trusted",
            "--json",
            "--verbose",
        ],
        "compare": [
            "--python",
            "--python-version",
            "--module",
            "--build-arg",
            "--rebuild",
            "--capabilities",
            "--trusted",
            "--no-trusted",
            "--json",
            "--verbose",
        ],
        "test": [
            "--suite",
            "--python-version",
            "--trusted",
            "--no-trusted",
            "--json",
            "--verbose",
        ],
        "diff": [
            "--python-version",
            "--trusted",
            "--no-trusted",
            "--json",
            "--verbose",
        ],
        "bench": ["--wasm", "--script", "--json", "--verbose"],
        "profile": ["--json", "--verbose"],
        "lint": ["--json", "--verbose"],
        "doctor": ["--strict", "--json", "--verbose"],
        "package": [
            "--output",
            "--deterministic",
            "--no-deterministic",
            "--deterministic-warn",
            "--no-deterministic-warn",
            "--capabilities",
            "--sbom",
            "--no-sbom",
            "--sbom-output",
            "--sbom-format",
            "--signature",
            "--signature-output",
            "--sign",
            "--no-sign",
            "--signer",
            "--signing-key",
            "--signing-identity",
            "--json",
            "--verbose",
        ],
        "publish": [
            "--registry",
            "--registry-token",
            "--registry-user",
            "--registry-password",
            "--registry-timeout",
            "--dry-run",
            "--deterministic",
            "--no-deterministic",
            "--deterministic-warn",
            "--no-deterministic-warn",
            "--capabilities",
            "--require-signature",
            "--no-require-signature",
            "--verify-signature",
            "--no-verify-signature",
            "--trusted-signers",
            "--signer",
            "--signing-key",
            "--json",
            "--verbose",
        ],
        "verify": [
            "--package",
            "--manifest",
            "--artifact",
            "--require-checksum",
            "--require-deterministic",
            "--require-signature",
            "--no-require-signature",
            "--verify-signature",
            "--no-verify-signature",
            "--trusted-signers",
            "--signer",
            "--signing-key",
            "--capabilities",
            "--json",
            "--verbose",
        ],
        "deps": ["--include-dev", "--json", "--verbose"],
        "vendor": [
            "--include-dev",
            "--output",
            "--dry-run",
            "--allow-non-tier-a",
            "--extras",
            "--deterministic",
            "--no-deterministic",
            "--deterministic-warn",
            "--no-deterministic-warn",
            "--json",
            "--verbose",
        ],
        "clean": [
            "--all",
            "--cache",
            "--no-cache",
            "--artifacts",
            "--no-artifacts",
            "--bins",
            "--no-bins",
            "--repo-artifacts",
            "--no-repo-artifacts",
            "--include-venvs",
            "--cargo-target",
            "--no-cargo-target",
            "--json",
            "--verbose",
        ],
        "config": ["--file", "--json", "--verbose"],
        "completion": ["--shell", "--json", "--verbose"],
    }
    if shell == "bash":
        lines = [
            "_molt_complete() {",
            "  local cur prev",
            "  COMPREPLY=()",
            '  cur="${COMP_WORDS[COMP_CWORD]}"',
            '  prev="${COMP_WORDS[COMP_CWORD-1]}"',
            "  if [[ ${COMP_CWORD} -eq 1 ]]; then",
            f'    COMPREPLY=( $(compgen -W "{" ".join(commands)}" -- "$cur") )',
            "    return 0",
            "  fi",
            '  case "${COMP_WORDS[1]}" in',
        ]
        for cmd in commands:
            opts = " ".join(options.get(cmd, []))
            lines.append(f'    {cmd}) opts="{opts}" ;;')
        lines.extend(
            [
                '    *) opts="" ;;',
                "  esac",
                '  COMPREPLY=( $(compgen -W "$opts" -- "$cur") )',
                "}",
                "complete -F _molt_complete molt",
            ]
        )
        return "\n".join(lines) + "\n"
    if shell == "zsh":
        lines = [
            "#compdef molt",
            "_molt() {",
            "  local -a commands",
            f"  commands=({' '.join(commands)})",
            "  if (( CURRENT == 2 )); then",
            "    compadd $commands",
            "    return",
            "  fi",
            "  local -a opts",
            "  case $words[2] in",
        ]
        for cmd in commands:
            opts = " ".join(options.get(cmd, []))
            lines.append(f"    {cmd}) opts=({opts}) ;;")
        lines.extend(
            [
                "    *) opts=() ;;",
                "  esac",
                "  compadd $opts",
                "}",
                "compdef _molt molt",
            ]
        )
        return "\n".join(lines) + "\n"
    if shell == "fish":
        lines = [
            f"complete -c molt -f -n '__fish_use_subcommand' -a \"{' '.join(commands)}\"",
        ]
        for cmd in commands:
            for opt in options.get(cmd, []):
                opt_name = opt.lstrip("-")
                lines.append(
                    f"complete -c molt -n '__fish_seen_subcommand_from {cmd}' -l {opt_name}"
                )
        return "\n".join(lines) + "\n"
    raise ValueError(f"Unsupported shell: {shell}")


def completion(shell: str, json_output: bool = False, verbose: bool = False) -> int:
    try:
        script = _completion_script(shell)
    except ValueError as exc:
        return _fail(str(exc), json_output, command="completion")
    if json_output:
        payload = _json_payload(
            "completion",
            "ok",
            data={"shell": shell, "script": script},
        )
        _emit_json(payload, json_output=True)
    else:
        print(script, end="")
    return 0


def _strip_leading_double_dash(args: list[str]) -> list[str]:
    if args and args[0] == "--":
        return args[1:]
    return args


def _extract_output_arg(args: list[str]) -> Path | None:
    for idx, arg in enumerate(args):
        if arg == "--output" and idx + 1 < len(args):
            return Path(args[idx + 1])
        if arg.startswith("--output="):
            return Path(arg.split("=", 1)[1])
    return None


def _extract_out_dir_arg(args: list[str]) -> Path | None:
    for idx, arg in enumerate(args):
        if arg == "--out-dir" and idx + 1 < len(args):
            return Path(args[idx + 1])
        if arg.startswith("--out-dir="):
            return Path(arg.split("=", 1)[1])
    return None


def _extract_emit_arg(args: list[str]) -> str | None:
    for idx, arg in enumerate(args):
        if arg == "--emit" and idx + 1 < len(args):
            return args[idx + 1]
        if arg.startswith("--emit="):
            return arg.split("=", 1)[1]
    return None


def _build_args_has_cache_flag(args: list[str]) -> bool:
    for arg in args:
        if arg in {"--cache", "--no-cache", "--rebuild"}:
            return True
    return False


def _resolve_binary_output(path_str: str) -> Path | None:
    path = Path(path_str)
    if path.exists():
        return path
    fallback = path.with_suffix(".exe")
    if fallback.exists():
        return fallback
    return None


def _build_args_has_trusted_flag(args: list[str]) -> bool:
    for arg in args:
        if arg in {"--trusted", "--no-trusted"}:
            return True
    return False


def _build_args_has_capabilities_flag(args: list[str]) -> bool:
    for arg in args:
        if arg == "--capabilities" or arg.startswith("--capabilities="):
            return True
    return False


def main() -> int:
    parser = argparse.ArgumentParser(prog="molt")
    subparsers = parser.add_subparsers(dest="command", required=True)

    build_parser = subparsers.add_parser("build", help="Compile a Python file")
    build_parser.add_argument("file", nargs="?", help="Path to Python source")
    build_parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    build_parser.add_argument(
        "--target",
        default=None,
        help="Target backend: native, wasm, or a target triple.",
    )
    build_parser.add_argument(
        "--codec",
        choices=["msgpack", "cbor", "json"],
        default=None,
        help="Structured codec for parse calls (default from config or msgpack).",
    )
    build_parser.add_argument(
        "--type-hints",
        choices=["ignore", "trust", "check"],
        default=None,
        help="Apply type annotations to guide lowering and specialization.",
    )
    build_parser.add_argument(
        "--fallback",
        choices=["error", "bridge"],
        default=None,
        help="Fallback policy for unsupported constructs.",
    )
    build_parser.add_argument(
        "--type-facts",
        help="Path to type facts JSON from `molt check`.",
    )
    build_parser.add_argument(
        "--pgo-profile",
        help="Path to a Molt profile artifact (molt_profile.json) for PGO hints.",
    )
    build_parser.add_argument(
        "--output",
        help=(
            "Output path for the native binary or wasm artifact "
            "(relative to --out-dir when set, otherwise project root). "
            "If the path is a directory (or ends with a path separator), "
            "the default filename is used within that directory."
        ),
    )
    build_parser.add_argument(
        "--out-dir",
        help=(
            "Output directory for final artifacts (binary/wasm/object). "
            "Intermediates stay under MOLT_HOME/build/<entry> by default. "
            "Native binaries otherwise default to MOLT_BIN."
        ),
    )
    build_parser.add_argument(
        "--sysroot",
        help=(
            "Sysroot path for native linking (relative paths resolve under the project "
            "root; defaults to MOLT_SYSROOT or MOLT_CROSS_SYSROOT when set)."
        ),
    )
    build_parser.add_argument(
        "--emit",
        choices=["bin", "obj", "wasm"],
        default=None,
        help="Select which artifact to emit (native: bin/obj, wasm: wasm).",
    )
    build_parser.add_argument(
        "--linked",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Emit a linked wasm artifact (output_linked.wasm) alongside output.wasm.",
    )
    build_parser.add_argument(
        "--linked-output",
        help=(
            "Output path for the linked wasm artifact "
            "(relative to --out-dir when set, otherwise project root)."
        ),
    )
    build_parser.add_argument(
        "--require-linked",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require linked wasm output for wasm targets (fails if linking is unavailable).",
    )
    build_parser.add_argument(
        "--emit-ir",
        help="Write the lowered IR JSON to a file path.",
    )
    build_parser.add_argument(
        "--profile",
        choices=["dev", "release"],
        default=None,
        help="Build profile for backend/runtime (default: release).",
    )
    build_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic inputs (lockfiles).",
    )
    build_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    build_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments (native only).",
    )
    build_parser.add_argument(
        "--cache",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Enable build cache under MOLT_CACHE (defaults to the OS cache).",
    )
    build_parser.add_argument(
        "--cache-dir",
        help="Override the build cache directory (default: MOLT_CACHE).",
    )
    build_parser.add_argument(
        "--cache-report",
        action="store_true",
        help="Print cache hit/miss details even without --verbose.",
    )
    build_parser.add_argument(
        "--rebuild",
        action="store_true",
        help="Disable the build cache (alias for --no-cache).",
    )
    build_parser.add_argument(
        "--respect-pythonpath",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Include PYTHONPATH entries as module roots during compilation.",
    )
    build_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    build_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    build_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    check_parser = subparsers.add_parser(
        "check", help="Generate a type facts artifact (ty-backed when available)"
    )
    check_parser.add_argument("path", help="Python file or package directory")
    check_parser.add_argument(
        "--output",
        default="type_facts.json",
        help="Output path for type facts JSON.",
    )
    check_parser.add_argument(
        "--strict",
        action="store_true",
        help="Mark facts as trusted (strict tier).",
    )
    check_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic inputs (lockfiles).",
    )
    check_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    check_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    check_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    run_parser = subparsers.add_parser(
        "run", help="Compile with Molt and run the native binary"
    )
    run_parser.add_argument("file", nargs="?", help="Path to Python source")
    run_parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    run_parser.add_argument(
        "--build-arg",
        action="append",
        default=[],
        help="Extra args passed to `molt build`.",
    )
    run_parser.add_argument(
        "--rebuild",
        action="store_true",
        help="Disable build cache for `molt build`.",
    )
    run_parser.add_argument(
        "--timing",
        action="store_true",
        help="Emit timing summary (compile + run).",
    )
    run_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    run_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    run_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    run_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    run_parser.add_argument(
        "script_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the script (use -- to separate).",
    )

    compare_parser = subparsers.add_parser(
        "compare", help="Compare CPython vs Molt outputs and timing"
    )
    compare_parser.add_argument("file", nargs="?", help="Path to Python source")
    compare_parser.add_argument(
        "--module",
        help="Entry module name (uses pkg.__main__ when present).",
    )
    compare_parser.add_argument(
        "--python",
        help="Python interpreter (path) or version (e.g. 3.12).",
    )
    compare_parser.add_argument(
        "--python-version",
        help="Python version alias (e.g. 3.12).",
    )
    compare_parser.add_argument(
        "--build-arg",
        action="append",
        default=[],
        help="Extra args passed to `molt build` for the Molt side.",
    )
    compare_parser.add_argument(
        "--rebuild",
        action="store_true",
        help="Disable build cache for the Molt build.",
    )
    compare_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    compare_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    compare_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    compare_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    compare_parser.add_argument(
        "script_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the script (use -- to separate).",
    )

    test_parser = subparsers.add_parser("test", help="Run Molt test suites")
    test_parser.add_argument(
        "--suite",
        choices=["dev", "diff", "pytest"],
        default="dev",
        help="Test suite to run.",
    )
    test_parser.add_argument(
        "--python-version",
        help="Python version for diff suite (e.g. 3.13).",
    )
    test_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    test_parser.add_argument("path", nargs="?", help="Optional test path.")
    test_parser.add_argument(
        "pytest_args",
        nargs=argparse.REMAINDER,
        help="Extra pytest args when --suite pytest (use -- to separate).",
    )
    test_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    test_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    diff_parser = subparsers.add_parser(
        "diff", help="Run differential tests against CPython"
    )
    diff_parser.add_argument("path", nargs="?", help="File or directory to test.")
    diff_parser.add_argument(
        "--python-version", help="Python version to test against (e.g. 3.13)."
    )
    diff_parser.add_argument(
        "--trusted",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Disable capability checks for trusted deployments.",
    )
    diff_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    diff_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    bench_parser = subparsers.add_parser("bench", help="Run benchmark suites")
    bench_parser.add_argument(
        "--wasm", action="store_true", help="Use the WASM bench harness."
    )
    bench_parser.add_argument(
        "--script",
        action="append",
        dest="bench_script",
        default=[],
        help="Benchmark a custom script path (repeatable).",
    )
    bench_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    bench_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    bench_parser.add_argument(
        "bench_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the bench tool (use -- to separate).",
    )

    profile_parser = subparsers.add_parser("profile", help="Profile benchmarks")
    profile_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    profile_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    profile_parser.add_argument(
        "profile_args",
        nargs=argparse.REMAINDER,
        help="Arguments passed to the profile tool (use -- to separate).",
    )

    lint_parser = subparsers.add_parser("lint", help="Run linting checks")
    lint_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    lint_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    doctor_parser = subparsers.add_parser("doctor", help="Check toolchain setup")
    doctor_parser.add_argument(
        "--strict",
        action="store_true",
        help="Return non-zero exit on missing requirements.",
    )
    doctor_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    doctor_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    package_parser = subparsers.add_parser(
        "package", help="Bundle a Molt package artifact"
    )
    package_parser.add_argument("artifact", help="Path to the package artifact.")
    package_parser.add_argument(
        "manifest",
        help="Path to manifest JSON (fields per docs/spec/0018_MOLT_PACKAGE_ABI.md).",
    )
    package_parser.add_argument(
        "--output",
        help="Output .moltpkg path (default dist/<name>-<version>-<target>.moltpkg).",
    )
    package_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic package metadata.",
    )
    package_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    package_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    package_parser.add_argument(
        "--sbom",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Emit a CycloneDX SBOM sidecar (default: enabled).",
    )
    package_parser.add_argument(
        "--sbom-output",
        help="Override the SBOM output path (defaults next to the package).",
    )
    package_parser.add_argument(
        "--sbom-format",
        choices=["cyclonedx", "spdx"],
        default="cyclonedx",
        help="SBOM format to emit (default: cyclonedx).",
    )
    package_parser.add_argument(
        "--signature",
        help="Path to a signature file to attach and record in metadata.",
    )
    package_parser.add_argument(
        "--signature-output",
        help="Override the signature sidecar output path (defaults next to the package).",
    )
    package_parser.add_argument(
        "--sign",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Sign the artifact with cosign or codesign.",
    )
    package_parser.add_argument(
        "--signer",
        choices=["auto", "cosign", "codesign"],
        default="auto",
        help="Select the signing tool (default: auto).",
    )
    package_parser.add_argument(
        "--signing-key",
        help="Signing key path for cosign (or set COSIGN_KEY).",
    )
    package_parser.add_argument(
        "--signing-identity",
        help="Signing identity for codesign (or set MOLT_CODESIGN_IDENTITY).",
    )
    package_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    package_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    publish_parser = subparsers.add_parser(
        "publish", help="Publish a Molt package to a registry path or URL"
    )
    publish_parser.add_argument("package", help="Path to the .moltpkg file.")
    publish_parser.add_argument(
        "--registry",
        default="dist/registry",
        help="Registry directory, file path, or HTTP(S) URL.",
    )
    publish_parser.add_argument(
        "--registry-token",
        help=(
            "Bearer token for remote registry auth (or MOLT_REGISTRY_TOKEN; "
            "prefix @ for file)."
        ),
    )
    publish_parser.add_argument(
        "--registry-user",
        help="Username for basic auth (or MOLT_REGISTRY_USER).",
    )
    publish_parser.add_argument(
        "--registry-password",
        help=(
            "Password for basic auth (or MOLT_REGISTRY_PASSWORD; prefix @ for file)."
        ),
    )
    publish_parser.add_argument(
        "--registry-timeout",
        type=float,
        help="Registry request timeout in seconds (or MOLT_REGISTRY_TIMEOUT).",
    )
    publish_parser.add_argument(
        "--dry-run", action="store_true", help="Print the publish plan only."
    )
    publish_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Verify package determinism before publishing.",
    )
    publish_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    publish_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    publish_parser.add_argument(
        "--require-signature",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require a package signature when publishing.",
    )
    publish_parser.add_argument(
        "--verify-signature",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Verify package signatures when publishing.",
    )
    publish_parser.add_argument(
        "--trusted-signers",
        help="Path to a trust policy for allowed signers.",
    )
    publish_parser.add_argument(
        "--signer",
        choices=["auto", "cosign", "codesign"],
        default="auto",
        help="Select the verification tool (default: auto).",
    )
    publish_parser.add_argument(
        "--signing-key",
        help="Verification key path for cosign (or set COSIGN_KEY).",
    )
    publish_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    publish_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    verify_parser = subparsers.add_parser(
        "verify", help="Verify a Molt package manifest and checksum"
    )
    verify_parser.add_argument(
        "--package",
        help="Path to the .moltpkg archive (alternative to --manifest/--artifact).",
    )
    verify_parser.add_argument("--manifest", help="Manifest JSON path.")
    verify_parser.add_argument("--artifact", help="Artifact path.")
    verify_parser.add_argument(
        "--require-checksum",
        action="store_true",
        help="Fail when checksum is missing.",
    )
    verify_parser.add_argument(
        "--require-deterministic",
        action="store_true",
        help="Fail when manifest is not deterministic.",
    )
    verify_parser.add_argument(
        "--require-signature",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require a package signature.",
    )
    verify_parser.add_argument(
        "--verify-signature",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Verify package signatures when present.",
    )
    verify_parser.add_argument(
        "--trusted-signers",
        help="Path to a trust policy for allowed signers.",
    )
    verify_parser.add_argument(
        "--signer",
        choices=["auto", "cosign", "codesign"],
        default="auto",
        help="Select the verification tool (default: auto).",
    )
    verify_parser.add_argument(
        "--signing-key",
        help="Verification key path for cosign (or set COSIGN_KEY).",
    )
    verify_parser.add_argument(
        "--capabilities",
        help="Capability profiles/tokens or path to manifest (toml/json).",
    )
    verify_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    verify_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    deps_parser = subparsers.add_parser(
        "deps", help="Show dependency compatibility info"
    )
    deps_parser.add_argument(
        "--include-dev", action="store_true", help="Include dev dependencies"
    )
    deps_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    deps_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )
    vendor_parser = subparsers.add_parser(
        "vendor", help="Vendor pure Python dependencies"
    )
    vendor_parser.add_argument(
        "--include-dev", action="store_true", help="Include dev dependencies"
    )
    vendor_parser.add_argument(
        "--output",
        help="Output directory for vendored artifacts (default: vendor).",
    )
    vendor_parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show vendoring plan without downloading artifacts.",
    )
    vendor_parser.add_argument(
        "--allow-non-tier-a",
        action="store_true",
        help="Proceed even if non-Tier A dependencies are present.",
    )
    vendor_parser.add_argument(
        "--extras",
        action="append",
        help="Extras to include from project optional-dependencies.",
    )
    vendor_parser.add_argument(
        "--deterministic",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Require deterministic inputs (lockfiles).",
    )
    vendor_parser.add_argument(
        "--deterministic-warn",
        action=argparse.BooleanOptionalAction,
        default=None,
        help="Warn instead of failing when deterministic lockfile checks fail.",
    )
    vendor_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    vendor_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    clean_parser = subparsers.add_parser(
        "clean", help="Remove Molt caches, build artifacts, and repo outputs"
    )
    clean_parser.add_argument(
        "--all",
        action="store_true",
        help="Remove all caches, build artifacts, repo outputs, and cargo targets.",
    )
    clean_parser.add_argument(
        "--cache",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Remove build caches under MOLT_CACHE.",
    )
    clean_parser.add_argument(
        "--artifacts",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Remove build artifacts under MOLT_HOME/build.",
    )
    clean_parser.add_argument(
        "--bins",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Remove Molt binaries under MOLT_BIN.",
    )
    clean_parser.add_argument(
        "--repo-artifacts",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Remove repo-local artifacts (vendor/, logs/, caches, output*.wasm).",
    )
    clean_parser.add_argument(
        "--include-venvs",
        action="store_true",
        help="Also clean virtualenv caches when removing repo artifacts.",
    )
    clean_parser.add_argument(
        "--cargo-target",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="Remove Cargo target/ build artifacts in the repo root.",
    )
    clean_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    clean_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    config_parser = subparsers.add_parser(
        "config", help="Show Molt configuration defaults"
    )
    config_parser.add_argument(
        "--file",
        help="Resolve project root from a source file path.",
    )
    config_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    config_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    completion_parser = subparsers.add_parser(
        "completion", help="Generate shell completion scripts"
    )
    completion_parser.add_argument(
        "--shell",
        choices=["bash", "zsh", "fish"],
        default="bash",
        help="Shell type to emit.",
    )
    completion_parser.add_argument(
        "--json", action="store_true", help="Emit JSON output for tooling."
    )
    completion_parser.add_argument(
        "--verbose", action="store_true", help="Emit verbose diagnostics."
    )

    args = parser.parse_args()

    config_root = _find_project_root(Path.cwd())
    if getattr(args, "file", None):
        try:
            config_root = _find_project_root(Path(args.file).resolve())
        except OSError:
            config_root = _find_project_root(Path.cwd())
    config = _load_molt_config(config_root)
    build_cfg = _resolve_build_config(config)
    run_cfg = _resolve_command_config(config, "run")
    test_cfg = _resolve_command_config(config, "test")
    diff_cfg = _resolve_command_config(config, "diff")
    publish_cfg = _resolve_command_config(config, "publish")
    cfg_capabilities = _resolve_capabilities_config(config)

    if args.command == "build":
        target = args.target or build_cfg.get("target") or "native"
        codec = args.codec or build_cfg.get("codec") or "msgpack"
        type_hints = args.type_hints or build_cfg.get("type_hints") or "ignore"
        fallback = args.fallback or build_cfg.get("fallback") or "error"
        output = args.output or build_cfg.get("output")
        out_dir = args.out_dir or build_cfg.get("out_dir") or build_cfg.get("out-dir")
        sysroot = (
            args.sysroot
            or build_cfg.get("sysroot")
            or build_cfg.get("sysroot_path")
            or build_cfg.get("sysroot-path")
        )
        emit = args.emit or build_cfg.get("emit")
        emit_ir = args.emit_ir or build_cfg.get("emit_ir") or build_cfg.get("emit-ir")
        pgo_profile = (
            args.pgo_profile
            or build_cfg.get("pgo_profile")
            or build_cfg.get("pgo-profile")
        )
        build_profile = (
            args.profile
            or build_cfg.get("profile")
            or build_cfg.get("build_profile")
            or "release"
        )
        linked_output_path = (
            args.linked_output
            or build_cfg.get("linked_output")
            or build_cfg.get("linked-output")
        )
        require_linked = args.require_linked
        if require_linked is None:
            require_linked = _coerce_bool(
                build_cfg.get("require_linked") or build_cfg.get("require-linked"),
                False,
            )
        type_facts = args.type_facts or build_cfg.get("type_facts")
        deterministic = (
            args.deterministic
            if args.deterministic is not None
            else _coerce_bool(build_cfg.get("deterministic"), True)
        )
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(build_cfg.get("trusted"), False)
        linked = args.linked
        if linked is None:
            linked = _coerce_bool(build_cfg.get("linked"), False)
        cache = (
            args.cache
            if args.cache is not None
            else _coerce_bool(build_cfg.get("cache"), True)
        )
        if args.rebuild:
            cache = False
        cache_dir = (
            args.cache_dir or build_cfg.get("cache_dir") or build_cfg.get("cache-dir")
        )
        cache_report = args.cache_report or _coerce_bool(
            build_cfg.get("cache_report") or build_cfg.get("cache-report"), False
        )
        respect_pythonpath = args.respect_pythonpath
        if respect_pythonpath is None:
            respect_pythonpath = _coerce_bool(
                build_cfg.get("respect_pythonpath")
                or build_cfg.get("respect-pythonpath"),
                False,
            )
        capabilities = (
            args.capabilities or build_cfg.get("capabilities") or cfg_capabilities
        )
        if args.file and args.module:
            return _fail(
                "Use a file path or --module, not both.", args.json, command="build"
            )
        if not args.file and not args.module:
            return _fail("Missing entry file or module.", args.json, command="build")
        return build(
            args.file,
            target,
            codec,
            type_hints,
            fallback,
            type_facts,
            pgo_profile,
            output,
            args.json,
            args.verbose,
            deterministic,
            deterministic_warn,
            trusted,
            capabilities,
            cache,
            cache_dir,
            cache_report,
            sysroot,
            emit_ir,
            emit,
            out_dir,
            build_profile,
            linked,
            linked_output_path,
            require_linked,
            respect_pythonpath,
            args.module,
        )
    if args.command == "check":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(build_cfg.get("deterministic"), True)
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        return check(
            args.path,
            args.output,
            args.strict,
            args.json,
            args.verbose,
            deterministic,
            deterministic_warn,
        )
    if args.command == "run":
        build_args = _strip_leading_double_dash(args.build_arg)
        if args.rebuild and not _build_args_has_cache_flag(build_args):
            build_args.append("--no-cache")
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(run_cfg.get("trusted"), False)
        capabilities = (
            args.capabilities or run_cfg.get("capabilities") or cfg_capabilities
        )
        return run_script(
            args.file,
            args.module,
            _strip_leading_double_dash(args.script_args),
            args.json,
            args.verbose,
            args.timing,
            trusted,
            capabilities,
            build_args,
        )
    if args.command == "compare":
        python_exe = args.python or args.python_version
        build_args = _strip_leading_double_dash(args.build_arg)
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(run_cfg.get("trusted"), False)
        capabilities = (
            args.capabilities or run_cfg.get("capabilities") or cfg_capabilities
        )
        return compare(
            args.file,
            args.module,
            python_exe,
            _strip_leading_double_dash(args.script_args),
            args.json,
            args.verbose,
            trusted,
            capabilities,
            build_args,
            args.rebuild,
        )
    if args.command == "test":
        pytest_args = _strip_leading_double_dash(args.pytest_args)
        if args.suite == "dev" and (args.path or pytest_args) and args.verbose:
            print("Ignoring extra args for suite=dev.")
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(test_cfg.get("trusted"), False)
        return test(
            args.suite,
            args.path,
            args.python_version,
            pytest_args,
            trusted,
            args.json,
            args.verbose,
        )
    if args.command == "diff":
        trusted = args.trusted
        if trusted is None:
            trusted = _coerce_bool(diff_cfg.get("trusted"), False)
        return diff(
            args.path,
            args.python_version,
            trusted,
            args.json,
            args.verbose,
        )
    if args.command == "bench":
        return bench(
            args.wasm,
            _strip_leading_double_dash(args.bench_args),
            args.bench_script,
            args.json,
            args.verbose,
        )
    if args.command == "profile":
        return profile(
            _strip_leading_double_dash(args.profile_args),
            args.json,
            args.verbose,
        )
    if args.command == "lint":
        return lint(args.json, args.verbose)
    if args.command == "doctor":
        return doctor(args.json, args.verbose, args.strict)
    if args.command == "package":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(build_cfg.get("deterministic"), True)
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        capabilities = args.capabilities or cfg_capabilities
        sbom_enabled = args.sbom
        if sbom_enabled is None:
            sbom_enabled = True
        return package(
            args.artifact,
            args.manifest,
            args.output,
            json_output=args.json,
            verbose=args.verbose,
            deterministic=deterministic,
            deterministic_warn=deterministic_warn,
            capabilities=capabilities,
            sbom=sbom_enabled,
            sbom_output=args.sbom_output,
            sbom_format=args.sbom_format,
            signature=args.signature,
            signature_output=args.signature_output,
            sign=args.sign,
            signer=args.signer,
            signing_key=args.signing_key,
            signing_identity=args.signing_identity,
        )
    if args.command == "publish":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(build_cfg.get("deterministic"), True)
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        explicit_require = args.require_signature is not None
        explicit_verify = args.verify_signature is not None
        require_signature = args.require_signature
        if require_signature is None:
            require_signature = _coerce_bool(
                publish_cfg.get("require_signature")
                or publish_cfg.get("require-signature")
                or os.environ.get("MOLT_REQUIRE_SIGNATURE"),
                False,
            )
        verify_signature = args.verify_signature
        if verify_signature is None:
            verify_signature = _coerce_bool(
                publish_cfg.get("verify_signature")
                or publish_cfg.get("verify-signature")
                or os.environ.get("MOLT_VERIFY_SIGNATURE"),
                False,
            )
        if explicit_require and not require_signature and not explicit_verify:
            verify_signature = False
        trusted_signers = (
            args.trusted_signers
            or publish_cfg.get("trusted_signers")
            or publish_cfg.get("trusted-signers")
            or os.environ.get("MOLT_TRUSTED_SIGNERS")
        )
        if _is_remote_registry(args.registry):
            if not explicit_require:
                require_signature = True
            if not explicit_verify and require_signature:
                verify_signature = True
            if trusted_signers is None and (require_signature or verify_signature):
                return _fail(
                    "Remote publish requires --trusted-signers or MOLT_TRUSTED_SIGNERS "
                    "(disable with --no-require-signature/--no-verify-signature).",
                    args.json,
                    command="publish",
                )
        capabilities = args.capabilities or cfg_capabilities
        return publish(
            args.package,
            args.registry,
            args.dry_run,
            args.json,
            args.verbose,
            deterministic,
            deterministic_warn,
            capabilities,
            require_signature,
            verify_signature,
            trusted_signers,
            args.signer,
            args.signing_key,
            args.registry_token,
            args.registry_user,
            args.registry_password,
            args.registry_timeout,
        )
    if args.command == "verify":
        require_signature = args.require_signature
        if require_signature is None:
            require_signature = False
        verify_signature = args.verify_signature
        if verify_signature is None:
            verify_signature = False
        return verify(
            args.package,
            args.manifest,
            args.artifact,
            args.require_checksum,
            args.json,
            args.verbose,
            args.require_deterministic,
            args.capabilities or cfg_capabilities,
            require_signature,
            verify_signature,
            args.trusted_signers,
            args.signer,
            args.signing_key,
        )
    if args.command == "deps":
        return deps(args.include_dev, args.json, args.verbose)
    if args.command == "vendor":
        deterministic = args.deterministic
        if deterministic is None:
            deterministic = _coerce_bool(build_cfg.get("deterministic"), True)
        deterministic_warn = args.deterministic_warn
        if deterministic_warn is None:
            deterministic_warn = _coerce_bool(
                build_cfg.get("deterministic_warn")
                or build_cfg.get("deterministic-warn"),
                False,
            )
        return vendor(
            args.include_dev,
            args.json,
            args.verbose,
            args.output,
            args.dry_run,
            args.allow_non_tier_a,
            args.extras,
            deterministic,
            deterministic_warn,
        )
    if args.command == "clean":
        return clean(
            args.json,
            args.verbose,
            args.cache,
            args.artifacts,
            args.bins,
            args.repo_artifacts,
            args.cargo_target,
            args.all,
            args.include_venvs,
        )
    if args.command == "config":
        return show_config(config_root, config, args.json, args.verbose)
    if args.command == "completion":
        return completion(args.shell, args.json, args.verbose)

    return 2


def _load_toml(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {}
    return tomllib.loads(path.read_text())


def _normalize_name(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


def _marker_environment() -> dict[str, str]:
    version = sys.version_info
    return {
        "python_version": f"{version.major}.{version.minor}",
        "python_full_version": f"{version.major}.{version.minor}.{version.micro}",
        "os_name": os.name,
        "sys_platform": sys.platform,
        "platform_python_implementation": platform.python_implementation(),
        "platform_system": platform.system(),
        "platform_machine": platform.machine(),
        "platform_release": platform.release(),
        "platform_version": platform.version(),
        "implementation_name": sys.implementation.name,
        "implementation_version": sys.implementation.version.__str__(),
    }


def _parse_requirement(spec: str) -> tuple[str, set[str], str | None]:
    try:
        req = Requirement(spec)
    except InvalidRequirement:
        return "", set(), None
    marker = str(req.marker) if req.marker else None
    return req.name, set(req.extras), marker


def _marker_satisfied(
    marker: str,
    env: dict[str, str],
    extras: set[str],
) -> bool:
    try:
        parsed = Marker(marker)
    except InvalidMarker:
        return False
    base_env = dict(env)
    base_env.setdefault("extra", "")
    if "extra" in marker:
        if extras:
            return any(
                parsed.evaluate({**base_env, "extra": extra}) for extra in extras
            )
        return parsed.evaluate(base_env)
    return parsed.evaluate(base_env)


def _collect_dep_specs(
    pyproject: dict[str, Any],
    include_dev: bool,
    extras: set[str] | None = None,
) -> tuple[list[str], dict[str, set[str]], list[str]]:
    deps: list[str] = []
    root_extras: dict[str, set[str]] = {}
    skipped: list[str] = []
    entries: list[str] = []
    entries.extend(pyproject.get("project", {}).get("dependencies", []))
    if include_dev:
        entries.extend(pyproject.get("dependency-groups", {}).get("dev", []))
    extras = extras or set()
    optional = pyproject.get("project", {}).get("optional-dependencies", {})
    for extra in extras:
        entries.extend(optional.get(extra, []))
    env = _marker_environment()
    for entry in entries:
        name, req_extras, marker = _parse_requirement(entry)
        if not name:
            continue
        if marker and not _marker_satisfied(marker, env, extras):
            skipped.append(entry)
            continue
        norm = _normalize_name(name)
        deps.append(norm)
        if req_extras:
            root_extras.setdefault(norm, set()).update(req_extras)
    return deps, root_extras, skipped


def _collect_deps(pyproject: dict[str, Any], include_dev: bool) -> list[str]:
    deps: list[str] = []
    deps.extend(pyproject.get("project", {}).get("dependencies", []))
    if include_dev:
        deps.extend(pyproject.get("dependency-groups", {}).get("dev", []))
    return [re.split(r"[<=>\\[\\s;]", dep, 1)[0] for dep in deps]


def _lock_packages(lock: dict[str, Any]) -> dict[str, dict[str, Any]]:
    packages: dict[str, dict[str, Any]] = {}
    for pkg in lock.get("package", []):
        name = _normalize_name(pkg.get("name", ""))
        if name:
            packages[name] = pkg
    return packages


def _lock_package_graph(
    lock: dict[str, Any],
    env: dict[str, str] | None = None,
    selected_extras: dict[str, set[str]] | None = None,
) -> tuple[dict[str, dict[str, Any]], dict[str, list[str]], list[dict[str, Any]]]:
    packages: dict[str, dict[str, Any]] = {}
    deps: dict[str, list[str]] = {}
    skipped: list[dict[str, Any]] = []
    env = env or _marker_environment()
    selected_extras = selected_extras or {}
    for pkg in lock.get("package", []):
        name = _normalize_name(pkg.get("name", ""))
        if not name:
            continue
        packages[name] = pkg
        dep_names: list[str] = []
        extras = selected_extras.get(name, set())
        if isinstance(extras, list):
            extras = set(extras)
        for dep in pkg.get("dependencies", []):
            dep_name = _normalize_name(dep.get("name", ""))
            marker = dep.get("marker")
            extra = dep.get("extra")
            extra_tokens: list[str] = []
            if isinstance(extra, str):
                if extra:
                    extra_tokens = [extra]
            elif isinstance(extra, list):
                extra_tokens = [
                    item for item in extra if isinstance(item, str) and item
                ]
            if extra_tokens and extras.isdisjoint(extra_tokens):
                skipped.append(
                    {
                        "name": dep.get("name"),
                        "from": pkg.get("name"),
                        "marker": marker,
                        "extra": extra,
                    }
                )
                continue
            if marker and not _marker_satisfied(marker, env, extras):
                skipped.append(
                    {
                        "name": dep.get("name"),
                        "from": pkg.get("name"),
                        "marker": marker,
                        "extra": extra,
                    }
                )
                continue
            if dep_name:
                dep_names.append(dep_name)
        deps[name] = dep_names
    return packages, deps, skipped


def _resolve_dependency_closure(
    roots: list[str],
    deps: dict[str, list[str]],
) -> tuple[list[str], list[str]]:
    seen: set[str] = set()
    missing: list[str] = []
    queue = list(roots)
    while queue:
        name = queue.pop(0)
        if name in seen:
            continue
        seen.add(name)
        if name not in deps:
            missing.append(name)
            continue
        for child in deps.get(name, []):
            if child not in seen:
                queue.append(child)
    return sorted(seen), sorted(set(missing))


def _pick_vendor_artifact(pkg: dict[str, Any]) -> tuple[str, dict[str, Any]] | None:
    for wheel in pkg.get("wheels", []):
        url = wheel.get("url", "")
        if "py3-none-any" in url:
            return "wheel", wheel
    sdist = pkg.get("sdist")
    if sdist:
        return "sdist", sdist
    wheels = pkg.get("wheels", [])
    if wheels:
        return "wheel", wheels[0]
    return None


def _vendor_cache_path(url: str, expected_hash: str) -> Path | None:
    if not expected_hash:
        return None
    algo = "sha256"
    digest = expected_hash
    if ":" in expected_hash:
        algo, digest = expected_hash.split(":", 1)
    if not digest:
        return None
    suffixes = Path(urllib.parse.urlparse(url).path).suffixes
    suffix = "".join(suffixes) if suffixes else ""
    cache_root = _default_molt_cache() / "vendor"
    try:
        cache_root.mkdir(parents=True, exist_ok=True)
    except OSError:
        return None
    return cache_root / f"{algo}-{digest}{suffix}"


def _read_cached_artifact(cache_path: Path, expected_digest: str) -> bytes | None:
    try:
        data = cache_path.read_bytes()
    except OSError:
        return None
    digest = hashlib.sha256(data).hexdigest()
    if digest != expected_digest:
        try:
            cache_path.unlink()
        except OSError:
            pass
        return None
    return data


def _write_cached_artifact(cache_path: Path, data: bytes) -> None:
    tmp_path = cache_path.with_name(f"{cache_path.name}.tmp")
    try:
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        tmp_path.write_bytes(data)
        tmp_path.replace(cache_path)
    except OSError:
        try:
            if tmp_path.exists():
                tmp_path.unlink()
        except OSError:
            pass


def _download_artifact(url: str, expected_hash: str) -> bytes:
    if not url or not expected_hash:
        raise ValueError("missing url or hash")
    cache_path = _vendor_cache_path(url, expected_hash)
    expected = expected_hash.split(":", 1)[-1]
    if cache_path is not None:
        cached = _read_cached_artifact(cache_path, expected)
        if cached is not None:
            return cached
    with urllib.request.urlopen(url) as response:
        data = response.read()
    digest = hashlib.sha256(data).hexdigest()
    if digest != expected:
        raise ValueError("hash mismatch")
    if cache_path is not None:
        _write_cached_artifact(cache_path, data)
    return data


def _classify_tier(
    name: str,
    pkg: dict[str, Any] | None,
    allow: dict[str, set[str]],
) -> tuple[str, str]:
    norm = _normalize_name(name)
    if norm in allow["tier_a"]:
        return "Tier A", _append_feature_notes("allowlisted", pkg)
    if norm in allow["tier_b"]:
        return "Tier B", _append_feature_notes("allowlisted", pkg)
    if norm in allow["tier_c"]:
        return "Tier C", _append_feature_notes("allowlisted", pkg)
    if norm in allow["native_wheels"]:
        return "Tier B", _append_feature_notes("allowlisted native wheels", pkg)

    molt_packages = {"molt_json", "molt_msgpack", "molt_cbor"}
    if norm in molt_packages:
        return "Tier B", _append_feature_notes("molt package", pkg)
    if pkg is None:
        return "Tier A", _append_feature_notes("unresolved (assumed pure python)", pkg)
    source = pkg.get("source", {})
    if source.get("git") or source.get("path"):
        return "Tier A", _append_feature_notes("local/git source", pkg)
    wheels = pkg.get("wheels", [])
    has_universal = any("py3-none-any" in wheel.get("url", "") for wheel in wheels)
    has_abi3 = any("abi3" in wheel.get("url", "") for wheel in wheels)
    if wheels and not has_universal and not has_abi3:
        return "Tier C", _append_feature_notes("platform wheels only", pkg)
    if has_abi3 and not has_universal:
        return "Tier B", _append_feature_notes("abi3 wheels", pkg)
    if wheels:
        return "Tier A", _append_feature_notes("universal wheels", pkg)
    if pkg.get("sdist"):
        return "Tier A", _append_feature_notes("sdist only", pkg)
    return "Tier A", _append_feature_notes("assumed pure python", pkg)


def _dep_allowlists(pyproject: dict[str, Any]) -> dict[str, set[str]]:
    tool_cfg = pyproject.get("tool", {}).get("molt", {}).get("deps", {})
    return {
        "tier_a": {_normalize_name(name) for name in tool_cfg.get("tier_a", [])},
        "tier_b": {_normalize_name(name) for name in tool_cfg.get("tier_b", [])},
        "tier_c": {_normalize_name(name) for name in tool_cfg.get("tier_c", [])},
        "native_wheels": {
            _normalize_name(name) for name in tool_cfg.get("native_wheels", [])
        },
    }


def _append_feature_notes(reason: str, pkg: dict[str, Any] | None) -> str:
    if not pkg:
        return reason
    metadata = pkg.get("metadata", {})
    requires = metadata.get("requires-dist", [])
    markers = any("marker" in dep for dep in requires)
    extras = any("extra" in dep for dep in requires)
    notes: list[str] = []
    if markers:
        notes.append("markers")
    if extras:
        notes.append("extras")
    if notes:
        return f"{reason}; {', '.join(notes)}"
    return reason


def _collect_py_files(target: Path) -> list[Path]:
    if target.is_file():
        return [target]
    return sorted(path for path in target.rglob("*.py") if path.is_file())


def _run_ty_check(path: Path) -> tuple[bool, str]:
    commands = [
        ["uv", "run", "ty", "check", str(path), "--output-format", "concise"],
        ["ty", "check", str(path), "--output-format", "concise"],
    ]
    for cmd in commands:
        try:
            result = subprocess.run(cmd, capture_output=True, text=True, check=False)
        except FileNotFoundError:
            continue
        if result.returncode == 0:
            return True, result.stdout.strip()
        combined = (result.stdout + result.stderr).strip()
        return False, combined
    return False, "ty is not available; install it with `uv add ty`."


def _collect_type_facts_for_build(
    paths: list[Path], type_hint_policy: TypeHintPolicy, ty_target: Path
) -> tuple[Any | None, bool]:
    trust = "trusted" if type_hint_policy == "trust" else "guarded"
    ty_ok, _ = _run_ty_check(ty_target)
    facts = collect_type_facts_from_paths(paths, trust, infer=ty_ok)
    if ty_ok:
        facts.tool = "molt-check+ty+infer"
    return facts, ty_ok


def check(
    path: str,
    output: str,
    strict: bool,
    json_output: bool = False,
    verbose: bool = False,
    deterministic: bool = True,
    deterministic_warn: bool = False,
) -> int:
    target = Path(path)
    if not target.exists():
        return _fail(f"Path not found: {target}", json_output, command="check")
    project_root = _find_project_root(target.resolve())
    warnings: list[str] = []
    lock_error = _check_lockfiles(
        project_root,
        json_output,
        warnings,
        deterministic,
        deterministic_warn,
        "check",
    )
    if lock_error is not None:
        return lock_error
    files = _collect_py_files(target)
    if not files:
        return _fail(
            f"No Python files found under: {target}",
            json_output,
            command="check",
        )
    trust = "trusted" if strict else "guarded"
    ty_ok, ty_output = _run_ty_check(target)
    if ty_ok:
        facts = collect_type_facts_from_paths(files, trust, infer=True)
        facts.tool = "molt-check+ty+infer"
        if verbose and not json_output:
            print("ty check passed; trusting inferred hints.")
    elif ty_output:
        warnings.append(ty_output)
        if not json_output:
            print(ty_output, file=sys.stderr)
        if strict:
            return _fail(
                "ty check failed; refusing strict type facts.",
                json_output,
                command="check",
            )
        facts = collect_type_facts_from_paths(files, trust, infer=False)
    else:
        facts = collect_type_facts_from_paths(files, trust, infer=False)
    output_path = Path(output)
    write_type_facts(output_path, facts)
    if json_output:
        payload = _json_payload(
            "check",
            "ok",
            data={
                "output": str(output_path),
                "strict": strict,
                "ty_ok": ty_ok,
                "deterministic": deterministic,
            },
            warnings=warnings,
        )
        _emit_json(payload, json_output)
    else:
        print(f"Wrote type facts to {output_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
