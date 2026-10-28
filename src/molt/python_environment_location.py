"""Path-only Python environment location before live custody is armed."""

from __future__ import annotations

import importlib.metadata as metadata
from collections.abc import Mapping
import json
import os
from pathlib import Path
import re
import shutil
import stat
import sys
import sysconfig
from typing import cast
import urllib.parse

from molt import python_environment_contract as _contract
from molt.exact_json import ExactJsonError, canonical_json_sha256, loads_exact
from molt.python_identity_common import (
    PythonEnvironmentIdentityError,
    canonical_absolute_path,
    identity_validator,
)
from molt.python_native_locations import loaded_native_module_paths
from molt.toolchain_identity import stable_executable_probe
from molt.file_publication import metadata_is_link_like


_INSTALLATION_DIRECTORIES = frozenset(("lib", "lib64", "Lib", "bin", "Scripts"))
_WRITE_BITS = stat.S_IWUSR | stat.S_IWGRP | stat.S_IWOTH


def environment_installation_entry(relative: str) -> bool:
    """The installed namespace sealed by publication, on every host layout."""
    return relative.split("/", 1)[0] in _INSTALLATION_DIRECTORIES


def installed_access_entry(row: Mapping[str, object]) -> bool:
    """Return whether ``row`` reports the access of an installed entry."""
    installed = environment_installation_entry(str(row["path"])) and row["kind"] in {
        "directory",
        "file",
        "hardlink",
    }
    # Internal aliases report their target's access, not the symlink mode.
    return installed or (
        row.get("target_owner") == "same-root"
        and environment_installation_entry(str(row.get("target", "")))
    )


def attestation_records_writable_installation(
    manifest: Mapping[str, object],
) -> bool:
    """Return whether a legacy attestation still records writable installed entries.

    Only such an image needs the one-way seal before its capture. A sealed
    attestation skips the installed-tree walk; admission then refuses any
    write bit that reappeared, because a sealed entry cannot become writable.
    """
    image = manifest.get("realized_image", manifest.get("realized_environment"))
    if isinstance(image, Mapping) and "image" in image:
        image = cast(Mapping[str, object], image)["image"]
    if not isinstance(image, Mapping):
        return False  # Malformed attestations fail admission without a seal.
    tree = cast(Mapping[str, object], image).get("tree")
    rows = (
        cast(Mapping[str, object], tree).get("entries")
        if isinstance(tree, Mapping)
        else None
    )
    if not isinstance(rows, list):
        return False
    for row in cast(list[object], rows):
        if not isinstance(row, Mapping):
            continue
        entry = cast(Mapping[str, object], row)
        access = entry.get("access")
        if (
            isinstance(entry.get("path"), str)
            and "kind" in entry
            and isinstance(access, Mapping)
            and cast(Mapping[str, object], access).get("writable") is True
            and installed_access_entry(entry)
        ):
            return True
    return False


def _detach_shared_installation_file(root: Path, aliases: list[Path]) -> None:
    """Keep image-local alias topology without chmod of an external cache inode."""
    from molt.file_publication import resolve_owned_path, unlink_owned_file

    source = aliases[0]
    metadata = source.stat()
    staged = root / (".seal-" + os.urandom(16).hex())
    staged_identity = None
    try:
        with staged.open("xb") as output:
            staged_identity = os.fstat(output.fileno())
            with source.open("rb") as input_file:
                if not os.path.samestat(metadata, os.fstat(input_file.fileno())):
                    raise OSError(f"installation file changed before sealing: {source}")
                shutil.copyfileobj(input_file, output)
        shutil.copystat(source, staged, follow_symlinks=False)
        for alias in aliases:
            parent = resolve_owned_path(alias.parent)
            if not parent.is_relative_to(root) or not os.path.samestat(
                metadata, alias.lstat()
            ):
                raise OSError(f"installation alias changed before sealing: {alias}")
            mode = stat.S_IMODE(parent.stat().st_mode)
            replacement = root / (".seal-" + os.urandom(16).hex())
            replacement_identity = None
            try:
                replacement.hardlink_to(staged)
                replacement_identity = replacement.lstat()
                # A legacy image may already have sealed lib directories.
                # Restore that exact mode after the namespace replacement.
                if not mode & stat.S_IWUSR:
                    parent.chmod(mode | stat.S_IWUSR)
                os.replace(replacement, alias)
            finally:
                if not mode & stat.S_IWUSR:
                    parent.chmod(mode)
                unlink_owned_file(replacement, replacement_identity)
    finally:
        unlink_owned_file(staged, staged_identity)


