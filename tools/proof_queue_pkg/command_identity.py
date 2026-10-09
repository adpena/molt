"""Executable, interpreter, and toolchain identity authority."""

from __future__ import annotations

from molt.llvm_toolchain import capture_wasi_sdk_selection

import contextlib
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import stat
import subprocess
import sys
from typing import Any, BinaryIO, Iterable, Mapping, Sequence, cast

from molt import file_publication
from molt.dx import _reject_onedrive
from molt.toolchain_identity import executable_environment_value, find_executable
from molt.exact_json import ExactJsonError, canonical_json_sha256, loads_exact
from molt.rust_toolchain import cargo_config_arguments, cargo_configuration_paths
from molt.python_environment_identity import (
    PYTHON_CAPTURE_SCHEMA,
    PythonEnvironmentIdentityError,
    python_environment_executable_files,
    python_identity_probe_arguments,
    validate_python_capture,
    validate_python_environment_location,
)
from molt.source_extension_link_inputs import SOURCE_EXTENSION_LINK_INPUTS_ENV
from tools import proof_plan
from tools.proof_queue_pkg import command_admission as admission
from tools.proof_queue_pkg import process_image_capture, toolchain_capture
from tools.toolchain_probe import resolve_single_file_path


def _hash_file(path: Path) -> str:
    try:
        path = process_image_capture.custody_path(path)
        with path.open("rb") as handle:
            return hashlib.file_digest(handle, "sha256").hexdigest()
    except OSError as exc:
        return f"unavailable:{type(exc).__name__}"


def _directory_manifest_identity(
    path: Path, *, label: str, strict_owned: bool = False
) -> dict[str, object]:
    if strict_owned:
        return _owned_directory_manifest_identity(path, label=label)
    root = path.resolve(strict=True)
    if not root.is_dir():
        raise ValueError(f"{label} is not a directory: {root}")
    files: list[dict[str, object]] = []
    for candidate in sorted(root.rglob("*"), key=lambda value: value.as_posix()):
        if candidate.is_symlink() and candidate.is_dir():
            raise ValueError(
                f"{label} contains an unowned directory symlink: {candidate}"
            )
        if not candidate.is_file():
            continue
        resolved = candidate.resolve(strict=True)
        try:
            resolved.relative_to(root)
        except ValueError as exc:
            raise ValueError(
                f"{label} file escapes its package root: {candidate} -> {resolved}"
            ) from exc
        size = resolved.stat().st_size
        digest = _hash_file(resolved)
        if re.fullmatch(r"[0-9a-f]{64}", digest) is None:
            raise ValueError(f"{label} file has no content identity: {resolved}")
        files.append(
            {
                "relative_path": candidate.relative_to(root).as_posix(),
                "lexical_path": str(candidate.absolute()),
                "resolved_path": str(resolved),
                "symlinked": os.path.normcase(str(candidate.absolute()))
                != os.path.normcase(str(resolved)),
                "size": size,
                "sha256": digest,
            }
        )
    return {
        "root": str(root),
        "file_count": len(files),
        "files": files,
        "manifest_sha256": canonical_json_sha256(files),
    }


def _validate_directory_manifest_identity(
    value: object, *, selected_root: Path
) -> None:
    """Validate the existing directory projection at receipt receivers."""
    if (
        not isinstance(value, Mapping)
        or set(value) != {"root", "file_count", "files", "manifest_sha256"}
        or value["root"] != str(selected_root.resolve(strict=False))
        or not selected_root.is_absolute()
        or not isinstance(value["files"], list)
        or type(value["file_count"]) is not int
        or value["file_count"] != len(value["files"])
        or value["manifest_sha256"] != canonical_json_sha256(value["files"])
    ):
        raise ValueError("directory resource identity is malformed or substituted")
    root = Path(value["root"])
    members = []
    for row in value["files"]:
        if not isinstance(row, Mapping) or set(row) != {
            "relative_path",
            "lexical_path",
            "resolved_path",
            "symlinked",
            "size",
            "sha256",
        }:
            raise ValueError("directory resource member identity is malformed")
        relative = row["relative_path"]
        if (
            not isinstance(relative, str)
            or not relative
            or "\\" in relative
            or "\0" in relative
            or any(part in {"", ".", ".."} for part in relative.split("/"))
            or Path(relative).is_absolute()
            or re.match(r"^[A-Za-z]:", relative)
            or row["lexical_path"] != str(root / relative)
            or not isinstance(row["resolved_path"], str)
            or not Path(row["resolved_path"]).is_absolute()
            or ".." in Path(row["resolved_path"]).parts
            or str(Path(row["resolved_path"])) != row["resolved_path"]
            or not Path(row["resolved_path"]).is_relative_to(root)
            or type(row["size"]) is not int
            or row["size"] < 0
            or not isinstance(row["sha256"], str)
            or re.fullmatch(r"[0-9a-f]{64}", row["sha256"]) is None
            or type(row["symlinked"]) is not bool
            or row["symlinked"]
            != (
                os.path.normcase(row["lexical_path"])
                != os.path.normcase(row["resolved_path"])
            )
        ):
            raise ValueError(
                "directory resource member escapes or lacks content custody"
            )
        members.append(relative)
    if members != sorted(set(members)):
        raise ValueError("directory resource members are not canonical")


def _revalidate_directory_manifest_identity(
    value: Mapping[str, object], *, selected_root: Path, label: str
) -> None:
    _validate_directory_manifest_identity(value, selected_root=selected_root)
    if _directory_manifest_identity(selected_root, label=label) != value:
        raise ValueError(
            f"{label} membership or content changed while live custody armed"
        )


def _validate_node_package_identity(
    policy: proof_plan.ToolchainPolicy,
    identity: Mapping[str, object],
    *,
    full_capture: bool = False,
) -> Mapping[str, object] | None:
    expected = policy.data.get("node_package")
    value = identity.get("node_package")
    if expected is None:
        if value is not None:
            raise ValueError("unexpected node package closure")
        return None
    if (
        not isinstance(value, Mapping)
        or set(value)
        != {"name", "entry", "manifest", "resolver", "selection_files", "package"}
        or value["name"] != expected
        or not isinstance(value["package"], Mapping)
        or not isinstance(value["package"].get("root"), str)
    ):
        raise ValueError("node package closure is incomplete")
    root = Path(value["package"]["root"])
    if not root.is_absolute() or str(root.resolve(strict=False)) != str(root):
        raise ValueError("node package selection root is not canonical")
    facts = value["selection_files"]
    if (
        not isinstance(facts, list)
        or len(facts) != 2
        or any(
            not isinstance(row, Mapping)
            or set(row) != {"path", "size_bytes", "sha256"}
            or not isinstance(row["path"], str)
            or not Path(row["path"]).is_relative_to(root)
            or ".." in Path(row["path"]).parts
            or type(row["size_bytes"]) is not int
            or row["size_bytes"] < 0
            or not isinstance(row["sha256"], str)
            or re.fullmatch(r"[0-9a-f]{64}", row["sha256"]) is None
            for row in facts
        )
        or [row["path"] for row in facts] != [value["entry"], value["manifest"]]
    ):
        raise ValueError("node package finite selection custody is incomplete")
    if set(value["package"]) == {"root"}:
        if full_capture:
            raise ValueError("node package requires an armed content inventory")
        return value["package"]
    _validate_directory_manifest_identity(value["package"], selected_root=root)
    captured = {
        row["resolved_path"]: (row["size"], row["sha256"])
        for row in value["package"]["files"]
    }
    if any(
        captured.get(row["path"]) != (row["size_bytes"], row["sha256"]) for row in facts
    ):
        raise ValueError("node package finite selection changed before armed capture")
    return value["package"]


