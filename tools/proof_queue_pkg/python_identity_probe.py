"""Capture the complete selected-interpreter identity exactly once."""

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
import stat as stat_module
import subprocess
import sys
import sysconfig
import time
import urllib.parse
import urllib.request

if len(sys.argv) < 2:
    raise SystemExit("usage: python_identity_probe.py SOURCE_ROOT [HASH_WORKERS]")
sys.path[0] = str(pathlib.Path(sys.argv[1]).resolve(strict=True))

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
