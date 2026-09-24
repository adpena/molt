"""Exact active editable-import trees and a closed declarative finder subset.

Only absolute directory entries in owned UTF-8 .pth files are declarative.
Executable .pth lines, custom finders and editable mapping hooks are rejected,
except for the exact reviewed upstream startup capabilities below. No Git
status, ignore policy, package-name heuristic or source overlay grants custody.
"""

from __future__ import annotations

import _thread
import ast
from collections.abc import Mapping, Sequence
import hashlib
import importlib.machinery as machinery
import io
import os
from pathlib import Path, PurePath, PurePosixPath
import sys
from types import CodeType, FunctionType, ModuleType
from typing import cast
import unicodedata
import zipimport

from molt.exact_json import canonical_json_sha256
from molt.python_file_node_custody import (
    PythonFileCaptureContext,
    _FileNodePool,
    _is_file_entry,
    _root_forest,
    _stable_tree_inventory,
    _validate_file_nodes,
    _validate_inventory_entries,
    resolve_native_tree_path,
)
from molt.python_identity_common import (
    PythonEnvironmentIdentityError,
    canonical_absolute_path,
    _valid_relative_payload_path,
)

PYTHON_EXTERNAL_IMPORT_SCHEMA = "molt.python-external-import-custody.v2"
PYTHON_IMPORT_FINDER_POLICY = (
    "cpython-standard-finders-reviewed-startup-absolute-pth.v2"
)

# Reviewed upstream artifact, not a digest minted from the local environment.
# https://github.com/astral-sh/uv/blob/0.11.24/crates/uv-virtualenv/src/_virtualenv.py
# The GitHub tag tree and immutable blob were independently fetched; their bytes
# matched the uv=0.11.24 pyvenv.cfg environment. New templates require review.
_UV_BOOTSTRAP_PROVENANCE = {
    "project": "astral-sh/uv",
    "version": "0.11.24",
    "path": "crates/uv-virtualenv/src/_virtualenv.py",
    "git_blob_sha1": "c4af24d43878f22da47efa43a3ef7899c6a7c8aa",
    "sha256": "cfb3db86aaa53bb62b5ff764970bec2d71c9228590a0ebec57f6ec926cc0bf1a",
    "size": 5246,
}

# These wheel artifacts are pinned in uv.lock. Their hashes and these member
# hashes were independently verified against the immutable PyPI downloads.
# Coverage's reviewed wheels differ only in the final newline of the .pth file;
# both exact variants are recorded, never inferred from the installed name.
_COVERAGE_RELEASE = {"project": "nedbat/coveragepy", "version": "7.16.1"}
_SETUPTOOLS_WHEEL = {
    "project": "pypa/setuptools",
    "version": "83.0.0",
    "url": "https://files.pythonhosted.org/packages/5d/40/e1e72872c6354b306daef1703549e8e83b4d43cfea356311bf722a043752/setuptools-83.0.0-py3-none-any.whl",
    "artifact_sha256": "29b23c360f22f414dc7336bb39178cc7bcbf6021ed2733cde173f09dba19abb3",
}
_STARTUP_CAPABILITIES: dict[str, dict[str, object]] = {
    "uv-virtualenv.v1": {
        "declaration": "_virtualenv.pth",
        "distribution": None,
        "version": None,
        "declaration_artifacts": [],
        "module": "_virtualenv",
        "module_path": "_virtualenv.py",
        "module_artifact": _UV_BOOTSTRAP_PROVENANCE,
        "finder": "_Finder",
        "gate": {},
    },
    "coverage-inactive.v1": {
        "declaration": "a1_coverage.pth",
        "distribution": "coverage",
        "version": _COVERAGE_RELEASE["version"],
        "declaration_artifacts": [
            {
                **_COVERAGE_RELEASE,
                "url": "https://files.pythonhosted.org/packages/96/1a/d6d16babd0a5fe4c3fae40702158c570351694e74516d8d81b86c5637448/coverage-7.16.1-py3-none-any.whl",
                "artifact_sha256": "3d8bd4e58b6a5c2018d808f297905393c6c61da466a48c3f0596a76a4900ebe4",
                "path": "a1_coverage.pth",
                "sha256": "ef2ed06d19867ec669c09a804060666a9cd5e383af0a9d11aa2de79b77d448e8",
                "size": 205,
            },
            {
                **_COVERAGE_RELEASE,
                "url": "https://files.pythonhosted.org/packages/73/27/ec3d032375735dd331477caa051419678079ff90fcb53d0284a6c2bfb757/coverage-7.16.1-cp312-cp312-win_amd64.whl",
                "artifact_sha256": "d0f02c633630e2b74522108ee95a84ad6e1204a8016a6cca5297f335ea27147e",
                "path": "a1_coverage.pth",
                "sha256": "f1498191b7f52180654ccdb6195233612805e26344100c093058343ea04afd36",
                "size": 206,
            },
        ],
        "module": None,
        "module_path": None,
        "module_artifact": None,
        "finder": None,
        "gate": {"COVERAGE_PROCESS_START": False, "COVERAGE_PROCESS_CONFIG": False},
    },
    "setuptools-local-distutils.v1": {
        "declaration": "distutils-precedence.pth",
        "distribution": "setuptools",
        "version": _SETUPTOOLS_WHEEL["version"],
        "declaration_artifacts": [
            {
                **_SETUPTOOLS_WHEEL,
                "path": "distutils-precedence.pth",
                "sha256": "2638ce9e2500e572a5e0de7faed6661eb569d1b696fcba07b0dd223da5f5d224",
                "size": 151,
            }
        ],
        "module": "_distutils_hack",
        "module_path": "_distutils_hack/__init__.py",
        "module_artifact": {
            **_SETUPTOOLS_WHEEL,
            "path": "_distutils_hack/__init__.py",
            "sha256": "df81e6bcba34ee3e3952f776551fb669143b9490fdd6c4caeb32609f97e985b4",
            "size": 6755,
        },
        "finder": "DistutilsMetaFinder",
        "gate": {"SETUPTOOLS_USE_DISTUTILS": "local", "cwd_pybuilddir_file": False},
    },
}


