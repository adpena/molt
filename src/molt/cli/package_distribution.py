from __future__ import annotations

from contextlib import redirect_stderr, redirect_stdout
import hashlib
import io
import json
import os
from pathlib import Path
import tempfile
import tomllib
from typing import Any
import zipfile

from molt.cli.atomic_io import (
    _atomic_copy_file,
    _atomic_write_bytes,
    _atomic_write_json,
    _atomic_zip_file,
)
from molt.cli.capability_spec import (
    CapabilityInput,
    CapabilityManifest,
    _allowed_capabilities_for_package,
    _allowed_effects_for_package,
    _parse_capabilities_spec,
)
from molt.cli.extension_manifest import (
    ExtensionManifestValidation,
    _MOLT_C_API_VERSION_RE,
    _is_extension_manifest,
    _load_manifest,
    _manifest_errors,
    _normalize_effects,
    _validate_extension_manifest,
    _write_zip_member,
)
from molt.cli.file_hashing import _normalize_sha256, _sha256_file
from molt.cli.lockfiles import _check_lockfiles
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import fail as _fail
from molt.cli.output import json_payload as _json_payload
from molt.cli.package_registry import (
    _is_remote_registry,
    _remote_registry_destination,
    _remote_sidecar_url,
    _resolve_registry_auth,
    _resolve_registry_timeout,
    _upload_registry_file,
    _validate_registry_url,
)
from molt.cli.project_roots import _find_project_root
from molt.cli.sbom import _build_sbom
from molt.cli.signing import (
    TrustPolicy,
    _is_macho,
    _load_trust_policy,
    _resolve_signature_tool,
    _sign_artifact,
    _signature_metadata,
    _trust_policy_allows,
    _verify_codesign_signature,
    _verify_cosign_signature,
)


def _resolve_sidecar_path(output_path: Path, override: str | None, suffix: str) -> Path:
    if override:
        path = Path(override).expanduser()
        if not path.is_absolute():
            path = (output_path.parent / path).absolute()
        return path
    return output_path.with_name(output_path.stem + suffix)