def seal_environment_installation(root: Path) -> None:
    """Remove write bits from installed files/directories, never link targets.

    Publication calls this before capture. Existing-image admission calls it
    before opening its capture fence; only the exact one-way access transition
    is admitted against the original attestation. Records, locks and bytecode
    retain their existing writable namespaces. This is a mode seal, not an ACL
    or privileged-user sandbox.
    """
    from molt.file_publication import resolve_owned_path

    root = resolve_owned_path(root)
    entries: list[tuple[Path, os.stat_result]] = []

    def scan_error(error: OSError) -> None:
        raise error

    for name in sorted(_INSTALLATION_DIRECTORIES):
        directory = root / name
        try:
            metadata = directory.lstat()
        except FileNotFoundError:
            continue
        if metadata_is_link_like(metadata):
            continue  # e.g. lib64 -> lib, or bin/python -> external runtime
        if not stat.S_ISDIR(metadata.st_mode):
            raise PythonEnvironmentIdentityError(
                f"installation namespace is not a directory: {directory}"
            )
        for current, directories, files in os.walk(
            directory, followlinks=False, onerror=scan_error
        ):
            directories[:] = [
                child
                for child in directories
                if not metadata_is_link_like((Path(current) / child).lstat())
            ]
            entries.append((Path(current), Path(current).lstat()))
            for child in files:
                path = Path(current) / child
                info = path.lstat()
                if not metadata_is_link_like(info):
                    entries.append((path, info))

    shared = {
        (metadata.st_dev, metadata.st_ino): metadata
        for _path, metadata in entries
        if stat.S_ISREG(metadata.st_mode)
        and metadata.st_mode & _WRITE_BITS
        and metadata.st_nlink > 1
    }
    if shared:
        aliases: dict[tuple[int, int], list[Path]] = {key: [] for key in shared}
        for current, directories, files in os.walk(
            root, followlinks=False, onerror=scan_error
        ):
            directories[:] = [
                child
                for child in directories
                if not metadata_is_link_like((Path(current) / child).lstat())
            ]
            for name in files:
                path = Path(current) / name
                metadata = path.lstat()
                key = (metadata.st_dev, metadata.st_ino)
                if stat.S_ISREG(metadata.st_mode) and key in aliases:
                    aliases[key].append(path)
        for key, paths in aliases.items():
            if len(paths) < shared[key].st_nlink:
                _detach_shared_installation_file(root, paths)

    # Children before parents keeps a partially sealed publication recoverable.
    for path, _metadata in reversed(entries):
        metadata = path.lstat()
        if metadata_is_link_like(metadata):
            raise PythonEnvironmentIdentityError(
                f"installation entry became indirect while sealing: {path}"
            )
        if not (stat.S_ISDIR(metadata.st_mode) or stat.S_ISREG(metadata.st_mode)):
            raise PythonEnvironmentIdentityError(
                f"installation entry is not a file or directory: {path}"
            )
        if metadata.st_mode & _WRITE_BITS:
            mode = stat.S_IMODE(metadata.st_mode) & ~_WRITE_BITS
            if os.name == "posix":
                descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
                try:
                    if not os.path.samestat(metadata, os.fstat(descriptor)):
                        raise OSError(
                            f"installation entry changed while sealing: {path}"
                        )
                    os.fchmod(descriptor, mode)
                finally:
                    os.close(descriptor)
            else:
                path.chmod(mode)


def environment_bytecode_prefix(root: Path) -> Path:
    """Project an image's cache location; this does not admit its contents."""
    return root.absolute() / _contract.ENVIRONMENT_BYTECODE_DIRECTORY