def empty_external_import_custody() -> dict[str, object]:
    return {
        "schema": PYTHON_EXTERNAL_IMPORT_SCHEMA,
        "finder_policy": PYTHON_IMPORT_FINDER_POLICY,
        "trees": [],
        "path_declarations": [],
        "reviewed_startup": [],
        "finder_order": [],
    }


def _region_path(value: object) -> PurePosixPath:
    if value != "." and not _valid_relative_payload_path(value):
        raise PythonEnvironmentIdentityError("external import region path is invalid")
    return PurePosixPath(cast(str, value))


def _active_regions(
    active_import_roots: Sequence[Mapping[str, object]],
) -> dict[str, tuple[str, PurePosixPath]]:
    return {
        str(row["role"]): (str(row["root"]), _region_path(row["path"]))
        for row in active_import_roots
        if row.get("owner") == "external"
    }


def _external_forest(
    regions: Mapping[str, tuple[str, PurePosixPath]],
) -> tuple[list[tuple[str, PurePosixPath]], list[dict[str, object]]]:
    roots, references = _root_forest(
        [(role, PurePosixPath(root) / path) for role, (root, path) in regions.items()],
        root_prefix="external-tree",
    )
    return [
        (path.parts[0], PurePosixPath(*path.parts[1:])) for _root, path in roots
    ], references


def _pth_lines(content: str, *, virtualenv: bool) -> list[PurePath]:
    paths: list[PurePath] = []
    executable_lines = 0
    for raw in io.StringIO(content, newline=None):
        line = raw.rstrip()
        if not line or line.startswith("#"):
            continue
        if line.startswith(("import ", "import\t")):
            if not virtualenv or line != "import _virtualenv":
                raise PythonEnvironmentIdentityError(
                    f"unsupported executable .pth directive: {line!r}; "
                    "only owned absolute directory declarations and the reviewed uv bootstrap are supported"
                )
            executable_lines += 1
            continue
        paths.append(canonical_absolute_path(line))
    if virtualenv and (executable_lines != 1 or paths):
        raise PythonEnvironmentIdentityError(
            "uv bootstrap .pth must only import _virtualenv once"
        )
    if len(paths) != len(set(paths)):
        raise PythonEnvironmentIdentityError(
            ".pth repeats an external import directory"
        )
    return paths


def _expected_declaration_paths(
    tree_entries: Mapping[str, Mapping[str, object]], site_roots: Sequence[str]
) -> list[str]:
    sites = set(site_roots)
    return sorted(
        (
            path
            for path, row in tree_entries.items()
            if PurePosixPath(path).parent.as_posix() in sites
            and PurePosixPath(path).suffix == ".pth"
            and not PurePosixPath(path).name.startswith(".")
            and _is_file_entry(row)
        ),
        key=lambda path: (path.casefold(), path),
    )


def _filefinder_hook_matches(hook: object) -> bool:
    expected = machinery.FileFinder.path_hook(
        (machinery.ExtensionFileLoader, machinery.EXTENSION_SUFFIXES),
        (machinery.SourceFileLoader, machinery.SOURCE_SUFFIXES),
        (machinery.SourcelessFileLoader, machinery.BYTECODE_SUFFIXES),
    )
    return (
        type(hook) is FunctionType
        and type(expected) is FunctionType
        and hook.__code__ is expected.__code__
        and hook.__closure__ is not None
        and expected.__closure__ is not None
        and tuple(cell.cell_contents for cell in hook.__closure__)
        == tuple(cell.cell_contents for cell in expected.__closure__)
    )


def _code_child(code: CodeType, name: str) -> CodeType:
    matches = [
        value
        for value in code.co_consts
        if isinstance(value, CodeType) and value.co_name == name
    ]
    if len(matches) != 1:
        raise PythonEnvironmentIdentityError(f"reviewed startup code is missing {name}")
    return matches[0]


def _require_artifact(
    source: bytes, artifact: Mapping[str, object], *, label: str
) -> None:
    observed = hashlib.sha256(source).hexdigest()
    if observed != artifact["sha256"] or len(source) != artifact["size"]:
        raise PythonEnvironmentIdentityError(
            f"unsupported {label} template sha256={observed}; expected "
            f"{artifact['sha256']} from {artifact['project']} {artifact['version']}; "
            "new upstream templates require review, not fallback admission"
        )


