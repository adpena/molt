"""Typed proof-command custody from admission through guarded execution.

The queue persists exactly one envelope derived from the submitted argv.  The
same envelope is validated by the guarded child that fingerprints the selected
toolchains and launches the command.  No ambient interpreter is invented for a
non-Python command and no identity subprocess runs outside memory-guard custody.
"""

from __future__ import annotations

import argparse
import functools
import hashlib
import hmac
import json
import os
import platform
import re
import secrets
import shlex
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Mapping, Sequence

_SOURCE_ROOT = Path(__file__).resolve().parents[2]
if str(_SOURCE_ROOT) not in sys.path:
    sys.path.insert(0, str(_SOURCE_ROOT))
_PYTHON_IDENTITY_PROBE = (
    _SOURCE_ROOT / "tools" / "proof_queue_pkg" / "python_identity_probe.py"
)

from molt.cargo_execution_policy import normalize_cargo_environment  # noqa: E402
from tools import proof_plan  # noqa: E402

ENVELOPE_SCHEMA = "molt.proof-command-envelope.v2"
EXECUTION_SCHEMA = "molt.proof-command-execution.v1"

_PYTHON_COMMAND = re.compile(r"^python(?:\d+(?:\.\d+)*)?(?:\.exe)?$", re.IGNORECASE)
_PY_LAUNCHERS = frozenset({"py", "py.exe"})
_PY_SELECTOR = re.compile(
    r"(?:-\d+(?:\.\d+)?(?:-(?:32|64))?|-V:[^\s/:]+(?:/[^\s/:]+)?)",
    re.IGNORECASE,
)
_SHELL_LAUNCHERS = frozenset(
    {
        "bash",
        "bash.exe",
        "cmd",
        "cmd.exe",
        "fish",
        "nu",
        "nu.exe",
        "powershell",
        "powershell.exe",
        "pwsh",
        "pwsh.exe",
        "sh",
        "sh.exe",
        "zsh",
        "zsh.exe",
    }
)
_PYTHON_CONSOLE_MODULES = {
    "pytest": "pytest",
    "pytest.exe": "pytest",
    "py.test": "pytest",
    "py.test.exe": "pytest",
    "pip-audit": "pip_audit",
    "pip-audit.exe": "pip_audit",
}
_PYTHON_CONSOLE_SCRIPTS = frozenset(_PYTHON_CONSOLE_MODULES)
# One closed authority for every admitted ``uv run`` option.  Input-bearing
# options are either assigned an immutable custody role or rejected here; no
# second parser is allowed to infer their semantics later.
_UV_OPTION_SEMANTICS: dict[str, tuple[str, str]] = {
    "--active": ("flag", "environment-selection"),
    "--all-extras": ("flag", "project-selection"),
    "--exact": ("flag", "environment-selection"),
    "--frozen": ("flag", "project-lock"),
    "--inexact": ("flag", "environment-selection"),
    "--isolated": ("flag", "environment-selection"),
    "--locked": ("flag", "project-lock"),
    "--no-config": ("flag", "environment-selection"),
    "--no-default-groups": ("flag", "project-selection"),
    "--no-dev": ("flag", "project-selection"),
    "--no-project": ("flag", "project-selection"),
    "--no-sync": ("flag", "environment-selection"),
    "--offline": ("flag", "network-denial"),
    "--directory": ("value", "source-directory"),
    "--extra": ("value", "project-selection"),
    "--group": ("value", "project-selection"),
    "--only-group": ("value", "project-selection"),
    "--project": ("value", "project-directory"),
    "--python": ("value", "python-selection"),
    "-p": ("value", "python-selection"),
    "--with-requirements": ("value", "requirements-file"),
    # These can inject source, configuration, or network state that is not
    # represented by the admitted project snapshot.  Reject them structurally
    # rather than growing exception-shaped partial custody.
    "--default-index": ("reject", "network-source"),
    "--env-file": ("reject", "environment-file"),
    "--find-links": ("reject", "package-source"),
    "--index": ("reject", "network-source"),
    "--with": ("reject", "package-overlay"),
    "--with-editable": ("reject", "editable-source"),
}
_UV_VALUE_OPTIONS = frozenset(
    option
    for option, (shape, _role) in _UV_OPTION_SEMANTICS.items()
    if shape in {"value", "reject"}
)
_PROBE_SCRIPT = r"""
import base64
import concurrent.futures
import hashlib
import hmac
import importlib.machinery
import importlib.metadata as metadata
import json
import os
import pathlib
import platform
import re
import site
import stat as stat_module
import subprocess
import sys
import sysconfig
import time
import urllib.parse
import urllib.request

hash_workers = int(sys.argv[2])
if hash_workers < 1 or hash_workers > 32:
    raise RuntimeError(f"invalid proof hash worker custody: {hash_workers}")

def file_key(path, stat):
    if stat.st_ino:
        return f"inode:{stat.st_dev}:{stat.st_ino}"
    return "path:" + os.path.normcase(str(path))

def resolve_file(path, *, label):
    resolved = pathlib.Path(path).resolve(strict=True)
    stat = resolved.stat()
    if not stat_module.S_ISREG(stat.st_mode):
        raise RuntimeError(f"{label} is not a file: {resolved}")
    return resolved, stat, file_key(resolved, stat)

def hash_work(item):
    key, path_text, algorithms = item
    path = pathlib.Path(path_text)
    before = path.stat()
    hashers = {algorithm: hashlib.new(algorithm) for algorithm in algorithms}
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            for digest in hashers.values():
                digest.update(chunk)
    after = path.stat()
    before_identity = (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
    after_identity = (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
    if before_identity != after_identity:
        raise RuntimeError(f"file changed while proof custody hashed it: {path}")
    return key, {
        "path": str(path),
        "size": after.st_size,
        "hashes": {name: digest.digest() for name, digest in hashers.items()},
    }

def hash_worklist(work):
    items = [
        (key, str(value["path"]), tuple(sorted(value["algorithms"])))
        for key, value in sorted(
            work.items(),
            key=lambda item: (os.path.normcase(str(item[1]["path"])), item[0]),
        )
    ]
    batch_size = max(1, (len(items) + hash_workers - 1) // hash_workers)
    batches = [items[index:index + batch_size] for index in range(0, len(items), batch_size)]
    with concurrent.futures.ThreadPoolExecutor(max_workers=hash_workers) as executor:
        results = executor.map(lambda batch: [hash_work(item) for item in batch], batches)
        return dict(item for batch in results for item in batch)

def fused_hash_work(item):
    preliminary, path_text, label, algorithms = item
    resolve_started = time.perf_counter()
    path, before, identity = resolve_file(path_text, label=label)
    resolve_elapsed = time.perf_counter() - resolve_started
    hash_started = time.perf_counter()
    hashers = {algorithm: hashlib.new(algorithm) for algorithm in algorithms}
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            for digest in hashers.values():
                digest.update(chunk)
    after = path.stat()
    hash_elapsed = time.perf_counter() - hash_started
    before_identity = (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
    after_identity = (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
    if before_identity != after_identity:
        raise RuntimeError(f"file changed while proof custody hashed it: {path}")
    return preliminary, identity, {
        "path": str(path),
        "size": after.st_size,
        "hashes": {name: digest.digest() for name, digest in hashers.items()},
        "resolve_stat_s": resolve_elapsed,
        "hash_s": hash_elapsed,
    }

def fused_hash_worklist(work):
    items = [
        (
            preliminary,
            str(value["path"]),
            value["label"],
            tuple(sorted(value["algorithms"])),
        )
        for preliminary, value in sorted(
            work.items(),
            key=lambda item: (os.path.normcase(str(item[1]["path"])), item[0]),
        )
    ]
    batch_size = max(1, (len(items) + hash_workers - 1) // hash_workers)
    batches = [items[index:index + batch_size] for index in range(0, len(items), batch_size)]
    with concurrent.futures.ThreadPoolExecutor(max_workers=hash_workers) as executor:
        results = executor.map(
            lambda batch: [fused_hash_work(item) for item in batch], batches
        )
        flat = [item for batch in results for item in batch]
    references = {}
    identities = {}
    for preliminary, identity, payload in flat:
        references[preliminary] = identity
        existing = identities.get(identity)
        if existing is None:
            identities[identity] = payload
            continue
        if (
            existing["size"] != payload["size"]
            or existing["hashes"]["sha256"] != payload["hashes"]["sha256"]
        ):
            raise RuntimeError(
                f"resolved file identity collision changed bytes: {identity}"
            )
        for algorithm, digest in payload["hashes"].items():
            prior = existing["hashes"].get(algorithm)
            if prior is not None and not hmac.compare_digest(prior, digest):
                raise RuntimeError(
                    f"resolved file identity collision changed {algorithm}: {identity}"
                )
            existing["hashes"][algorithm] = digest
        if os.path.normcase(payload["path"]) < os.path.normcase(existing["path"]):
            existing["path"] = payload["path"]
    return references, identities

def editable_path(payload):
    try:
        parsed = json.loads(payload)
    except json.JSONDecodeError:
        return None
    if not isinstance(parsed, dict) or not parsed.get("dir_info", {}).get("editable"):
        return None
    parsed_url = urllib.parse.urlparse(str(parsed.get("url", "")))
    if parsed_url.scheme != "file" or parsed_url.netloc not in {"", "localhost"}:
        raise RuntimeError("editable distribution must use a local file URL")
    raw = urllib.request.url2pathname(urllib.parse.unquote(parsed_url.path))
    if os.name == "nt" and re.match(r"^/[A-Za-z]:", raw):
        raw = raw[1:]
    return pathlib.Path(raw).resolve(strict=True)

def resolve_contained_path(path, root, *, label):
    resolved = pathlib.Path(path).resolve(strict=True)
    canonical_root = pathlib.Path(root).resolve(strict=True)
    try:
        relative = resolved.relative_to(canonical_root)
    except ValueError as exc:
        raise RuntimeError(f"{label} escapes {canonical_root}: {path}") from exc
    return resolved, relative

def contained_relative(path, root):
    try:
        return pathlib.Path(path).relative_to(pathlib.Path(root))
    except ValueError:
        return None

def python_runtime_manifest(admitted_root, admitted_import_roots):
    # Bind the CPython launcher, base runtime, stdlib, native modules, and import roots.
    started = time.perf_counter()
    admitted_root = pathlib.Path(admitted_root).resolve(strict=True)
    prefix = pathlib.Path(sys.prefix).resolve(strict=True)
    base_prefix = pathlib.Path(sys.base_prefix).resolve(strict=True)
    base_executable = pathlib.Path(
        getattr(sys, "_base_executable", None) or sys.executable
    ).resolve(strict=True)
    implementation = platform.python_implementation()
    if implementation != "CPython":
        raise RuntimeError(
            f"proof Python runtime must be CPython, got {implementation!r}"
        )

    declared_roots = {
        "admitted-source": admitted_root,
        "python-prefix": prefix,
        "python-base-prefix": base_prefix,
    }
    for index, (name, raw_root) in enumerate(admitted_import_roots):
        editable_root = pathlib.Path(raw_root).resolve(strict=True)
        declared_roots[f"editable-source:{index}:{name}"] = editable_root
    runtime_roots = {}
    for label, raw in (
        ("stdlib", sysconfig.get_path("stdlib")),
        ("platstdlib", sysconfig.get_path("platstdlib")),
        ("base-dlls", str(base_prefix / "DLLs")),
        (
            "base-lib-dynload",
            str(
                base_prefix
                / "lib"
                / f"python{sys.version_info.major}.{sys.version_info.minor}"
                / "lib-dynload"
            ),
        ),
    ):
        if not raw:
            continue
        candidate = pathlib.Path(raw)
        if not candidate.exists():
            continue
        resolved = candidate.resolve(strict=True)
        if not resolved.is_dir():
            raise RuntimeError(f"Python runtime root is not a directory: {resolved}")
        if label == "platstdlib" and not (resolved / "os.py").is_file():
            # Windows venvs report <venv>/Lib as platstdlib even though it only
            # owns site-packages; the base stdlib remains the runtime authority.
            continue
        runtime_roots[label] = resolved
        declared_roots[f"runtime:{label}"] = resolved

    entries = []
    work = {}
    seen_lexical = set()

    def owner_for(path):
        matches = []
        for owner, root in declared_roots.items():
            relative = contained_relative(path, root)
            if relative is not None:
                matches.append((len(root.parts), owner, root, relative))
        if not matches:
            raise RuntimeError(f"Python runtime input has no declared owner: {path}")
        _depth, owner, root, relative = max(matches)
        return owner, root, relative

    def add_file(raw, *, authority, allow_declared_external=False):
        lexical = pathlib.Path(raw).absolute()
        lexical_key = os.path.normcase(str(lexical))
        if lexical_key in seen_lexical:
            return
        seen_lexical.add(lexical_key)
        resolved, _stat, identity = resolve_file(
            lexical, label=f"Python runtime {authority}"
        )
        try:
            owner, owner_root, owner_relative = owner_for(resolved)
        except RuntimeError:
            if not allow_declared_external:
                raise
            owner = f"declared-runtime-file:{authority}"
            owner_root = resolved.parent
            owner_relative = pathlib.Path(resolved.name)
        existing = work.get(identity)
        if existing is None:
            work[identity] = {"path": resolved, "algorithms": {"sha256"}}
        elif os.path.normcase(str(resolved)) < os.path.normcase(str(existing["path"])):
            existing["path"] = resolved
        entries.append(
            {
                "authority": authority,
                "lexical_path": str(lexical),
                "resolved_path": str(resolved),
                "owner": owner,
                "owner_root": str(owner_root),
                "owner_relative": owner_relative.as_posix(),
                "symlinked": os.path.normcase(str(lexical)) != os.path.normcase(str(resolved)),
                "identity": identity,
            }
        )

    add_file(sys.executable, authority="venv-executable")
    add_file(base_executable, authority="base-executable", allow_declared_external=True)
    for cfg in dict.fromkeys(
        (
            prefix / "pyvenv.cfg",
            pathlib.Path(sys.executable).resolve(strict=True).parent.parent / "pyvenv.cfg",
        )
    ):
        if cfg.is_file():
            add_file(cfg, authority="pyvenv-config")

    shared_library_names = {
        str(value)
        for value in (
            sysconfig.get_config_var("LDLIBRARY"),
            sysconfig.get_config_var("LIBRARY"),
            sysconfig.get_config_var("INSTSONAME"),
        )
        if value
    }
    shared_search_roots = {
        base_prefix,
        pathlib.Path(base_executable).parent,
        pathlib.Path(sys.executable).resolve(strict=True).parent,
    }
    libdir = sysconfig.get_config_var("LIBDIR")
    if libdir:
        shared_search_roots.add(pathlib.Path(str(libdir)))
    for directory in sorted(shared_search_roots, key=lambda path: os.path.normcase(str(path))):
        if not directory.is_dir():
            continue
        patterns = ["python*.dll"] if os.name == "nt" else ["libpython*.so*", "libpython*.dylib"]
        for name in shared_library_names:
            candidate = directory / name
            if candidate.is_file():
                add_file(candidate, authority="python-shared-library", allow_declared_external=True)
        for pattern in patterns:
            for candidate in sorted(directory.glob(pattern)):
                if candidate.is_file():
                    add_file(candidate, authority="python-shared-library", allow_declared_external=True)

    excluded_runtime_parts = {"site-packages", "dist-packages"}
    for authority, root in sorted(runtime_roots.items()):
        for candidate in sorted(root.rglob("*")):
            relative = candidate.relative_to(root)
            if excluded_runtime_parts.intersection(relative.parts):
                continue
            if candidate.is_symlink() and candidate.is_dir():
                raise RuntimeError(
                    f"Python runtime directory symlink is not admitted: {candidate}"
                )
            if not candidate.is_file():
                continue
            resolved = candidate.resolve(strict=True)
            resolved_relative = contained_relative(resolved, root)
            if resolved_relative is not None and excluded_runtime_parts.intersection(
                resolved_relative.parts
            ):
                raise RuntimeError(
                    f"Python runtime file redirects into package installation state: {candidate}"
                )
            add_file(candidate, authority=f"runtime-root:{authority}")

    import_paths = []
    for raw in sys.path:
        lexical = admitted_root if raw == "" else pathlib.Path(raw).absolute()
        if lexical.exists():
            resolved = lexical.resolve(strict=True)
            owner, owner_root, relative = owner_for(resolved)
            if resolved.is_file():
                add_file(resolved, authority="python-import-path")
            kind = "file" if resolved.is_file() else "directory"
            exists = True
        else:
            resolved = lexical.resolve(strict=False)
            owner, owner_root, relative = owner_for(resolved)
            kind = "absent"
            exists = False
        import_paths.append(
            {
                "lexical_path": str(lexical),
                "resolved_path": str(resolved),
                "owner": owner,
                "owner_root": str(owner_root),
                "owner_relative": relative.as_posix(),
                "kind": kind,
                "exists": exists,
                "symlinked": os.path.normcase(str(lexical)) != os.path.normcase(str(resolved)),
            }
        )

    hashed = hash_worklist(work)
    manifest_rows = []
    symlinks = []
    ownership_counts = {}
    for entry in sorted(entries, key=lambda item: (item["authority"], os.path.normcase(item["lexical_path"]))):
        payload = hashed[entry["identity"]]
        row = {
            key: value for key, value in entry.items() if key != "identity"
        }
        row.update(
            {
                "size": payload["size"],
                "sha256": payload["hashes"]["sha256"].hex(),
            }
        )
        manifest_rows.append(row)
        ownership_counts[row["owner"]] = ownership_counts.get(row["owner"], 0) + 1
        if row["symlinked"]:
            symlinks.append(row)
    manifest = json.dumps(manifest_rows, separators=(",", ":"), sort_keys=True)
    import_manifest = json.dumps(import_paths, separators=(",", ":"), sort_keys=True)
    explicit_authorities = [
        row for row in manifest_rows
        if not str(row["authority"]).startswith("runtime-root:")
    ]
    extension_suffixes = tuple(importlib.machinery.EXTENSION_SUFFIXES)
    native_extensions = [
        row for row in manifest_rows
        if str(row["resolved_path"]).endswith(extension_suffixes)
    ]
    return {
        "implementation": implementation,
        "version": platform.python_version(),
        "cache_tag": getattr(sys.implementation, "cache_tag", None),
        "soabi": sysconfig.get_config_var("SOABI"),
        "platform": sysconfig.get_platform(),
        "prefix": str(prefix),
        "base_prefix": str(base_prefix),
        "base_executable": str(base_executable),
        "runtime_roots": {
            name: str(root) for name, root in sorted(runtime_roots.items())
        },
        "runtime_file_count": len(manifest_rows),
        "runtime_unique_file_count": len(hashed),
        "runtime_bytes": sum(value["size"] for value in hashed.values()),
        "runtime_manifest_sha256": hashlib.sha256(manifest.encode()).hexdigest(),
        "explicit_authority_files": explicit_authorities,
        "native_extension_files": native_extensions,
        "ownership_counts": {
            name: ownership_counts[name] for name in sorted(ownership_counts)
        },
        "symlink_inputs": symlinks,
        "import_paths": import_paths,
        "import_path_manifest_sha256": hashlib.sha256(import_manifest.encode()).hexdigest(),
        "elapsed_s": time.perf_counter() - started,
    }

def top_level_source_owners(distribution, root, *, label):
    top_level = distribution.read_text("top_level.txt") or ""
    names = sorted({line.strip() for line in top_level.splitlines() if line.strip()})
    if not names:
        raise RuntimeError(f"{label} has no top_level.txt custody")
    owners = []
    for name in names:
        matches = []
        for candidate in (
            root / name,
            root / f"{name}.py",
            root / "src" / name,
            root / "src" / f"{name}.py",
        ):
            if candidate.exists():
                resolved, _relative = resolve_contained_path(
                    candidate, root, label=f"{label} top-level owner {name!r}"
                )
                matches.append(resolved)
        matches = list(dict.fromkeys(matches))
        if len(matches) != 1:
            raise RuntimeError(
                f"{label} has {len(matches)} top-level owners for {name!r}"
            )
        owners.append(matches[0])
    return owners

def source_distribution_root(distribution, metadata_root):
    git_root = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        cwd=metadata_root,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if git_root.returncode != 0:
        raise RuntimeError(
            f"source-owned distribution at {metadata_root} has no Git source root"
        )
    root = pathlib.Path(git_root.stdout.strip()).resolve(strict=True)
    resolve_contained_path(
        metadata_root, root, label="source-owned distribution metadata"
    )
    top_level_source_owners(
        distribution,
        root,
        label=f"source-owned distribution {metadata_root}",
    )
    return root

def editable_manifest(distribution, root, admitted_root):
    scan_started = time.perf_counter()
    file_rows = []
    work = {}
    candidates = top_level_source_owners(
        distribution,
        root,
        label=f"editable distribution {distribution.metadata.get('Name')}",
    )
    for metadata_name in ("pyproject.toml", "uv.lock", "setup.cfg", "setup.py"):
        candidate = root / metadata_name
        if candidate.is_file():
            candidates.append(candidate.resolve(strict=True))
    seen = set()
    for candidate in candidates:
        paths = [candidate] if candidate.is_file() else sorted(candidate.rglob("*"))
        for path in paths:
            if not path.is_file() or "__pycache__" in path.parts:
                continue
            resolved, relative = resolve_contained_path(
                path, root, label="editable source input"
            )
            relative_key = relative.as_posix()
            if relative_key in seen:
                continue
            seen.add(relative_key)
            resolved, _stat, identity = resolve_file(
                resolved, label="editable source input"
            )
            existing = work.get(identity)
            if existing is None:
                work[identity] = {"path": resolved, "algorithms": {"sha256"}}
            elif os.path.normcase(str(resolved)) < os.path.normcase(str(existing["path"])):
                existing["path"] = resolved
            file_rows.append((relative_key, identity))
    if not file_rows:
        raise RuntimeError(f"editable distribution has no source files under {root}")
    resolve_elapsed = time.perf_counter() - scan_started
    hash_started = time.perf_counter()
    hashed = hash_worklist(work)
    hash_elapsed = time.perf_counter() - hash_started
    files = [
        (
            relative,
            hashed[identity]["size"],
            hashed[identity]["hashes"]["sha256"].hex(),
        )
        for relative, identity in sorted(file_rows)
    ]
    git_started = time.perf_counter()
    git = subprocess.run(
        ["git", "status", "--porcelain=v1", "-z", "--untracked-files=all", "--ignore-submodules=none"],
        cwd=root, check=False, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    head = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=root, check=False,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    tree = subprocess.run(
        ["git", "rev-parse", "HEAD^{tree}"], cwd=root, check=False,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    git_elapsed = time.perf_counter() - git_started
    manifest = json.dumps(files, separators=(",", ":"), sort_keys=True)
    try:
        root.relative_to(admitted_root)
        inside = True
    except ValueError:
        inside = False
    return {
        "root": str(root),
        "inside_admitted_source": inside,
        "files": len(files),
        "content_sha256": hashlib.sha256(manifest.encode()).hexdigest(),
        "git_available": git.returncode == 0 and head.returncode == 0 and tree.returncode == 0,
        "git_clean": git.returncode == 0 and not git.stdout,
        "git_status_sha256": hashlib.sha256(git.stdout if git.returncode == 0 else git.stderr).hexdigest(),
        "git_commit": head.stdout.strip().lower() if head.returncode == 0 else None,
        "git_tree": tree.stdout.strip().lower() if tree.returncode == 0 else None,
        "_profile": {
            "scan_resolve_stat_s": resolve_elapsed,
            "hash_s": hash_elapsed,
            "git_s": git_elapsed,
            "file_references": len(file_rows),
            "unique_files": len(work),
            "bytes": sum(value["size"] for value in hashed.values()),
        },
    }

executable = sys.executable
admitted_root = pathlib.Path(sys.argv[1]).resolve(strict=True)
total_started = time.perf_counter()
discovery_started = time.perf_counter()
raw_distributions = list(metadata.distributions())
discovery_elapsed = time.perf_counter() - discovery_started
resolve_started = time.perf_counter()
distribution_rows = []
installed_work = {}
installed_references = 0
install_prefix = pathlib.Path(sys.prefix).resolve(strict=True)
for distribution in raw_distributions:
    name = re.sub(r"[-_.]+", "-", distribution.metadata.get("Name", "")).lower()
    rows = []
    source_owned_root = None
    source_metadata_root = None
    direct_url = distribution.read_text("direct_url.json") or ""
    direct_editable_root = editable_path(direct_url)
    distribution_files = list(distribution.files or ())
    if distribution_files:
        candidates = [
            (
                str(item).replace("\\", "/"),
                pathlib.Path(distribution.locate_file(item)).absolute(),
                item.hash.mode if item.hash is not None else None,
                item.hash.value.rstrip("=") if item.hash is not None else None,
                int(item.size) if item.size is not None else None,
            )
            for item in sorted(distribution_files, key=lambda value: str(value))
        ]
    else:
        metadata_root_raw = getattr(distribution, "_path", None)
        try:
            metadata_root = pathlib.Path(metadata_root_raw).resolve(strict=True)
        except (TypeError, OSError, ValueError) as exc:
            raise RuntimeError(
                f"installed distribution {name or '<unnamed>'} has no installed file inventory"
            ) from exc
        try:
            metadata_root.relative_to(pathlib.Path(sys.prefix).resolve(strict=True))
        except ValueError:
            pass
        else:
            raise RuntimeError(
                f"installed distribution {name or '<unnamed>'} has no installed file inventory"
            )
        source_owned_root = source_distribution_root(distribution, metadata_root)
        source_metadata_root = metadata_root
        metadata_files = sorted(
            path for path in metadata_root.rglob("*") if path.is_file()
        )
        if not metadata_files:
            raise RuntimeError(
                f"source-owned distribution {name or '<unnamed>'} has no metadata files"
            )
        candidates = []
        for path in metadata_files:
            resolved, relative = resolve_contained_path(
                path, metadata_root, label="source-owned distribution metadata file"
            )
            candidates.append(
                (
                    "@source-metadata/" + relative.as_posix(),
                    resolved,
                    None,
                    None,
                    None,
                )
            )
    for relative, candidate, algorithm, expected, declared_size in candidates:
        lexical = pathlib.Path(candidate).absolute()
        resolved = lexical.resolve(strict=True)
        owner = None
        owner_root = None
        owner_relative = None
        allowed_roots = [("install-prefix", install_prefix)]
        if direct_editable_root is not None:
            allowed_roots.append(("editable-source", direct_editable_root))
        if source_owned_root is not None:
            allowed_roots.append(("source-owned", source_owned_root))
        for candidate_owner, candidate_root in allowed_roots:
            relative_to_owner = contained_relative(resolved, candidate_root)
            if relative_to_owner is None:
                continue
            if owner_root is None or len(candidate_root.parts) > len(owner_root.parts):
                owner = candidate_owner
                owner_root = candidate_root
                owner_relative = relative_to_owner
        if owner is None or owner_root is None or owner_relative is None:
            raise RuntimeError(
                "installed distribution RECORD path has no admitted owner: "
                f"{name}:{relative} lexical={lexical} resolved={resolved} "
                f"install_prefix={install_prefix}"
            )
        preliminary = os.path.normcase(str(candidate))
        algorithms = {"sha256"}
        if algorithm is not None:
            algorithms.add(algorithm)
        existing = installed_work.get(preliminary)
        if existing is None:
            installed_work[preliminary] = {
                "path": candidate,
                "label": f"installed distribution file {name}:{relative}",
                "algorithms": algorithms,
            }
        else:
            existing["algorithms"].update(algorithms)
        rows.append(
            {
                "relative": relative,
                "preliminary": preliminary,
                "declared_algorithm": algorithm,
                "declared_hash": expected,
                "declared_size": declared_size,
                "lexical_path": str(lexical),
                "resolved_path": str(resolved),
                "owner": owner,
                "owner_root": str(owner_root),
                "owner_relative": owner_relative.as_posix(),
                "symlinked": os.path.normcase(str(lexical)) != os.path.normcase(str(resolved)),
            }
        )
        installed_references += 1
    distribution_rows.append(
        (
            distribution,
            name,
            rows,
            source_owned_root,
            source_metadata_root,
            direct_url,
            direct_editable_root,
        )
    )
installed_references_by_path, installed_hashed = fused_hash_worklist(installed_work)
resolve_hash_elapsed = time.perf_counter() - resolve_started
validation_started = time.perf_counter()
distributions = []
editable_pending = []
for (
    distribution,
    name,
    rows,
    source_owned_root,
    source_metadata_root,
    direct_url,
    direct_editable_root,
) in distribution_rows:
    files = []
    ownership_counts = {}
    symlink_files = []
    for row in rows:
        identity = installed_references_by_path[row["preliminary"]]
        hashed = installed_hashed[identity]
        actual_sha256 = hashed["hashes"]["sha256"].hex()
        declared = None
        algorithm = row["declared_algorithm"]
        expected = row["declared_hash"]
        if algorithm is not None:
            actual_digest = hashed["hashes"][algorithm]
            actual_declared = base64.urlsafe_b64encode(actual_digest).decode().rstrip("=")
            if not hmac.compare_digest(expected, actual_declared):
                raise RuntimeError(
                    f"installed distribution RECORD hash mismatch: {name}:{row['relative']}"
                )
            declared = f"{algorithm}={expected}"
        size = hashed["size"]
        if row["declared_size"] is not None and row["declared_size"] != size:
            raise RuntimeError(
                f"installed distribution RECORD size mismatch: {name}:{row['relative']}"
            )
        file_row = {
            "relative": row["relative"],
            "lexical_path": row["lexical_path"],
            "resolved_path": row["resolved_path"],
            "owner": row["owner"],
            "owner_root": row["owner_root"],
            "owner_relative": row["owner_relative"],
            "symlinked": row["symlinked"],
            "escape_classification": f"{row['owner']}-contained",
            "size": size,
            "sha256": actual_sha256,
            "declared": declared,
        }
        files.append(file_row)
        ownership_counts[row["owner"]] = ownership_counts.get(row["owner"], 0) + 1
        if row["symlinked"]:
            symlink_files.append(file_row)
    file_manifest = json.dumps(files, separators=(",", ":"), sort_keys=True)
    record = distribution.read_text("RECORD") or ""
    installer = distribution.read_text("INSTALLER") or ""
    editable_root = direct_editable_root
    if editable_root is None:
        editable_root = source_owned_root
    payload = {
            "name": name,
            "version": distribution.version,
            "record_sha256": hashlib.sha256(record.encode()).hexdigest(),
            "file_manifest_sha256": hashlib.sha256(file_manifest.encode()).hexdigest(),
            "installed_file_count": len(files),
            "install_prefix": str(install_prefix),
            "ownership_counts": {
                owner: ownership_counts[owner] for owner in sorted(ownership_counts)
            },
            "installed_files": files,
            "symlink_files": symlink_files,
            "direct_url_sha256": hashlib.sha256(direct_url.encode()).hexdigest(),
            "installer_sha256": hashlib.sha256(installer.encode()).hexdigest(),
            "editable_source": None,
        }
    distributions.append(payload)
    if editable_root is not None:
        editable_pending.append(
            (
                len(distributions) - 1,
                distribution,
                editable_root,
                source_metadata_root,
            )
        )
validation_elapsed = time.perf_counter() - validation_started
editable_scan_elapsed = 0.0
editable_hash_elapsed = 0.0
editable_git_elapsed = 0.0
editable_references = 0
editable_unique = 0
editable_bytes = 0
editable_cache = {}
editable_reused_roots = 0
for index, distribution, editable_root, source_metadata_root in editable_pending:
    cache_key = (
        os.path.normcase(str(editable_root)),
        distribution.read_text("top_level.txt") or "",
    )
    editable = editable_cache.get(cache_key)
    if editable is None:
        editable = editable_manifest(distribution, editable_root, admitted_root)
        editable_profile = editable.pop("_profile")
        editable_scan_elapsed += editable_profile["scan_resolve_stat_s"]
        editable_hash_elapsed += editable_profile["hash_s"]
        editable_git_elapsed += editable_profile["git_s"]
        editable_references += editable_profile["file_references"]
        editable_unique += editable_profile["unique_files"]
        editable_bytes += editable_profile["bytes"]
        editable_cache[cache_key] = editable
    else:
        editable_reused_roots += 1
    editable = dict(editable)
    if source_metadata_root is not None:
        editable["source_metadata_root"] = str(source_metadata_root)
        try:
            source_metadata_root.relative_to(admitted_root)
            metadata_inside = True
        except ValueError:
            metadata_inside = False
        editable["source_metadata_inside_admitted_source"] = metadata_inside
    distributions[index]["editable_source"] = editable
distributions.sort(key=lambda item: (item["name"], item["version"], item["file_manifest_sha256"]))
inventory = json.dumps(distributions, separators=(",", ":"), sort_keys=True)
runtime_import_roots = []
for distribution in distributions:
    editable_source = distribution.get("editable_source")
    if isinstance(editable_source, dict) and editable_source.get("root"):
        runtime_import_roots.append((distribution["name"], editable_source["root"]))
runtime = python_runtime_manifest(admitted_root, runtime_import_roots)
runtime_elapsed = runtime.pop("elapsed_s")
runtime_closure = json.dumps(runtime, separators=(",", ":"), sort_keys=True)
executable_path, _executable_stat, executable_key = resolve_file(
    executable, label="proof Python executable"
)
executable_hashed = hash_worklist(
    {executable_key: {"path": executable_path, "algorithms": {"sha256"}}}
)
executable_sha256 = executable_hashed[executable_key]["hashes"]["sha256"].hex()
profile = {
    "hash_workers": hash_workers,
    "distribution_discovery_s": discovery_elapsed,
    "installed_resolve_stat_hash_wall_s": resolve_hash_elapsed,
    "installed_resolve_stat_cpu_s": sum(
        value["resolve_stat_s"] for value in installed_hashed.values()
    ),
    "installed_hash_cpu_s": sum(
        value["hash_s"] for value in installed_hashed.values()
    ),
    "record_validation_s": validation_elapsed,
    "editable_scan_resolve_stat_s": editable_scan_elapsed,
    "editable_hash_s": editable_hash_elapsed,
    "editable_git_s": editable_git_elapsed,
    "installed_file_references": installed_references,
    "installed_unique_paths": len(installed_work),
    "installed_unique_files": len(installed_hashed),
    "installed_deduplicated_references": installed_references - len(installed_hashed),
    "installed_bytes": sum(value["size"] for value in installed_hashed.values()),
    "editable_file_references": editable_references,
    "editable_unique_files": editable_unique,
    "editable_reused_roots": editable_reused_roots,
    "editable_bytes": editable_bytes,
    "runtime_closure_s": runtime_elapsed,
    "runtime_file_count": runtime["runtime_file_count"],
    "runtime_unique_file_count": runtime["runtime_unique_file_count"],
    "runtime_bytes": runtime["runtime_bytes"],
    "total_s": time.perf_counter() - total_started,
}
print(
    json.dumps(
        {
            "executable": executable,
            "implementation": platform.python_implementation(),
            "version": platform.python_version(),
            "executable_sha256": executable_sha256,
            "runtime": runtime,
            "runtime_closure_sha256": hashlib.sha256(runtime_closure.encode()).hexdigest(),
            "distributions": distributions,
            "distribution_inventory_sha256": hashlib.sha256(inventory.encode()).hexdigest(),
            "inventory_profile": profile,
        },
        sort_keys=True,
    )
)
"""
_WHICH_SCRIPT = (
    "import json,pathlib,shutil,sys;"
    "v=sys.argv[1];c=pathlib.Path(v);"
    "p=str(c.resolve()) if (c.is_absolute() or c.parent != pathlib.Path('.')) and c.exists() else shutil.which(v);"
    "print(json.dumps({'path':p},sort_keys=True))"
)


