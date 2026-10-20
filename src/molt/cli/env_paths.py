from __future__ import annotations

from molt.process_environment import ProcessEnvironmentBuilder

import functools
import os
from pathlib import Path

from molt.cli.deps import _molt_venv_path
from molt.cli.python_import_topology import PythonImportTopology
from molt.source_root import MOLT_SOURCE_ROOT_ENV


def _vendor_roots(
    project_root: Path, *, import_topology: PythonImportTopology | None = None
) -> list[Path]:
    topology = import_topology or PythonImportTopology()
    vendor_root = project_root / "vendor"
    roots: list[Path] = []
    for name in ("packages", "local"):
        candidate = vendor_root / name
        if topology.probe(candidate) is not None:
            roots.append(candidate)
    return roots


def _venv_site_packages(
    venv: Path, *, import_topology: PythonImportTopology | None = None
) -> list[Path]:
    topology = import_topology or PythonImportTopology()
    if not topology.is_dir(venv):
        return []
    results: list[Path] = []
    library = venv / "lib"
    if topology.is_dir(library):
        for name in sorted(topology.directory_names(library)):
            if Path(name).match("python*"):
                sp = library / name / "site-packages"
                if topology.is_dir(sp):
                    results.append(sp)
    win_sp = venv / "Lib" / "site-packages"
    if topology.is_dir(win_sp) and win_sp not in results:
        results.append(win_sp)
    return results


def _molt_venv_site_packages(
    project_root: Path, *, import_topology: PythonImportTopology | None = None
) -> list[Path]:
    return _venv_site_packages(
        _molt_venv_path(project_root), import_topology=import_topology
    )


def _base_env(
    root: Path,
    script_path: Path | None = None,
    *,
    molt_root: Path | None = None,
) -> ProcessEnvironmentBuilder:
    env = ProcessEnvironmentBuilder(os.environ)
    paths = [env.get("PYTHONPATH", "")]
    if script_path is not None:
        paths.append(str(script_path.parent))
    roots: list[Path] = []
    if molt_root is not None and molt_root != root:
        roots.append(molt_root)
    roots.append(root)
    for base in roots:
        paths.extend([str(base / "src"), str(base)])
        paths.extend(str(path) for path in _vendor_roots(base))
        paths.extend(str(sp) for sp in _molt_venv_site_packages(base))
    env["PYTHONPATH"] = os.pathsep.join(p for p in paths if p)
    env.setdefault("PYTHONHASHSEED", "0")
    if molt_root is not None:
        env.setdefault(MOLT_SOURCE_ROOT_ENV, str(molt_root))
    return env


@functools.lru_cache(maxsize=128)
def _resolve_env_path_cached(var: str, raw: str, default_str: str) -> Path:
    if raw:
        path = Path(raw).expanduser()
        if path.is_absolute():
            return path
        return Path.cwd() / path
    return Path(default_str)


def _resolve_env_path(var: str, default: Path) -> Path:
    return _resolve_env_path_cached(
        var,
        os.environ.get(var, ""),
        os.fspath(default),
    )