def _observe_startup_gate(capability: str) -> dict[str, object]:
    expected = cast(Mapping[str, object], _STARTUP_CAPABILITIES[capability]["gate"])
    if capability == "coverage-inactive.v1":
        for name in expected:
            if os.environ.get(name):
                raise PythonEnvironmentIdentityError(
                    f"coverage startup capability requires inactive {name}; "
                    "active coverage startup/configuration is outside the verified subset"
                )
    elif capability == "setuptools-local-distutils.v1":
        if os.environ.get("SETUPTOOLS_USE_DISTUTILS", "local") != "local":
            raise PythonEnvironmentIdentityError(
                "setuptools startup capability requires the local-distutils branch; "
                "other SETUPTOOLS_USE_DISTUTILS modes need separate conformance"
            )
        if Path("pybuilddir.txt").is_file():
            raise PythonEnvironmentIdentityError(
                "setuptools finder would take its cwd/pybuilddir.txt CPython-build branch; "
                "that environmental suppression is outside the verified subset"
            )
    return dict(expected)


def _exact_gate(value: object, expected: Mapping[str, object]) -> bool:
    if not isinstance(value, Mapping) or set(value) != set(expected):
        return False
    observed = cast(Mapping[str, object], value)
    return all(
        type(observed[key]) is type(item) and observed[key] == item
        for key, item in expected.items()
    )


def _verify_code_owner(
    owner: ModuleType | type,
    body: Sequence[ast.stmt],
    code: CodeType,
    module: ModuleType,
) -> None:
    """Compare loaded definitions with compiled sealed source without executing it."""
    members = vars(owner)
    declared = {
        item.name for item in body if isinstance(item, (ast.FunctionDef, ast.ClassDef))
    }
    if isinstance(owner, type):
        declared.update(
            target.id
            for item in body
            if isinstance(item, ast.Assign)
            for target in item.targets
            if isinstance(target, ast.Name)
        )
        allowed_metadata = {
            "__module__",
            "__doc__",
            "__dict__",
            "__weakref__",
            "__annotations__",
            "__firstlineno__",
            "__static_attributes__",
        }
        generated = (
            {"spec_for_test.test_distutils"}
            if owner.__name__ == "DistutilsMetaFinder"
            else set()
        )
        if (
            owner.__bases__ != (object,)
            or set(members) - declared - allowed_metadata - generated
        ):
            raise PythonEnvironmentIdentityError(
                f"reviewed startup class shape differs: {owner.__name__}"
            )
    for item in body:
        if isinstance(item, ast.Import):
            for alias in item.names:
                local = alias.asname or alias.name.split(".")[0]
                if members.get(local) is not sys.modules.get(alias.name):
                    raise PythonEnvironmentIdentityError(
                        f"reviewed startup import binding differs: {local}"
                    )
        elif isinstance(item, ast.ClassDef):
            klass = members.get(item.name)
            if type(klass) is not type:
                raise PythonEnvironmentIdentityError(
                    f"reviewed startup class differs: {item.name}"
                )
            _verify_code_owner(klass, item.body, _code_child(code, item.name), module)
        elif isinstance(item, ast.FunctionDef):
            function = members.get(item.name)
            decorators = [
                decorator.id if isinstance(decorator, ast.Name) else None
                for decorator in item.decorator_list
            ]
            if decorators:
                descriptor = {
                    "staticmethod": staticmethod,
                    "classmethod": classmethod,
                }.get(str(decorators[0]))
                if (
                    len(decorators) != 1
                    or descriptor is None
                    or not isinstance(function, (staticmethod, classmethod))
                    or type(function) is not descriptor
                ):
                    raise PythonEnvironmentIdentityError(
                        f"reviewed startup descriptor differs: {item.name}"
                    )
                function = function.__func__
            defaults = (
                tuple(ast.literal_eval(value) for value in item.args.defaults) or None
            )
            kwdefaults = {
                arg.arg: ast.literal_eval(value)
                for arg, value in zip(
                    item.args.kwonlyargs, item.args.kw_defaults, strict=True
                )
                if value is not None
            } or None
            if (
                type(function) is not FunctionType
                or function.__code__ != _code_child(code, item.name)
                or function.__globals__ is not vars(module)
                or function.__defaults__ != defaults
                or function.__kwdefaults__ != kwdefaults
                or function.__closure__ is not None
            ):
                raise PythonEnvironmentIdentityError(
                    f"reviewed startup code/defaults differ: {item.name}"
                )