def _basename(value: str) -> str:
    return value.replace("\\", "/").rsplit("/", 1)[-1].casefold()


def _executable_registry_names(value: str) -> frozenset[str]:
    basename = _basename(value)
    suffixes = (".exe", ".cmd", ".bat", ".ps1")
    stem = next(
        (basename[: -len(suffix)] for suffix in suffixes if basename.endswith(suffix)),
        basename,
    )
    return frozenset({stem, *(stem + suffix for suffix in suffixes)})


def _uv_prefix_and_payload(argv: Sequence[str]) -> tuple[list[str], list[str]]:
    if len(argv) < 3 or argv[1] != "run":
        raise ValueError("proof queue only models `uv run` execution envelopes")
    index = 2
    while index < len(argv):
        value = argv[index]
        if value == "--":
            index += 1
            break
        option = value.split("=", 1)[0]
        semantics = _UV_OPTION_SEMANTICS.get(option)
        if semantics is None:
            if value.startswith("-"):
                raise ValueError(
                    f"unmodeled uv run option {value!r}; executable proof custody "
                    "requires an exact, typed launch prefix"
                )
            break
        shape, role = semantics
        if shape == "reject":
            raise ValueError(
                f"uv option {option!r} is non-hermetic ({role}) and is not "
                "admitted by proof custody"
            )
        if shape == "flag":
            if "=" in value:
                raise ValueError(f"uv flag {option!r} does not accept a value")
            index += 1
            continue
        if shape == "value":
            if "=" in value:
                if not value.split("=", 1)[1]:
                    raise ValueError(f"uv option {option!r} has an empty value")
                index += 1
            else:
                if index + 1 >= len(argv) or not argv[index + 1]:
                    raise ValueError(f"uv option {option!r} needs a value")
                index += 2
            continue
        raise AssertionError(f"unknown uv option shape {shape!r}")
    payload = [str(value) for value in argv[index:]]
    if not payload:
        raise ValueError("uv run proof envelope has no payload command")
    return [str(value) for value in argv[:index]], payload