def validate_managed_bytecode_prefix(root: Path) -> Path:
    """Validate the physical source-only read namespace at selection or capture."""
    prefix = environment_bytecode_prefix(root)
    if (
        not prefix.is_dir()
        or prefix.is_symlink()
        or metadata_is_link_like(prefix.lstat())
        or prefix.resolve(strict=True) != prefix
        or any(prefix.iterdir())
    ):
        raise PythonEnvironmentIdentityError(
            "managed source-only image requires a physical empty bytecode prefix"
        )
    return prefix


def environment_bytecode_arguments(root: Path) -> tuple[str, ...]:
    """Select the image's read namespace before CPython/site startup."""
    return ("-B", "-X", f"pycache_prefix={environment_bytecode_prefix(root)}")


def environment_provisioning_record_path(root: Path) -> Path:
    """The existing publisher's sibling record, outside the immutable image."""
    return root.parent / ".provisioning" / f"{root.name}.json"


def environment_python_path(root: Path) -> Path:
    """The host interpreter entrypoint of a standard Python environment.

    The environment entrypoint preserves its prefix and installed packages;
    resolving its symlink would select the base interpreter instead.
    """
    return root / ("Scripts/python.exe" if os.name == "nt" else "bin/python")


def editable_direct_url_path(data: bytes, *, distribution: str) -> Path:
    """Decode one strict local editable PEP 610 source authority."""

    try:
        payload = loads_exact(data.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError, ExactJsonError) as exc:
        raise PythonEnvironmentIdentityError(
            f"installed distribution {distribution!r} has invalid direct_url.json"
        ) from exc
    if (
        not isinstance(payload, dict)
        or set(payload) != {"url", "dir_info"}
        or not isinstance(payload.get("url"), str)
        or not isinstance(payload.get("dir_info"), dict)
        or set(payload["dir_info"]) != {"editable"}
        or payload["dir_info"]["editable"] is not True
    ):
        raise PythonEnvironmentIdentityError(
            f"installed distribution {distribution!r} is not an admitted editable source"
        )
    parsed = urllib.parse.urlparse(str(payload["url"]))
    if (
        parsed.scheme != "file"
        or parsed.netloc.casefold() not in {"", "localhost"}
        or parsed.params
        or parsed.query
        or parsed.fragment
        or not parsed.path
    ):
        raise PythonEnvironmentIdentityError(
            f"installed distribution {distribution!r} has a non-local editable source"
        )
    encoded_path = parsed.path
    percent_index = 0
    while True:
        percent_index = encoded_path.find("%", percent_index)
        if percent_index < 0:
            break
        if (
            re.fullmatch(
                r"[0-9A-Fa-f]{2}", encoded_path[percent_index + 1 : percent_index + 3]
            )
            is None
        ):
            raise PythonEnvironmentIdentityError(
                f"installed distribution {distribution!r} has an invalid file URL"
            )
        percent_index += 3
    # Decode only the already-admitted local path, not the URI authority again.
    # Keep filesystem-byte semantics stable across supported Python versions.
    try:
        raw = os.fsdecode(urllib.parse.unquote_to_bytes(encoded_path))
    except (ValueError, OSError) as exc:
        raise PythonEnvironmentIdentityError(
            f"installed distribution {distribution!r} has an invalid file URL"
        ) from exc
    if re.search(r"%[0-9A-Fa-f]{2}", raw):
        raise PythonEnvironmentIdentityError(
            f"installed distribution {distribution!r} has a multiply encoded file URL"
        )
    if os.name == "nt":
        # RFC 8089 Appendix E drive spellings apply only to Windows. POSIX
        # colons and bars are ordinary filename bytes, never DOS syntax.
        drive = re.match(r"^/?([A-Za-z])[:|](?=/|$)", raw)
        if drive is not None:
            raw = drive[1].upper() + ":" + raw[drive.end() :]
    if raw.startswith(("//", "\\\\")):
        raise PythonEnvironmentIdentityError(
            f"installed distribution {distribution!r} has a non-local editable source"
        )
    lexical = Path(raw)
    if not lexical.is_absolute():
        raise PythonEnvironmentIdentityError(
            f"installed distribution {distribution!r} has a non-absolute editable source"
        )
    try:
        root = lexical.resolve(strict=True)
    except (OSError, ValueError) as exc:
        raise PythonEnvironmentIdentityError(
            f"installed distribution {distribution!r} editable source is unavailable"
        ) from exc
    if not lexical.is_dir() or lexical.is_symlink() or lexical.is_junction():
        raise PythonEnvironmentIdentityError(
            f"installed distribution {distribution!r} editable source is not a real directory"
        )
    if root != lexical:
        raise PythonEnvironmentIdentityError(
            f"installed distribution {distribution!r} editable source uses path indirection"
        )
    return root