def _verify_reviewed_module(
    capability: str, path: Path, source: bytes, finder: object
) -> None:
    policy = _STARTUP_CAPABILITIES[capability]
    artifact = cast(Mapping[str, object], policy["module_artifact"])
    _require_artifact(source, artifact, label=capability)
    module = sys.modules.get(str(policy["module"]))
    if (
        module is None
        or Path(str(getattr(module, "__file__", ""))).absolute() != path
        or getattr(getattr(module, "__spec__", None), "origin", None) != str(path)
        or type(finder) is not getattr(module, str(policy["finder"]), None)
    ):
        raise PythonEnvironmentIdentityError(
            f"{capability} finder differs from its sealed module origin"
        )
    syntax = ast.parse(source, filename=str(path))
    compiled = compile(
        source, str(path), "exec", dont_inherit=True, optimize=sys.flags.optimize
    )
    _verify_code_owner(module, syntax.body, compiled, module)
    if capability == "uv-virtualenv.v1":
        lock = getattr(type(finder), "lock", None)
        if (
            getattr(module, "_DISTUTILS_PATCH", None)
            != ("distutils.dist", "setuptools.dist")
            or getattr(finder, "fullname", None) is not None
            or set(vars(finder)) - {"fullname"}
            or not isinstance(lock, list)
            or len(lock) > 1
            or any(type(item) is not _thread.LockType or item.locked() for item in lock)
        ):
            raise PythonEnvironmentIdentityError(
                "uv bootstrap finder has unsupported role/active state"
            )
    elif capability == "setuptools-local-distutils.v1":
        klass = type(finder)
        if (
            getattr(module, "DISTUTILS_FINDER", None) is not finder
            or vars(finder)
            or getattr(klass, "sensitive_tests", None) != ["test.test_distutils"]
            or vars(klass).get("spec_for_test.test_distutils")
            is not vars(klass).get("spec_for_sensitive_tests")
        ):
            raise PythonEnvironmentIdentityError(
                "setuptools finder has unsupported alias/active state"
            )


def validate_active_import_finders(
    *,
    reviewed_startup: Sequence[Mapping[str, object]] = (),
    module_sources: Mapping[str, tuple[Path, bytes]] | None = None,
    bootstrap_pending: bool = False,
) -> list[str]:
    """Validate the live reviewed finders and return their semantic order.

    A pending call is only early rejection, never receipt admission. Final
    capture binds every declared capability to its origin, code and gate.
    """
    standard = [
        machinery.BuiltinImporter,
        machinery.FrozenImporter,
        machinery.PathFinder,
    ]
    meta = list(sys.meta_path)
    if len(meta) < len(standard) or meta[-3:] != standard:
        raise PythonEnvironmentIdentityError(
            "unsupported Python meta finder order; standard finders must remain the suffix"
        )
    candidates = {
        (str(policy["module"]), str(policy["finder"])): name
        for name, policy in _STARTUP_CAPABILITIES.items()
        if policy["finder"] is not None
    }
    order: list[str] = []
    sources: Mapping[str, tuple[Path, bytes]] = (
        {} if module_sources is None else module_sources
    )
    for row in reviewed_startup:
        capability = str(row["capability"])
        gate = _observe_startup_gate(capability)
        if not _exact_gate(row.get("gate"), gate):
            raise PythonEnvironmentIdentityError(
                f"reviewed startup environment gate changed: {capability}"
            )
    expected = {
        str(row["capability"])
        for row in reviewed_startup
        if _STARTUP_CAPABILITIES[str(row["capability"])]["finder"] is not None
    }
    for finder in meta[:-3]:
        capability = candidates.get((type(finder).__module__, type(finder).__name__))
        if capability is None or capability in order:
            raise PythonEnvironmentIdentityError(
                "unsupported or repeated Python meta finder; editable mapping/custom hooks require an explicit verified capability"
            )
        order.append(capability)
        if not bootstrap_pending:
            if capability not in expected or capability not in sources:
                raise PythonEnvironmentIdentityError(
                    f"{capability} finder lacks an owned reviewed startup declaration"
                )
            source_path, source_bytes = sources[capability]
            _verify_reviewed_module(capability, source_path, source_bytes, finder)
    if not bootstrap_pending and set(order) != expected:
        raise PythonEnvironmentIdentityError(
            "declared reviewed startup finders are absent from the live interpreter"
        )
    hooks = list(sys.path_hooks)
    if (
        len(hooks) != 2
        or hooks[0] is not zipimport.zipimporter
        or not _filefinder_hook_matches(hooks[1])
    ):
        raise PythonEnvironmentIdentityError(
            "unsupported Python path hook; only standard zipimport/FileFinder are supported"
        )
    expected_loaders = [
        (suffix, loader)
        for loader, suffixes in (
            (machinery.ExtensionFileLoader, machinery.EXTENSION_SUFFIXES),
            (machinery.SourceFileLoader, machinery.SOURCE_SUFFIXES),
            (machinery.SourcelessFileLoader, machinery.BYTECODE_SUFFIXES),
        )
        for suffix in suffixes
    ]
    for key, finder in sys.path_importer_cache.items():
        if finder is None:
            continue
        if type(finder) is zipimport.zipimporter:
            if (
                Path(str(key)).absolute()
                != (Path(finder.archive) / finder.prefix).absolute()
            ):
                raise PythonEnvironmentIdentityError(
                    f"cached zip importer has a different archive path: {key!r}"
                )
            continue
        if (
            type(finder) is not machinery.FileFinder
            or vars(finder).get("_loaders") != expected_loaders
            or Path(str(key)).absolute() != Path(finder.path).absolute()
        ):
            raise PythonEnvironmentIdentityError(
                f"unsupported cached Python path finder: {key!r}"
            )
    return order


