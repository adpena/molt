from __future__ import annotations

import datetime as dt
import hashlib
import importlib
import ipaddress
import os
import platform
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import tomllib
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

from packaging.markers import InvalidMarker, Marker
from packaging.requirements import InvalidRequirement, Requirement

from molt import process_guard as _process_guard
from molt.cli.atomic_io import _atomic_copy_file
from molt.cli.lockfiles import _check_lockfiles

MOLT_VENV_DIR = ".molt-venv"
_CLI_MEMORY_GUARD_PREFIX = _process_guard.CLI_MEMORY_GUARD_PREFIX


def _cli_module() -> Any:
    return importlib.import_module("molt.cli")


def _molt_venv_path(project_root: Path) -> Path:
    return project_root / MOLT_VENV_DIR


def _run_completed_command(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._run_completed_command(*args, **kwargs)


def _find_molt_root(*candidates: Path) -> Path:
    return _cli_module()._find_molt_root(*candidates)


def _require_molt_root(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._require_molt_root(*args, **kwargs)


def _json_payload(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._json_payload(*args, **kwargs)


def _emit_json(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._emit_json(*args, **kwargs)


def _fail(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._fail(*args, **kwargs)


def _replace_directory_tree_from_source(*args: Any, **kwargs: Any) -> Any:
    from molt.cli import non_native_output as _non_native_output

    return _non_native_output._replace_directory_tree_from_source(*args, **kwargs)


def _atomic_write_bytes(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._atomic_write_bytes(*args, **kwargs)


def _atomic_write_json(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._atomic_write_json(*args, **kwargs)


def _default_molt_cache() -> Path:
    return _cli_module()._default_molt_cache()


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


_GIT_SOURCE_COMMAND_TIMEOUT_SEC = 300.0


def _run_git_source_command(
    cmd: list[str],
    *,
    cwd: Path,
    timeout: float = _GIT_SOURCE_COMMAND_TIMEOUT_SEC,
) -> subprocess.CompletedProcess[str]:
    return _run_completed_command(
        cmd,
        cwd=cwd,
        env=None,
        capture_output=True,
        memory_guard_prefix="MOLT_BUILD",
        timeout=timeout,
    )


def _resolve_git_ref(
    url: str,
    ref: str,
    *,
    project_root: Path,
) -> tuple[str | None, str | None]:
    try:
        result = _run_git_source_command(
            ["git", "ls-remote", url, ref],
            cwd=project_root,
            timeout=60.0,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
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
    project_root: Path,
    subdirectory: str | None = None,
) -> tuple[str, str]:
    tmp_root = dest.parent
    with tempfile.TemporaryDirectory(dir=tmp_root, prefix="git_vendor_") as tmpdir:
        repo_dir = Path(tmpdir) / "repo"
        try:
            clone = _run_git_source_command(
                [
                    "git",
                    "clone",
                    "--filter=blob:none",
                    "--no-checkout",
                    url,
                    str(repo_dir),
                ],
                cwd=project_root,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            raise RuntimeError(f"Failed to clone git repo {url}: {exc}") from exc
        if clone.returncode != 0:
            detail = (clone.stderr or clone.stdout).strip() or "unknown error"
            raise RuntimeError(f"Failed to clone git repo {url}: {detail}")
        fetch = _run_git_source_command(
            ["git", "-C", str(repo_dir), "fetch", "--depth", "1", "origin", ref],
            cwd=project_root,
        )
        if fetch.returncode != 0:
            detail = (fetch.stderr or fetch.stdout).strip() or "unknown error"
            raise RuntimeError(f"Failed to fetch git ref {ref}: {detail}")
        checkout = _run_git_source_command(
            ["git", "-C", str(repo_dir), "checkout", "--detach", ref],
            cwd=project_root,
        )
        if checkout.returncode != 0:
            detail = (checkout.stderr or checkout.stdout).strip() or "unknown error"
            raise RuntimeError(f"Failed to checkout git ref {ref}: {detail}")
        rev = _run_git_source_command(
            ["git", "-C", str(repo_dir), "rev-parse", "HEAD"],
            cwd=project_root,
        )
        if rev.returncode != 0 or not rev.stdout.strip():
            detail = (rev.stderr or rev.stdout).strip() or "unknown error"
            raise RuntimeError(f"Failed to resolve git revision for {ref}: {detail}")
        resolved_commit = rev.stdout.strip()
        tree = _run_git_source_command(
            ["git", "-C", str(repo_dir), "rev-parse", "HEAD^{tree}"],
            cwd=project_root,
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
        if source_dir.is_dir():
            _replace_directory_tree_from_source(
                source_dir,
                dest,
                ignore=shutil.ignore_patterns(".git"),
            )
        else:
            dest.parent.mkdir(parents=True, exist_ok=True)
            _atomic_copy_file(source_dir, dest)
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


# ---------------------------------------------------------------------------
# molt install — UV-based package management
# ---------------------------------------------------------------------------


def _ensure_uv() -> str | None:
    """Return the path to the ``uv`` binary, or *None* if unavailable."""
    uv = shutil.which("uv")
    if uv:
        return uv
    return None


def _ensure_molt_venv(
    project_root: Path,
    *,
    json_output: bool = False,
    verbose: bool = False,
) -> tuple[Path, bool]:
    """Create ``.molt-venv/`` under *project_root* using ``uv venv`` if absent.

    Returns ``(venv_path, created)`` where *created* is ``True`` when the venv
    was freshly created.
    """
    venv = _molt_venv_path(project_root)
    if venv.exists():
        return venv, False
    uv = _ensure_uv()
    if uv is None:
        raise RuntimeError(
            "uv is not installed. Install it with: curl -LsSf "
            "https://astral.sh/uv/install.sh | sh"
        )
    cmd = [
        uv,
        "venv",
        str(venv),
        "--python",
        f"{sys.version_info.major}.{sys.version_info.minor}",
    ]
    if verbose:
        print(f"[molt install] creating venv: {' '.join(cmd)}")
    result = _run_completed_command(
        cmd,
        cwd=project_root,
        env=None,
        capture_output=True,
        memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"uv venv creation failed (exit {result.returncode}):\n{result.stderr}"
        )
    return venv, True


def _read_requirements_txt(path: Path) -> list[str]:
    """Read non-comment, non-empty lines from a requirements.txt file."""
    if not path.exists():
        return []
    reqs: list[str] = []
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if line and not line.startswith("#") and not line.startswith("-"):
            reqs.append(line)
    return reqs


def _read_pyproject_deps(project_root: Path) -> list[str]:
    """Read ``[project.dependencies]`` from pyproject.toml."""
    pyproject_path = project_root / "pyproject.toml"
    if not pyproject_path.exists():
        return []
    data = _load_toml(pyproject_path)
    return list(data.get("project", {}).get("dependencies", []))


def install(
    packages: list[str] | None = None,
    *,
    requirements: str | None = None,
    json_output: bool = False,
    verbose: bool = False,
    sync: bool = False,
) -> int:
    """Install packages into ``.molt-venv/`` using UV.

    If *packages* are given on the CLI they are installed directly.
    If *requirements* points to a file (``requirements.txt``), its contents are
    used.  If *sync* is ``True`` (or no explicit packages / requirements file),
    dependencies are read from ``pyproject.toml`` and ``requirements.txt`` (if
    present) and the venv is synced to match.
    """
    uv = _ensure_uv()
    if uv is None:
        return _fail(
            "uv is not installed. Install it with: "
            "curl -LsSf https://astral.sh/uv/install.sh | sh",
            json_output,
            command="install",
        )

    project_root = _find_molt_root(Path.cwd())

    # Ensure the venv exists.
    try:
        venv, created = _ensure_molt_venv(
            project_root, json_output=json_output, verbose=verbose
        )
    except RuntimeError as exc:
        return _fail(str(exc), json_output, command="install")

    if created and not json_output:
        print(f"Created {MOLT_VENV_DIR}/ in {project_root}")

    # Decide what to install.
    specs: list[str] = []
    if packages:
        specs.extend(packages)
    elif requirements:
        req_path = Path(requirements).expanduser()
        if not req_path.exists():
            return _fail(
                f"Requirements file not found: {req_path}",
                json_output,
                command="install",
            )
        specs.extend(_read_requirements_txt(req_path))
    else:
        # Gather install specs from the local project when no explicit source is provided.
        specs.extend(_read_pyproject_deps(project_root))
        specs.extend(_read_requirements_txt(project_root / "requirements.txt"))

    if not specs:
        if not json_output:
            print("Nothing to install (no dependencies found).")
        if json_output:
            payload = _json_payload(
                "install", "ok", data={"installed": [], "venv": str(venv)}
            )
            _emit_json(payload, json_output)
        return 0

    # Run uv pip install into the .molt-venv.
    cmd = [uv, "pip", "install", "--python", str(venv / "bin" / "python")]
    cmd.extend(specs)

    if verbose and not json_output:
        print(f"[molt install] {' '.join(cmd)}")

    result = _run_completed_command(
        cmd,
        cwd=project_root,
        env=None,
        capture_output=json_output,
        memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
    )
    if result.returncode != 0:
        msg = f"uv pip install failed (exit {result.returncode})"
        if json_output and result.stderr:
            msg += f":\n{result.stderr}"
        return _fail(msg, json_output, command="install")

    if json_output:
        payload = _json_payload(
            "install",
            "ok",
            data={"installed": specs, "venv": str(venv)},
        )
        _emit_json(payload, json_output)
    elif not verbose:
        print(f"Installed {len(specs)} package(s) into {MOLT_VENV_DIR}/")

    return 0


def install_add(
    packages: list[str],
    *,
    json_output: bool = False,
    verbose: bool = False,
) -> int:
    """Add one or more packages: install into .molt-venv and append to
    ``[project.dependencies]`` in pyproject.toml via ``uv add``."""
    uv = _ensure_uv()
    if uv is None:
        return _fail(
            "uv is not installed. Install it with: "
            "curl -LsSf https://astral.sh/uv/install.sh | sh",
            json_output,
            command="install",
        )

    if not packages:
        return _fail("No packages specified.", json_output, command="install")

    project_root = _find_molt_root(Path.cwd())

    # Ensure venv exists.
    try:
        venv, created = _ensure_molt_venv(
            project_root, json_output=json_output, verbose=verbose
        )
    except RuntimeError as exc:
        return _fail(str(exc), json_output, command="install")

    if created and not json_output:
        print(f"Created {MOLT_VENV_DIR}/ in {project_root}")

    # Use `uv pip install` into the molt venv.
    pip_cmd = [
        uv,
        "pip",
        "install",
        "--python",
        str(venv / "bin" / "python"),
        *packages,
    ]
    if verbose and not json_output:
        print(f"[molt install add] {' '.join(pip_cmd)}")

    result = _run_completed_command(
        pip_cmd,
        cwd=project_root,
        env=None,
        capture_output=json_output,
        memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
    )
    if result.returncode != 0:
        msg = f"uv pip install failed (exit {result.returncode})"
        if json_output and result.stderr:
            msg += f":\n{result.stderr}"
        return _fail(msg, json_output, command="install")

    # Also run `uv add` to persist the dependency in pyproject.toml.
    pyproject_path = project_root / "pyproject.toml"
    if pyproject_path.exists():
        add_cmd = [uv, "add", *packages]
        if verbose and not json_output:
            print(f"[molt install add] {' '.join(add_cmd)}")
        add_result = _run_completed_command(
            add_cmd,
            cwd=project_root,
            env=None,
            capture_output=json_output,
            memory_guard_prefix=_CLI_MEMORY_GUARD_PREFIX,
        )
        if add_result.returncode != 0 and verbose and not json_output:
            print(
                f"Warning: uv add failed (dependencies installed but not "
                f"persisted to pyproject.toml): {add_result.stderr}"
            )

    if json_output:
        payload = _json_payload(
            "install",
            "ok",
            data={"added": packages, "venv": str(venv)},
        )
        _emit_json(payload, json_output)
    else:
        print(f"Added {', '.join(packages)} to {MOLT_VENV_DIR}/")

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
                if src_path.is_dir():
                    _replace_directory_tree_from_source(src_path, dest)
                else:
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    _atomic_copy_file(src_path, dest)
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
                resolved_ref, resolved_error = _resolve_git_ref(
                    url,
                    ref,
                    project_root=root,
                )
            if resolved_error:
                return _fail(
                    resolved_error,
                    json_output,
                    command="vendor",
                )
            if resolved_ref is None:
                return _fail(
                    "unable to resolve git ref",
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
                        url,
                        resolved_ref,
                        dest,
                        project_root=root,
                        subdirectory=subdir,
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
            _atomic_write_bytes(dest, data)
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
        _atomic_write_json(manifest_path, manifest, indent=2)

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
        raw_extras = selected_extras.get(name, set())
        extras: set[str] = {
            item for item in raw_extras if isinstance(item, str) and item
        }
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
        return None
    return data


def _write_cached_artifact(cache_path: Path, data: bytes) -> None:
    try:
        _atomic_write_bytes(cache_path, data)
    except OSError:
        pass


def _is_private_ip(host: str) -> bool:
    """Check if a hostname or IP is private/link-local/metadata."""
    # Check hostname-based blocklist first
    lower = host.lower()
    if lower in ("metadata.google.internal", "metadata.internal"):
        return True
    # Fast path: if host is already a bare IP literal, check directly
    # without DNS resolution (avoids syscall and resolver-down false positives).
    try:
        bare = ipaddress.ip_address(host)
        if isinstance(bare, ipaddress.IPv6Address) and bare.ipv4_mapped is not None:
            bare = bare.ipv4_mapped
        return (
            bare.is_private
            or bare.is_loopback
            or bare.is_link_local
            or bare.is_reserved
            or str(bare) == "169.254.169.254"
        )
    except ValueError:
        pass  # not a bare IP — fall through to DNS resolution
    # Resolve DNS and check all resulting IPs
    try:
        infos = socket.getaddrinfo(host, None, socket.AF_UNSPEC, socket.SOCK_STREAM)
    except socket.gaierror:
        return True  # unresolvable hosts are blocked conservatively
    for _family, _type, _proto, _canonname, sockaddr in infos:
        ip_str = sockaddr[0]
        if not isinstance(ip_str, str):
            return True
        # Strip IPv6 scope-id suffix (e.g. "fe80::1%eth0") before parsing
        bare_ip = ip_str.split("%")[0]
        try:
            addr = ipaddress.ip_address(bare_ip)
        except ValueError:
            return True  # fail-closed: unrecognizable address is blocked
        # Unwrap IPv4-mapped IPv6 addresses (e.g. ::ffff:169.254.169.254)
        # so the private/loopback/link-local checks work correctly.
        if isinstance(addr, ipaddress.IPv6Address) and addr.ipv4_mapped is not None:
            addr = addr.ipv4_mapped
        if (
            addr.is_private
            or addr.is_loopback
            or addr.is_link_local
            or addr.is_reserved
            or str(addr) == "169.254.169.254"
        ):
            return True
    return False


class _NoRedirectHandler(urllib.request.HTTPRedirectHandler):
    """Block all HTTP redirects to prevent SSRF via redirect bypass."""

    def redirect_request(
        self,
        req: urllib.request.Request,
        fp: Any,
        code: int,
        msg: str,
        headers: Any,
        newurl: str,
    ) -> urllib.request.Request | None:
        parsed = urllib.parse.urlparse(newurl)
        if parsed.scheme != "https":
            raise ValueError(f"redirect to non-HTTPS URL blocked: {newurl}")
        redir_host = parsed.hostname or ""
        if _is_private_ip(redir_host):
            raise ValueError(
                f"redirect to private/metadata address blocked: {redir_host}"
            )
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def _download_artifact(url: str, expected_hash: str) -> bytes:
    if not url or not expected_hash:
        raise ValueError("missing url or hash")
    parsed = urllib.parse.urlparse(url)
    if parsed.scheme != "https":
        raise ValueError(f"only https URLs are allowed, got {parsed.scheme!r}")
    host = parsed.hostname or ""
    if _is_private_ip(host):
        raise ValueError(f"URL resolves to private/metadata address: {host}")
    if ":" not in expected_hash:
        raise ValueError(
            f"hash must be in 'algorithm:hex' format, got {expected_hash!r}"
        )
    algo, expected = expected_hash.split(":", 1)
    if (
        algo != "sha256"
        or len(expected) != 64
        or not all(c in "0123456789abcdef" for c in expected)
    ):
        raise ValueError(f"unsupported or malformed hash: {expected_hash!r}")
    cache_path = _vendor_cache_path(url, expected_hash)
    if cache_path is not None:
        cached = _read_cached_artifact(cache_path, expected)
        if cached is not None:
            return cached
    opener = urllib.request.build_opener(
        _NoRedirectHandler, urllib.request.HTTPSHandler()
    )
    with opener.open(url) as response:
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