def _pyvenv_config_candidates(selected: Path) -> tuple[Path, Path]:
    return selected.parent / "pyvenv.cfg", selected.parent.parent / "pyvenv.cfg"


def python_startup_configuration_paths(
    selected: Path, base: Path, runtime_library: Path | None
) -> tuple[Path, ...]:
    """Selection files only; CPython remains the configuration parser.

    Include absent candidates: a new pyvenv.cfg or ._pth changes the next
    interpreter even when every currently admitted runtime file is unchanged.
    pybuilddir.txt covers CPython's in-tree executable path selection.
    """
    executables = (selected.absolute(), base.absolute())
    candidates = {
        path
        for executable in executables
        for path in (
            *_pyvenv_config_candidates(executable),
            executable.with_suffix("._pth"),
            executable.parent / "pybuilddir.txt",
        )
    }
    if runtime_library is not None:
        candidates.add(runtime_library.with_suffix("._pth"))
    return tuple(sorted(candidates))


def _active_environment_prefix() -> Path:
    """Derive venv ownership without importing ``site`` or executing ``.pth``."""

    selected = Path(os.path.abspath(sys.executable))
    candidates = _pyvenv_config_candidates(selected)
    pyvenv_roots = [
        candidate.parent
        for candidate in candidates
        if candidate.is_file() and not candidate.is_symlink()
    ]
    if len(pyvenv_roots) > 1:
        raise PythonEnvironmentIdentityError(
            "active Python launcher has ambiguous pyvenv.cfg ownership"
        )
    raw = pyvenv_roots[0] if pyvenv_roots else Path(sys.prefix)
    resolved = raw.resolve(strict=True)
    if not resolved.is_dir():
        raise PythonEnvironmentIdentityError(
            f"active Python environment prefix is not a directory: {resolved}"
        )
    return resolved


def _environment_sysconfig_path(name: str, root: Path) -> str | None:
    variables = {
        "base": str(root),
        "platbase": str(root),
        "installed_base": str(root),
        "installed_platbase": str(root),
    }
    return sysconfig.get_path(name, vars=variables)


def _site_roots(root: Path) -> tuple[Path, ...]:
    paths: list[Path] = []
    seen: set[str] = set()
    for scheme in ("purelib", "platlib"):
        raw = _environment_sysconfig_path(scheme, root)
        if not raw:
            raise PythonEnvironmentIdentityError(f"environment has no {scheme} path")
        lexical = Path(os.path.abspath(raw))
        try:
            lexical.relative_to(root)
        except ValueError as exc:
            raise PythonEnvironmentIdentityError(
                f"{scheme} root escapes environment root: {lexical}"
            ) from exc
        resolved = lexical.resolve(strict=True)
        if not resolved.is_dir() or lexical.is_symlink() or lexical.is_junction():
            raise PythonEnvironmentIdentityError(
                f"environment {scheme} is not a real directory: {lexical}"
            )
        key = os.path.normcase(str(resolved))
        if key not in seen:
            seen.add(key)
            paths.append(resolved)
    return tuple(paths)