def _startup_for_declaration(path: str) -> str | None:
    name = PurePosixPath(path).name
    return next(
        (
            key
            for key, policy in _STARTUP_CAPABILITIES.items()
            if policy["declaration"] == name
        ),
        None,
    )


def _reviewed_declaration_artifact(
    capability: str, source: bytes
) -> Mapping[str, object] | None:
    if capability == "uv-virtualenv.v1":
        _pth_lines(source.decode("utf-8"), virtualenv=True)
        return None
    policy = _STARTUP_CAPABILITIES[capability]
    observed = hashlib.sha256(source).hexdigest()
    for artifact in cast(
        Sequence[Mapping[str, object]], policy["declaration_artifacts"]
    ):
        if artifact["sha256"] == observed and artifact["size"] == len(source):
            return artifact
    raise PythonEnvironmentIdentityError(
        f"unsupported {capability} startup declaration sha256={observed}; "
        "new upstream executable .pth templates require explicit review"
    )


def _installed_file_owners(
    distributions: Sequence[Mapping[str, object]],
) -> dict[str, Mapping[str, object]]:
    return {
        str(row["path"]): distribution
        for distribution in distributions
        for row in cast(Sequence[Mapping[str, object]], distribution["installed_files"])
    }


def _check_startup_owner(
    capability: str,
    path: str,
    owners: Mapping[str, Mapping[str, object]],
    bootstrap_paths: Sequence[str],
) -> None:
    policy = _STARTUP_CAPABILITIES[capability]
    if policy["distribution"] is None:
        if path not in bootstrap_paths or path in owners:
            raise PythonEnvironmentIdentityError(
                f"{capability} startup file lacks explicit bootstrap custody: {path}"
            )
    else:
        owner = owners.get(path)
        if (
            owner is None
            or owner.get("name") != policy["distribution"]
            or owner.get("version") != policy["version"]
        ):
            raise PythonEnvironmentIdentityError(
                f"{capability} startup file lacks its pinned distribution ownership: {path}"
            )


def capture_reviewed_startup(
    *,
    environment_root: Path,
    external_roots: Sequence[tuple[str, Path]],
    active_import_roots: Sequence[Mapping[str, object]],
    distributions: Sequence[Mapping[str, object]],
    tree_entries: Mapping[str, Mapping[str, object]],
    tree_nodes: Mapping[str, Mapping[str, object]],
    tree_pool: _FileNodePool,
    site_roots: Sequence[str],
    bootstrap_paths: Sequence[str],
) -> dict[str, object]:
    """Capture actual startup behavior from already-bound files, without tree scans.

    The normal environment capture and bounded isolated-interpreter consumer
    proof both use this authority. It never executes or copies vendor code.
    """
    roots = dict(external_roots)
    regions = _active_regions(active_import_roots)
    native_regions = {
        role: resolve_native_tree_path(roots[root], relative.as_posix())
        for role, (root, relative) in regions.items()
    }
    owners = _installed_file_owners(distributions)
    declarations: list[dict[str, object]] = []
    startup: list[dict[str, object]] = []
    sources: dict[str, tuple[Path, bytes]] = {}
    seen: set[str] = set()
    for path in _expected_declaration_paths(tree_entries, site_roots):
        node = str(tree_entries[path]["node"])
        content = tree_pool.read_bound(node, label=f"site declaration {path}")
        try:
            text = content.decode("utf-8")
        except UnicodeDecodeError as exc:
            raise PythonEnvironmentIdentityError(
                f"site declaration is outside the UTF-8 subset: {path}"
            ) from exc
        capability = _startup_for_declaration(path)
        roles: list[str] = []
        if capability is not None:
            if capability in seen:
                raise PythonEnvironmentIdentityError(
                    f"reviewed startup capability is duplicated: {capability}"
                )
            seen.add(capability)
            policy = _STARTUP_CAPABILITIES[capability]
            _check_startup_owner(capability, path, owners, bootstrap_paths)
            artifact = _reviewed_declaration_artifact(capability, content)
            module_record: dict[str, object] | None = None
            if policy["module"] is not None:
                module_path = (
                    PurePosixPath(path).parent / str(policy["module_path"])
                ).as_posix()
                _check_startup_owner(capability, module_path, owners, bootstrap_paths)
                module_entry = tree_entries.get(module_path)
                if module_entry is None or module_entry.get("kind") not in {
                    "file",
                    "hardlink",
                }:
                    raise PythonEnvironmentIdentityError(
                        f"reviewed startup module lacks direct-file custody: {module_path}"
                    )
                module_node = str(module_entry["node"])
                module_artifact = cast(Mapping[str, object], policy["module_artifact"])
                metadata = tree_nodes.get(module_node, {})
                if (
                    metadata.get("sha256") != module_artifact["sha256"]
                    or metadata.get("size") != module_artifact["size"]
                ):
                    raise PythonEnvironmentIdentityError(
                        f"unsupported {capability} module sha256={metadata.get('sha256')}; "
                        "expected reviewed upstream bytes; new templates require review"
                    )
                source = tree_pool.read_bound(
                    module_node, label=f"reviewed startup {capability}"
                )
                sources[capability] = (environment_root / module_path, source)
                module_record = {
                    "path": module_path,
                    "node": module_node,
                    "provenance": dict(module_artifact),
                }
            startup.append(
                {
                    "capability": capability,
                    "declaration": path,
                    "declaration_provenance": dict(artifact)
                    if artifact is not None
                    else None,
                    "module": module_record,
                    "gate": _observe_startup_gate(capability),
                }
            )
        else:
            for target in _pth_lines(text, virtualenv=False):
                lexical = Path(str(target))
                if (
                    not lexical.is_dir()
                    or lexical.resolve(strict=True) != lexical
                    or lexical.is_symlink()
                    or lexical.is_junction()
                ):
                    raise PythonEnvironmentIdentityError(
                        f"declarative editable import is not a direct directory: {target}"
                    )
                matches = [
                    role for role, native in native_regions.items() if native == lexical
                ]
                if len(matches) != 1:
                    raise PythonEnvironmentIdentityError(
                        f".pth directory has no unique active external import role: {target}"
                    )
                roles.extend(matches)
        declarations.append(
            {"path": path, "node": node, "content": text, "roles": sorted(roles)}
        )
    order = validate_active_import_finders(
        reviewed_startup=startup, module_sources=sources
    )
    return {
        "path_declarations": declarations,
        "reviewed_startup": startup,
        "finder_order": order,
    }


