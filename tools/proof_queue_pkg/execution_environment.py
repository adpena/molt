"""Deterministic execution-environment and source-custody authority."""

from __future__ import annotations

import hashlib
import hmac
import json
import os
from pathlib import Path
import re
import shlex
import stat
import sys
import time
from typing import Callable, Mapping, Sequence, cast

from molt.exact_json import canonical_json_sha256
from molt import file_publication
from molt.python_environment_identity import (
    PythonEnvironmentIdentityError,
    validate_python_environment_location,
)
from tools import proof_plan
from tools.proof_queue_pkg import command_admission as admission
from tools.proof_queue_pkg import command_identity
from tools.proof_queue_pkg import custody_cas
from tools.proof_queue_pkg import process_image_capture
from tools.proof_queue_pkg import supervisor_custody
from tools.proof_queue_pkg import toolchain_capture


def command_secret_policy_error(command: Sequence[str]) -> str | None:
    for index, value in enumerate(command):
        if re.search(r"://[^/@\s]+@", value):
            return f"command argument {index} embeds URL credentials"
        if command_identity._SECRET_ARGUMENT_FLAG.match(value):
            return (
                f"secret-bearing command option {value.split('=', 1)[0]!r} is forbidden"
            )
    return None


def _environment_name_class(name: str) -> str | None:
    upper = name.upper()
    if upper in command_identity._QUEUE_CUSTODY_ENV_NAMES - {"PYTHONPATH"}:
        return "queue-owned-custody"
    if upper in command_identity._NONDETERMINISTIC_ENV_NAMES:
        return "denied-nondeterministic"
    if upper in command_identity._ENVIRONMENT_EXACT_NAMES:
        return "host-runtime"
    if upper in command_identity._ENVIRONMENT_BUILD_NAMES:
        return "build-toolchain"
    if any(
        upper.startswith(prefix) for prefix in command_identity._ENVIRONMENT_PREFIXES
    ):
        return "semantic-prefix"
    return None


def environment_override_policy_error(env_overrides: Mapping[str, str]) -> str | None:
    seen: dict[str, str] = {}
    for name, value in sorted(env_overrides.items()):
        if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name) is None:
            return f"non-canonical environment override name {name!r}"
        folded = name.casefold()
        if folded in seen:
            return (
                "case-ambiguous environment overrides are forbidden: "
                f"{seen[folded]!r}, {name!r}"
            )
        seen[folded] = name
        if (
            name.upper() in command_identity._CANONICAL_EXECUTION_ENV
            or name.upper() == "NODE_OPTIONS"
            or name.upper() in command_identity._QUEUE_CUSTODY_ENV_NAMES
        ):
            return f"queue-owned canonical environment override {name!r} is forbidden"
        classification = _environment_name_class(name)
        if classification is None:
            return f"unclassified environment override {name!r}"
        if classification == "denied-nondeterministic":
            return f"nondeterministic environment override {name!r} is forbidden"
        if command_identity._SECRET_ENV_NAME.search(name):
            return f"secret-bearing environment override {name!r} is forbidden"
        if "\x00" in value or "\n" in value or "\r" in value:
            return f"environment override {name!r} has non-canonical control characters"
        if re.search(r"://[^/@\s]+@", value):
            return f"environment override {name!r} embeds URL credentials"
    return None