def locate_editable_project(root: Path, project: str) -> str | None:
    """Find one project's PEP 610 record without importing installed packages.

    Run under the selected interpreter with site startup disabled: its sysconfig
    owns platform/version site paths, while the caller supplies the venv prefix.
    The returned path is relative to that prefix and is never a package import.
    """
    from molt.python_identity_common import _canonicalize_name

    root = root.resolve(strict=True)
    matches = [
        dist
        for dist in metadata.distributions(path=[str(p) for p in _site_roots(root)])
        if _canonicalize_name(str(dist.metadata.get("Name", "")))
        == _canonicalize_name(project)
    ]
    if len(matches) > 1:
        raise PythonEnvironmentIdentityError(
            f"multiple installed distributions for {project!r}"
        )
    if not matches:
        return None
    dist = matches[0]
    text = dist.read_text("direct_url.json")
    if text is None:
        return None  # A noneditable/manual environment retains its existing contract.
    try:
        payload = loads_exact(text)
    except (json.JSONDecodeError, ExactJsonError) as exc:
        raise PythonEnvironmentIdentityError(
            f"invalid direct_url.json for {project!r}"
        ) from exc
    if not isinstance(payload, dict):
        raise PythonEnvironmentIdentityError(f"invalid direct_url.json for {project!r}")
    if "dir_info" not in payload:
        return None  # Archive/VCS installs have no editable directory authority.
    directory = payload["dir_info"]
    if isinstance(directory, dict) and (
        "editable" not in directory or directory["editable"] is False
    ):
        return None  # PEP 610: absent or boolean false means noneditable.
    # All other directory markers must satisfy the existing strict editable
    # decoder. Unsupported fields and malformed markers cannot become manual.
    editable_direct_url_path(text.encode("utf-8"), distribution=project)
    candidates = [
        Path(str(dist.locate_file(entry))).resolve(strict=True)
        for entry in dist.files or ()
        if entry.name == "direct_url.json"
    ]
    if len(candidates) != 1:
        raise PythonEnvironmentIdentityError(
            f"editable project {project!r} has no unique recorded direct_url.json"
        )
    return candidates[0].relative_to(root).as_posix()


def locate_current_python_environment() -> dict[str, object]:
    """Locate runtime roots while binding the lexical base selector's image."""

    selector = Path(getattr(sys, "_base_executable", None) or sys.executable).absolute()
    canonical_absolute_path(str(selector))
    # CPython exposes the lexical startup selector separately from its image.
    # uv's Python trampoline intentionally preserves that spelling, including
    # managed-Python directory junctions and venv selection semantics.
    # https://github.com/astral-sh/uv/blob/main/crates/uv-trampoline/src/bounce.rs
    with stable_executable_probe(selector, label="Python base selector") as (
        _entrypoint,
        content,
    ):
        return _locate_current_python_environment(selector, content.path)


def _locate_current_python_environment(selector: Path, base: Path) -> dict[str, object]:
    prefix = _active_environment_prefix()
    selected = Path(os.path.abspath(sys.executable))
    if not selected.is_file() or not base.is_file():
        raise PythonEnvironmentIdentityError(
            "active Python environment has no executable launcher chain"
        )

    roots: dict[str, Path] = {}
    exact_files: dict[str, Path] = {}

    def add_root(raw: object) -> None:
        if not isinstance(raw, (str, os.PathLike)):
            return
        raw_path = os.fspath(raw)
        if not isinstance(raw_path, str) or not raw_path:
            return
        try:
            candidate = Path(raw_path).resolve(strict=True)
        except OSError:
            return
        if candidate.is_dir():
            roots[os.path.normcase(str(candidate))] = candidate
        elif candidate.is_file():
            exact_files[os.path.normcase(str(candidate))] = candidate

    site_roots = _site_roots(prefix)
    for raw in (
        prefix,
        sys.base_prefix,
        sys.exec_prefix,
        sys.base_exec_prefix,
        *sys.path,
        *site_roots,
        sysconfig.get_path("stdlib"),
        sysconfig.get_path("platstdlib"),
    ):
        add_root(raw)

    external: dict[str, Path] = {}
    for distribution in metadata.distributions(path=[str(path) for path in site_roots]):
        try:
            text = distribution.read_text("direct_url.json")
        except UnicodeError as exc:
            name = str(distribution.metadata.get("Name") or "<unnamed>")
            raise PythonEnvironmentIdentityError(
                f"installed distribution {name!r} has invalid direct_url.json"
            ) from exc
        if text is None:
            continue
        name = str(distribution.metadata.get("Name") or "<unnamed>")
        root = editable_direct_url_path(text.encode("utf-8"), distribution=name)
        external[os.path.normcase(str(root))] = root
        add_root(root)
    ordered_roots = sorted(
        roots.values(), key=lambda path: (os.path.normcase(str(path)), str(path))
    )
    ordered_external = sorted(
        external.values(), key=lambda path: (os.path.normcase(str(path)), str(path))
    )
    material: dict[str, object] = {
        "schema": _contract.PYTHON_ENVIRONMENT_LOCATION_SCHEMA,
        "prefix": str(prefix),
        "selected_executable": str(selected),
        "base_executable": str(base),
        "base_executable_selector": str(selector),
        "roots": [str(path) for path in ordered_roots],
        "external_roots": [str(path) for path in ordered_external],
        # Import archives and native images are finite inputs, not parent
        # trees. Content capture still owns a fresh fenced native census and
        # optional-declaration receipt; prearming does not close future loads.
        "file_paths": sorted(
            {str(path) for path in exact_files.values()}
            | {str(path) for path in loaded_native_module_paths()},
            key=lambda value: (os.path.normcase(value), value),
        ),
    }
    return {**material, "identity_sha256": canonical_json_sha256(material)}