@functools.lru_cache(maxsize=1)
def _proof_command_registry() -> dict[str, object]:
    """Project the proof plan into the one admitted executable/toolchain registry."""
    plan = proof_plan.ProofPlan.load()
    exact: dict[tuple[str, ...], dict[str, object]] = {}
    console_tools: dict[str, set[str]] = {}
    policy_executables: dict[str, str] = {}
    for policy in plan.toolchain_policies:
        executable = str(policy.data.get("executable") or policy.name)
        if executable == "{python}":
            continue
        for basename in _executable_registry_names(executable):
            prior = policy_executables.get(basename)
            if prior is not None and prior != policy.name:
                raise ValueError(
                    f"proof plan executable {basename!r} has ambiguous toolchain policies"
                )
            policy_executables[basename] = policy.name
    for command in plan.commands:
        argv = tuple(str(value) for value in command.argv)
        declared = tuple(command.toolchains)
        existing = exact.get(argv)
        if existing is None:
            exact[argv] = {"ids": [command.id], "toolchains": declared}
        else:
            if existing["toolchains"] != declared:
                raise ValueError(
                    "identical proof-plan argv has conflicting toolchain authorities: "
                    f"{existing['ids']!r}, {command.id!r}"
                )
            ids = existing["ids"]
            assert isinstance(ids, list)
            ids.append(command.id)
        if argv and _basename(argv[0]) in {"uv", "uv.exe"}:
            _prefix, payload = _uv_prefix_and_payload(argv)
            payload_name = _basename(payload[0])
            if not _PYTHON_COMMAND.fullmatch(payload_name):
                console_tools.setdefault(payload_name, set()).update(command.toolchains)
    return {
        "exact": exact,
        "console_tools": {
            name: tuple(sorted(toolchains))
            for name, toolchains in sorted(console_tools.items())
        },
        "policy_executables": policy_executables,
    }