def capture_external_import_custody(
    *,
    environment_root: Path,
    external_roots: Sequence[tuple[str, Path]],
    active_import_roots: Sequence[Mapping[str, object]],
    distributions: Sequence[dict[str, object]],
    tree_entries: Mapping[str, Mapping[str, object]],
    tree_nodes: Mapping[str, Mapping[str, object]],
    tree_pool: _FileNodePool,
    site_roots: Sequence[str],
    bootstrap_paths: Sequence[str],
    capture_context: PythonFileCaptureContext,
) -> dict[str, object]:
    regions = _active_regions(active_import_roots)
    roots = dict(external_roots)
    custody = empty_external_import_custody()
    custody.update(
        capture_reviewed_startup(
            environment_root=environment_root,
            external_roots=external_roots,
            active_import_roots=active_import_roots,
            distributions=distributions,
            tree_entries=tree_entries,
            tree_nodes=tree_nodes,
            tree_pool=tree_pool,
            site_roots=site_roots,
            bootstrap_paths=bootstrap_paths,
        )
    )
    declarations = cast(Sequence[Mapping[str, object]], custody["path_declarations"])
    trees: list[dict[str, object]] = []
    forest, references = _external_forest(regions)
    for index, (source_root, source_path) in enumerate(forest):
        path = resolve_native_tree_path(roots[source_root], source_path.as_posix())
        pool = _FileNodePool(capture_context=capture_context)
        tree, _paths, _metadata = _stable_tree_inventory(
            path,
            root_id=f"external-tree-{index}",
            label="active external Python imports",
            pool=pool,
        )
        bindings = [
            {"role": row["role"], "path": row["path"]}
            for row in references
            if row["root"] == tree["id"]
        ]
        trees.append(
            {
                **tree,
                "source_root": source_root,
                "source_path": source_path.as_posix(),
                "roles": bindings,
                "file_nodes": pool.nodes,
            }
        )
    custody["trees"] = trees
    for distribution in distributions:
        source = distribution.get("external_source")
        if source is not None:
            installed = {
                str(row["path"])
                for row in cast(
                    Sequence[Mapping[str, object]], distribution["installed_files"]
                )
            }
            roles = sorted(
                {
                    role
                    for declaration in declarations
                    if declaration["path"] in installed
                    for role in cast(list[str], declaration["roles"])
                }
            )
            distribution["external_source"] = {
                **cast(Mapping[str, object], source),
                "import_roles": roles,
            }
    validate_external_import_custody(
        custody,
        external_roots={key: str(path) for key, path in external_roots},
        active_import_roots=active_import_roots,
        distributions=distributions,
        tree_entries=tree_entries,
        tree_nodes=tree_nodes,
        site_roots=site_roots,
        bootstrap_paths=bootstrap_paths,
    )
    return custody


