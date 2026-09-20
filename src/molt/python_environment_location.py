"""Path-only Python environment location before live custody is armed."""

from __future__ import annotations

import importlib.metadata as metadata
import json
import os
from pathlib import Path
import re
import sys
import sysconfig
from typing import cast
import urllib.parse

from molt.exact_json import ExactJsonError, canonical_json_sha256, loads_exact
from molt.python_identity_common import (
    PythonEnvironmentIdentityError,
    identity_validator,
)
from molt.python_native_locations import loaded_native_module_paths


PYTHON_ENVIRONMENT_LOCATION_SCHEMA = "molt.python-environment-location.v1"


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
        or payload.get("dir_info") != {"editable": True}
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


def _active_environment_prefix() -> Path:
    """Derive venv ownership without importing ``site`` or executing ``.pth``."""

    selected = Path(os.path.abspath(sys.executable))
    candidates = (selected.parent, selected.parent.parent)
    pyvenv_roots = [
        candidate
        for candidate in candidates
        if (candidate / "pyvenv.cfg").is_file()
        and not (candidate / "pyvenv.cfg").is_symlink()
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


def locate_current_python_environment() -> dict[str, object]:
    """Locate the active environment's broad roots without hashing them."""

    prefix = _active_environment_prefix()
    selected = Path(os.path.abspath(sys.executable))
    base = Path(getattr(sys, "_base_executable", None) or sys.executable).resolve(
        strict=True
    )
    if not selected.is_file() or not base.is_file():
        raise PythonEnvironmentIdentityError(
            "active Python environment has no executable launcher chain"
        )

    roots: dict[str, Path] = {}

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
        root = candidate if candidate.is_dir() else candidate.parent
        roots[os.path.normcase(str(root))] = root

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
        "schema": PYTHON_ENVIRONMENT_LOCATION_SCHEMA,
        "prefix": str(prefix),
        "selected_executable": str(selected),
        "base_executable": str(base),
        "roots": [str(path) for path in ordered_roots],
        "external_roots": [str(path) for path in ordered_external],
        # Path-only prearming does not close future loads. Content capture owns
        # a fresh fenced native census and optional-declaration receipt.
        "file_paths": [str(path) for path in loaded_native_module_paths()],
    }
    return {**material, "identity_sha256": canonical_json_sha256(material)}


@identity_validator("Python environment location")
def validate_python_environment_location(payload: object) -> dict[str, object]:
    """Validate one path-only pre-arm location receipt."""

    if not isinstance(payload, dict) or set(payload) != {
        "base_executable",
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
    if payload.get("schema") != PYTHON_ENVIRONMENT_LOCATION_SCHEMA or payload.get(
        "identity_sha256"
    ) != canonical_json_sha256(material):
        raise PythonEnvironmentIdentityError(
            "Python environment location digest is invalid"
        )
    prefix_raw = payload.get("prefix")
    selected_raw = payload.get("selected_executable")
    base_raw = payload.get("base_executable")
    roots = payload.get("roots")
    external = payload.get("external_roots")

    def canonical_path(
        raw: object, *, label: str, directory: bool, preserve_symlink: bool = False
    ) -> str:
        if not isinstance(raw, str) or not Path(raw).is_absolute():
            raise PythonEnvironmentIdentityError(
                f"Python environment location {label} is invalid"
            )
        lexical = Path(os.path.abspath(raw))
        if str(lexical) != raw:
            raise PythonEnvironmentIdentityError(
                f"Python environment location {label} is not canonical"
            )
        try:
            resolved = lexical.resolve(strict=True)
        except (OSError, ValueError) as exc:
            raise PythonEnvironmentIdentityError(
                f"Python environment location {label} is unavailable"
            ) from exc
        if (directory and not resolved.is_dir()) or (
            not directory and not lexical.is_file()
        ):
            raise PythonEnvironmentIdentityError(
                f"Python environment location {label} has the wrong file kind"
            )
        if not preserve_symlink and str(resolved) != raw:
            raise PythonEnvironmentIdentityError(
                f"Python environment location {label} is not resolved"
            )
        return raw

    if not isinstance(external, list):
        raise PythonEnvironmentIdentityError(
            "Python environment location has no executable prefix chain"
        )
    prefix_path = canonical_path(prefix_raw, label="prefix", directory=True)
    selected_path = canonical_path(
        selected_raw,
        label="selected executable",
        directory=False,
        preserve_symlink=True,
    )
    canonical_path(base_raw, label="base executable", directory=False)
    if not Path(selected_path).is_relative_to(Path(prefix_path)):
        raise PythonEnvironmentIdentityError(
            "Python environment selected executable escapes its prefix"
        )

    def validated_paths(
        raw: object, *, label: str, directory: bool = True
    ) -> list[str]:
        if not isinstance(raw, list) or not raw:
            raise PythonEnvironmentIdentityError(
                f"Python environment location {label} are invalid"
            )
        values = [
            canonical_path(value, label=label, directory=directory) for value in raw
        ]
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
    validated_paths(payload.get("file_paths"), label="native files", directory=False)
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
        Path(path) == Path(prefix_path) or Path(path).is_relative_to(Path(prefix_path))
        for path in external_paths
    ):
        raise PythonEnvironmentIdentityError(
            "Python environment external roots overlap its prefix"
        )
    return cast(dict[str, object], payload)