def _registered_console_toolchains(name: str) -> tuple[str, ...] | None:
    registry = _proof_command_registry()
    console_tools = registry["console_tools"]
    assert isinstance(console_tools, dict)
    value = console_tools.get(name)
    return tuple(value) if isinstance(value, tuple) else None


def _command_registration(
    argv: Sequence[str], *, has_python: bool, has_uv: bool
) -> tuple[str, list[str], list[str]]:
    registry = _proof_command_registry()
    exact = registry["exact"]
    assert isinstance(exact, dict)
    exact_match = exact.get(tuple(str(value) for value in argv))
    if isinstance(exact_match, dict):
        command_ids = exact_match["ids"]
        declared = exact_match["toolchains"]
        assert isinstance(command_ids, list) and isinstance(declared, tuple)
        toolchains = [str(name) for name in declared]
        if not toolchains:
            raise ValueError(
                f"proof-plan commands {command_ids!r} have no toolchain authority"
            )
        return "proof-plan", toolchains, [str(command_id) for command_id in command_ids]

    toolchains: list[str] = []

    def add(name: str) -> None:
        if name not in toolchains:
            toolchains.append(name)

    if has_python:
        add("python")
        if has_uv:
            add("uv")
        if argv and _basename(argv[0]) in {"uv", "uv.exe"}:
            _prefix, payload = _uv_prefix_and_payload(argv)
            console = _registered_console_toolchains(_basename(payload[0]))
            if console is not None:
                for name in console:
                    add(name)
        return "python", toolchains, []

    if not argv:
        raise ValueError("proof command has no executable registration")
    first = _basename(argv[0])
    policy_executables = registry["policy_executables"]
    assert isinstance(policy_executables, dict)
    policy_name = policy_executables.get(first)
    if not isinstance(policy_name, str):
        raise ValueError(
            f"unknown proof executable kind {argv[0]!r}; add it to the proof-plan "
            "toolchain registry or invoke a registered typed command family"
        )
    add(policy_name)
    if policy_name == "cargo":
        add("rustc")
        if len(argv) > 1 and argv[1] == "deny":
            add("cargo-deny")
        elif len(argv) > 1 and argv[1] == "audit":
            add("cargo-audit")
    return "toolchain", toolchains, []