def _validate_reviewed_startup(
    payload: Mapping[str, object],
    *,
    distributions: Sequence[Mapping[str, object]],
    tree_entries: Mapping[str, Mapping[str, object]],
    tree_nodes: Mapping[str, Mapping[str, object]],
    site_roots: Sequence[str],
    bootstrap_paths: Sequence[str],
) -> dict[str, Mapping[str, object]]:
    records = payload.get("reviewed_startup")
    expected = [
        (path, capability)
        for path in _expected_declaration_paths(tree_entries, site_roots)
        if (capability := _startup_for_declaration(path)) is not None
    ]
    if not isinstance(records, list) or len(records) != len(expected):
        raise PythonEnvironmentIdentityError(
            "reviewed startup does not cover every known executable declaration"
        )
    owners = _installed_file_owners(distributions)
    by_path: dict[str, Mapping[str, object]] = {}
    seen: set[str] = set()
    finder_capabilities: set[str] = set()
    for raw, (path, capability) in zip(records, expected, strict=True):
        policy = _STARTUP_CAPABILITIES[capability]
        if (
            not isinstance(raw, Mapping)
            or set(raw)
            != {"capability", "declaration", "declaration_provenance", "module", "gate"}
            or raw.get("capability") != capability
            or raw.get("declaration") != path
            or capability in seen
            or not _exact_gate(
                raw.get("gate"), cast(Mapping[str, object], policy["gate"])
            )
        ):
            raise PythonEnvironmentIdentityError(
                "reviewed startup capability shape/gate is invalid"
            )
        record = cast(Mapping[str, object], raw)
        seen.add(capability)
        _check_startup_owner(capability, path, owners, bootstrap_paths)
        provenance = record.get("declaration_provenance")
        if capability == "uv-virtualenv.v1":
            if provenance is not None:
                raise PythonEnvironmentIdentityError(
                    "uv startup directive provenance must use its reviewed grammar"
                )
        elif provenance not in cast(
            Sequence[Mapping[str, object]], policy["declaration_artifacts"]
        ):
            raise PythonEnvironmentIdentityError(
                "reviewed startup declaration has unknown upstream provenance"
            )
        module = record.get("module")
        if policy["module"] is None:
            if module is not None:
                raise PythonEnvironmentIdentityError(
                    "inactive startup capability must not invent a loaded module"
                )
        else:
            module_path = (
                PurePosixPath(path).parent / str(policy["module_path"])
            ).as_posix()
            _check_startup_owner(capability, module_path, owners, bootstrap_paths)
            artifact = cast(Mapping[str, object], policy["module_artifact"])
            if (
                not isinstance(module, Mapping)
                or set(module) != {"path", "node", "provenance"}
                or module.get("path") != module_path
                or module.get("provenance") != artifact
            ):
                raise PythonEnvironmentIdentityError(
                    "reviewed startup module provenance is invalid"
                )
            entry = tree_entries.get(module_path, {})
            node = tree_nodes.get(str(module.get("node")), {})
            if (
                entry.get("kind") not in {"file", "hardlink"}
                or entry.get("node") != module.get("node")
                or node.get("sha256") != artifact["sha256"]
                or node.get("size") != artifact["size"]
            ):
                raise PythonEnvironmentIdentityError(
                    "reviewed startup module has no exact file-node custody"
                )
            finder_capabilities.add(capability)
        by_path[path] = record
    order = payload.get("finder_order")
    if (
        not isinstance(order, list)
        or not all(isinstance(item, str) for item in order)
        or len(order) != len(finder_capabilities)
        or set(order) != finder_capabilities
    ):
        raise PythonEnvironmentIdentityError(
            "reviewed startup finder order is incomplete or duplicated"
        )
    return by_path