def _owned_directory_manifest_identity(path: Path, *, label: str) -> dict[str, object]:
    """Project existing handle-bound file and no-follow topology custody."""
    from molt.python_file_node_custody import (
        PythonFileCaptureContext,
        _tree_membership_snapshot,
        _snapshot_fingerprint,
        _snapshot_difference,
    )

    root = file_publication.resolve_owned_path(path)
    before_root, before = _tree_membership_snapshot(root, label=label)
    expected = _snapshot_fingerprint(before_root, before)
    regular: list[tuple[Path, os.stat_result]] = []
    directories: list[str] = []
    hardlinks: dict[tuple[int, int], tuple[int, int]] = {}
    for relative, candidate, metadata in before:
        if stat.S_ISDIR(metadata.st_mode):
            directories.append(relative)
        elif stat.S_ISREG(metadata.st_mode):
            regular.append((candidate, metadata))
            key = metadata.st_dev, metadata.st_ino
            count, links = hardlinks.get(key, (0, metadata.st_nlink))
            if links != metadata.st_nlink:
                raise ValueError(f"{label} hard-link custody changed: {candidate}")
            hardlinks[key] = count + 1, links
        else:
            raise ValueError(
                f"{label} contains a link or junction or special entry: {candidate}"
            )
    if any(count != links for count, links in hardlinks.values()):
        raise ValueError(f"{label} has hard links outside the owned root")
    capture = PythonFileCaptureContext(hash_workers=1)
    files: list[dict[str, object]] = []
    identities = capture.bind_many(regular, label=label)
    for (candidate, metadata), identity in zip(regular, identities, strict=True):
        files.append(
            {
                "relative_path": candidate.relative_to(root).as_posix(),
                "size": identity.size,
                "sha256": identity.sha256,
                "mode": stat.S_IMODE(metadata.st_mode),
                "mtime_ns": metadata.st_mtime_ns,
            }
        )

    def verify_membership() -> None:
        actual = _snapshot_fingerprint(*_tree_membership_snapshot(root, label=label))
        if expected != actual:
            raise ValueError(
                f"{label} changed during inventory: {_snapshot_difference(expected, actual)}"
            )

    capture.register_verification_fence(verify_membership)
    capture.verify()
    return {
        "root": str(root),
        "file_count": len(files),
        "files": files,
        "directories": directories,
        "manifest_sha256": canonical_json_sha256(
            {"files": files, "directories": directories}
        ),
    }


def _executable_identity(path: Path) -> dict[str, object]:
    lexical = process_image_capture.custody_path(path)
    try:
        resolved = lexical.resolve(strict=True)
        size = lexical.stat().st_size
    except OSError as exc:
        resolved = lexical
        size = -1
        digest = f"unavailable:{type(exc).__name__}"
    else:
        digest = _hash_file(lexical)
    identity: dict[str, object] = {
        "path": str(lexical),
        "resolved_path": str(resolved),
        "symlinked": process_image_capture._image_path_key(lexical)
        != process_image_capture._image_path_key(resolved),
        "size_bytes": size,
        "sha256": digest,
    }
    identity["identity_sha256"] = hashlib.sha256(
        json.dumps(identity, sort_keys=True).encode()
    ).hexdigest()
    return identity


def _content_identity_available(identity: Mapping[str, object]) -> bool:
    digest = identity.get("sha256")
    return (
        isinstance(identity.get("size_bytes"), int)
        and int(identity["size_bytes"]) >= 0
        and isinstance(digest, str)
        and re.fullmatch(r"[0-9a-f]{64}", digest) is not None
    )


def _run_captured(
    command: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    timeout: float = 30.0,
    text: bool = True,
) -> subprocess.CompletedProcess[Any]:
    process_image_capture.require_custody_coordinate(Path(command[0]))
    return admission._COMMANDS.run(
        list(command),
        cwd=cwd,
        env=dict(env),
        check=False,
        capture_output=True,
        text=text,
        timeout=timeout,
    )


def _resolve_outer_executable(token: str, *, cwd: Path, env: Mapping[str, str]) -> Path:
    if token in {"wasm-ld", "wasm-ld.exe"}:
        from molt.llvm_toolchain import LlvmToolchainConfigError, resolve_wasi_sdk_tool

        try:
            token = str(
                resolve_wasi_sdk_tool(proof_plan.ROOT, "wasm-ld", environ=dict(env))
            )
        except LlvmToolchainConfigError as exc:
            raise ValueError(f"wasm-ld toolchain selection failed: {exc}") from exc
    selected = find_executable(token, environment=env, cwd=cwd)
    if selected is None:
        if Path(token).is_absolute() or any(separator in token for separator in "/\\"):
            raise ValueError(f"proof executable {token!r} is unavailable")
        raise ValueError(f"proof executable {token!r} is not on the execution PATH")
    process_image_capture.require_custody_coordinate(selected)
    return selected


def _bound_tool_payload(
    envelope: Mapping[str, object], exact: Sequence[str], requested: str
) -> str | None:
    """Use the admitted role, before executable binding changes its basename."""
    delegated = envelope.get("delegated")
    owner = delegated if isinstance(delegated, Mapping) else envelope
    submitted = owner.get("argv")
    if (
        isinstance(owner.get("python"), Mapping)
        or not isinstance(submitted, list)
        or not submitted
        or admission._basename(str(submitted[0]))
        not in admission._executable_registry_names(requested)
    ):
        return None
    payload = admission._nested_command(exact) if delegated is not None else exact
    if not payload:
        raise ValueError(f"typed {requested} command has no exact payload")
    return str(payload[0])


def _cargo_executable_path(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    token: str | None = None,
) -> Path:
    """Select Cargo once, preserving a typed command's explicit executable.

    Bare Cargo roles use CARGO before command-effective PATH. Once bound, the
    actual payload path owns both capture and execution, including Rust probes.
    Python families also capture their environment-selected Cargo dependency.
    """
    cargo_names = admission._executable_registry_names("cargo")
    if token is None:
        token = _bound_tool_payload(envelope, exact, "cargo")
        if token is None:
            # Python drivers can invoke literal cargo as well as an explicit
            # CARGO hook. PATH owns the declared dependency; executable-env
            # custody independently captures the hook and its physical image.
            return _which_in_command_environment(
                "cargo", envelope, exact, cwd=cwd, env=env
            )
    if token.casefold() in cargo_names:
        selected = executable_environment_value(env, "CARGO")
        if selected:
            return _resolve_outer_executable(selected, cwd=cwd, env=env)
        token = token or "cargo"
        return _which_in_command_environment(token, envelope, exact, cwd=cwd, env=env)
    return _resolve_outer_executable(token, cwd=cwd, env=env)


def _exact_command(
    envelope: Mapping[str, object], *, cwd: Path, env: Mapping[str, str]
) -> list[str]:
    argv = [str(value) for value in envelope["argv"]]  # type: ignore[index]
    wrapper = envelope.get("wrapper")
    if isinstance(wrapper, Mapping) and wrapper.get("kind") == "venv":
        from tools import venv_exec

        selected_venv = env.get("VIRTUAL_ENV")
        if not selected_venv:
            raise ValueError("modeled venv wrapper has no bound environment")
        argv = venv_exec.resolve_command(argv, venv=Path(selected_venv))
    python = envelope.get("python")
    if isinstance(python, Mapping) and python.get("kind") in {
        "uv",
        "uv-console-script",
    }:
        prefix, _effective = admission._canonical_uv_prefix(envelope, cwd=cwd)
        raw_prefix = python.get("prefix")
        assert isinstance(raw_prefix, list)
        argv = [*prefix, *argv[len(raw_prefix) :]]
    argv[0] = str(
        _cargo_executable_path(envelope, argv, cwd=cwd, env=env, token=argv[0])
        if not isinstance(python, Mapping)
        and admission._basename(argv[0])
        in admission._executable_registry_names("cargo")
        else _resolve_outer_executable(argv[0], cwd=cwd, env=env)
    )
    if isinstance(python, Mapping) and python.get("kind") == "uv-console-script":
        prefix = python.get("prefix")
        assert isinstance(prefix, list)
        console = admission._basename(str(python["console_script"]))
        payload_index = len(prefix)
        module = admission._PYTHON_CONSOLE_MODULES.get(console)
        if module is not None:
            argv = [
                *argv[:payload_index],
                "python",
                "-m",
                module,
                *argv[payload_index + 1 :],
            ]
        else:
            payload_path = _which_in_command_environment(
                argv[payload_index], envelope, argv, cwd=cwd, env=env
            )
            argv[payload_index] = str(payload_path)
    return argv


def _payload_executable_identity(
    envelope: Mapping[str, object], exact: Sequence[str]
) -> dict[str, object] | None:
    python = envelope.get("python")
    if not isinstance(python, Mapping) or python.get("kind") != "uv-console-script":
        return None
    console = admission._basename(str(python.get("console_script") or ""))
    if console in admission._PYTHON_CONSOLE_MODULES:
        return None
    prefix = python.get("prefix")
    if not isinstance(prefix, list):
        raise ValueError("uv console command has no exact prefix")
    return _executable_identity(Path(str(exact[len(prefix)])))