def _uv_option_values(prefix: Sequence[str], name: str) -> list[str]:
    values: list[str] = []
    index = 2
    while index < len(prefix):
        value = str(prefix[index])
        option = value.split("=", 1)[0]
        if option == name:
            if "=" in value:
                values.append(value.split("=", 1)[1])
                index += 1
            else:
                values.append(str(prefix[index + 1]))
                index += 2
            continue
        index += 2 if option in _UV_VALUE_OPTIONS and "=" not in value else 1
    return values


def _uv_option_value_indices(prefix: Sequence[str], name: str) -> list[int]:
    """Return indices of values for ``name`` in the persisted uv prefix."""
    indices: list[int] = []
    index = 2
    while index < len(prefix):
        value = str(prefix[index])
        option = value.split("=", 1)[0]
        semantics = _UV_OPTION_SEMANTICS.get(option)
        if semantics is None:
            break
        shape, _role = semantics
        if option == name:
            if "=" in value:
                indices.append(index)
                index += 1
            else:
                indices.append(index + 1)
                index += 2
            continue
        index += 2 if shape == "value" and "=" not in value else 1
    return indices


def _path_inside(root: Path, raw: str, *, base: Path, label: str) -> Path:
    candidate = Path(raw)
    resolved = (candidate if candidate.is_absolute() else base / candidate).resolve(
        strict=True
    )
    try:
        resolved.relative_to(root.resolve())
    except ValueError as exc:
        raise ValueError(
            f"{label} {raw!r} escapes admitted source root {root}"
        ) from exc
    return resolved


_HASHED_REQUIREMENT = re.compile(
    r"^[A-Za-z0-9_.-]+(?:\[[A-Za-z0-9_.,-]+\])?=="
    r"(?:[0-9]+!)?[0-9]+(?:\.[0-9]+)*(?:(?:a|b|rc)[0-9]+)?"
    r"(?:\.post[0-9]+)?(?:\.dev[0-9]+)?"
    r"(?:\+[A-Za-z0-9]+(?:[._-][A-Za-z0-9]+)*)?"
    r"(?:\s+--hash=sha256:[0-9a-fA-F]{64})+$"
)


def _validate_requirements_file(path: Path) -> None:
    """Admit only offline, hash-locked package requirements."""
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        raise ValueError(f"requirements custody cannot read {path}") from exc
    logical: list[str] = []
    pending = ""
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        pending = f"{pending} {line}".strip()
        if pending.endswith("\\"):
            pending = pending[:-1].rstrip()
            continue
        logical.append(pending)
        pending = ""
    if pending:
        raise ValueError(f"requirements file {path} ends in a continuation")
    if not logical:
        raise ValueError(f"requirements file {path} has no locked requirements")
    for line in logical:
        if not _HASHED_REQUIREMENT.fullmatch(line):
            raise ValueError(
                "proof requirements must be exact name==version entries with one "
                f"or more sha256 hashes; rejected {line!r} in {path}"
            )


def _execution_source_paths(
    envelope: Mapping[str, object], *, cwd: Path
) -> tuple[Path, list[Path]]:
    python = envelope.get("python")
    if not isinstance(python, Mapping) or python.get("kind") not in {
        "uv",
        "uv-console-script",
    }:
        return cwd.resolve(strict=True), []
    prefix = python.get("prefix")
    if not isinstance(prefix, list):
        raise ValueError("uv command envelope has no prefix")
    directories = _uv_option_values(prefix, "--directory")
    if len(directories) > 1:
        raise ValueError("uv command envelope has multiple --directory authorities")
    effective = (
        _path_inside(cwd, directories[0], base=cwd, label="uv --directory")
        if directories
        else cwd.resolve(strict=True)
    )
    projects = _uv_option_values(prefix, "--project")
    if len(projects) > 1:
        raise ValueError("uv command envelope has multiple --project authorities")
    if projects:
        project = _path_inside(cwd, projects[0], base=effective, label="uv --project")
        if project != effective:
            raise ValueError(
                "uv --project must equal the effective command cwd so one source "
                "snapshot owns every consumed project input"
            )
    overlay_inputs = [
        _path_inside(cwd, raw, base=effective, label="uv --with-requirements")
        for raw in _uv_option_values(prefix, "--with-requirements")
    ]
    if overlay_inputs and "--offline" not in prefix:
        raise ValueError("uv --with-requirements proofs require --offline custody")
    for overlay in overlay_inputs:
        if not overlay.is_file():
            raise ValueError(f"requirements authority is not a file: {overlay}")
        _validate_requirements_file(overlay)
    return effective, overlay_inputs


def _canonical_uv_prefix(
    envelope: Mapping[str, object], *, cwd: Path
) -> tuple[list[str], Path, list[Path]]:
    python = envelope.get("python")
    if not isinstance(python, Mapping) or python.get("kind") not in {
        "uv",
        "uv-console-script",
    }:
        return [], cwd.resolve(strict=True), []
    prefix = python.get("prefix")
    if not isinstance(prefix, list):
        raise ValueError("uv command envelope has no prefix")
    exact_prefix = [str(value) for value in prefix]
    effective, overlays = _execution_source_paths(envelope, cwd=cwd)
    replacements = {
        "--directory": [effective] if _uv_option_values(prefix, "--directory") else [],
        "--project": [effective] if _uv_option_values(prefix, "--project") else [],
        "--with-requirements": overlays,
    }
    for option, paths in replacements.items():
        indices = _uv_option_value_indices(prefix, option)
        if len(indices) != len(paths):
            raise ValueError(f"uv {option} custody index mismatch")
        for index, path in zip(indices, paths, strict=True):
            original = exact_prefix[index]
            exact_prefix[index] = (
                f"{option}={path}" if original.startswith(f"{option}=") else str(path)
            )
    return exact_prefix, effective, overlays


def _guarded_exec_invocation(argv: Sequence[str]) -> dict[str, object] | None:
    """Parse every canonical spelling of the queue's guarded delegation seam."""
    if not argv:
        return None
    if _basename(argv[0]) in {"uv", "uv.exe"}:
        prefix, payload = _uv_prefix_and_payload(argv)
        offset = len(prefix)
    else:
        payload = [str(value) for value in argv]
        offset = 0
    if not payload:
        return None
    first = _basename(payload[0])
    python_index = 1
    if _PYTHON_COMMAND.fullmatch(first) or first in _PY_LAUNCHERS:
        if (
            first in _PY_LAUNCHERS
            and len(payload) > 1
            and _PY_SELECTOR.fullmatch(payload[1])
        ):
            python_index = 2
    else:
        if any("guarded_exec" in str(value).casefold() for value in payload):
            raise ValueError("guarded_exec delegation must be the direct Python target")
        return None
    if python_index >= len(payload):
        return None
    target = payload[python_index]
    mode: str | None = None
    target_indices: list[int] = []
    after_target = python_index + 1
    if target == "-m":
        if after_target >= len(payload):
            return None
        module = payload[after_target]
        if module == "tools.guarded_exec":
            mode = "module"
            target_indices = [offset + python_index, offset + after_target]
            after_target += 1
        elif "guarded_exec" in module.casefold():
            raise ValueError(f"ambiguous guarded_exec module authority {module!r}")
    elif _basename(target) == "guarded_exec.py":
        mode = "script"
        target_indices = [offset + python_index]
    if mode is None:
        if any(
            "guarded_exec" in str(value).casefold()
            for value in payload[python_index + 1 :]
        ):
            raise ValueError("guarded_exec delegation must be the direct Python target")
        return None
    try:
        separator = payload.index("--", after_target)
    except ValueError:
        raise ValueError("guarded_exec delegation requires an explicit `--` boundary")
    nested = payload[separator + 1 :]
    if not nested:
        raise ValueError("guarded_exec delegation has no delegated command")
    return {
        "mode": mode,
        "target_indices": target_indices,
        "delegated_index": offset + separator + 1,
        "nested": [str(value) for value in nested],
    }


def _nested_command(argv: Sequence[str]) -> list[str] | None:
    invocation = _guarded_exec_invocation(argv)
    if invocation is None:
        return None
    nested = invocation["nested"]
    assert isinstance(nested, list)
    return [str(value) for value in nested]


