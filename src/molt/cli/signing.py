from __future__ import annotations

from dataclasses import dataclass
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
import tomllib
from typing import Any

from molt.cli.command_runtime import _run_completed_command
from molt.cli.file_hashing import _normalize_sha256, _sha256_file


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
        result = _run_completed_command(
            cmd,
            capture_output=True,
            env=None,
            cwd=artifact_path.parent,
            memory_guard_prefix="MOLT_BUILD",
        )
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
    result = _run_completed_command(
        ["codesign", "--display", "--verbose=4", str(artifact_path)],
        capture_output=True,
        env=None,
        cwd=artifact_path.parent,
        memory_guard_prefix="MOLT_BUILD",
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
    result = _run_completed_command(
        cmd,
        capture_output=True,
        env=None,
        cwd=artifact_path.parent,
        memory_guard_prefix="MOLT_BUILD",
    )
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


def _load_trust_policy(path: Path) -> TrustPolicy:
    if not path.exists():
        raise FileNotFoundError(f"Trust policy not found: {path}")
    if path.suffix == ".json":
        data = json.loads(path.read_text())
    else:
        data = tomllib.loads(path.read_text())
    cosign = data.get("cosign", {})
    codesign = data.get("codesign", {})
    cosign_keys: set[str] = set()
    for key in cosign.get("keys", []):
        if not isinstance(key, str):
            continue
        normalized = _normalize_sha256(key)
        if normalized:
            cosign_keys.add(normalized)
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
        result = _run_completed_command(
            cmd,
            capture_output=True,
            env=None,
            cwd=artifact_path.parent,
            memory_guard_prefix="MOLT_BUILD",
        )
        if result.returncode != 0:
            detail = (result.stderr or result.stdout).strip() or "unknown error"
            raise RuntimeError(f"cosign verify-blob failed: {detail}")


def _verify_codesign_signature(artifact_path: Path) -> None:
    result = _run_completed_command(
        ["codesign", "--verify", "--verbose=4", str(artifact_path)],
        capture_output=True,
        env=None,
        cwd=artifact_path.parent,
        memory_guard_prefix="MOLT_BUILD",
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip() or "unknown error"
        raise RuntimeError(f"codesign verify failed: {detail}")