def _bind_delegated_command(
    envelope: Mapping[str, object],
    exact: list[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
) -> tuple[dict[str, object] | None, dict[str, object] | None]:
    invocation = envelope.get("guarded_exec")
    delegated = envelope.get("delegated")
    if invocation is None:
        if delegated is not None:
            raise ValueError("delegated envelope has no canonical guarded_exec launch")
        return None, None
    if not isinstance(invocation, Mapping):
        raise ValueError("guarded_exec invocation authority is malformed")
    if not isinstance(delegated, Mapping):
        raise ValueError("canonical guarded_exec launch has no delegated envelope")
    guarded_exec_path = admission._path_inside(
        cwd,
        "tools/guarded_exec.py",
        base=cwd,
        label="canonical guarded_exec",
    )
    if not guarded_exec_path.is_file():
        raise ValueError("canonical guarded_exec authority is not a file")
    target_indices = invocation.get("target_indices")
    delegated_index_raw = invocation.get("delegated_index")
    mode = invocation.get("mode")
    if (
        not isinstance(target_indices, list)
        or not all(isinstance(index, int) for index in target_indices)
        or not isinstance(delegated_index_raw, int)
    ):
        raise ValueError("guarded_exec invocation indices are malformed")
    delegated_index = delegated_index_raw
    if mode == "script" and len(target_indices) == 1:
        script_index = int(target_indices[0])
        submitted = Path(str(envelope["argv"][script_index]))  # type: ignore[index]
        if submitted.is_absolute():
            if submitted.resolve(strict=True) != guarded_exec_path:
                raise ValueError(
                    "absolute guarded_exec path is not the canonical source authority"
                )
        else:
            normalized = str(submitted).replace("\\", "/")
            while normalized.startswith("./"):
                normalized = normalized[2:]
            if normalized != "tools/guarded_exec.py":
                raise ValueError("relative guarded_exec path is not canonical")
        exact[script_index] = str(guarded_exec_path)
    elif mode == "module" and len(target_indices) == 2:
        module_flag, module_name = (int(index) for index in target_indices)
        if module_name != module_flag + 1:
            raise ValueError("guarded_exec module authority is not contiguous")
        exact[module_flag : module_name + 1] = [str(guarded_exec_path)]
        delegated_index -= 1
    else:
        raise ValueError("unknown guarded_exec invocation mode")
    delegated_path = (
        _cargo_executable_path(
            envelope, exact, cwd=cwd, env=env, token=exact[delegated_index]
        )
        if admission._basename(str(delegated["argv"][0]))  # type: ignore[index]
        in admission._executable_registry_names("cargo")
        else _which_in_command_environment(
            exact[delegated_index], envelope, exact, cwd=cwd, env=env
        )
    )
    exact[delegated_index] = str(delegated_path)
    return _file_identity(guarded_exec_path), _executable_identity(delegated_path)


def _python_auxiliary_command(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    arguments: Sequence[str],
    no_site: bool = False,
) -> list[str] | None:
    python = envelope.get("python")
    if not isinstance(python, Mapping):
        return None
    kind = python.get("kind")
    probe = python_identity_probe_arguments(arguments, no_site=no_site)
    if kind == "direct":
        return [exact[0], *probe]
    if kind == "py-launcher":
        command = [exact[0]]
        selector = python.get("selector")
        if isinstance(selector, str) and selector:
            command.append(selector)
        return [*command, *probe]
    if kind in {"uv", "uv-console-script"}:
        prefix = python.get("prefix")
        if not isinstance(prefix, list) or len(prefix) < 2:
            raise ValueError("uv proof envelope has no exact prefix")
        return [
            *exact[: len(prefix)],
            "python",
            *probe,
        ]
    raise ValueError(f"unknown proof Python envelope kind {kind!r}")


def _python_probe_command(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    source_root: Path,
    external_roots: Sequence[Path] = (),
    hash_workers: int = 1,
) -> list[str] | None:
    admitted_roots = sorted(
        {Path(root).resolve(strict=True) for root in (source_root, *external_roots)},
        key=lambda path: (os.path.normcase(str(path)), str(path)),
    )
    return _python_auxiliary_command(
        envelope,
        exact,
        arguments=(
            "--capture-active-environment",
            "--with-custody",
            "--hash-workers",
            str(hash_workers),
            "--admit-virtualenv-bootstrap",
            *(
                value
                for root in admitted_roots
                for value in ("--admit-external-root", str(root))
            ),
        ),
    )


def _parse_json_output(
    completed: subprocess.CompletedProcess[str], *, purpose: str
) -> dict[str, object]:
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise ValueError(
            f"{purpose} failed with exit code {completed.returncode}: {detail}"
        )
    try:
        payload = loads_exact(completed.stdout.strip())
    except (json.JSONDecodeError, ExactJsonError) as exc:
        raise ValueError(f"{purpose} returned invalid JSON") from exc
    if not isinstance(payload, dict):
        raise ValueError(f"{purpose} returned a non-object identity")
    return payload


def _python_identity(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    source_root: Path,
    selection: Mapping[str, object],
    hash_workers: int,
) -> dict[str, object] | None:
    if envelope.get("python") is None:
        return None
    location_raw = selection.get("location")
    try:
        location = validate_python_environment_location(location_raw)
    except PythonEnvironmentIdentityError as exc:
        raise ValueError(f"proof Python location identity is invalid: {exc}") from exc
    if set(selection) != {
        "base_executable",
        "base_executable_sha256",
        "executable",
        "executable_sha256",
        "external_roots",
        "location",
        "prefix",
    }:
        raise ValueError("proof Python selection identity shape is invalid")
    _reject_python_location_onedrive(location)
    selected_external_roots = selection.get("external_roots")
    if not isinstance(selected_external_roots, list) or not all(
        isinstance(value, str) and value for value in selected_external_roots
    ):
        raise ValueError("proof Python selection has no external-root authority")
    if selected_external_roots != location.get("external_roots"):
        raise ValueError("proof Python selection external roots differ from location")
    command = _python_probe_command(
        envelope,
        exact,
        source_root=source_root,
        external_roots=[
            Path(value) for value in cast(list[str], selected_external_roots)
        ],
        hash_workers=hash_workers,
    )
    if command is None:
        return None
    payload = _parse_json_output(
        _run_captured(command, cwd=cwd, env=env, timeout=120.0),
        purpose="proof Python identity probe",
    )
    try:
        capture = validate_python_capture(payload)
        environment = cast(Mapping[str, object], capture["identity"])
    except PythonEnvironmentIdentityError as exc:
        raise ValueError(f"proof Python identity is invalid: {exc}") from exc
    prefix_raw = selection.get("prefix")
    executable_raw = selection.get("executable")
    base_executable_raw = selection.get("base_executable")
    if not all(
        isinstance(value, str) and value
        for value in (prefix_raw, executable_raw, base_executable_raw)
    ):
        raise ValueError("proof Python selection identity is incomplete")
    assert isinstance(prefix_raw, str)
    assert isinstance(executable_raw, str)
    assert isinstance(base_executable_raw, str)
    if (
        prefix_raw != location.get("prefix")
        or executable_raw != location.get("selected_executable")
        or base_executable_raw != location.get("base_executable")
        or selection.get("executable_sha256") != _hash_file(Path(executable_raw))
        or selection.get("base_executable_sha256")
        != _hash_file(Path(base_executable_raw))
    ):
        raise ValueError("proof Python selection differs from its location receipt")
    source_root = source_root.resolve(strict=True)
    try:
        process_images = _python_process_images(
            environment, location, source_root=source_root
        )
    except PythonEnvironmentIdentityError as exc:
        raise ValueError(f"proof Python launcher closure is invalid: {exc}") from exc
    material: dict[str, object] = {
        "schema": "molt.proof-python-toolchain.v3",
        "identity_kind": "executable",
        "location": location,
        "source_root": str(source_root),
        "environment": environment,
        "file_custody": capture["file_custody"],
        "node_custody": capture["node_custody"],
        "process_images": process_images,
    }
    return {
        **material,
        "identity_sha256": canonical_json_sha256(material),
        "inventory_profile": capture["inventory_profile"],
    }


def _python_process_images(
    environment: Mapping[str, object],
    location: Mapping[str, object],
    *,
    source_root: Path,
) -> list[dict[str, object]]:
    prefix = Path(str(location["prefix"]))
    selected = str(location["selected_executable"])
    base = str(location["base_executable"])
    launchers = python_environment_executable_files(
        environment,
        prefix,
        base_executable=Path(base),
    )
    process_images = process_image_capture.canonical_images(
        [
            {
                "schema": process_image_capture.PROCESS_IMAGE_SCHEMA,
                "role": str(launcher["role"]),
                "path": str(launcher["path"]),
                "sha256": launcher["sha256"],
                "size_bytes": launcher["size"],
                "path_kind": "selection",
            }
            for launcher in launchers
        ]
    )
    selected_images = [
        row for row in process_images if row["role"] == "selected-interpreter"
    ]
    base_images = [row for row in process_images if row["role"] == "base-interpreter"]

    def lexical(value: object) -> str:
        return process_image_capture._image_path_key(Path(str(value)))

    if len(selected_images) != 1 or lexical(selected_images[0]["path"]) != lexical(
        selected
    ):
        raise ValueError("proof Python selection is absent from launcher closure")
    if len(base_images) != 1 or process_image_capture._image_path_key(
        process_image_capture.custody_path(Path(str(base_images[0]["path"]))).resolve(
            strict=True
        )
    ) != process_image_capture._image_path_key(
        process_image_capture.custody_path(Path(base)).resolve(strict=True)
    ):
        raise ValueError("proof Python base executable differs from launcher closure")
    external_rows = environment.get("external_roots")
    if not isinstance(external_rows, list) or not all(
        isinstance(row, Mapping) and isinstance(row.get("path"), str)
        for row in external_rows
    ):
        raise PythonEnvironmentIdentityError(
            "Python environment external-root closure is invalid"
        )
    from molt.python_environment_custody import _canonical_external_roots

    expected_external = [
        {"id": root_id, "path": str(path)}
        for root_id, path in _canonical_external_roots(
            [
                source_root,
                *(
                    Path(value)
                    for value in cast(Sequence[str], location["external_roots"])
                ),
            ],
            prefix,
        )
    ]
    typed_external_rows = cast(list[Mapping[str, object]], external_rows)
    if typed_external_rows != expected_external:
        raise ValueError(
            "proof Python environment external roots differ from pre-arm location"
        )
    return process_image_capture.revalidate_images(process_images)


def _reject_python_location_onedrive(
    location: Mapping[str, object], *, source_root: Path | None = None
) -> None:
    roles: list[tuple[str, Path]] = [
        ("environment prefix", Path(str(location["prefix"]))),
        ("selected executable", Path(str(location["selected_executable"]))),
        ("base executable", Path(str(location["base_executable"]))),
    ]
    roles.extend(
        ("custody root", Path(value))
        for value in cast(Sequence[str], location["roots"])
    )
    roles.extend(
        ("external editable root", Path(str(value)))
        for value in cast(Sequence[str], location["external_roots"])
    )
    roles.extend(
        ("native dependency", Path(value))
        for value in cast(Sequence[str], location["file_paths"])
    )
    if source_root is not None:
        roles.append(("proof source root", source_root))
    for role, path in roles:
        _reject_onedrive(path, f"proof Python {role}")


def _file_identity(path: Path) -> dict[str, object]:
    return {
        "path": str(path),
        "size_bytes": path.stat().st_size,
        "sha256": _hash_file(path),
    }


def execution_transcript_paths(result_path: Path) -> dict[str, Path]:
    """The execution result, never receipt-supplied paths, owns both streams."""
    return {
        name: result_path.with_suffix(f".{name}.bin") for name in ("stdout", "stderr")
    }


def execution_record_paths(log_path: Path) -> tuple[Path, Path]:
    """Request/result pair for one queue log; never accept a supplied fallback."""
    return (
        log_path.with_suffix(".execution-request.json"),
        log_path.with_suffix(".execution.json"),
    )


def opened_transcript_identity(path: Path, handle: BinaryIO) -> dict[str, object]:
    """Bind a live append-only stream to its opened file, not mutable bytes."""
    metadata = os.fstat(handle.fileno())
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_ino <= 0:
        raise ValueError("live command transcript requires a regular file identity")
    return {
        "path": str(path),
        "device": metadata.st_dev,
        "inode": metadata.st_ino,
    }


_TEST_COUNT_PATTERN = re.compile(
    r"(?P<count>\d+)\s+(?P<kind>passed|failed|ignored|skipped|deselected|xfailed|xpassed)",
    re.IGNORECASE,
)


def _transcript_identity(path: Path) -> dict[str, object]:
    identity = _file_identity(path)
    counts: dict[str, int] = {}
    with path.open("r", encoding="utf-8", errors="replace") as handle:
        for line in handle:
            for match in _TEST_COUNT_PATTERN.finditer(line):
                kind = match.group("kind").casefold()
                counts[kind] = counts.get(kind, 0) + int(match.group("count"))
    identity["test_counts"] = {name: counts[name] for name in sorted(counts)}
    identity["structured_test_output"] = bool(counts)
    return identity


def _replay_transcript(path: Path, stream: object) -> None:
    binary = getattr(stream, "buffer", None)
    if binary is not None:
        with path.open("rb") as source:
            shutil.copyfileobj(source, binary, length=1024 * 1024)
        binary.flush()
        return
    with path.open("r", encoding="utf-8", errors="replace") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), ""):
            if not chunk:
                break
            stream.write(chunk)  # type: ignore[attr-defined]
    stream.flush()  # type: ignore[attr-defined]


