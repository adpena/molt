"""Executable, interpreter, and toolchain identity authority."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
from typing import Any, Mapping, Sequence

from tools import proof_plan
from tools.proof_queue_pkg import command_admission as admission
from tools.proof_queue_pkg import process_image_capture, toolchain_capture
from tools.toolchain_probe import resolve_single_file_path


def _hash_file(path: Path) -> str:
    try:
        with path.open("rb") as handle:
            return hashlib.file_digest(handle, "sha256").hexdigest()
    except OSError as exc:
        return f"unavailable:{type(exc).__name__}"


def _directory_manifest_identity(path: Path, *, label: str) -> dict[str, object]:
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
    manifest = json.dumps(files, sort_keys=True, separators=(",", ":"))
    return {
        "root": str(root),
        "file_count": len(files),
        "files": files,
        "manifest_sha256": hashlib.sha256(manifest.encode()).hexdigest(),
    }


def _executable_identity(path: Path) -> dict[str, object]:
    lexical = Path(os.path.abspath(path))
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
        "symlinked": os.path.normcase(str(lexical)) != os.path.normcase(str(resolved)),
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
    candidate = Path(token)
    if candidate.is_absolute() or candidate.parent != Path("."):
        path = candidate if candidate.is_absolute() else cwd / candidate
        try:
            lexical = Path(os.path.abspath(path))
            if not lexical.is_file():
                raise FileNotFoundError(lexical)
            return lexical
        except OSError as exc:
            raise ValueError(f"proof executable {token!r} is unavailable") from exc
    found = shutil.which(token, path=env.get("PATH"))
    if found is None:
        raise ValueError(f"proof executable {token!r} is not on the execution PATH")
    lexical = Path(os.path.abspath(found))
    if not lexical.is_file():
        raise ValueError(f"proof executable {token!r} is unavailable")
    return lexical


def _exact_command(
    envelope: Mapping[str, object], *, cwd: Path, env: Mapping[str, str]
) -> list[str]:
    argv = [str(value) for value in envelope["argv"]]  # type: ignore[index]
    python = envelope.get("python")
    if isinstance(python, Mapping) and python.get("kind") in {
        "uv",
        "uv-console-script",
    }:
        prefix, _effective, _overlays = admission._canonical_uv_prefix(
            envelope, cwd=cwd
        )
        raw_prefix = python.get("prefix")
        assert isinstance(raw_prefix, list)
        argv = [*prefix, *argv[len(raw_prefix) :]]
    argv[0] = str(_resolve_outer_executable(argv[0], cwd=cwd, env=env))
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
    delegated_path = _which_in_command_environment(
        exact[delegated_index], envelope, exact, cwd=cwd, env=env
    )
    exact[delegated_index] = str(delegated_path)
    return _file_identity(guarded_exec_path), _executable_identity(delegated_path)


def _python_auxiliary_command(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    authority: Path,
    arguments: Sequence[str],
) -> list[str] | None:
    python = envelope.get("python")
    if not isinstance(python, Mapping):
        return None
    kind = python.get("kind")
    if kind == "direct":
        return [exact[0], str(authority), *arguments]
    if kind == "py-launcher":
        command = [exact[0]]
        selector = python.get("selector")
        if isinstance(selector, str) and selector:
            command.append(selector)
        return [*command, str(authority), *arguments]
    if kind in {"uv", "uv-console-script"}:
        prefix = python.get("prefix")
        if not isinstance(prefix, list) or len(prefix) < 2:
            raise ValueError("uv proof envelope has no exact prefix")
        return [
            *exact[: len(prefix)],
            "python",
            str(authority),
            *arguments,
        ]
    raise ValueError(f"unknown proof Python envelope kind {kind!r}")


def _python_probe_command(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    source_root: Path,
    hash_workers: int = 1,
) -> list[str] | None:
    return _python_auxiliary_command(
        envelope,
        exact,
        authority=admission._PYTHON_IDENTITY_PROBE,
        arguments=(str(source_root), str(hash_workers)),
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
        payload = json.loads(completed.stdout.strip())
    except json.JSONDecodeError as exc:
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
    hash_workers: int,
) -> dict[str, object] | None:
    command = _python_probe_command(
        envelope,
        exact,
        source_root=source_root,
        hash_workers=hash_workers,
    )
    if command is None:
        return None
    payload = _parse_json_output(
        _run_captured(command, cwd=cwd, env=env, timeout=120.0),
        purpose="proof Python identity probe",
    )
    required = (
        "executable",
        "implementation",
        "version",
        "executable_sha256",
        "runtime_closure_sha256",
        "distribution_inventory_sha256",
    )
    if not all(
        isinstance(payload.get(name), str) and payload[name] for name in required
    ):
        raise ValueError("proof Python identity probe returned incomplete identity")
    distributions = payload.get("distributions")
    if not isinstance(distributions, list):
        raise ValueError("proof Python identity has no distribution inventory")
    runtime = payload.get("runtime")
    if not isinstance(runtime, dict):
        raise ValueError("proof Python identity has no CPython runtime closure")
    identity: dict[str, object] = {name: str(payload[name]) for name in required}
    identity["runtime"] = runtime
    identity["distributions"] = distributions
    inventory_profile = payload.get("inventory_profile")
    if not isinstance(inventory_profile, dict):
        raise ValueError("proof Python identity has no inventory profile")
    identity["identity_sha256"] = hashlib.sha256(
        json.dumps(identity, sort_keys=True).encode()
    ).hexdigest()
    identity["inventory_profile"] = inventory_profile
    return identity


def _file_identity(path: Path) -> dict[str, object]:
    return {
        "path": str(path),
        "size_bytes": path.stat().st_size,
        "sha256": _hash_file(path),
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
    argv = [str(value) for value in envelope["argv"]]  # type: ignore[index]
    nested = admission._nested_command(argv)
    payload = nested if nested is not None else argv
    lowered = [admission._basename(value) for value in payload]
    if any(value in admission._PYTHON_CONSOLE_SCRIPTS for value in lowered):
        return True
    for index, value in enumerate(payload[:-1]):
        if value == "-m" and payload[index + 1] in {"pytest", "py.test"}:
            return True
    return bool(
        payload
        and admission._basename(payload[0]) in {"cargo", "cargo.exe"}
        and "test" in payload[1:]
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
    name: str, *, cwd: Path, env: Mapping[str, str]
) -> list[dict[str, object]]:
    candidates: list[Path] = []
    if name in {"cargo", "rustc", "rustfmt", "cargo-deny", "cargo-audit"}:
        for parent in (cwd, *cwd.parents):
            candidates.extend(
                (parent / ".cargo" / "config.toml", parent / ".cargo" / "config")
            )
        cargo_home = env.get("CARGO_HOME")
        if cargo_home:
            candidates.extend(
                (Path(cargo_home) / "config.toml", Path(cargo_home) / "config")
            )
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


def _rust_target(exact: Sequence[str], env: Mapping[str, str]) -> str | None:
    selected_command = admission._nested_command(exact) or [
        str(value) for value in exact
    ]
    selected: list[str] = []
    before_separator = True
    index = 1
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


def _tool_identity(
    plan: proof_plan.ProofPlan,
    name: str,
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
) -> dict[str, object]:
    policies = {policy.name: policy for policy in plan.toolchain_policies}
    try:
        policy = policies[name]
    except KeyError as exc:
        raise ValueError(f"proof plan has no {name!r} toolchain policy") from exc
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
    python_authority = envelope.get("python")
    if (
        not isinstance(python_authority, Mapping)
        and exact
        and admission._basename(exact[0])
        in admission._executable_registry_names(requested)
    ):
        path = _resolve_outer_executable(exact[0], cwd=probe_cwd, env=env)
    else:
        path = _which_in_command_environment(
            requested, envelope, exact, cwd=probe_cwd, env=env
        )
    raw_version_args = policy.data.get("version_args")
    if not isinstance(raw_version_args, list) or not all(
        isinstance(value, str) and value for value in raw_version_args
    ):
        raise ValueError(f"{name} toolchain policy has no typed version command")
    version_args = tuple(raw_version_args)
    completed = _run_captured(
        _in_python_environment(envelope, exact, (str(path), *version_args)),
        cwd=probe_cwd,
        env=env,
    )
    if completed.returncode != 0:
        raise ValueError(f"{name} version probe failed: {completed.stderr.strip()}")
    content_path = path
    content_command = policy.data.get("content_path_command")
    content_resolver_identity: dict[str, object] | None = None
    if content_command is not None:
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
    process_images: list[dict[str, object]] = []
    launcher_image = process_image_capture.capture_image(
        f"{name}-launcher", path, preserve_path=True
    )
    process_images.append(launcher_image)
    if os.path.normcase(str(content_path)) == os.path.normcase(str(path)):
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
        "policy_sha256": hashlib.sha256(
            json.dumps(policy.data, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest(),
        "configuration_files": _tool_configuration_identities(name, cwd=cwd, env=env),
    }
    if content_resolver_identity is not None:
        material["content_resolver"] = content_resolver_identity
    if name == "rustc":
        requested_toolchains = envelope.get("toolchains")
        cargo_path = None
        if isinstance(requested_toolchains, list) and "cargo" in requested_toolchains:
            cargo_path = _which_in_command_environment(
                "cargo", envelope, exact, cwd=probe_cwd, env=env
            )
        linker_images, linker_telemetry = (
            toolchain_capture.capture_rust_link_process_images(
                rustc=content_path,
                cargo=cargo_path,
                cwd=probe_cwd,
                env=env,
                target=_rust_target(exact, env),
                command_argv=admission._nested_command(exact) or exact,
                linker_process_helpers=(
                    policy.data.get("linker_process_helpers")
                    if isinstance(policy.data.get("linker_process_helpers"), Mapping)
                    else {}
                ),
            )
        )
        process_images.extend(linker_images)
        material["link_selection"] = linker_telemetry
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
            "package": _directory_manifest_identity(
                package_root, label=f"{name} node package"
            ),
        }
    material["identity_sha256"] = hashlib.sha256(
        json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    return material


def _validate_toolchain_identity(
    plan: proof_plan.ProofPlan,
    name: str,
    identity: Mapping[str, object],
) -> None:
    policies = {policy.name: policy for policy in plan.toolchain_policies}
    try:
        policy = policies[name]
    except KeyError as exc:
        raise ValueError(f"proof plan has no {name!r} toolchain policy") from exc
    version = identity.get("version")
    if name == "python" and isinstance(version, str):
        version = f"Python {version}"
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
    if name == "python":
        runtime_digest = identity.get("runtime_closure_sha256")
        runtime = identity.get("runtime")
        if (
            not isinstance(runtime_digest, str)
            or re.fullmatch(r"[0-9a-f]{64}", runtime_digest) is None
            or not isinstance(runtime, Mapping)
            or not isinstance(runtime.get("runtime_file_count"), int)
            or int(runtime["runtime_file_count"]) <= 0
        ):
            raise ValueError("python identity has no complete CPython runtime closure")
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
    "CI_",
    "CMAKE_",
    "CXX_",
    "GITHUB_",
    "LC_",
    "LLVM_",
    "MOLT_",
    "PYO3_",
    "PYTHON",
    "RUST",
    "SCCACHE_",
    "UV_",
    "WASM_",
    "XDG_",
)
_ENVIRONMENT_BUILD_NAMES = frozenset(
    {
        "AR",
        "CC",
        "CFLAGS",
        "CL",
        "CLANG",
        "CMAKE",
        "CXX",
        "CXXFLAGS",
        "DLLTOOL",
        "INCLUDE",
        "LDFLAGS",
        "LIB",
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
        "MOLT_PROOF_CHILD_CUSTODY_JSON",
        "MOLT_PROOF_CHILD_CUSTODY_ENDPOINT",
        "MOLT_PROOF_CHILD_CUSTODY_TOKEN",
    }
)
_EXECUTABLE_ENV_NAMES = frozenset(
    {
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
        "NM",
        "OBJCOPY",
        "PERL",
        "PKG_CONFIG",
        "RANLIB",
        "RC",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUNNER",
        "CLANG",
        "LLVM_CONFIG",
        "STRIP",
        "WASM_BINDGEN",
        "WASM_OPT",
        "YASM",
    }
)
_EXECUTABLE_ENV_PATTERNS = (
    re.compile(r"(?:AR|CC|CXX|RANLIB|RC|STRIP)_[A-Z0-9_]+"),
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