def validate_external_import_custody(
    payload: object,
    *,
    external_roots: Mapping[str, str],
    active_import_roots: Sequence[Mapping[str, object]],
    distributions: Sequence[Mapping[str, object]],
    tree_entries: Mapping[str, Mapping[str, object]],
    tree_nodes: Mapping[str, Mapping[str, object]],
    site_roots: Sequence[str],
    bootstrap_paths: Sequence[str],
) -> None:
    if (
        not isinstance(payload, Mapping)
        or set(payload) != set(empty_external_import_custody())
        or payload.get("schema") != PYTHON_EXTERNAL_IMPORT_SCHEMA
        or payload.get("finder_policy") != PYTHON_IMPORT_FINDER_POLICY
    ):
        raise PythonEnvironmentIdentityError(
            "external import custody/finder policy is invalid"
        )
    payload = cast(Mapping[str, object], payload)
    regions = _active_regions(active_import_roots)
    expected_regions, references_by_role = _external_forest(regions)
    trees = payload.get("trees")
    if not isinstance(trees, list) or len(trees) != len(expected_regions):
        raise PythonEnvironmentIdentityError(
            "external import trees do not cover the minimal active regions"
        )
    fields = {
        "id",
        "source_root",
        "source_path",
        "roles",
        "file_count",
        "node_ids",
        "entries",
        "manifest_sha256",
        "file_nodes",
    }
    for index, (tree, region) in enumerate(zip(trees, expected_regions, strict=True)):
        if (
            not isinstance(tree, Mapping)
            or set(tree) != fields
            or tree.get("id") != f"external-tree-{index}"
            or (tree.get("source_root"), tree.get("source_path"))
            != (region[0], region[1].as_posix())
        ):
            raise PythonEnvironmentIdentityError(
                "external import tree region/shape is invalid"
            )
        tree = cast(Mapping[str, object], tree)
        nodes, by_id = _validate_file_nodes(
            tree.get("file_nodes"), label="external import"
        )
        entries, _paths, references = _validate_inventory_entries(
            tree.get("entries"), label="external import", nodes=by_id
        )
        if (
            references != set(by_id)
            or tree.get("node_ids") != [node["id"] for node in nodes]
            or type(tree.get("file_count")) is not int
            or tree.get("file_count") != sum(_is_file_entry(row) for row in entries)
            or tree.get("manifest_sha256") != canonical_json_sha256(entries)
        ):
            raise PythonEnvironmentIdentityError(
                "external import tree has incomplete node/topology custody"
            )
        bindings = [
            {"role": row["role"], "path": row["path"]}
            for row in references_by_role
            if row["root"] == tree["id"]
        ]
        directories = {
            ".",
            *(str(row["path"]) for row in entries if row.get("kind") == "directory"),
        }
        if tree.get("roles") != bindings or any(
            row["path"] not in directories for row in bindings
        ):
            raise PythonEnvironmentIdentityError(
                "external import tree roles are incomplete or absent"
            )
    startup_by_path = _validate_reviewed_startup(
        payload,
        distributions=distributions,
        tree_entries=tree_entries,
        tree_nodes=tree_nodes,
        site_roots=site_roots,
        bootstrap_paths=bootstrap_paths,
    )
    declarations = payload.get("path_declarations")
    if not isinstance(declarations, list):
        raise PythonEnvironmentIdentityError(
            "external import path declarations are invalid"
        )
    expected_paths = _expected_declaration_paths(tree_entries, site_roots)
    if len(declarations) != len(expected_paths):
        raise PythonEnvironmentIdentityError(
            "external import path declarations omit active .pth files"
        )
    roots = {
        key: canonical_absolute_path(unicodedata.normalize("NFC", value))
        for key, value in external_roots.items()
    }
    if any(
        key != other_key and path.is_relative_to(other)
        for key, path in roots.items()
        for other_key, other in roots.items()
    ):
        raise PythonEnvironmentIdentityError(
            "external source admission roots overlap or alias"
        )
    active_paths = {
        role: roots[root].joinpath(*path.parts)
        for role, (root, path) in regions.items()
    }
    declared_roles: dict[str, set[str]] = {}
    for declaration, path in zip(declarations, expected_paths, strict=True):
        if (
            not isinstance(declaration, Mapping)
            or set(declaration) != {"path", "node", "content", "roles"}
            or declaration.get("path") != path
            or declaration.get("node") != tree_entries[path].get("node")
            or not isinstance(declaration.get("content"), str)
        ):
            raise PythonEnvironmentIdentityError(
                "external import path declaration shape is invalid"
            )
        declaration = cast(Mapping[str, object], declaration)
        content = cast(str, declaration["content"])
        data = content.encode("utf-8")
        node = tree_nodes.get(str(declaration["node"]), {})
        if (
            node.get("size") != len(data)
            or node.get("sha256") != hashlib.sha256(data).hexdigest()
        ):
            raise PythonEnvironmentIdentityError(
                "external import .pth declaration differs from sealed bytes"
            )
        startup = startup_by_path.get(path)
        if startup is None:
            paths = _pth_lines(content, virtualenv=False)
        else:
            artifact = _reviewed_declaration_artifact(str(startup["capability"]), data)
            if startup["declaration_provenance"] != artifact:
                raise PythonEnvironmentIdentityError(
                    "startup declaration bytes differ from claimed provenance"
                )
            paths = []
        roles: set[str] = set()
        for target in paths:
            matches = {
                role
                for role, active in active_paths.items()
                if active == type(target)(unicodedata.normalize("NFC", str(target)))
            }
            if len(matches) != 1:
                raise PythonEnvironmentIdentityError(
                    "external import .pth has no unique active directory role"
                )
            roles.update(matches)
        if declaration.get("roles") != sorted(roles):
            raise PythonEnvironmentIdentityError(
                "external import .pth role binding is invalid"
            )
        declared_roles[path] = roles
    owners = _installed_file_owners(distributions)
    covered: set[str] = set()
    for path, roles in declared_roles.items():
        if not roles:
            continue
        owner = owners.get(path)
        source = owner.get("external_source") if owner is not None else None
        if not isinstance(source, Mapping):
            raise PythonEnvironmentIdentityError(
                "external import .pth is not owned by an editable distribution"
            )
        for role in roles:
            root, region = regions[role]
            if source.get("root") != root or not region.is_relative_to(
                _region_path(source.get("path"))
            ):
                raise PythonEnvironmentIdentityError(
                    "editable import region escapes its declared source"
                )
        covered.update(roles)
    if covered != set(regions):
        raise PythonEnvironmentIdentityError(
            "active external imports lack complete owned .pth declarations"
        )
    for distribution in distributions:
        source = distribution.get("external_source")
        if source is None:
            continue
        installed = {
            str(row["path"])
            for row in cast(
                Sequence[Mapping[str, object]], distribution["installed_files"]
            )
        }
        distribution_roles = sorted(
            {
                role
                for path, values in declared_roles.items()
                if path in installed
                for role in values
            }
        )
        if (
            not isinstance(source, Mapping)
            or not distribution_roles
            or source.get("import_roles") != distribution_roles
        ):
            raise PythonEnvironmentIdentityError(
                "editable distribution has no exact declared external import region"
            )