def _requires_structured_test_counts(envelope: Mapping[str, object]) -> bool:
    return admission.command_proof_kind(envelope) == "test-execution"


def validate_structured_test_counts(
    envelope: Mapping[str, object], transcript: Mapping[str, object], *, returncode: int
) -> None:
    """Require execution counts only for commands that actually run tests."""
    if returncode == 0 and _requires_structured_test_counts(envelope):
        if not any(
            isinstance(value, Mapping) and value.get("structured_test_output") is True
            for value in (transcript.get("stdout"), transcript.get("stderr"))
        ):
            raise ValueError(
                "successful test command produced no structured test-count authority"
            )


def _in_python_environment(
    envelope: Mapping[str, object], exact: Sequence[str], payload: Sequence[str]
) -> list[str]:
    python = envelope.get("python")
    if isinstance(python, Mapping) and python.get("kind") in {
        "uv",
        "uv-console-script",
    }:
        prefix = python.get("prefix")
        assert isinstance(prefix, list)
        return [*exact[: len(prefix)], *payload]
    return list(payload)


def _which_in_command_environment(
    name: str,
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
) -> Path:
    if name in {"wasm-ld", "wasm-ld.exe"}:
        # uv's PATH is not a different authority for a manifest-owned SDK role.
        # The returned absolute entrypoint is used for both launch and capture.
        return _resolve_outer_executable(name, cwd=cwd, env=env)
    python = envelope.get("python")
    if isinstance(python, Mapping) and python.get("kind") in {
        "uv",
        "uv-console-script",
    }:
        command = _in_python_environment(
            envelope, exact, ("python", "-c", admission._WHICH_SCRIPT, name)
        )
        payload = _parse_json_output(
            _run_captured(command, cwd=cwd, env=env), purpose=f"{name} path resolution"
        )
        found = payload.get("path")
        if not isinstance(found, str) or not found:
            raise ValueError(f"{name} is not on the proof command PATH")
        return _resolve_outer_executable(found, cwd=cwd, env=env)
    return _resolve_outer_executable(name, cwd=cwd, env=env)


def _tool_configuration_identities(
    name: str, *, cwd: Path, env: Mapping[str, str], command_argv: Sequence[str] = ()
) -> list[dict[str, object]]:
    candidates: list[Path] = []
    if name in {"cargo", "rustc", "rustdoc", "rustfmt", "cargo-deny", "cargo-audit"}:
        candidates.extend(
            Path(value)
            for value in cargo_config_arguments(command_argv, cwd=cwd)[1::2]
            if "=" not in value
        )
        candidates.extend(cargo_configuration_paths(cwd, env))
    if name == "lean":
        candidates.append(cwd / "formal" / "lean" / "lean-toolchain")
    identities: list[dict[str, object]] = []
    seen: set[str] = set()
    for candidate in candidates:
        if not candidate.is_file():
            continue
        resolved = candidate.resolve(strict=True)
        key = os.path.normcase(str(resolved))
        if key in seen:
            continue
        seen.add(key)
        identities.append(_file_identity(resolved))
    return sorted(identities, key=lambda item: os.path.normcase(str(item["path"])))


_RUST_TOOL_NAMES = frozenset({"cargo", "cargo.exe", "rustc", "rustc.exe"})


def _rust_target(envelope: Mapping[str, object], env: Mapping[str, str]) -> str | None:
    delegated = envelope.get("delegated")
    owner = delegated if isinstance(delegated, Mapping) else envelope
    selected_command = owner.get("argv")
    if not isinstance(selected_command, list) or not selected_command:
        raise ValueError("Rust target selection requires an admitted command envelope")
    selected: list[str] = []
    before_separator = True
    index = 1
    # `--target` is a Rust target triple only on a Rust tool's own argv. A
    # Python payload such as `molt extension produce-set --target wasm` names a
    # Molt target alias, which must never reach rustc as a triple.
    is_rust_tool = bool(selected_command) and (
        admission._basename(str(selected_command[0])) in _RUST_TOOL_NAMES
    )
    if not is_rust_tool:
        index = len(selected_command)
    while index < len(selected_command) and before_separator:
        value = str(selected_command[index])
        if value == "--":
            before_separator = False
            break
        if value == "--target":
            if index + 1 >= len(selected_command):
                raise ValueError("Rust --target requires a value")
            selected.append(str(selected_command[index + 1]))
            index += 2
            continue
        if value.startswith("--target="):
            selected.append(value.split("=", 1)[1])
        index += 1
    environment_target = env.get("CARGO_BUILD_TARGET", "").strip()
    if environment_target:
        selected.append(environment_target)
    unique = list(dict.fromkeys(selected))
    if len(unique) > 1:
        raise ValueError(f"Rust target selection is ambiguous: {unique!r}")
    return unique[0] if unique else None