def envelope_for_command(command: Sequence[str]) -> dict[str, object]:
    """Derive and validate the sole executable/toolchain authority for ``command``."""
    argv = [str(value) for value in command]
    if not argv or not argv[0]:
        raise ValueError("proof command must have a non-empty executable")
    first = _basename(argv[0])
    if first in _SHELL_LAUNCHERS:
        raise ValueError(
            "opaque shell wrappers are not executable proof evidence; submit a "
            "typed argv command or a declared queue command family"
        )

    python: dict[str, object] | None = None
    if first in {"uv", "uv.exe"}:
        prefix, payload = _uv_prefix_and_payload(argv)
        payload_first = _basename(payload[0])
        if _PYTHON_COMMAND.fullmatch(payload_first):
            python = {"kind": "uv", "prefix": prefix, "payload_executable": payload[0]}
        elif (
            payload_first in _PYTHON_CONSOLE_SCRIPTS
            or _registered_console_toolchains(payload_first) is not None
        ):
            python = {
                "kind": "uv-console-script",
                "prefix": prefix,
                "console_script": payload[0],
            }
        elif payload_first in _SHELL_LAUNCHERS:
            raise ValueError(
                "opaque shell payloads under uv are not executable proof evidence"
            )
        elif payload_first in {"cargo", "cargo.exe", "rustc", "rustc.exe"}:
            raise ValueError(
                "direct Rust payloads under uv bypass canonical queue custody; use "
                "the queue Cargo command family or a direct typed rustc argv"
            )
        else:
            raise ValueError(
                f"uv payload {payload[0]!r} may be an interpreter-bound console "
                "script; invoke it as `python -m ...` or declare a typed command family"
            )
    elif _PYTHON_COMMAND.fullmatch(first):
        python = {"kind": "direct", "executable": argv[0]}
    elif first in _PY_LAUNCHERS:
        selector: str | None = None
        if len(argv) > 1 and (argv[1].startswith("-") or argv[1].startswith("/")):
            if not _PY_SELECTOR.fullmatch(argv[1]):
                raise ValueError(f"unsupported Windows py selector {argv[1]!r}")
            selector = argv[1]
        python = {"kind": "py-launcher", "launcher": argv[0], "selector": selector}
    elif first in _PYTHON_CONSOLE_SCRIPTS:
        raise ValueError(
            "raw Python console scripts do not identify an interpreter; use "
            "`python -m pytest` or an exact `uv run ... pytest` envelope"
        )

    registration_kind, toolchains, proof_plan_command_ids = _command_registration(
        argv,
        has_python=python is not None,
        has_uv=first in {"uv", "uv.exe"},
    )
    guarded_exec = _guarded_exec_invocation(argv)
    nested_command = (
        [str(value) for value in guarded_exec["nested"]]
        if guarded_exec is not None
        else None
    )
    delegated = (
        envelope_for_command(nested_command) if nested_command is not None else None
    )
    if delegated is not None:
        if delegated.get("python") is not None:
            raise ValueError(
                "guarded_exec may not delegate another Python authority; invoke the "
                "final Python command directly"
            )
        if (
            delegated.get("guarded_exec") is not None
            or delegated.get("delegated") is not None
        ):
            raise ValueError(
                "nested guarded_exec delegation is limited to one typed layer"
            )
        for name in delegated["toolchains"]:  # type: ignore[union-attr]
            if name not in toolchains:
                toolchains.append(str(name))
    return {
        "schema": ENVELOPE_SCHEMA,
        "kind": registration_kind,
        "argv": argv,
        "python": python,
        "toolchains": toolchains,
        "proof_plan_command_ids": proof_plan_command_ids,
        "guarded_exec": (
            {key: value for key, value in guarded_exec.items() if key != "nested"}
            if guarded_exec is not None
            else None
        ),
        "delegated": delegated,
    }


def admission_envelope(command: Sequence[str]) -> dict[str, object]:
    """Persist rejected argv without fabricating any executable authority."""
    try:
        return envelope_for_command(command)
    except ValueError as exc:
        return {
            "schema": ENVELOPE_SCHEMA,
            "kind": "rejected",
            "argv": [str(value) for value in command],
            "python": None,
            "toolchains": [],
            "proof_plan_command_ids": [],
            "guarded_exec": None,
            "delegated": None,
            "error": str(exc),
        }