@identity_validator("Python environment location")
def validate_python_environment_location(payload: object) -> dict[str, object]:
    """Validate a path-only receipt without reading host files.

    The producer proves physical locations and the selector's target under
    executable generation custody. Pure admission preserves those declared
    coordinates; it must never reinterpret an arbitrary launcher target.
    """

    if not isinstance(payload, dict) or set(payload) != {
        "base_executable",
        "base_executable_selector",
        "external_roots",
        "file_paths",
        "identity_sha256",
        "prefix",
        "roots",
        "schema",
        "selected_executable",
    }:
        raise PythonEnvironmentIdentityError(
            "Python environment location shape is invalid"
        )
    material = {
        key: value for key, value in payload.items() if key != "identity_sha256"
    }
    if payload.get(
        "schema"
    ) != _contract.PYTHON_ENVIRONMENT_LOCATION_SCHEMA or payload.get(
        "identity_sha256"
    ) != canonical_json_sha256(material):
        raise PythonEnvironmentIdentityError(
            "Python environment location digest is invalid"
        )
    prefix_raw = payload.get("prefix")
    selected_raw = payload.get("selected_executable")
    base_raw = payload.get("base_executable")
    selector_raw = payload.get("base_executable_selector")
    roots = payload.get("roots")
    external = payload.get("external_roots")

    def canonical_path(raw: object) -> str:
        canonical_absolute_path(raw)
        return cast(str, raw)

    if not isinstance(external, list):
        raise PythonEnvironmentIdentityError(
            "Python environment location has no executable prefix chain"
        )
    prefix_path = canonical_path(prefix_raw)
    selected_path = canonical_path(selected_raw)
    canonical_path(base_raw)
    canonical_path(selector_raw)
    if not canonical_absolute_path(selected_path).is_relative_to(
        canonical_absolute_path(prefix_path)
    ):
        raise PythonEnvironmentIdentityError(
            "Python environment selected executable escapes its prefix"
        )

    def validated_paths(raw: object, *, label: str) -> list[str]:
        if not isinstance(raw, list) or not raw:
            raise PythonEnvironmentIdentityError(
                f"Python environment location {label} are invalid"
            )
        values = [canonical_path(value) for value in raw]
        expected = sorted(
            set(values), key=lambda value: (os.path.normcase(value), value)
        )
        if values != expected or len(
            {os.path.normcase(value) for value in values}
        ) != len(values):
            raise PythonEnvironmentIdentityError(
                f"Python environment location {label} are not canonical"
            )
        return values

    root_paths = validated_paths(roots, label="roots")
    validated_paths(payload.get("file_paths"), label="exact files")
    external_paths = (
        validated_paths(external, label="external roots") if external else []
    )
    if prefix_path not in root_paths:
        raise PythonEnvironmentIdentityError(
            "Python environment prefix escapes broad custody"
        )
    if not set(external_paths).issubset(root_paths):
        raise PythonEnvironmentIdentityError(
            "Python environment external roots escape broad custody"
        )
    if any(
        canonical_absolute_path(path).is_relative_to(
            canonical_absolute_path(prefix_path)
        )
        for path in external_paths
    ):
        raise PythonEnvironmentIdentityError(
            "Python environment external roots overlap its prefix"
        )
    return cast(dict[str, object], payload)