def _runtime_c_environment_name(name: str) -> bool:
    from molt.cli.runtime_cargo_plan import runtime_c_environment_name

    return runtime_c_environment_name(name)


TOOL_IDENTITY_REUSE_SCHEMA = "molt.proof-tool-identity-reuse.v1"

# Environment names whose values define compiled output or compiler/linker
# behaviour. One authority serves the supervisor build digest and toolchain
# probe reuse; operational output placement is transport, never an input.
COMPILE_ENVIRONMENT_NAMES = frozenset(
    {
        "CARGO",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTUP_TOOLCHAIN",
        "SOURCE_DATE_EPOCH",
        "CC",
        "CXX",
        "AR",
        "CFLAGS",
        "CXXFLAGS",
        "CPPFLAGS",
        "LDFLAGS",
        "SDKROOT",
        "MACOSX_DEPLOYMENT_TARGET",
        "INCLUDE",
        "LIB",
        "LIBPATH",
        "CL",
        "_CL_",
        "LINK",
        "_LINK_",
    }
)
COMPILE_ENVIRONMENT_PREFIXES = (
    "CARGO_PROFILE_",
    "CARGO_TARGET_",
    "CARGO_BUILD_",
    "CC_",
    "CXX_",
    "AR_",
    "CFLAGS_",
    "CXXFLAGS_",
)
OPERATIONAL_CARGO_NAMES = frozenset(
    {
        "CARGO_BUILD_JOBS",
        "CARGO_BUILD_BUILD_DIR",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_TARGET_DIR",
        "CARGO_TARGET_TMPDIR",
    }
)
# Names that additionally decide how a probe resolves and loads nested tools.
_PROBE_RESOLUTION_NAMES = frozenset(
    {
        "PATH",
        "HOME",
        "USERPROFILE",
        "RUSTUP_HOME",
        "CARGO_HOME",
        "LD_LIBRARY_PATH",
        "DYLD_LIBRARY_PATH",
        "DYLD_FALLBACK_LIBRARY_PATH",
        "DEVELOPER_DIR",
        "LLVM_CONFIG_PATH",
        "CLANG_PATH",
        "LIBCLANG_PATH",
        "RUSTFMT",
    }
)
_PROBE_RESOLUTION_PREFIXES = ("RUSTUP_", "CARGO_", "DYLD_", "LDFLAGS_")
# Identity fields that re-verification can prove unchanged from file bytes.
# Anything else (a runtime probe payload, a package tree) is captured fresh.
_REUSABLE_IDENTITY_FIELDS = frozenset(
    {
        "path",
        "launcher_sha256",
        "content_path",
        "executable_sha256",
        "version",
        "probe_cwd",
        "policy_sha256",
        "configuration_files",
        "content_resolver",
        "process_images",
        "link_selection",
        "wasi_sdk",
        "identity_sha256",
    }
)
_MAX_REUSE_RECORD_BYTES = 4 * 1024 * 1024


def compile_environment_selection(
    env: Mapping[str, str], *, configured_names: Iterable[str] = ()
) -> dict[str, str]:
    """Hash every environment value that defines compiled output."""
    configured = set(configured_names)
    return {
        name: canonical_json_sha256(value)
        for name, value in sorted(env.items())
        if name not in OPERATIONAL_CARGO_NAMES
        and (
            name in COMPILE_ENVIRONMENT_NAMES
            or _runtime_c_environment_name(name)
            or name in configured
            or name.startswith(COMPILE_ENVIRONMENT_PREFIXES)
        )
    }


def _probe_environment_selection(env: Mapping[str, str]) -> dict[str, str]:
    """Hash the environment a toolchain probe can observe beyond tool bytes."""
    selected = compile_environment_selection(env)
    for name, value in sorted(env.items()):
        if name in OPERATIONAL_CARGO_NAMES or name in selected:
            continue
        if name in _PROBE_RESOLUTION_NAMES or name.startswith(
            _PROBE_RESOLUTION_PREFIXES
        ):
            selected[name] = canonical_json_sha256(value)
    return dict(sorted(selected.items()))


def _tool_identity_reuse_key(
    *,
    name: str,
    policy_sha256: str,
    envelope: Mapping[str, object],
    command_argv: Sequence[str],
    cwd: Path,
    probe_cwd: Path,
    launcher: Path,
    selected_content_path: Path | None,
    path_dependency_images: Sequence[Mapping[str, object]],
    env: Mapping[str, str],
) -> dict[str, object]:
    python_authority = envelope.get("python")
    toolchains = envelope.get("toolchains")
    return {
        "schema": TOOL_IDENTITY_REUSE_SCHEMA,
        "platform": sys.platform,
        "toolchain": name,
        "policy_sha256": policy_sha256,
        "cwd": str(cwd),
        "probe_cwd": str(probe_cwd),
        "command_argv": [str(value) for value in command_argv],
        "python": (
            {str(key): value for key, value in python_authority.items()}
            if isinstance(python_authority, Mapping)
            else None
        ),
        "cargo_native_c_units": envelope.get("cargo_native_c_units", []),
        "toolchains": (
            [str(value) for value in toolchains]
            if isinstance(toolchains, list)
            else None
        ),
        "launcher": _executable_identity(launcher),
        "selected_content_path": str(selected_content_path)
        if selected_content_path is not None
        else None,
        "path_dependency_images": list(path_dependency_images),
        "environment": _probe_environment_selection(env),
    }


def _reuse_record_path(reuse_root: Path, key_sha256: str) -> Path:
    return reuse_root / f"{key_sha256}.json"


def _load_reuse_record(path: Path) -> dict[str, object] | None:
    try:
        if path.stat().st_size > _MAX_REUSE_RECORD_BYTES:
            return None
        payload = loads_exact(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, ExactJsonError, json.JSONDecodeError):
        return None
    if (
        not isinstance(payload, dict)
        or payload.get("schema") != TOOL_IDENTITY_REUSE_SCHEMA
        or not isinstance(payload.get("key"), dict)
        or not isinstance(payload.get("identity"), dict)
    ):
        return None
    return payload