def _deterministic_execution_environment(
    inherited: Mapping[str, str], *, override_names: Sequence[str]
) -> tuple[dict[str, str], dict[str, object]]:
    override_keys = [name.casefold() for name in override_names]
    if len(override_keys) != len(set(override_keys)):
        raise ValueError("environment overrides contain case-ambiguous names")
    overrides = set(override_keys)
    selected: dict[str, str] = {}
    omitted: list[str] = []
    seen_names: set[str] = set()
    for name, value in inherited.items():
        folded = name.casefold()
        if folded in seen_names:
            raise ValueError(f"execution environment has case-ambiguous name {name!r}")
        seen_names.add(folded)
        classification = _environment_name_class(name)
        if classification in {
            None,
            "denied-nondeterministic",
        } or command_identity._SECRET_ENV_NAME.search(name):
            omitted.append(name)
            continue
        selected[name] = str(value)
    missing = sorted(
        name
        for name in override_names
        if name.casefold() not in {key.casefold() for key in selected}
    )
    if missing:
        raise ValueError(
            "classified environment overrides disappeared: " + ", ".join(missing)
        )
    contract: dict[str, object] = {
        "schema": "molt.proof-execution-environment.v1",
        "passed_names": sorted(selected, key=str.casefold),
        "override_names": sorted(
            (name for name in selected if name.casefold() in overrides),
            key=str.casefold,
        ),
        "omitted_names": sorted(omitted, key=str.casefold),
    }
    return selected, contract


def _execution_environment_authority(
    env: Mapping[str, str],
    *,
    applied_cargo_policies: Sequence[str],
    fingerprint_key: bytes,
    contract: Mapping[str, object],
) -> dict[str, object]:
    names = sorted(env, key=str.casefold)
    values: dict[str, object] = {}
    for name in names:
        normalized = str(env[name]).replace("\\", "/")
        values[name] = {
            "class": (
                "queue-owned-custody"
                if name.upper() == "PYTHONPATH"
                else _environment_name_class(name)
            ),
            "fingerprint": hmac.new(
                fingerprint_key,
                f"{name.casefold()}\0{normalized}".encode(),
                hashlib.sha256,
            ).hexdigest(),
            "redacted": True,
        }
    payload: dict[str, object] = {
        **dict(contract),
        "variables": values,
        "cargo_policies": list(applied_cargo_policies),
        "fingerprint_key_id": hashlib.sha256(fingerprint_key).hexdigest(),
        "canonical_values_sha256": _canonical_environment_sha256(env),
    }
    payload["identity_sha256"] = hashlib.sha256(
        json.dumps(payload, sort_keys=True).encode()
    ).hexdigest()
    return payload