def validate_envelope(envelope: Mapping[str, object], command: Sequence[str]) -> None:
    expected = envelope_for_command(command)
    if dict(envelope) != expected:
        raise ValueError(
            "persisted proof command envelope does not match submitted argv"
        )


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
    command: Sequence[str], *, cwd: Path, env: Mapping[str, str], timeout: float = 30.0
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(command),
        cwd=cwd,
        env=dict(env),
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
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
        prefix, _effective, _overlays = _canonical_uv_prefix(envelope, cwd=cwd)
        raw_prefix = python.get("prefix")
        assert isinstance(raw_prefix, list)
        argv = [*prefix, *argv[len(raw_prefix) :]]
    argv[0] = str(_resolve_outer_executable(argv[0], cwd=cwd, env=env))
    if isinstance(python, Mapping) and python.get("kind") == "uv-console-script":
        prefix = python.get("prefix")
        assert isinstance(prefix, list)
        console = _basename(str(python["console_script"]))
        payload_index = len(prefix)
        module = _PYTHON_CONSOLE_MODULES.get(console)
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
    console = _basename(str(python.get("console_script") or ""))
    if console in _PYTHON_CONSOLE_MODULES:
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
    guarded_exec_path = _path_inside(
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


def _python_probe_command(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    source_root: Path,
    hash_workers: int = 1,
) -> list[str] | None:
    python = envelope.get("python")
    if not isinstance(python, Mapping):
        return None
    kind = python.get("kind")
    if kind == "direct":
        return [
            exact[0],
            str(_PYTHON_IDENTITY_PROBE),
            str(source_root),
            str(hash_workers),
        ]
    if kind == "py-launcher":
        command = [exact[0]]
        selector = python.get("selector")
        if isinstance(selector, str) and selector:
            command.append(selector)
        return [
            *command,
            str(_PYTHON_IDENTITY_PROBE),
            str(source_root),
            str(hash_workers),
        ]
    if kind in {"uv", "uv-console-script"}:
        prefix = python.get("prefix")
        if not isinstance(prefix, list) or len(prefix) < 2:
            raise ValueError("uv proof envelope has no exact prefix")
        return [
            *exact[: len(prefix)],
            "python",
            str(_PYTHON_IDENTITY_PROBE),
            str(source_root),
            str(hash_workers),
        ]
    raise ValueError(f"unknown proof Python envelope kind {kind!r}")


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
        _run_captured(command, cwd=cwd, env=env), purpose="proof Python identity probe"
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
    nested = _nested_command(argv)
    payload = nested if nested is not None else argv
    lowered = [_basename(value) for value in payload]
    if any(value in _PYTHON_CONSOLE_SCRIPTS for value in lowered):
        return True
    for index, value in enumerate(payload[:-1]):
        if value == "-m" and payload[index + 1] in {"pytest", "py.test"}:
            return True
    return bool(
        payload
        and _basename(payload[0]) in {"cargo", "cargo.exe"}
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
            envelope, exact, ("python", "-c", _WHICH_SCRIPT, name)
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
        and _basename(exact[0]) in _executable_registry_names(requested)
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
        if resolved.returncode != 0 or not resolved.stdout.strip():
            raise ValueError(
                f"{name} content-path probe failed: "
                + (resolved.stderr.strip() or resolved.stdout.strip())
            )
        candidate = Path(resolved.stdout.strip())
        if not candidate.is_file():
            raise ValueError(
                f"{name} content-path probe returned no executable: {candidate}"
            )
        content_path = candidate.resolve(strict=True)
        content_resolver_identity = _executable_identity(resolver)
    material: dict[str, object] = {
        "path": str(path),
        "launcher_sha256": _hash_file(path),
        "content_path": str(content_path),
        "executable_sha256": _hash_file(content_path),
        "version": (completed.stdout or completed.stderr).strip(),
        "probe_cwd": str(probe_cwd),
        "policy_sha256": hashlib.sha256(
            json.dumps(policy.data, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest(),
        "configuration_files": _tool_configuration_identities(name, cwd=cwd, env=env),
    }
    if content_resolver_identity is not None:
        material["content_resolver"] = content_resolver_identity
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
        "CXX",
        "CXXFLAGS",
        "INCLUDE",
        "LDFLAGS",
        "LIB",
        "LINK",
        "LLVM_CONFIG",
        "MAKEFLAGS",
        "NINJAFLAGS",
        "RUSTC",
        "RUSTFLAGS",
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
_EXECUTABLE_ENV_NAMES = frozenset(
    {
        "AR",
        "CC",
        "CXX",
        "LINK",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUNNER",
        "CLANG",
        "LLVM_CONFIG",
        "WASM_BINDGEN",
        "WASM_OPT",
    }
)
_EXECUTABLE_ENV_PATTERNS = (
    re.compile(r"(?:AR|CC|CXX)_[A-Z0-9_]+"),
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


def command_secret_policy_error(command: Sequence[str]) -> str | None:
    for index, value in enumerate(command):
        if re.search(r"://[^/@\s]+@", value):
            return f"command argument {index} embeds URL credentials"
        if _SECRET_ARGUMENT_FLAG.match(value):
            return (
                f"secret-bearing command option {value.split('=', 1)[0]!r} is forbidden"
            )
    return None


def _environment_name_class(name: str) -> str | None:
    upper = name.upper()
    if upper in _NONDETERMINISTIC_ENV_NAMES:
        return "denied-nondeterministic"
    if upper in _ENVIRONMENT_EXACT_NAMES:
        return "host-runtime"
    if upper in _ENVIRONMENT_BUILD_NAMES:
        return "build-toolchain"
    if any(upper.startswith(prefix) for prefix in _ENVIRONMENT_PREFIXES):
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
        if name.upper() in _CANONICAL_EXECUTION_ENV or name.upper() == "NODE_OPTIONS":
            return f"queue-owned canonical environment override {name!r} is forbidden"
        classification = _environment_name_class(name)
        if classification is None:
            return f"unclassified environment override {name!r}"
        if classification == "denied-nondeterministic":
            return f"nondeterministic environment override {name!r} is forbidden"
        if _SECRET_ENV_NAME.search(name):
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
        } or _SECRET_ENV_NAME.search(name):
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
            "class": _environment_name_class(name),
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
    }
    payload["identity_sha256"] = hashlib.sha256(
        json.dumps(payload, sort_keys=True).encode()
    ).hexdigest()
    return payload


def _execution_environment_executable_identities(
    env: Mapping[str, str], *, cwd: Path
) -> dict[str, object]:
    identities: dict[str, object] = {}
    for name, value in sorted(env.items()):
        upper = name.upper()
        if upper not in _EXECUTABLE_ENV_NAMES and not any(
            pattern.fullmatch(upper) for pattern in _EXECUTABLE_ENV_PATTERNS
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
        path = _resolve_outer_executable(token, cwd=cwd, env=env)
        identity = _executable_identity(path)
        if not _content_identity_available(identity):
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
    proof_python = _python_identity(
        envelope,
        exact,
        cwd=cwd,
        env=env,
        source_root=source_root,
        hash_workers=hash_workers,
    )
    if proof_python is None and "python" in requested:
        synthetic_envelope = envelope_for_command(
            [sys.executable, "-c", "raise SystemExit('identity-only')"]
        )
        proof_python = _python_identity(
            synthetic_envelope,
            [sys.executable, "-c", "raise SystemExit('identity-only')"],
            cwd=cwd,
            env=env,
            source_root=source_root,
            hash_workers=hash_workers,
        )
    toolchains: dict[str, object] = {}
    if proof_python is not None:
        toolchains["python"] = proof_python
    for name in requested:
        if name == "python":
            continue
        toolchains[name] = _tool_identity(plan, name, envelope, exact, cwd=cwd, env=env)
    if set(toolchains) != set(requested):
        raise ValueError(
            "proof command toolchain capture is incomplete: "
            f"requested={sorted(requested)!r} captured={sorted(toolchains)!r}"
        )
    return proof_python, toolchains


def _stable_toolchain_custody(
    toolchains: Mapping[str, object],
) -> dict[str, object]:
    stable = json.loads(json.dumps(toolchains, sort_keys=True))
    python = stable.get("python")
    if isinstance(python, dict):
        python.pop("inventory_profile", None)
    return stable


def _python_editable_ineligible_reasons(
    identity: Mapping[str, object] | None,
    *,
    source_snapshot: Mapping[str, object],
) -> list[str]:
    if identity is None:
        return []
    reasons: list[str] = []
    distributions = identity.get("distributions")
    if not isinstance(distributions, list):
        return ["python-distribution-inventory-malformed"]
    for distribution in distributions:
        if not isinstance(distribution, Mapping):
            reasons.append("python-distribution-inventory-malformed")
            continue
        editable = distribution.get("editable_source")
        if not isinstance(editable, Mapping):
            continue
        name = str(distribution.get("name") or "unknown")
        if editable.get("inside_admitted_source") is not True:
            reasons.append(f"python-editable-source-outside:{name}")
        if (
            editable.get("source_metadata_root") is not None
            and editable.get("source_metadata_inside_admitted_source") is not True
        ):
            reasons.append(f"python-source-metadata-outside:{name}")
        if editable.get("git_available") is not True:
            reasons.append(f"python-editable-source-git-unavailable:{name}")
        elif editable.get("git_clean") is not True:
            reasons.append(f"python-editable-source-dirty:{name}")
        if editable.get("git_commit") != source_snapshot.get("commit"):
            reasons.append(f"python-editable-source-commit-mismatch:{name}")
        if editable.get("git_tree") != source_snapshot.get("tree"):
            reasons.append(f"python-editable-source-tree-mismatch:{name}")
    return reasons


def _python_editable_change_reasons(
    before: Mapping[str, object] | None,
    after: Mapping[str, object] | None,
) -> list[str]:
    def manifests(identity: Mapping[str, object] | None) -> dict[str, object]:
        if identity is None or not isinstance(identity.get("distributions"), list):
            return {}
        result: dict[str, object] = {}
        for distribution in identity["distributions"]:  # type: ignore[index]
            if not isinstance(distribution, Mapping):
                continue
            editable = distribution.get("editable_source")
            if isinstance(editable, Mapping):
                result[str(distribution.get("name") or "unknown")] = dict(editable)
        return result

    before_manifests = manifests(before)
    after_manifests = manifests(after)
    names = sorted(set(before_manifests) | set(after_manifests))
    return [
        f"python-editable-source-changed:{name}"
        for name in names
        if before_manifests.get(name) != after_manifests.get(name)
    ]


def _git_snapshot(cwd: Path, env: Mapping[str, str]) -> dict[str, object]:
    head = _run_captured(("git", "rev-parse", "HEAD"), cwd=cwd, env=env)
    if head.returncode != 0 or not re.fullmatch(
        r"[0-9a-fA-F]{40}|[0-9a-fA-F]{64}", head.stdout.strip()
    ):
        return {
            "available": False,
            "clean": False,
            "commit": None,
            "status_sha256": None,
        }
    root = _run_captured(("git", "rev-parse", "--show-toplevel"), cwd=cwd, env=env)
    if root.returncode != 0:
        return {
            "available": False,
            "clean": False,
            "commit": head.stdout.strip().lower(),
            "status_sha256": None,
        }
    source_root = Path(root.stdout.strip()).resolve(strict=True)
    tree = _run_captured(("git", "rev-parse", "HEAD^{tree}"), cwd=cwd, env=env)
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
    status = subprocess.run(
        [
            "git",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
        cwd=cwd,
        env=dict(env),
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
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


def _atomic_json(path: Path, payload: Mapping[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + f".{os.getpid()}.tmp")
    temporary.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    os.replace(temporary, path)


def execution_custody_sha256(
    context: Mapping[str, object], *, run_id: str, returncode: int
) -> str:
    custody_context = dict(context)
    for field in (
        "execution_custody_sha256",
        "guard_receipt",
        "terminal_evidence_sha256",
    ):
        custody_context.pop(field, None)
    material = {
        "run_id": run_id,
        "execution_nonce_sha256": custody_context.get("execution_nonce_sha256"),
        "command_returncode": returncode,
        "receipt_context": custody_context,
    }
    return hashlib.sha256(
        json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def terminal_evidence_sha256(
    context: Mapping[str, object], *, run_id: str, returncode: int
) -> str:
    terminal_context = dict(context)
    terminal_context.pop("terminal_evidence_sha256", None)
    material = {
        "run_id": run_id,
        "execution_nonce_sha256": terminal_context.get("execution_nonce_sha256"),
        "command_returncode": returncode,
        "receipt_context": terminal_context,
    }
    return hashlib.sha256(
        json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def execute_guarded_request(request_path: Path) -> int:
    """Run identity, preflight, proof, and completion custody under one guard."""
    request = json.loads(request_path.read_text(encoding="utf-8"))
    if not isinstance(request, dict):
        raise ValueError("proof execution request must be an object")
    if request.get("schema") != EXECUTION_SCHEMA:
        raise ValueError("proof execution request schema mismatch")
    command = request.get("command")
    envelope = request.get("envelope")
    result_path = Path(str(request["result_path"]))
    cwd = Path(str(request["cwd"]))
    run_id = request.get("run_id")
    execution_nonce = request.get("execution_nonce")
    override_names = request.get("env_override_names", [])
    if not isinstance(command, list) or not isinstance(envelope, dict):
        raise ValueError("proof execution request has no typed command envelope")
    if not isinstance(run_id, str) or not run_id:
        raise ValueError("proof execution request has no run identity")
    if not isinstance(execution_nonce, str) or not re.fullmatch(
        r"[0-9a-f]{64}", execution_nonce
    ):
        raise ValueError("proof execution request has no canonical nonce")
    if not isinstance(override_names, list) or not all(
        isinstance(name, str) for name in override_names
    ):
        raise ValueError(
            "proof execution request has malformed environment override names"
        )
    command = [str(value) for value in command]
    validate_envelope(envelope, command)
    result: dict[str, object] = {
        "schema": EXECUTION_SCHEMA,
        "run_id": run_id,
        "execution_nonce": execution_nonce,
        "envelope": envelope,
        "phase": "identity",
        "command_started": False,
    }
    try:
        inherited_env = dict(os.environ)
        applied_cargo_policies: tuple[str, ...] = ()
        if "cargo" in envelope.get("toolchains", []):
            inherited_env, applied_cargo_policies = normalize_cargo_environment(
                inherited_env
            )
        canonical_env = dict(_CANONICAL_EXECUTION_ENV)
        if "node" in envelope.get("toolchains", []):
            canonical_env["NODE_OPTIONS"] = "--no-global-search-paths"
        inherited_env.update(canonical_env)
        execution_env, environment_contract = _deterministic_execution_environment(
            inherited_env,
            override_names=[
                *[str(name) for name in override_names],
                *sorted(canonical_env),
            ],
        )
        environment_fingerprint_key = secrets.token_bytes(32)
        exact = _exact_command(envelope, cwd=cwd, env=execution_env)
        payload_executable_pre = _payload_executable_identity(envelope, exact)
        effective_cwd, overlay_paths = _execution_source_paths(envelope, cwd=cwd)
        guarded_exec_pre, delegated_pre = _bind_delegated_command(
            envelope,
            exact,
            cwd=cwd,
            env=execution_env,
        )
        executable_pre = _executable_identity(Path(exact[0]))
        environment_executables_pre = _execution_environment_executable_identities(
            execution_env, cwd=cwd
        )
        overlay_pre = [_file_identity(path) for path in overlay_paths]
        pre_identities = [executable_pre, *overlay_pre]
        if payload_executable_pre is not None:
            pre_identities.append(payload_executable_pre)
        if guarded_exec_pre is not None:
            pre_identities.append(guarded_exec_pre)
        if delegated_pre is not None:
            pre_identities.append(delegated_pre)
        if not all(
            _content_identity_available(identity) for identity in pre_identities
        ):
            raise ValueError(
                "proof command or overlay input has unavailable content identity"
            )
        pre_source = _git_snapshot(effective_cwd, execution_env)
        plan = proof_plan.ProofPlan.load()
        proof_python, toolchains = _capture_toolchains(
            envelope,
            exact,
            cwd=cwd,
            env=execution_env,
            source_root=effective_cwd,
            hash_workers=plan.inventory_hash_workers,
        )
        for name, identity in toolchains.items():
            assert isinstance(identity, Mapping)
            _validate_toolchain_identity(plan, name, identity)
        python_version = "none"
        if proof_python is not None:
            match = re.match(r"(\d+\.\d+)", str(proof_python["version"]))
            if match is None:
                raise ValueError("proof Python identity has no major.minor version")
            python_version = match.group(1)
        context: dict[str, object] = {
            "schema": plan.receipt_schema,
            "authority_sha256": proof_plan._authority_sha256(plan),
            "run_id": run_id,
            "execution_nonce_sha256": hashlib.sha256(
                execution_nonce.encode()
            ).hexdigest(),
            "source_commit": pre_source.get("commit"),
            "source_tree": pre_source.get("tree"),
            "source_tree_state": "clean" if pre_source.get("clean") else "dirty",
            "environment": {
                "os": proof_plan._normalized_os(),
                "arch": proof_plan._normalized_arch(),
                "python": python_version,
            },
            "toolchains": toolchains,
            "toolchain_custody": {"prelaunch": toolchains},
            "command_envelope": envelope,
            "command_envelope_sha256": hashlib.sha256(
                json.dumps(envelope, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest(),
            "exact_command_sha256": hashlib.sha256(
                json.dumps(exact, separators=(",", ":")).encode()
            ).hexdigest(),
            "command_executable": {"prelaunch": executable_pre},
            "payload_command_executable": (
                {"prelaunch": payload_executable_pre}
                if payload_executable_pre is not None
                else None
            ),
            "guarded_exec": (
                {"prelaunch": guarded_exec_pre}
                if guarded_exec_pre is not None
                else None
            ),
            "delegated_command_executable": (
                {"prelaunch": delegated_pre} if delegated_pre is not None else None
            ),
            "execution_environment": {
                "prelaunch": _execution_environment_authority(
                    execution_env,
                    applied_cargo_policies=applied_cargo_policies,
                    fingerprint_key=environment_fingerprint_key,
                    contract=environment_contract,
                ),
                "executable_inputs": {"prelaunch": environment_executables_pre},
            },
            "python_interpreters": {
                "queue_control_plane": {
                    "executable": sys.executable,
                    "implementation": platform.python_implementation(),
                    "version": platform.python_version(),
                    "role": "queue-runner-and-memory-guard",
                },
                "proof_command": (
                    {**proof_python, "role": "proof-command-envelope"}
                    if proof_python is not None
                    else {"kind": "none", "role": "proof-command-envelope"}
                ),
            },
            "source_custody": {
                "row_cwd": str(cwd.resolve(strict=True)),
                "effective_cwd": str(effective_cwd),
                "prelaunch": pre_source,
                "overlay_inputs": {"prelaunch": overlay_pre},
            },
        }
        result.update(
            {
                "phase": "command",
                "receipt_context": context,
                "exact_command_sha256": context["exact_command_sha256"],
            }
        )
        _atomic_json(result_path, result)
        # Toolchain provisioning/contract checks are descendants of the same
        # queue guard and therefore appear in its resource and timeout summary.
        from tools.proof_queue_pkg import policy

        preflight = policy._ensure_run_toolchain_preflight(
            repo_root=cwd, resource_family=str(request["resource_family"])
        )
        if preflight:
            raise ValueError("toolchain preflight failed: " + "; ".join(preflight))
        result["command_started"] = True
        stdout_path = result_path.with_suffix(".stdout.bin")
        stderr_path = result_path.with_suffix(".stderr.bin")
        for transcript_path in (stdout_path, stderr_path):
            try:
                transcript_path.unlink()
            except FileNotFoundError:
                pass
        with (
            stdout_path.open("xb") as stdout_handle,
            stderr_path.open("xb") as stderr_handle,
        ):
            completed = subprocess.run(
                exact,
                cwd=cwd,
                env=execution_env,
                check=False,
                stdout=stdout_handle,
                stderr=stderr_handle,
            )
            stdout_handle.flush()
            stderr_handle.flush()
            os.fsync(stdout_handle.fileno())
            os.fsync(stderr_handle.fileno())
        _replay_transcript(stdout_path, sys.stdout)
        _replay_transcript(stderr_path, sys.stderr)
        result["command_returncode"] = int(completed.returncode)
        transcript = {
            "stdout": _transcript_identity(stdout_path),
            "stderr": _transcript_identity(stderr_path),
        }
        transcript["identity_sha256"] = hashlib.sha256(
            json.dumps(transcript, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        if int(completed.returncode) == 0 and _requires_structured_test_counts(
            envelope
        ):
            if not any(
                isinstance(value, Mapping)
                and value.get("structured_test_output") is True
                for key, value in transcript.items()
                if key in {"stdout", "stderr"}
            ):
                raise ValueError(
                    "successful test command produced no structured test-count authority"
                )
        context["command_transcript"] = transcript
        post_source = _git_snapshot(effective_cwd, execution_env)
        overlay_post = [_file_identity(path) for path in overlay_paths]
        executable_post = _executable_identity(Path(exact[0]))
        payload_executable_post = _payload_executable_identity(envelope, exact)
        environment_executables_post = _execution_environment_executable_identities(
            execution_env, cwd=cwd
        )
        guarded_exec_post = (
            _file_identity(Path(str(guarded_exec_pre["path"])))
            if guarded_exec_pre is not None
            else None
        )
        delegated_post = (
            _executable_identity(Path(str(delegated_pre["path"])))
            if delegated_pre is not None
            else None
        )
        proof_python_post, toolchains_post = _capture_toolchains(
            envelope,
            exact,
            cwd=cwd,
            env=execution_env,
            source_root=effective_cwd,
            hash_workers=plan.inventory_hash_workers,
        )
        for name, identity in toolchains_post.items():
            assert isinstance(identity, Mapping)
            _validate_toolchain_identity(plan, name, identity)
        environment_post = _execution_environment_authority(
            execution_env,
            applied_cargo_policies=applied_cargo_policies,
            fingerprint_key=environment_fingerprint_key,
            contract=environment_contract,
        )
        source_identical = pre_source == post_source
        executable_identical = executable_pre == executable_post
        payload_executable_identical = payload_executable_pre == payload_executable_post
        guarded_exec_identical = guarded_exec_pre == guarded_exec_post
        delegated_identical = delegated_pre == delegated_post
        toolchains_identical = _stable_toolchain_custody(
            toolchains
        ) == _stable_toolchain_custody(toolchains_post)
        environment_pre_container = context["execution_environment"]
        assert isinstance(environment_pre_container, dict)
        environment_pre = environment_pre_container["prelaunch"]
        environment_identical = environment_pre == environment_post
        environment_executables_identical = (
            environment_executables_pre == environment_executables_post
        )
        ineligible_reasons: list[str] = []
        if not pre_source.get("available") or not post_source.get("available"):
            ineligible_reasons.append("source-unavailable")
        if not pre_source.get("clean"):
            ineligible_reasons.append("source-dirty-prelaunch")
        if not post_source.get("clean"):
            ineligible_reasons.append("source-dirty-postcompletion")
        if not source_identical:
            ineligible_reasons.append("source-snapshot-changed")
        if not executable_identical:
            ineligible_reasons.append("command-executable-changed")
        if not _content_identity_available(executable_post):
            ineligible_reasons.append("command-executable-unavailable-postcompletion")
        if not payload_executable_identical:
            ineligible_reasons.append("payload-command-executable-changed")
        if not guarded_exec_identical:
            ineligible_reasons.append("guarded-exec-changed")
        if not delegated_identical:
            ineligible_reasons.append("delegated-command-executable-changed")
        if not toolchains_identical:
            ineligible_reasons.append("toolchain-or-python-distribution-changed")
        if not environment_identical:
            ineligible_reasons.append("execution-environment-changed")
        if not environment_executables_identical:
            ineligible_reasons.append("execution-environment-executable-changed")
        ineligible_reasons.extend(
            _python_editable_ineligible_reasons(
                proof_python,
                source_snapshot=pre_source,
            )
        )
        ineligible_reasons.extend(
            reason
            for reason in _python_editable_ineligible_reasons(
                proof_python_post,
                source_snapshot=post_source,
            )
            if reason not in ineligible_reasons
        )
        ineligible_reasons.extend(
            reason
            for reason in _python_editable_change_reasons(
                proof_python, proof_python_post
            )
            if reason not in ineligible_reasons
        )
        if overlay_pre != overlay_post:
            ineligible_reasons.append("overlay-input-changed")
        if not all(_content_identity_available(identity) for identity in overlay_post):
            ineligible_reasons.append("overlay-input-unavailable-postcompletion")
        eligible = not ineligible_reasons
        source_custody = context["source_custody"]
        assert isinstance(source_custody, dict)
        source_custody.update(
            {
                "postcompletion": post_source,
                "identical": source_identical,
                "evidence_eligible": eligible,
                "ineligible_reasons": ineligible_reasons,
            }
        )
        overlay_inputs = source_custody["overlay_inputs"]
        assert isinstance(overlay_inputs, dict)
        overlay_inputs.update(
            {
                "postcompletion": overlay_post,
                "identical": overlay_pre == overlay_post,
            }
        )
        command_executable = context["command_executable"]
        assert isinstance(command_executable, dict)
        command_executable.update(
            {
                "postcompletion": executable_post,
                "identical": executable_identical,
            }
        )
        if payload_executable_pre is not None:
            payload_executable = context["payload_command_executable"]
            assert isinstance(payload_executable, dict)
            payload_executable.update(
                {
                    "postcompletion": payload_executable_post,
                    "identical": payload_executable_identical,
                }
            )
        if guarded_exec_pre is not None:
            guarded_exec = context["guarded_exec"]
            assert isinstance(guarded_exec, dict)
            guarded_exec.update(
                {
                    "postcompletion": guarded_exec_post,
                    "identical": guarded_exec_identical,
                }
            )
        if delegated_pre is not None:
            delegated_executable = context["delegated_command_executable"]
            assert isinstance(delegated_executable, dict)
            delegated_executable.update(
                {"postcompletion": delegated_post, "identical": delegated_identical}
            )
        toolchain_custody = context["toolchain_custody"]
        assert isinstance(toolchain_custody, dict)
        toolchain_custody.update(
            {
                "postcompletion": toolchains_post,
                "identical": toolchains_identical,
            }
        )
        environment_pre_container.update(
            {
                "postcompletion": environment_post,
                "identical": environment_identical,
            }
        )
        executable_inputs = environment_pre_container["executable_inputs"]
        assert isinstance(executable_inputs, dict)
        executable_inputs.update(
            {
                "postcompletion": environment_executables_post,
                "identical": environment_executables_identical,
            }
        )
        context["execution_custody_sha256"] = execution_custody_sha256(
            context,
            run_id=run_id,
            returncode=int(completed.returncode),
        )
        result["phase"] = "complete"
        _atomic_json(result_path, result)
        return int(completed.returncode)
    except BaseException as exc:
        result.update(
            {
                "phase": "failed",
                "error": f"{type(exc).__name__}: {exc}",
            }
        )
        _atomic_json(result_path, result)
        print(
            f"proof command envelope failed: {type(exc).__name__}: {exc}",
            file=sys.stderr,
        )
        return 2


def _main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--request", required=True)
    args = parser.parse_args(argv)
    return execute_guarded_request(Path(args.request))


if __name__ == "__main__":
    raise SystemExit(_main())