def _store_reuse_record(path: Path, payload: Mapping[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    data = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
    staging = path.with_name(f".{path.name}.{os.getpid()}.{secrets.token_hex(4)}.tmp")
    try:
        with open(staging, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(staging, path)
    finally:
        with contextlib.suppress(OSError):
            staging.unlink()


def _validate_wasi_sdk_policy(
    policy: proof_plan.ToolchainPolicy,
    identity: Mapping[str, object],
    *,
    full_capture: bool = False,
) -> None:
    required = policy.data.get("wasi_sdk_tool") is not None
    if ("wasi_sdk" in identity) != required or (
        required and not isinstance(identity["wasi_sdk"], Mapping)
    ):
        raise ValueError(f"{policy.name} SDK closure differs from toolchain policy")
    if required:
        toolchain_capture.validate_wasi_sdk_closure(
            identity,
            selected_role=str(policy.data["wasi_sdk_tool"]),
            full_capture=full_capture,
        )


def _reused_identity_is_current(
    policy: proof_plan.ToolchainPolicy,
    identity: Mapping[str, object],
    *,
    cwd: Path,
    env: Mapping[str, str],
    command_argv: Sequence[str],
    native_c_units: Sequence[str] = (),
) -> bool:
    """Re-prove a stored identity from file bytes before it is reused.

    Every recorded process image is rehashed by exact path, configuration
    files are re-identified, the content resolver is re-hashed, and the
    identity digest is recomputed. Any drift is a miss, never an error.
    """
    if not set(identity) <= _REUSABLE_IDENTITY_FIELDS:
        return False
    material = dict(identity)
    digest = material.pop("identity_sha256", None)
    if (
        digest
        != hashlib.sha256(
            json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
    ):
        return False
    images = identity.get("process_images")
    if not isinstance(images, list) or not images:
        return False
    try:
        _validate_wasi_sdk_policy(policy, identity)
        if policy.name == "rustc":
            selection = toolchain_capture.validate_rust_link_selection(
                identity, required_native_c=native_c_units, command_argv=command_argv
            )
            toolchain_capture.revalidate_rust_artifact_manifests(selection)
            if any(row["resources"] is not None for row in selection["native_c"]):
                return False  # Reuse stores selections; each armed operation captures contents.
            current = toolchain_capture.select_cargo_native_c_units(
                required=native_c_units,
                target=selection["target"],
                host=selection["compiler_host"],
                cwd=cwd,
                env=env,
            )
            if current != [row["selection"] for row in selection["native_c"]]:
                return False
            for row in selection["native_c"]:
                if not toolchain_capture.native_compiler_selection_is_current(
                    row["compiler"], env=env
                ):
                    return False
        if policy.data.get("wasi_sdk_tool") is not None:
            toolchain_capture.revalidate_wasi_sdk_selection(identity)
        else:
            process_image_capture.revalidate_images(images)
        configuration = _tool_configuration_identities(
            policy.name, cwd=cwd, env=env, command_argv=command_argv
        )
    except (OSError, ValueError, KeyError, TypeError):
        return False
    if configuration != identity.get("configuration_files"):
        return False
    resolver = identity.get("content_resolver")
    if resolver is not None:
        if not isinstance(resolver, Mapping) or not isinstance(
            resolver.get("path"), str
        ):
            return False
        if _executable_identity(Path(str(resolver["path"]))) != dict(resolver):
            return False
    return True


def _tool_identity(
    plan: proof_plan.ProofPlan,
    name: str,
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    reuse_root: Path | None = None,
    reuse_telemetry: list[dict[str, object]] | None = None,
) -> dict[str, object]:
    """Capture one toolchain identity, reusing a content-addressed record.

    With ``reuse_root`` the probe transcripts of an earlier capture are reused
    when the key (policy, launcher bytes, probe cwd, command, Python
    authority, resolution and compiler environment) matches and every recorded
    image and configuration file still hashes to the stored identity. The
    supervisor, its image inventories and the proof command itself never
    reuse anything; only deterministic version and link probes are skipped.
    """
    policies = {policy.name: policy for policy in plan.toolchain_policies}
    try:
        policy = policies[name]
    except KeyError as exc:
        raise ValueError(f"proof plan has no {name!r} toolchain policy") from exc
    if policy.identity_kind == "target-derived":
        from tools.proof_queue_pkg.target_derived_toolchains import capture_identity

        return capture_identity(policy, envelope, environment=env)
    requested = str(policy.data.get("executable") or name)
    if requested == "{python}":
        raise ValueError("Python toolchain identity must use the runtime-closure probe")
    probe_cwd = cwd
    configured_probe_cwd = policy.data.get("probe_cwd")
    if configured_probe_cwd is not None:
        if not isinstance(configured_probe_cwd, str) or not configured_probe_cwd:
            raise ValueError(f"{name} toolchain probe cwd is malformed")
        relative_probe_cwd = Path(configured_probe_cwd)
        if relative_probe_cwd.is_absolute():
            raise ValueError(f"{name} toolchain probe cwd must be repository-relative")
        probe_cwd = (proof_plan.ROOT / relative_probe_cwd).resolve(strict=True)
    sdk_role = policy.data.get("wasi_sdk_tool")
    if sdk_role is not None:
        from molt.llvm_toolchain import resolve_wasi_sdk_tool

        path = resolve_wasi_sdk_tool(proof_plan.ROOT, sdk_role, environ=dict(env))
    elif name == "cargo":
        path = _cargo_executable_path(envelope, exact, cwd=probe_cwd, env=env)
    elif payload := _bound_tool_payload(envelope, exact, requested):
        path = _resolve_outer_executable(payload, cwd=probe_cwd, env=env)
    elif name == "rustc" and (
        selected := executable_environment_value(env, "RUSTC")
        or executable_environment_value(env, "CARGO_BUILD_RUSTC")
    ):
        path = _resolve_outer_executable(selected, cwd=probe_cwd, env=env)
    else:
        path = _which_in_command_environment(
            requested, envelope, exact, cwd=probe_cwd, env=env
        )
    path = process_image_capture.custody_path(path)
    selected_content_path = None
    if policy.data.get("fingerprint_domain") == "rustup":
        from molt.rust_toolchain import resolve_rustup_proxy

        # Rustup overrides can change while proxy bytes and environment stay
        # fixed. Resolve before reuse; physical tools need no rustup lookup.
        selected_content_path = resolve_rustup_proxy(
            path, role=name, root=probe_cwd, env=env
        ).resolve(strict=True)
    path_dependency_images: list[dict[str, object]] = []
    delegated = envelope.get("delegated")
    owner = delegated if isinstance(delegated, Mapping) else envelope
    if name == "rustc" and isinstance(owner.get("python"), Mapping):
        # Registered Python build drivers query literal PATH rustc for host
        # metadata even when Cargo uses an explicit compiler. Capture those
        # executable bytes separately; the primary compiler still owns all
        # version/sysroot/linker metadata and no second linker is implied.
        dependency = _which_in_command_environment(
            "rustc", envelope, exact, cwd=probe_cwd, env=env
        )
        if dependency != path:
            dependency_content = resolve_rustup_proxy(
                dependency, role="rustc", root=probe_cwd, env=env
            ).resolve(strict=True)
            path_dependency_images.append(
                process_image_capture.capture_image(
                    "rustc-path-metadata", dependency, preserve_path=True
                )
            )
            if dependency_content != dependency:
                path_dependency_images.append(
                    process_image_capture.capture_image(
                        "rustc-path-metadata", dependency_content
                    )
                )
            path_dependency_images = process_image_capture.canonical_images(
                path_dependency_images
            )
    policy_sha256 = hashlib.sha256(
        json.dumps(policy.data, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    command_argv = admission._nested_command(exact) or [str(value) for value in exact]
    record_path: Path | None = None
    key: dict[str, object] | None = None
    key_sha256 = ""
    if reuse_root is not None:
        key = _tool_identity_reuse_key(
            name=name,
            policy_sha256=policy_sha256,
            envelope=envelope,
            command_argv=command_argv,
            cwd=cwd,
            probe_cwd=probe_cwd,
            launcher=path,
            selected_content_path=selected_content_path,
            path_dependency_images=path_dependency_images,
            env=env,
        )
        key_sha256 = canonical_json_sha256(key)
        record_path = _reuse_record_path(reuse_root, key_sha256)
        record = _load_reuse_record(record_path)
        reason = "absent"
        if record is not None:
            stored_identity = cast(dict[str, object], record["identity"])
            if record["key"] != key:
                reason = "key-collision"
            elif (
                stored_identity.get("path") != str(path)
                or stored_identity.get("policy_sha256") != policy_sha256
                or stored_identity.get("probe_cwd") != str(probe_cwd)
            ):
                reason = "selection-drift"
            elif _reused_identity_is_current(
                policy,
                stored_identity,
                cwd=cwd,
                env=env,
                command_argv=command_argv,
                native_c_units=envelope.get("cargo_native_c_units", []),
            ):
                if reuse_telemetry is not None:
                    reuse_telemetry.append(
                        {
                            "toolchain": name,
                            "state": "hit",
                            "key_sha256": key_sha256,
                            "record": str(record_path),
                            "revalidated_images": len(
                                cast(list[object], stored_identity["process_images"])
                            ),
                        }
                    )
                return dict(stored_identity)
            else:
                reason = "revalidation-drift"
        if reuse_telemetry is not None:
            reuse_telemetry.append(
                {
                    "toolchain": name,
                    "state": "miss",
                    "reason": reason,
                    "key_sha256": key_sha256,
                    "record": str(record_path),
                }
            )
    material = _capture_tool_identity(
        policy,
        name,
        envelope,
        exact,
        path=path,
        selected_content_path=selected_content_path,
        probe_cwd=probe_cwd,
        policy_sha256=policy_sha256,
        cwd=cwd,
        env=env,
    )
    if path_dependency_images:
        material["process_images"] = process_image_capture.canonical_images(
            [*material["process_images"], *path_dependency_images]
        )
        material.pop("identity_sha256", None)
        material["identity_sha256"] = canonical_json_sha256(material)
    if record_path is not None and set(material) <= _REUSABLE_IDENTITY_FIELDS:
        _store_reuse_record(
            record_path,
            {"schema": TOOL_IDENTITY_REUSE_SCHEMA, "key": key, "identity": material},
        )
    return material


def _capture_tool_identity(
    policy: proof_plan.ToolchainPolicy,
    name: str,
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    path: Path,
    selected_content_path: Path | None,
    probe_cwd: Path,
    policy_sha256: str,
    cwd: Path,
    env: Mapping[str, str],
) -> dict[str, object]:
    raw_version_args = policy.data.get("version_args")
    if not isinstance(raw_version_args, list) or not all(
        isinstance(value, str) and value for value in raw_version_args
    ):
        raise ValueError(f"{name} toolchain policy has no typed version command")
    version_args = tuple(raw_version_args)
    completed = _run_captured(
        _in_python_environment(
            envelope, exact, (str(selected_content_path or path), *version_args)
        ),
        cwd=probe_cwd,
        env=env,
    )
    if completed.returncode != 0:
        raise ValueError(f"{name} version probe failed: {completed.stderr.strip()}")
    content_path = selected_content_path or path
    content_command = policy.data.get("content_path_command")
    content_resolver_identity: dict[str, object] | None = None
    if selected_content_path is None and content_command is not None:
        if not isinstance(content_command, list) or not all(
            isinstance(value, str) and value for value in content_command
        ):
            raise ValueError(f"{name} content-path command is malformed")
        resolver = _which_in_command_environment(
            content_command[0], envelope, exact, cwd=probe_cwd, env=env
        )
        resolved = _run_captured(
            _in_python_environment(
                envelope, exact, (str(resolver), *content_command[1:])
            ),
            cwd=probe_cwd,
            env=env,
        )
        if resolved.returncode != 0:
            raise ValueError(
                f"{name} content-path probe failed: "
                + (resolved.stderr.strip() or resolved.stdout.strip())
            )
        try:
            content_path = resolve_single_file_path(
                resolved.stdout,
                probe_cwd=probe_cwd,
            )
        except (OSError, ValueError) as exc:
            raise ValueError(f"{name} content-path probe is invalid: {exc}") from exc
        content_resolver_identity = _executable_identity(resolver)
    sdk_closure = None
    if policy.data.get("wasi_sdk_tool") is not None:
        sdk_closure = capture_wasi_sdk_selection(root=proof_plan.ROOT, env=env)
        process_images = toolchain_capture.capture_wasi_sdk_images(sdk_closure)
        launcher_image = next(
            (image for image in process_images if image["path"] == str(path)), None
        )
        if launcher_image is None:
            raise ValueError(
                "WASI compiler selection differs from captured SDK helpers"
            )
    else:
        launcher_image = process_image_capture.capture_image(
            f"{name}-launcher", path, preserve_path=True
        )
        process_images = [launcher_image]
    if process_image_capture._image_path_key(
        content_path
    ) == process_image_capture._image_path_key(path):
        content_image = launcher_image
    else:
        content_image = process_image_capture.capture_image(name, content_path)
        process_images.append(content_image)
    material: dict[str, object] = {
        "path": str(path),
        "launcher_sha256": launcher_image["sha256"],
        "content_path": str(content_path),
        "executable_sha256": content_image["sha256"],
        "version": (completed.stdout or completed.stderr).strip(),
        "probe_cwd": str(probe_cwd),
        "policy_sha256": policy_sha256,
        "configuration_files": _tool_configuration_identities(
            name,
            cwd=cwd,
            env=env,
            command_argv=admission._nested_command(exact) or exact,
        ),
    }
    if content_resolver_identity is not None:
        material["content_resolver"] = content_resolver_identity
    if name == "rustc":
        requested_toolchains = envelope.get("toolchains")
        cargo_path = None
        if isinstance(requested_toolchains, list) and "cargo" in requested_toolchains:
            cargo_path = _cargo_executable_path(envelope, exact, cwd=probe_cwd, env=env)
        linker_images, linker_telemetry = (
            toolchain_capture.capture_rust_link_process_images(
                rustc=content_path,
                rustc_version=str(material["version"]),
                native_c_units=envelope.get("cargo_native_c_units", []),
                admitted_command=envelope.get("submitted_argv", envelope["argv"]),
                cargo=cargo_path,
                cwd=probe_cwd,
                env=env,
                target=_rust_target(envelope, env),
                command_argv=admission._nested_command(exact) or exact,
                linker_process_helpers=(
                    policy.data.get("linker_process_helpers")
                    if isinstance(policy.data.get("linker_process_helpers"), Mapping)
                    else {}
                ),
                linker_build_tools=(
                    policy.data.get("linker_build_tools")
                    if isinstance(policy.data.get("linker_build_tools"), Mapping)
                    else {}
                ),
            )
        )
        process_images.extend(linker_images)
        material["link_selection"] = linker_telemetry
    if sdk_closure is not None:
        material["wasi_sdk"] = sdk_closure
    material["process_images"] = process_images
    if name == "node":
        node_probe = (
            "const m=require('module');"
            "console.log(JSON.stringify({execPath:process.execPath,"
            "versions:process.versions,config:process.config,globalPaths:m.globalPaths}))"
        )
        runtime = _run_captured(
            (str(content_path), "-e", node_probe), cwd=probe_cwd, env=env
        )
        runtime_payload = _parse_json_output(runtime, purpose="node runtime closure")
        exec_path = runtime_payload.get("execPath")
        if (
            not isinstance(exec_path, str)
            or Path(exec_path).resolve(strict=True) != content_path
        ):
            raise ValueError("node runtime closure resolved a substituted executable")
        material["runtime"] = runtime_payload
        material["runtime_sha256"] = hashlib.sha256(
            json.dumps(runtime_payload, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
    node_package = policy.data.get("node_package")
    if node_package is not None:
        if not isinstance(node_package, str) or not node_package:
            raise ValueError(f"{name} node package authority is malformed")
        node_path = _which_in_command_environment(
            "node", envelope, exact, cwd=probe_cwd, env=env
        )
        package_probe = (
            "const fs=require('fs'),p=require('path'),name=process.argv[1];"
            "const entry=require.resolve(name);let root=p.dirname(entry);"
            "for(;;){const manifest=p.join(root,'package.json');"
            "if(fs.existsSync(manifest)){const data=JSON.parse(fs.readFileSync(manifest));"
            "if(data.name===name){console.log(JSON.stringify({entry,manifest,root}));break;}}"
            "const parent=p.dirname(root);if(parent===root)throw new Error('package root not found');"
            "root=parent;}"
        )
        resolved_package = _parse_json_output(
            _run_captured(
                _in_python_environment(
                    envelope, exact, (str(node_path), "-e", package_probe, node_package)
                ),
                cwd=probe_cwd,
                env=env,
            ),
            purpose=f"{name} node package closure",
        )
        package_root_raw = resolved_package.get("root")
        entry_raw = resolved_package.get("entry")
        manifest_raw = resolved_package.get("manifest")
        if not all(
            isinstance(value, str) and value
            for value in (package_root_raw, entry_raw, manifest_raw)
        ):
            raise ValueError(f"{name} node package closure is malformed")
        package_root = Path(str(package_root_raw)).resolve(strict=True)
        entry = Path(str(entry_raw)).resolve(strict=True)
        manifest_path = Path(str(manifest_raw)).resolve(strict=True)
        for candidate, label in ((entry, "entry"), (manifest_path, "manifest")):
            try:
                candidate.relative_to(package_root)
            except ValueError as exc:
                raise ValueError(
                    f"{name} node package {label} escapes its resolved package root"
                ) from exc
        material["node_package"] = {
            "name": node_package,
            "entry": str(entry),
            "manifest": str(manifest_path),
            "resolver": _executable_identity(node_path),
            "selection_files": [_file_identity(entry), _file_identity(manifest_path)],
            "package": {"root": str(package_root)},
        }
    material["identity_sha256"] = hashlib.sha256(
        json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    return material


def _validate_toolchain_identity(
    plan: proof_plan.ProofPlan,
    name: str,
    identity: Mapping[str, object],
    *,
    full_capture: bool = False,
) -> None:
    policies = {policy.name: policy for policy in plan.toolchain_policies}
    try:
        policy = policies[name]
    except KeyError as exc:
        raise ValueError(f"proof plan has no {name!r} toolchain policy") from exc
    if policy.identity_kind == "target-derived":
        from tools.proof_queue_pkg.target_derived_toolchains import validate_identity

        validate_identity(policy, identity, full_capture=full_capture)
        return
    if name == "python":
        environment = identity.get("environment")
        location = identity.get("location")
        source_root_raw = identity.get("source_root")
        if (
            set(identity)
            != {
                "environment",
                "file_custody",
                "node_custody",
                "inventory_profile",
                "identity_kind",
                "identity_sha256",
                "location",
                "process_images",
                "schema",
                "source_root",
            }
            or identity.get("schema") != "molt.proof-python-toolchain.v3"
            or identity.get("identity_kind") != "executable"
            or not isinstance(environment, Mapping)
            or not isinstance(location, Mapping)
            or not isinstance(source_root_raw, str)
        ):
            raise ValueError("python identity has no complete environment closure")
        material = dict(identity)
        digest = material.pop("identity_sha256", None)
        material.pop("inventory_profile", None)
        if digest != canonical_json_sha256(material):
            raise ValueError("python toolchain identity digest is invalid")
        try:
            capture = validate_python_capture(
                {
                    "schema": PYTHON_CAPTURE_SCHEMA,
                    "identity": environment,
                    "file_custody": identity.get("file_custody"),
                    "node_custody": identity.get("node_custody"),
                    "inventory_profile": identity.get("inventory_profile"),
                }
            )
            environment = cast(Mapping[str, object], capture["identity"])
            location = validate_python_environment_location(dict(location))
        except PythonEnvironmentIdentityError as exc:
            raise ValueError(f"python environment closure is invalid: {exc}") from exc
        source_root = Path(source_root_raw)
        if (
            not source_root.is_absolute()
            or not source_root.is_dir()
            or str(source_root.resolve(strict=True)) != source_root_raw
        ):
            raise ValueError("python identity source root is invalid")
        _reject_python_location_onedrive(location, source_root=source_root)
        expected_images = _python_process_images(
            environment, location, source_root=source_root
        )
        if identity.get("process_images") != expected_images:
            raise ValueError("python process images differ from environment closure")
        version = environment.get("version")
        pattern = str(policy.data["version_pattern"])
        if (
            not isinstance(version, str)
            or re.search(pattern, f"Python {version}") is None
        ):
            raise ValueError(
                f"python identity version {version!r} violates canonical policy {pattern!r}"
            )
        process_image_capture.toolchain_images(name, identity)
        return
    _validate_wasi_sdk_policy(policy, identity, full_capture=full_capture)
    _validate_node_package_identity(policy, identity, full_capture=full_capture)
    if name == "rustc":
        toolchain_capture.validate_rust_link_selection(
            identity, full_capture=full_capture
        )
    version = identity.get("version")
    pattern = str(policy.data["version_pattern"])
    if not isinstance(version, str) or re.search(pattern, version) is None:
        raise ValueError(
            f"{name} identity version {version!r} violates canonical policy {pattern!r}"
        )
    hash_values = [
        value
        for key, value in identity.items()
        if key in {"sha256", "launcher_sha256", "executable_sha256"}
    ]
    if not hash_values or any(
        not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{64}", value) is None
        for value in hash_values
    ):
        raise ValueError(f"{name} identity has no available executable content hash")
    process_image_capture.toolchain_images(name, identity)
    probes = policy.data.get("process_image_probes", [])
    inventories = identity.get("process_image_inventories", [])
    if not isinstance(probes, list) or not isinstance(inventories, list):
        raise ValueError(f"{name} process-image inventory authority is malformed")
    if len(inventories) != len(probes):
        raise ValueError(f"{name} process-image inventory closure is incomplete")
    for inventory in inventories:
        if (
            not isinstance(inventory, Mapping)
            or inventory.get("schema") != "molt.proof-process-image-inventory.v1"
            or not isinstance(inventory.get("observed_image_count"), int)
            or int(inventory["observed_image_count"]) <= 0
            or any(
                not isinstance(inventory.get(field), str)
                or re.fullmatch(r"[0-9a-f]{64}", str(inventory[field])) is None
                for field in (
                    "probe_argv_sha256",
                    "stdout_sha256",
                    "stderr_sha256",
                    "receipt_identity_sha256",
                )
            )
        ):
            raise ValueError(f"{name} process-image inventory receipt is malformed")
    if name == "node":
        runtime_digest = identity.get("runtime_sha256")
        if (
            not isinstance(runtime_digest, str)
            or re.fullmatch(r"[0-9a-f]{64}", runtime_digest) is None
        ):
            raise ValueError("node identity has no runtime/configuration closure")


_ENVIRONMENT_EXACT_NAMES = frozenset(
    {
        "APPDATA",
        "CI",
        "COMSPEC",
        "HOME",
        "HOMEDRIVE",
        "HOMEPATH",
        "LANG",
        "LOCALAPPDATA",
        "LOGNAME",
        "NUMBER_OF_PROCESSORS",
        "NODE_OPTIONS",
        "OS",
        "PATH",
        "PATHEXT",
        "PROCESSOR_ARCHITECTURE",
        "PROCESSOR_IDENTIFIER",
        "PROGRAMDATA",
        "RUNNER_ARCH",
        "RUNNER_OS",
        "RUNNER_TEMP",
        "SHELL",
        "SYSTEMDRIVE",
        "SYSTEMROOT",
        "TEMP",
        "TERM",
        "TMP",
        "TMPDIR",
        "USER",
        "USERNAME",
        "USERPROFILE",
        "VIRTUAL_ENV",
        "WINDIR",
    }
)
_ENVIRONMENT_PREFIXES = (
    "AR_",
    "CARGO_",
    "CC_",
    "CFLAGS_",
    "CI_",
    "CMAKE_",
    "CXX_",
    "CXXFLAGS_",
    "GITHUB_",
    "BINDGEN_EXTRA_CLANG_ARGS_",
    "LC_",
    "LLVM_",
    "MOLT_",
    "PYO3_",
    "PYTHON",
    "RANLIB_",
    "RUST",
    "SCCACHE_",
    "UV_",
    "WASM_",
    "WASI_",
    "XDG_",
)
_ENVIRONMENT_BUILD_NAMES = frozenset(
    {
        "CARGO",
        "AR",
        "BINDGEN_EXTRA_CLANG_ARGS",
        "CC",
        "CFLAGS",
        "CL",
        "CLANG",
        "CLANG_PATH",
        "CMAKE",
        "CXX",
        "CXXFLAGS",
        "DLLTOOL",
        "INCLUDE",
        "LDFLAGS",
        "LIB",
        "LIBCLANG_PATH",
        "LIBCLANG_STATIC_PATH",
        "LINK",
        "LLVM_CONFIG",
        "MAKE",
        "MAKEFLAGS",
        "MESON",
        "NASM",
        "NINJAFLAGS",
        "NINJA",
        "NM",
        "OBJCOPY",
        "PERL",
        "PKG_CONFIG",
        "RANLIB",
        "RC",
        "CARGO",
        "RUSTC",
        "RUSTFLAGS",
        "STRIP",
        "YASM",
    }
)
_NONDETERMINISTIC_ENV_NAMES = frozenset(
    {
        "PYTHONBREAKPOINT",
        "PYTHONHOME",
        "PYTHONINSPECT",
        "PYTHONPATH",
        "PYTHONSTARTUP",
        "PYTHONUSERBASE",
        "PYTEST_ADDOPTS",
        "PYTEST_PLUGINS",
        "PYTEST_DISABLE_PLUGIN_AUTOLOAD",
        "UV_CONFIG_FILE",
        "UV_DEFAULT_INDEX",
        "UV_EXTRA_INDEX_URL",
        "UV_FIND_LINKS",
        "UV_INDEX",
        "UV_INDEX_URL",
    }
)
_CANONICAL_EXECUTION_ENV = {
    "PYTHONDONTWRITEBYTECODE": "1",
    "PYTHONNOUSERSITE": "1",
}
_QUEUE_CUSTODY_ENV_NAMES = frozenset(
    {
        "MOLT_MEMORY_GUARD_STATE_ROOT",
        "MOLT_MEMORY_GUARD_ACTIVE",
        "MOLT_MEMORY_GUARD_PID",
        "MOLT_MEMORY_GUARD_TOKEN",
        "MOLT_MEMORY_GUARD_MARKER",
        "MOLT_GUARD_SCRATCH_ROOT",
        "MOLT_PROOF_QUEUE_RUN_ID",
        "MOLT_PROOF_SOURCE_ROOT",
        SOURCE_EXTENSION_LINK_INPUTS_ENV,
        "MOLT_PYTEST_CURRENT_TEST_FILE",
        "MOLT_PROOF_CHILD_CUSTODY_JSON",
        "MOLT_PROOF_CHILD_CUSTODY_ENDPOINT",
        "MOLT_PROOF_CHILD_CUSTODY_TOKEN",
    }
)
_EXECUTABLE_ENV_NAMES = frozenset(
    {
        "CARGO",
        "AR",
        "CC",
        "CMAKE",
        "CXX",
        "DLLTOOL",
        "LINK",
        "MAKE",
        "MESON",
        "NASM",
        "NINJA",
        "MOLT_WASM_LD",
        "MOLT_LLVM_NM",
        "NM",
        "OBJCOPY",
        "PERL",
        "PKG_CONFIG",
        "RANLIB",
        "RC",
        "CARGO",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTDOC",
        "RUSTFMT",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC",
        "CARGO_BUILD_RUSTDOC",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUNNER",
        "CLANG",
        "CLANG_PATH",
        "LLVM_CONFIG",
        "LLVM_CONFIG_PATH",
        "STRIP",
        "WASM_BINDGEN",
        "WASM_OPT",
        "YASM",
    }
)
_EXECUTABLE_ENV_PATTERNS = (
    re.compile(r"(?:AR|CC|CXX|RANLIB|RC|STRIP)_[A-Z0-9_.-]+"),
    re.compile(r"(?:HOST|TARGET)_(?:AR|CC|CXX|RANLIB)"),
    re.compile(r"CARGO_TARGET_[A-Z0-9_]+_(?:LINKER|RUNNER)"),
    re.compile(r"CMAKE_(?:C|CXX)_COMPILER"),
)
_SECRET_ENV_NAME = re.compile(
    r"(?:TOKEN|SECRET|PASSWORD|PASSWD|API_?KEY|PRIVATE_?KEY|CREDENTIAL|COOKIE|AUTH)",
    re.IGNORECASE,
)
_SECRET_ARGUMENT_FLAG = re.compile(
    r"^--?(?:api[-_]?key|auth|credential|password|passwd|private[-_]?key|secret|token)(?:=|$)",
    re.IGNORECASE,
)