def _resolve_extension_manifest_for_verify(
    wheel_path: Path,
) -> tuple[Path | None, tempfile.TemporaryDirectory[str] | None, str | None]:
    sibling_manifest = wheel_path.parent / "extension_manifest.json"
    if sibling_manifest.exists():
        return sibling_manifest, None, None
    try:
        with zipfile.ZipFile(wheel_path) as zf:
            manifest_bytes = zf.read("extension_manifest.json")
    except KeyError:
        return (
            None,
            None,
            "extension_manifest.json not found next to wheel or inside wheel.",
        )
    except (OSError, zipfile.BadZipFile) as exc:
        return None, None, f"Failed to inspect wheel {wheel_path}: {exc}"
    try:
        decoded = json.loads(manifest_bytes.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        return None, None, f"Invalid embedded extension_manifest.json: {exc}"
    if not isinstance(decoded, dict):
        return None, None, "Embedded extension_manifest.json must be a JSON object."
    tmpdir = tempfile.TemporaryDirectory(prefix="molt_ext_manifest_")
    manifest_path = Path(tmpdir.name) / "extension_manifest.json"
    _atomic_write_json(manifest_path, decoded, sort_keys=True, indent=2)
    return manifest_path, tmpdir, None


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
    with _atomic_zip_file(output_path) as zf:
        _write_zip_member(zf, "manifest.json", manifest_bytes)
        _write_zip_member(zf, f"artifact/{artifact_path.name}", artifact_bytes)
        if sbom_bytes is not None:
            _write_zip_member(zf, "sbom.json", sbom_bytes)
        _write_zip_member(zf, "signature.json", signature_meta_bytes)
        if signature_bytes is not None and signature_path is not None:
            _write_zip_member(zf, f"signature/{signature_path.name}", signature_bytes)

    if sbom_bytes is not None and sbom_path is not None:
        _atomic_write_bytes(sbom_path, sbom_bytes)
    _atomic_write_bytes(signature_meta_path, signature_meta_bytes)
    if signature_bytes is not None and signature_path is not None:
        _atomic_write_bytes(signature_path, signature_bytes)

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
    extension_manifest_tmpdir: tempfile.TemporaryDirectory[str] | None = None
    is_extension_wheel = source.suffix.lower() == ".whl"
    if verify_signature:
        require_signature = True
    should_verify = (
        deterministic
        or require_signature
        or verify_signature
        or trusted_signers is not None
    )

    def run_publish_verify(*verify_args: Any) -> tuple[int, str]:
        if not json_output:
            return verify(*verify_args), ""
        captured = io.StringIO()
        with redirect_stdout(captured), redirect_stderr(captured):
            code = verify(*verify_args)
        return code, captured.getvalue()

    if is_extension_wheel:
        manifest_path, extension_manifest_tmpdir, manifest_error = (
            _resolve_extension_manifest_for_verify(source)
        )
        if manifest_error is not None:
            if extension_manifest_tmpdir is not None:
                extension_manifest_tmpdir.cleanup()
            return _fail(manifest_error, json_output, command="publish")
        if manifest_path is None:
            if extension_manifest_tmpdir is not None:
                extension_manifest_tmpdir.cleanup()
            return _fail(
                "Failed to resolve extension manifest for wheel verification.",
                json_output,
                command="publish",
            )
        verify_code, verify_output = run_publish_verify(
            None,  # package_path
            str(manifest_path),  # manifest_path
            str(source),  # artifact_path
            True,  # require_checksum
            False,  # json_output
            verbose,
            deterministic,
            capabilities,
            require_signature,
            verify_signature,
            trusted_signers,
            signer,
            signing_key,
            True,  # require_extension_capabilities
            None,  # require_extension_abi
            True,  # extension_metadata
        )
        if verify_code != 0:
            if extension_manifest_tmpdir is not None:
                extension_manifest_tmpdir.cleanup()
            if json_output:
                verify_msg = (
                    verify_output.strip().splitlines()[-1]
                    if verify_output.strip()
                    else "extension publish verification failed"
                )
                return _fail(
                    f"Extension publish verification failed: {verify_msg}",
                    json_output,
                    command="publish",
                )
            return verify_code
    elif should_verify:
        verify_code, verify_output = run_publish_verify(
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
            if extension_manifest_tmpdir is not None:
                extension_manifest_tmpdir.cleanup()
            if json_output:
                verify_msg = (
                    verify_output.strip().splitlines()[-1]
                    if verify_output.strip()
                    else "publish verification failed"
                )
                return _fail(
                    f"Publish verification failed: {verify_msg}",
                    json_output,
                    command="publish",
                )
            return verify_code
    is_remote = _is_remote_registry(registry)
    sidecars: list[dict[str, str]] = []
    uploads: list[dict[str, Any]] = []
    auth_info = {"mode": "none", "source": "none"}
    if is_remote:
        url_error = _validate_registry_url(registry)
        if url_error:
            if extension_manifest_tmpdir is not None:
                extension_manifest_tmpdir.cleanup()
            return _fail(url_error, json_output, command="publish")
        try:
            headers, auth_info = _resolve_registry_auth(
                registry_token, registry_user, registry_password
            )
            timeout = _resolve_registry_timeout(registry_timeout)
        except RuntimeError as exc:
            if extension_manifest_tmpdir is not None:
                extension_manifest_tmpdir.cleanup()
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
                    if extension_manifest_tmpdir is not None:
                        extension_manifest_tmpdir.cleanup()
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
            _atomic_copy_file(source, dest)
        for suffix in (".sbom.json", ".sig.json", ".sig"):
            sidecar_src = source.with_name(source.stem + suffix)
            if not sidecar_src.exists():
                continue
            sidecar_dest = dest.with_name(dest.stem + suffix)
            sidecars.append({"source": str(sidecar_src), "dest": str(sidecar_dest)})
            if not dry_run:
                sidecar_dest.parent.mkdir(parents=True, exist_ok=True)
                _atomic_copy_file(sidecar_src, sidecar_dest)
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
                "extension_wheel": is_extension_wheel,
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
    if extension_manifest_tmpdir is not None:
        extension_manifest_tmpdir.cleanup()
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
    require_extension_capabilities: bool = False,
    require_extension_abi: str | None = None,
    extension_metadata: bool | None = None,
) -> int:
    errors: list[str] = []
    warnings: list[str] = []
    manifest: dict[str, Any] | None = None
    manifest_file: Path | None = None
    artifact_name = None
    artifact_bytes = None
    artifact_file: Path | None = None
    checksum: str | None = None
    extension_mode = False
    extension_validation: ExtensionManifestValidation | None = None
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
    required_extension_abi: str | None = None
    if require_extension_abi is not None:
        normalized = require_extension_abi.strip()
        if _MOLT_C_API_VERSION_RE.match(normalized) is None:
            return _fail(
                "Invalid --require-extension-abi value. Expected MAJOR[.MINOR[.PATCH]].",
                json_output,
                command="verify",
            )
        required_extension_abi = normalized

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
        manifest_file = Path(manifest_path)
        manifest = _load_manifest(manifest_file)
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
        extension_mode = (
            extension_metadata
            if extension_metadata is not None
            else _is_extension_manifest(manifest)
        )
        if extension_mode:
            if not _is_extension_manifest(manifest):
                errors.append(
                    "Manifest does not match extension metadata schema "
                    "(disable with --no-extension-metadata)."
                )
            else:
                manifest_dir = (
                    manifest_file.parent
                    if manifest_file is not None
                    else (
                        Path(package_path).parent
                        if package_path is not None
                        else Path.cwd()
                    )
                )
                extension_wheel: Path | None = None
                if artifact_file is not None and artifact_file.suffix == ".whl":
                    extension_wheel = artifact_file
                extension_validation = _validate_extension_manifest(
                    manifest,
                    manifest_dir=manifest_dir,
                    wheel_path=extension_wheel,
                    require_capabilities=require_extension_capabilities,
                    required_abi=required_extension_abi,
                    require_checksum=require_checksum,
                    warn_missing_checksum=not require_checksum,
                )
                errors.extend(extension_validation.errors)
                warnings.extend(extension_validation.warnings)
                wheel_checksum = manifest.get("wheel_sha256")
                checksum = wheel_checksum if isinstance(wheel_checksum, str) else None
                if require_deterministic and manifest.get("deterministic") is not True:
                    errors.append("manifest is not deterministic")
                required_caps = extension_validation.capabilities
                if capabilities_list is None and required_caps:
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
        else:
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
                            "effects missing from allowlist: "
                            + ", ".join(missing_effects)
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
            if artifact_file is None:
                filename = Path(artifact_name).name if artifact_name else "artifact.bin"
                artifact_fs_path = temp_dir / filename
                artifact_fs_path.write_bytes(artifact_bytes)
            else:
                artifact_fs_path = artifact_file
            tool = _resolve_signature_tool(
                signer, signer_meta, artifact_fs_path, signature_bytes
            )
            try:
                if tool == "cosign":
                    if signature_bytes is None:
                        raise RuntimeError("cosign signature file is missing")
                    if not key:
                        raise RuntimeError(
                            "cosign verification requires --signing-key or COSIGN_KEY"
                        )
                    _verify_cosign_signature(artifact_fs_path, signature_bytes, key)
                elif tool == "codesign":
                    if not _is_macho(artifact_fs_path):
                        raise RuntimeError(
                            "codesign verification requires a Mach-O artifact"
                        )
                    _verify_codesign_signature(artifact_fs_path)
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
        data: dict[str, Any] = {
            "artifact": artifact_name,
            "deterministic": require_deterministic,
            "capability_profiles": capability_profiles,
            "signature_status": signature_status
            or ("signed" if signed else "unsigned"),
            "signature_verified": signature_verified,
            "trust_status": trust_status,
        }
        if extension_mode:
            data["extension_metadata"] = True
            data["extension_require_capabilities"] = require_extension_capabilities
            data["extension_require_abi"] = required_extension_abi
            if extension_validation is not None:
                data["extension_wheel"] = (
                    str(extension_validation.wheel_path)
                    if extension_validation.wheel_path is not None
                    else None
                )
                data["extension_abi"] = extension_validation.abi_version
                data["extension_abi_tag"] = extension_validation.abi_tag
                data["extension_capabilities"] = extension_validation.capabilities
                data["extension_wheel_tags"] = (
                    {
                        "python": extension_validation.wheel_tags[0],
                        "abi": extension_validation.wheel_tags[1],
                        "platform": extension_validation.wheel_tags[2],
                    }
                    if extension_validation.wheel_tags is not None
                    else None
                )
        payload = _json_payload(
            "verify",
            status,
            data=data,
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