def _canonical_environment_sha256(env: Mapping[str, str]) -> str:
    """Bind every admitted environment value without publishing those values."""
    canonical = {name: str(env[name]) for name in sorted(env, key=str.casefold)}
    return hashlib.sha256(
        json.dumps(canonical, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def _execution_environment_executable_identities(
    env: Mapping[str, str], *, cwd: Path
) -> dict[str, object]:
    identities: dict[str, object] = {}
    for name, value in sorted(env.items()):
        upper = name.upper()
        if upper not in command_identity._EXECUTABLE_ENV_NAMES and not any(
            pattern.fullmatch(upper)
            for pattern in command_identity._EXECUTABLE_ENV_PATTERNS
        ):
            continue
        if not value:
            continue
        try:
            parts = shlex.split(value, posix=os.name != "nt")
        except ValueError as exc:
            raise ValueError(f"executable environment {name} is malformed") from exc
        if not parts:
            raise ValueError(f"executable environment {name} is empty")
        token = parts[0].strip('"')
        path = command_identity._resolve_outer_executable(token, cwd=cwd, env=env)
        identity = command_identity._executable_identity(path)
        if not command_identity._content_identity_available(identity):
            raise ValueError(f"executable environment {name} has no content identity")
        identities[name] = {
            "executable": identity,
            "argument_count": len(parts) - 1,
        }
    return identities


def _capture_toolchains(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    source_root: Path,
    hash_workers: int,
    located_toolchains: Mapping[str, object],
) -> tuple[dict[str, object] | None, dict[str, object]]:
    plan = proof_plan.ProofPlan.load()
    requested_raw = envelope.get("toolchains")
    if (
        not isinstance(requested_raw, list)
        or not requested_raw
        or not all(isinstance(name, str) and name for name in requested_raw)
    ):
        raise ValueError("proof command envelope has no non-empty toolchain authority")
    requested = [str(name) for name in requested_raw]
    if len(requested) != len(set(requested)):
        raise ValueError("proof command envelope has duplicate toolchain authorities")
    known = {policy.name for policy in plan.toolchain_policies}
    unknown = sorted(set(requested) - known)
    if unknown:
        raise ValueError(f"proof command envelope has unknown toolchains: {unknown!r}")
    located_python = located_toolchains.get("python")
    if "python" in requested and not isinstance(located_python, Mapping):
        raise ValueError("located Python selection identity is unavailable")
    proof_python = command_identity._python_identity(
        envelope,
        exact,
        cwd=cwd,
        env=env,
        source_root=source_root,
        selection=cast(Mapping[str, object], located_python),
        hash_workers=hash_workers,
    )
    if proof_python is None and "python" in requested:
        synthetic_envelope = admission.envelope_for_command(
            [sys.executable, "-c", "raise SystemExit('identity-only')"]
        )
        proof_python = command_identity._python_identity(
            synthetic_envelope,
            [sys.executable, "-c", "raise SystemExit('identity-only')"],
            cwd=cwd,
            env=env,
            source_root=source_root,
            selection=cast(Mapping[str, object], located_python),
            hash_workers=hash_workers,
        )
    toolchains: dict[str, object] = {}
    if proof_python is not None:
        toolchains["python"] = proof_python
    for name in requested:
        if name == "python":
            continue
        located = located_toolchains.get(name)
        if not isinstance(located, Mapping):
            raise ValueError(f"located {name} toolchain identity is unavailable")
        process_image_capture.revalidate_images(
            process_image_capture.toolchain_images(name, located)
        )
        if name == "rustc":
            toolchain_capture.revalidate_rust_link_process_images(
                located,
                target=command_identity._rust_target(exact, env),
                command_argv=admission._nested_command(exact) or exact,
            )
        for frozen in toolchain_capture.frozen_files({name: located}):
            path = Path(frozen.path)
            if command_identity._hash_file(path) != frozen.sha256 or (
                frozen.size is not None and path.stat().st_size != frozen.size
            ):
                raise ValueError(
                    f"{name} toolchain file changed while live custody armed: {path}"
                )
        toolchains[name] = dict(located)
    if set(toolchains) != set(requested):
        raise ValueError(
            "proof command toolchain capture is incomplete: "
            f"requested={sorted(requested)!r} captured={sorted(toolchains)!r}"
        )
    return proof_python, toolchains


def _locate_toolchain_watch_roots(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    supervisor_binary: Path,
) -> tuple[list[Path], dict[str, object], dict[str, object]]:
    """Locate broad roots and child executables without a full inventory."""
    started = time.perf_counter()
    plan = proof_plan.ProofPlan.load()
    requested_raw = envelope.get("toolchains")
    if not isinstance(requested_raw, list) or not all(
        isinstance(name, str) and name for name in requested_raw
    ):
        raise ValueError("proof command envelope has no toolchain authority")
    requested = [str(name) for name in requested_raw]
    policies = {policy.name: policy for policy in plan.toolchain_policies}
    roots: list[Path] = []
    policy_identities: dict[str, object] = {}
    if "python" in requested:
        python_envelope = envelope
        python_exact = list(exact)
        if envelope.get("python") is None:
            python_envelope = admission.envelope_for_command(
                [sys.executable, "-c", "raise SystemExit('location-only')"]
            )
            python_exact = [sys.executable, "-c", "raise SystemExit('location-only')"]
        command = command_identity._python_auxiliary_command(
            python_envelope,
            python_exact,
            authority=admission._PYTHON_IDENTITY_PROBE,
            arguments=("--locate-active-environment",),
            isolated=True,
            no_site=True,
        )
        if command is None:
            raise ValueError("proof Python locator has no selected interpreter")
        located = command_identity._parse_json_output(
            command_identity._run_captured(command, cwd=cwd, env=env),
            purpose="proof Python toolchain locator",
        )
        try:
            located = validate_python_environment_location(located)
        except PythonEnvironmentIdentityError as exc:
            raise ValueError(
                f"proof Python toolchain locator is invalid: {exc}"
            ) from exc
        command_identity._reject_python_location_onedrive(located)
        executable_raw = located.get("selected_executable")
        base_executable_raw = located.get("base_executable")
        prefix_raw = located.get("prefix")
        if (
            not isinstance(executable_raw, str)
            or not isinstance(base_executable_raw, str)
            or not isinstance(prefix_raw, str)
        ):
            raise ValueError("proof Python toolchain locator has no executable chain")
        executable = Path(os.path.abspath(executable_raw))
        if not executable.is_file():
            raise ValueError(
                "proof Python toolchain locator has no selected executable"
            )
        base_executable = Path(base_executable_raw).resolve(strict=True)
        prefix = Path(prefix_raw).resolve(strict=True)
        if not prefix.is_dir():
            raise ValueError("proof Python toolchain locator has no environment prefix")
        policy_identities["python"] = {
            "executable": str(executable),
            "executable_sha256": command_identity._hash_file(executable),
            "base_executable": str(base_executable),
            "base_executable_sha256": command_identity._hash_file(base_executable),
            "prefix": str(prefix),
            "external_roots": [],
            "location": located,
        }
        for field in ("roots", "external_roots"):
            values = located.get(field)
            if not isinstance(values, list) or not all(
                isinstance(value, str) for value in values
            ):
                raise ValueError(f"proof Python locator has malformed {field}")
            for value in cast(list[str], values):
                path = Path(value)
                if path.is_dir():
                    resolved = path.resolve(strict=True)
                    roots.append(resolved)
                    if field == "external_roots":
                        external = policy_identities["python"]["external_roots"]
                        assert isinstance(external, list)
                        external.append(str(resolved))
    for name in requested:
        if name == "python":
            continue
        identity = command_identity._tool_identity(
            plan, name, envelope, exact, cwd=cwd, env=env
        )
        probes = policies[name].data.get("process_image_probes", [])
        assert isinstance(probes, list)
        if probes:
            process_images = list(identity["process_images"])
            inventories: list[dict[str, object]] = []
            for probe in probes:
                assert isinstance(probe, list)
                observed, telemetry = (
                    supervisor_custody.capture_process_image_inventory(
                        binary=supervisor_binary,
                        role=name,
                        executable=Path(str(identity["path"])),
                        probe_args=[str(value) for value in probe],
                        cwd=cwd,
                        env=env,
                    )
                )
                process_images.extend(observed)
                inventories.append(telemetry)
            identity["process_images"] = process_image_capture.canonical_images(
                process_images
            )
            identity["process_image_inventories"] = inventories
            identity_without_digest = dict(identity)
            identity_without_digest.pop("identity_sha256", None)
            identity["identity_sha256"] = canonical_json_sha256(identity_without_digest)
        command_identity._validate_toolchain_identity(plan, name, identity)
        policy_identities[name] = identity
        roots.extend(_broad_toolchain_roots({name: identity}))
    roots = list(dict.fromkeys(roots))
    telemetry = {
        "schema": "molt.proof-toolchain-location-telemetry.v1",
        "non_python_selection_capture_count": sum(
            name != "python" for name in requested
        ),
        "root_count": len(roots),
        "locate_s": time.perf_counter() - started,
    }
    return roots, policy_identities, telemetry


def _python_editable_ineligible_reasons(
    identity: Mapping[str, object] | None,
    *,
    source_snapshot: Mapping[str, object],
) -> list[str]:
    if identity is None:
        return []
    reasons: list[str] = []
    environment = identity.get("environment")
    if not isinstance(environment, Mapping):
        return ["python-environment-closure-malformed"]
    external_roots = environment.get("external_roots")
    if not isinstance(external_roots, list):
        return ["python-external-root-closure-malformed"]
    roots = {
        str(row.get("id")): str(row.get("path"))
        for row in external_roots
        if isinstance(row, Mapping)
        and isinstance(row.get("id"), str)
        and isinstance(row.get("path"), str)
    }
    if len(roots) != len(external_roots):
        reasons.append("python-external-root-closure-malformed")
    distributions = environment.get("distributions")
    if not isinstance(distributions, list):
        return ["python-distribution-inventory-malformed"]
    source_root = source_snapshot.get("root")
    for distribution in distributions:
        if not isinstance(distribution, Mapping):
            reasons.append("python-distribution-inventory-malformed")
            continue
        external = distribution.get("external_source")
        if external is None:
            continue
        name = str(distribution.get("name") or "unknown")
        if not isinstance(external, Mapping):
            reasons.append(f"python-editable-source-malformed:{name}")
            continue
        root = roots.get(str(external.get("root")))
        inside_source = False
        if root is not None and isinstance(source_root, str):
            root_path = Path(root)
            source_path = Path(source_root)
            inside_source = os.path.normcase(str(root_path)) == os.path.normcase(
                str(source_path)
            ) or root_path.is_relative_to(source_path)
        if not inside_source:
            reasons.append(f"python-editable-source-outside:{name}")
        if source_snapshot.get("available") is not True:
            reasons.append(f"python-editable-source-git-unavailable:{name}")
        elif source_snapshot.get("clean") is not True:
            reasons.append(f"python-editable-source-dirty:{name}")
    return reasons


def _git_snapshot(cwd: Path, env: Mapping[str, str]) -> dict[str, object]:
    head = command_identity._run_captured(
        ("git", "rev-parse", "HEAD"), cwd=cwd, env=env
    )
    if head.returncode != 0 or not re.fullmatch(
        r"[0-9a-fA-F]{40}|[0-9a-fA-F]{64}", head.stdout.strip()
    ):
        return {
            "available": False,
            "clean": False,
            "commit": None,
            "status_sha256": None,
        }
    root = command_identity._run_captured(
        ("git", "rev-parse", "--show-toplevel"), cwd=cwd, env=env
    )
    if root.returncode != 0:
        return {
            "available": False,
            "clean": False,
            "commit": head.stdout.strip().lower(),
            "status_sha256": None,
        }
    source_root = Path(root.stdout.strip()).resolve(strict=True)
    tree = command_identity._run_captured(
        ("git", "rev-parse", "HEAD^{tree}"), cwd=cwd, env=env
    )
    if tree.returncode != 0 or not re.fullmatch(
        r"[0-9a-fA-F]{40}|[0-9a-fA-F]{64}", tree.stdout.strip()
    ):
        return {
            "available": False,
            "clean": False,
            "commit": head.stdout.strip().lower(),
            "tree": None,
            "status_sha256": None,
        }
    status = command_identity._run_captured(
        (
            "git",
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ),
        cwd=cwd,
        env=env,
        text=False,
    )
    if status.returncode != 0:
        return {
            "available": False,
            "clean": False,
            "commit": head.stdout.strip().lower(),
            "status_sha256": None,
        }
    return {
        "available": True,
        "root": str(source_root),
        "clean": not status.stdout,
        "commit": head.stdout.strip().lower(),
        "tree": tree.stdout.strip().lower(),
        "status_sha256": hashlib.sha256(status.stdout).hexdigest(),
    }


def _git_tracked_paths(cwd: Path, env: Mapping[str, str]) -> list[Path]:
    return [path for path in _git_source_paths(cwd, env) if path.is_file()]


def _git_source_paths(
    cwd: Path, env: Mapping[str, str], *, include_untracked: bool = False
) -> list[Path]:
    options = ("--others", "--exclude-standard") if include_untracked else ()
    completed = command_identity._run_captured(
        ("git", "ls-files", "--cached", *options, "--full-name", "-z"),
        cwd=cwd,
        env=env,
        text=False,
    )
    if completed.returncode != 0:
        raise ValueError("proof source custody cannot enumerate tracked inputs")
    root_result = command_identity._run_captured(
        ("git", "rev-parse", "--show-toplevel"), cwd=cwd, env=env
    )
    if root_result.returncode != 0:
        raise ValueError("proof source custody cannot resolve its Git root")
    root = Path(root_result.stdout.strip()).resolve(strict=True)
    paths: list[Path] = []
    for raw in completed.stdout.split(b"\0"):
        if not raw:
            continue
        relative = Path(os.fsdecode(raw))
        candidate = Path(os.path.abspath(root / relative))
        if not candidate.is_relative_to(root):
            raise ValueError("Git source input escaped its root")
        paths.append(candidate)
    return sorted(set(paths), key=lambda path: path.as_posix())


SOURCE_CONTENT_KIND = "molt.proof-source-content.v1"


def capture_source_content(
    *,
    source_root: Path,
    env: Mapping[str, str],
    overlays: Sequence[Path],
    cas_root: Path,
) -> tuple[dict[str, object], dict[str, object], Callable[[], None]]:
    """Capture Git-admitted source and explicit overlays, never local caches."""
    from molt.python_file_node_custody import PythonFileCaptureContext

    started = time.perf_counter()
    root = file_publication.resolve_owned_path(source_root)
    selected = _git_source_paths(root, env, include_untracked=True)
    paths = sorted(set(selected) | set(overlays), key=lambda path: path.as_posix())
    regular: list[tuple[Path, os.stat_result]] = []
    missing: list[Path] = []
    hardlinks: dict[tuple[int, int], tuple[int, int]] = {}
    for path in paths:
        if not path.is_relative_to(root):
            raise ValueError("source content overlay escaped admitted Git root")
        file_publication.resolve_owned_path(path)
        try:
            metadata = path.lstat()
        except FileNotFoundError:
            missing.append(path)
            continue
        if not stat.S_ISREG(metadata.st_mode):
            raise ValueError(
                f"source content input is not a direct regular file: {path}"
            )
        regular.append((path, metadata))
        key = metadata.st_dev, metadata.st_ino
        count, links = hardlinks.get(key, (0, metadata.st_nlink))
        hardlinks[key] = count + 1, links
    if any(count != links for count, links in hardlinks.values()):
        raise ValueError("source input has aliases outside admitted source content")
    capture = PythonFileCaptureContext(hash_workers=1)
    capture.prepare(regular, label="proof source input")
    files: list[dict[str, object]] = []
    for path, metadata in regular:
        identity = capture.bind(path, metadata, label="proof source input")
        files.append(
            {
                "path": path.relative_to(root).as_posix(),
                "sha256": identity.sha256,
                "size": identity.size,
                "mode": stat.S_IMODE(metadata.st_mode),
            }
        )

    def verify_selection() -> None:
        if selected != _git_source_paths(root, env, include_untracked=True):
            raise ValueError("source content admission set changed during capture")
        if any(path.exists() for path in missing):
            raise ValueError("missing source input appeared during capture")

    capture.register_verification_fence(verify_selection)
    capture.verify()
    payload = {
        "schema": custody_cas.ARTIFACT_SCHEMA,
        "kind": SOURCE_CONTENT_KIND,
        "root": str(root),
        "selection": "git-tracked-plus-nonignored-untracked-and-overlays",
        "files": files,
        "missing": [path.relative_to(root).as_posix() for path in missing],
    }
    reference = custody_cas.put_json(cas_root, payload).as_dict()
    capture.verify()
    return (
        reference,
        {
            "file_count": len(files),
            "bytes_hashed": sum(int(row["size"]) for row in files),
            "capture_s": time.perf_counter() - started,
        },
        capture.verify,
    )


def _broad_toolchain_roots(toolchains: Mapping[str, object]) -> list[Path]:
    roots: list[Path] = []
    for identity in toolchains.values():
        if not isinstance(identity, Mapping):
            continue
        node_package = identity.get("node_package")
        package = (
            node_package.get("package") if isinstance(node_package, Mapping) else None
        )
        raw_root = package.get("root") if isinstance(package, Mapping) else None
        if isinstance(raw_root, str) and Path(raw_root).is_dir():
            roots.append(Path(raw_root).resolve(strict=True))
    return list(dict.fromkeys(roots))
