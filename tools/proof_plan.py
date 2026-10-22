#!/usr/bin/env python3
"""Select and validate Molt proofs from the canonical proof-plan manifest.

This is policy-neutral execution code.  Path ownership, proof metadata, and
local gate commands live only in ``tools/proof_plan.toml``.
"""

from __future__ import annotations

from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import datetime as dt
import fnmatch
import functools
import hashlib
import heapq
import json
import os
import math
import platform
import re
import shlex
from dataclasses import dataclass, field as dataclass_field
from pathlib import Path, PurePosixPath
import subprocess
import sys
import threading
import time
import tomllib
import traceback
from types import MappingProxyType
from typing import TYPE_CHECKING, Any, Collection, Iterable, Literal, Mapping

if TYPE_CHECKING:
    from molt.python_runtime_admission import BuildPythonAdmission

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.command_execution import (  # noqa: E402
    CommandExecutor,
    GUARD_CANCELLATION_OBSERVATION_SECONDS,
)
from tools import proof_receipts, toolchain_probe  # noqa: E402
from molt import host_capacity  # noqa: E402
from molt.portable_paths import portable_relative_path  # noqa: E402
from molt.process_guard import run_completed_command  # noqa: E402
from molt.process_environment import ProcessEnvironment, ProcessEnvironmentBuilder  # noqa: E402
from molt.toolchain_identity import find_executable  # noqa: E402
from tools.git_identity import (  # noqa: E402
    changed_paths,
    clean_checkout_status_arguments,
    require_git_object_id,
)

_COMMANDS = CommandExecutor(prefix="MOLT_PROOF", repo_root=ROOT)

from molt.cargo_execution_policy import (  # noqa: E402
    PROOF_COMMAND_TIMEOUT_ENV,
    load_ci_cargo_policy,
    normalize_cargo_environment,
)

from tools.proof_executor import execute_commands as _execute_commands  # noqa: E402

from tools import proof_plan_cli as _proof_plan_cli  # type: ignore  # noqa: E402

DEFAULT_MANIFEST = ROOT / "tools" / "proof_plan.toml"
NULL_SHA = "0" * 40
# The one argv placeholder a command may use. The executor binds it to the
# commit it verified as the proof checkout, so a source-addressed receipt names
# the exact revision the proof ran on without a hand-supplied object ID.
PROOF_SOURCE_COMMIT_PLACEHOLDER = "{proof_source_commit}"
_ARGV_PLACEHOLDER = re.compile(r"\{[A-Za-z_][A-Za-z0-9_]*\}")
# Local integration gates bind the fetched upstream once, never a mutable ref
# name or an ambient environment value. Standalone tools retain their defaults.
INTEGRATION_UPSTREAM_COMMIT_PLACEHOLDER = "{integration_upstream_commit}"
REQUIRED_FAMILY_FIELDS = (
    "description",
    "inputs",
    "executor",
    "workflow",
    "job",
    "admission_workflow",
    "admission_job",
    "admission_needs",
    "tiers",
    "required",
    "timeout_minutes",
    "dependencies",
    "resource_class",
)
REQUIRED_SCHEDULED_FAMILY_FIELDS = (
    "description",
    "workflow",
    "job",
    "tiers",
    "timeout_minutes",
    "resource_class",
)
REQUIRED_NAMED_LANE_FIELDS = (
    "cache_domain",
    "description",
    "argv",
    "toolchains",
    "resource_family",
    "contention_key",
    "timeout_seconds",
)
REQUIRED_COMMAND_FIELDS = (
    "family",
    "cell",
    "tiers",
    "resource_class",
    "cache_domain",
    "dependencies",
    "argv",
    "toolchains",
)
REQUIRED_TOOLCHAIN_FIELDS = (
    "version_pattern",
    "setup_evidence",
)
TARGET_DERIVED_IDENTITY_PROVIDERS = frozenset({"source-extension"})
# The provider that owns a policy's managed install, which
# tools/toolchain_env.py projects into a shell (HF-231): the verified LLVM SDK,
# the provisioned wasi-sdk, a pinned tool release, or the Binaryen release.
TOOLCHAIN_PROJECTIONS = frozenset(
    {"llvm-sdk", "wasi-sdk", "tool-release", "runtime-tool-release", "binaryen"}
)
# A runtime tool release (npm) runs through molt.tool_releases on its pinned
# runtime; it is provisioned and identified, never projected onto PATH.
RELEASE_PROJECTIONS = frozenset({"tool-release", "runtime-tool-release"})


def _tool_release_kinds(root: Path) -> dict[str, str]:
    """Each pinned tool release and the projection that names it.

    Cached per manifest state, not per root: the landing runner validates
    batch after batch in one sandbox path, each with its own manifest.
    """
    from molt.tool_releases import tool_releases_path

    try:
        stat = tool_releases_path(root).stat()
        signature = (stat.st_ino, stat.st_mtime_ns, stat.st_size)
    except OSError:
        signature = None  # the loader reports the missing manifest
    return _tool_release_kinds_of(root, signature)


@functools.lru_cache(maxsize=8)
def _tool_release_kinds_of(
    root: Path, signature: tuple[int, int, int] | None
) -> dict[str, str]:
    from molt.tool_releases import load_tool_releases

    return {
        name: "tool-release" if release.runtime is None else "runtime-tool-release"
        for name, release in load_tool_releases(root).items()
    }


def family_matrix_output(family_name: str) -> str:
    """Name the classifier output that holds one matrix family's runner cells."""
    return f"{family_name}_matrix"


@dataclass(frozen=True, slots=True)
class ProofFamily:
    name: str
    data: dict[str, Any]

    @property
    def inputs(self) -> tuple[str, ...]:
        return tuple(self.data["inputs"])


@dataclass(frozen=True, slots=True)
class ScheduledFamily:
    name: str
    data: dict[str, Any]


@dataclass(frozen=True, slots=True)
class MatrixCell:
    id: str
    data: dict[str, str]


def cargo_build_python(data: Mapping[str, Any]) -> bool | None:
    """Declared Cargo generator role; absent ad hoc policy stays conservative."""
    if "cargo_build_python" not in data:
        return None
    value = data["cargo_build_python"]
    tools = data.get("toolchains", [])
    if type(value) is not bool or not isinstance(tools, list) or "cargo" not in tools:
        raise ValueError("cargo_build_python requires a boolean and Cargo toolchain")
    if "build-python" in tools:
        raise ValueError(
            "build-python is contextual, not a static toolchain prerequisite"
        )
    return value


def cargo_native_units(data: Mapping[str, Any]) -> dict[str, list[str]]:
    """The command alone grants native build roles and source languages."""
    raw = data.get("cargo_native_units", {})
    if (
        "cargo_native_c_units" in data
        or not isinstance(raw, dict)
        or set(raw) - {"target", "host"}
        or any(
            not isinstance(languages, list)
            or not languages
            or any(language not in ("c", "c++") for language in languages)
            or len(set(languages)) != len(languages)
            for languages in raw.values()
        )
    ):
        raise ValueError(
            "cargo_native_units requires target/host roles with unique c/c++ languages"
        )
    tools = data.get("toolchains", [])
    if raw and (not isinstance(tools, list) or not {"cargo", "rustc"}.issubset(tools)):
        raise ValueError("native C/C++ build units require cargo and rustc toolchains")
    return {
        unit: [language for language in ("c", "c++") if language in raw[unit]]
        for unit in ("target", "host")
        if unit in raw
    }


PYTEST_WORKER_MEMORY_OPTION = "--molt-worker-memory-gb"


def pytest_declaration_argv(data: Mapping[str, Any]) -> tuple[str, ...]:
    """The argv tail that a row's typed pytest declaration contributes.

    A pytest row declares its targets as ``pytest`` and its measured worker
    unit as ``pytest_worker_memory_gb``, as local rules declare ``pytest``
    (HF-179). The plan reads those facts as data; this projection is the only
    place they become argv, after the row's declared launch prefix and options.
    """
    unit = data.get("pytest_worker_memory_gb")
    targets = data.get("pytest", ())
    return (
        *((f"{PYTEST_WORKER_MEMORY_OPTION}={unit}",) if unit is not None else ()),
        *(str(target) for target in targets if isinstance(targets, list)),
    )


def pytest_target_spelling_error(
    target: str, *, root: Path, listings: dict[Path, frozenset[str]]
) -> str | None:
    """Why a declared pytest target is not the one spelling of an existing test.

    CI selection, focused admission and the ownership gate compare declared
    targets as text, so each test has one spelling (HF-179): a relative POSIX
    path, each of whose ``/``-separated segments is a name its parent directory
    lists, case included, then an optional non-empty ``::node``. No directory
    lists an empty, ``.`` or ``..`` name, so a leading or trailing or doubled
    slash and a relative segment have no place in it, and a case-insensitive
    file system cannot admit another case. ``listings`` caches each
    directory's names for one validation pass.
    """
    path, separator, node = target.partition("::")
    if separator and not node:
        return f"pytest target {target!r} names an empty node"
    directory = root
    for part in path.split("/"):
        names = listings.get(directory)
        if names is None:
            try:
                names = frozenset(os.listdir(directory))
            except OSError:
                names = frozenset()
            listings[directory] = names
        if part not in names:
            return f"pytest target does not exist with this exact spelling: {target!r}"
        directory = directory / part
    if separator and not directory.is_file():
        return f"pytest node id must name a file: {target!r}"
    return None


def pytest_declaration_errors(
    label: str,
    data: Mapping[str, Any],
    *,
    root: Path = ROOT,
    listings: dict[Path, frozenset[str]] | None = None,
) -> list[str]:
    """Check a row's typed pytest declaration without importing pytest.

    pytest's own reading of each declared row, including the checkout's
    addopts, is proven by the plan's tests in the project environment.
    """
    errors: list[str] = []
    argv = data.get("argv")
    if isinstance(argv, list) and any(
        isinstance(part, str)
        and (
            part == PYTEST_WORKER_MEMORY_OPTION
            or part.startswith(PYTEST_WORKER_MEMORY_OPTION + "=")
        )
        for part in argv
    ):
        errors.append(
            f"{label}: declare the worker unit as pytest_worker_memory_gb, "
            f"not as {PYTEST_WORKER_MEMORY_OPTION} in argv"
        )
    unit = data.get("pytest_worker_memory_gb")
    if unit is not None:
        size = unit * 1024**3 if isinstance(unit, (int, float)) else math.nan
        if isinstance(unit, bool) or not math.isfinite(size) or size < 1:
            errors.append(
                f"{label}: pytest_worker_memory_gb must describe a positive "
                "finite byte count"
            )
    targets = data.get("pytest")
    if targets is None:
        if unit is not None:
            errors.append(f"{label}: pytest_worker_memory_gb requires pytest targets")
        return errors
    if (
        not isinstance(targets, list)
        or not targets
        or not all(
            isinstance(target, str) and target and not target.startswith(("-", "@"))
            for target in targets
        )
    ):
        errors.append(f"{label}: pytest must be a non-empty list of test paths")
        return errors
    if len(set(targets)) != len(targets):
        errors.append(f"{label}: pytest targets must be unique")
    listings = {} if listings is None else listings
    for target in targets:
        error = pytest_target_spelling_error(target, root=root, listings=listings)
        if error is not None:
            errors.append(f"{label}: {error}")
    return errors


@dataclass(frozen=True, slots=True)
class ProofCommand:
    id: str
    data: dict[str, Any]

    @property
    def family(self) -> str:
        return str(self.data["family"])

    @property
    def dependencies(self) -> tuple[str, ...]:
        return tuple(self.data["dependencies"])

    @property
    def argv(self) -> tuple[str, ...]:
        """The executed argv: the declared prefix, then the pytest declaration."""
        return (*self.data["argv"], *pytest_declaration_argv(self.data))

    @property
    def pytest_targets(self) -> tuple[str, ...]:
        """The declared test targets of a pytest row; empty for other tools."""
        return pytest_declaration_argv({"pytest": self.data.get("pytest", [])})

    @property
    def toolchains(self) -> tuple[str, ...]:
        return tuple(self.data["toolchains"])

    @property
    def evidence_outputs(self) -> tuple[str, ...]:
        return tuple(self.data.get("evidence_outputs", ()))

    @property
    def binds_source_commit(self) -> bool:
        return PROOF_SOURCE_COMMIT_PLACEHOLDER in self.argv


@dataclass(frozen=True, slots=True)
class NamedLane:
    """A queue-owned proof lane that runs under host custody, not in a CI family.

    Its command prefix and default cases own child-process admission. Typed
    differential selection may replace only the declared case tail; interpreter,
    build options and toolchain permissions remain part of the fixed prefix.
    """

    id: str
    data: dict[str, Any]

    @property
    def argv(self) -> tuple[str, ...]:
        return self.command_prefix + self.pytest_targets + self.differential_cases

    @property
    def command_prefix(self) -> tuple[str, ...]:
        return tuple(str(value) for value in self.data.get("argv", ()))

    @property
    def pytest_targets(self) -> tuple[str, ...]:
        """The declared test targets of a pytest lane, as for a command."""
        return pytest_declaration_argv({"pytest": self.data.get("pytest", [])})

    @property
    def differential_cases(self) -> tuple[str, ...]:
        raw = self.data.get("differential_cases", ())
        return (
            tuple(str(value) for value in raw) if isinstance(raw, (list, tuple)) else ()
        )

    @property
    def toolchains(self) -> tuple[str, ...]:
        return tuple(str(value) for value in self.data.get("toolchains", ()))


@dataclass(frozen=True, slots=True)
class ToolchainPolicy:
    name: str
    data: dict[str, Any]
    root: Path = ROOT

    @property
    def identity_kind(self) -> str:
        """Classify policy identity from its single declared provider boundary."""

        if "members" in self.data:
            return "group"
        return "target-derived" if "identity_provider" in self.data else "executable"

    @property
    def projection(self) -> str | None:
        """The provider tools/toolchain_env.py projects, or None (host tool)."""
        projection = self.data.get("projection")
        if projection is not None and projection not in TOOLCHAIN_PROJECTIONS:
            raise ValueError(f"{self.name}: invalid projection {projection!r}")
        return projection

    @property
    def python_identity_role(self) -> Literal["environment", "runtime"] | None:
        role = self.data.get("python_identity_role")
        if role not in {None, "environment", "runtime"}:
            raise ValueError(f"{self.name}: invalid Python identity role {role!r}")
        return role

    @property
    def inferred_executable(self) -> str | None:
        """Bare launchers infer canonical images, never selected resolver views."""
        if (
            self.identity_kind != "executable"
            or self.data.get("wasi_sdk_tool") is not None
            or self.data.get("compiler_tool_selector") == "llvm-sdk"
            or self.data.get("selection_only", False)
        ):
            return None
        return str(self.data.get("executable") or self.name)


def _selected_target_python_versions(root: Path) -> tuple[str, ...]:
    """Query the selected source authority without rebinding driver imports."""
    source = root / "src/molt/target_python.py"
    search_path = [str(root / "src"), str(root)] + [
        entry
        for entry in sys.path
        if entry and not Path(entry).resolve().is_relative_to(ROOT)
    ]
    try:
        completed = run_completed_command(
            [
                sys.executable,
                "-S",
                "-P",
                "-B",
                "-c",
                "import json,runpy,sys;print(json.dumps(runpy.run_path(sys.argv[1])"
                "['SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS']))",
                str(source),
            ],
            cwd=root,
            env=ProcessEnvironment(os.environ).updated(
                {"PYTHONPATH": os.pathsep.join(search_path)}
            ),
            capture_output=True,
            text=True,
            timeout=5,
            memory_guard_prefix=None,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise ValueError(f"selected target-Python catalog probe failed: {exc}") from exc
    if completed.returncode:
        raise ValueError(
            f"selected target-Python catalog failed: {completed.stderr.strip()}"
        )
    versions = json.loads(completed.stdout)
    if (
        not isinstance(versions, list)
        or not versions
        or any(
            not isinstance(v, str) or re.fullmatch(r"3\.[0-9]+", v) is None
            for v in versions
        )
        or len(versions) != len(set(versions))
    ):
        raise ValueError("selected target-Python catalog is malformed")
    return tuple(versions)


def load_toolchain_policies(
    data: Mapping[str, Any], *, root: Path = ROOT, names: Iterable[str] | None = None
) -> tuple[ToolchainPolicy, ...]:
    """Resolve selected prerequisites from their existing declarative owners."""
    entries = data.get("toolchain_policy", [])
    if names is not None:
        required = set(names)
        pending = list(required)
        by_name = {entry["name"]: entry for entry in entries}
        while pending:
            name = pending.pop()
            if name not in by_name:
                raise ValueError(f"unknown toolchain dependency {name!r}")
            entry = by_name[name]
            dependencies = list(entry.get("dependencies", [])) + list(
                entry.get("members", [])
            )
            if entry.get("python_versions") == "supported-targets":
                dependencies.append("uv")
            if "generator_manifest" in entry:
                with (root / entry["generator_manifest"]).open("rb") as handle:
                    rows = tomllib.load(handle).get("generator", [])
                dependencies.extend(generator_toolchains(rows))
            for dependency in dependencies:
                if dependency not in required:
                    required.add(dependency)
                    pending.append(dependency)
        entries = [entry for entry in entries if entry["name"] in required]
    policies: list[ToolchainPolicy] = []
    for entry in entries:
        row = dict(entry)
        name = str(row.get("name", ""))
        if row.get("executable") == "{python}":
            row["python_identity_role"] = "environment"
        if row.get("python_versions") == "supported-targets":
            row["members"] = []
            for version in _selected_target_python_versions(root):
                child = f"{name}.{version}"
                row["members"].append(child)
                policies.append(
                    ToolchainPolicy(
                        child,
                        {
                            "name": child,
                            "executable": "uv",
                            "dependencies": ["uv"],
                            "version_args": ["--version"],
                            "version_from_content_path": True,
                            "selection_only": True,
                            "python_identity_role": "runtime",
                            "python_version": version,
                            "content_path_command": [
                                "uv",
                                "run",
                                "--no-project",
                                "--no-python-downloads",
                                "--python",
                                version,
                                "python",
                                "-B",
                                "-I",
                                "-S",
                                "-c",
                                "import sys;print(sys.executable)",
                            ],
                            "version_pattern": rf"^Python {re.escape(version)}\.[0-9]+$",
                            "setup_value": version,
                            "setup_evidence": [
                                "src/molt/target_python.py::SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS"
                            ],
                            "setup_command": f"uv python install {version}",
                        },
                        root,
                    )
                )
        if "generator_manifest" in row:
            with (root / row["generator_manifest"]).open("rb") as handle:
                generators = tomllib.load(handle).get("generator", [])
            row["members"] = list(generator_toolchains(generators))
        if "checkout_input" in row:
            # A lock-declared checkout tree: molt.checkout_provisioning owns its
            # locks and its provisioning command, and admission ensures it.
            from molt.checkout_provisioning import (
                CHECKOUT_INPUT_NAMES,
                locked_input_files,
                provisioning_command,
            )

            checkout_input = row["checkout_input"]
            if checkout_input not in CHECKOUT_INPUT_NAMES:
                raise ValueError(f"{name}: unknown checkout input {checkout_input!r}")
            if "setup_command" in row:
                raise ValueError(
                    f"{name}: a checkout input derives its setup command; "
                    "remove setup_command"
                )
            if "package_lock" in row and row["package_lock"] not in locked_input_files(
                checkout_input
            ):
                raise ValueError(
                    f"{name}: package_lock {row['package_lock']!r} is not a lock "
                    f"file of checkout input {checkout_input}"
                )
            row["setup_command"] = provisioning_command(checkout_input)
        if "package_lock" in row:
            if row.get("node_package_mode", "require") not in {"require", "import"}:
                raise ValueError(f"{name}: invalid node package resolution mode")
            package = Path("node_modules") / str(row["node_package"])
            lock_path = Path(str(row["package_lock"]))
            lock = json.loads((root / lock_path).read_text(encoding="utf-8"))
            lock_key = package.as_posix()
            version = str(lock["packages"][lock_key]["version"])
            relative = "./" + lock_key
            row.update(
                executable="node",
                selection_only=True,
                probe_cwd=lock_path.parent.as_posix(),
                version_args=[
                    "-p",
                    f"require({json.dumps(relative + '/package.json')}).version",
                ],
                content_path_command=[
                    "node",
                    "--input-type=module",
                    "-e",
                    "import {fileURLToPath} from 'node:url';"
                    + (
                        f"console.log(fileURLToPath(import.meta.resolve({json.dumps(row['node_package'])})))"
                        if row.get("node_package_mode", "require") == "import"
                        else f"import {{createRequire}} from 'node:module';console.log(createRequire(import.meta.url).resolve({json.dumps(row['node_package'])}))"
                    ),
                ],
                version_pattern="^" + re.escape(version) + "$",
                setup_value=version,
                setup_evidence=[f'{lock_path.as_posix()}::"version": "{version}"'],
            )
        policies.append(ToolchainPolicy(name, row, root))
    return tuple(policies)


def generator_toolchains(rows: Iterable[Mapping[str, Any]]) -> tuple[str, ...]:
    """Prerequisites of the manifest's default reproducible generator selection."""
    return tuple(
        dict.fromkeys(
            toolchain
            for row in rows
            if row.get("check_mode")
            and row.get("ci_checkable", True)
            and not row.get("discovery_only", False)
            for toolchain in row["toolchains"]
        )
    )


def toolchain_closure(
    policies: Iterable[ToolchainPolicy], names: Iterable[str]
) -> tuple[str, ...]:
    """Expand one selected dependency graph; groups are selection, not images."""
    by_name = {policy.name: policy for policy in policies}
    closed: list[str] = []
    seen: set[str] = set()
    visiting: set[str] = set()

    def add(name: str) -> None:
        if name in visiting:
            raise ValueError(f"toolchain dependency cycle at {name!r}")
        if name in seen:
            return
        try:
            policy = by_name[name]
        except KeyError as exc:
            raise ValueError(f"unknown toolchain dependency {name!r}") from exc
        visiting.add(name)
        dependencies = policy.data.get("dependencies", [])
        members = policy.data.get("members", [])
        for field, values in (("dependencies", dependencies), ("members", members)):
            if not isinstance(values, list) or not all(
                isinstance(value, str) and value for value in values
            ):
                raise ValueError(f"{name}: malformed toolchain {field}")
        if policy.identity_kind != "group":
            closed.append(name)
        for dependency in (*dependencies, *members):
            add(dependency)
        visiting.remove(name)
        seen.add(name)

    for name in names:
        add(name)
    return tuple(closed)


_LOCAL_RULE_FIELDS = frozenset(
    {
        "name",
        "description",
        "globs",
        "gates",
        "pytest",
        "pytest_select",
        "toolchains",
        "plan_entities",
    }
)

# The proof plan's top-level sections and the key that names each entry of a
# table section; a section without one changes as a whole. A local rule claims
# the parts of the plan it reads with `plan_entities`, never by globbing the
# plan file, so an edit selects only the consumers of the entries it changed
# (HF-457), as a Bazel BUILD-file edit invalidates only the targets it changes.
PLAN_SECTIONS: Mapping[str, str | None] = MappingProxyType(
    {
        "schema": None,
        "authority_inputs": None,
        "always_toolchains": None,
        "always": None,
        "receipt_schema": None,
        "executor_max_workers": None,
        "inventory_hash_workers": None,
        "resource_policy": "name",
        "toolchain_policy": "name",
        "matrix_cell": "id",
        "ci_family": "name",
        "scheduled_family": "name",
        "command": "id",
        "rule": "name",
        "named_lane": "id",
        "gate_flip": "name",
    }
)
PLAN_PATH = "tools/proof_plan.toml"
# (section, entry name); None names the whole section.
PlanEntity = tuple[str, str | None]


def plan_entities(data: Mapping[str, Any]) -> dict[PlanEntity, object]:
    """Every entry of a plan, by section and name; unknown sections fail."""
    unknown = sorted(set(data) - set(PLAN_SECTIONS))
    if unknown:
        raise ValueError("unknown proof plan sections: " + ", ".join(unknown))
    entities: dict[PlanEntity, object] = {}
    for section, value in data.items():
        key = PLAN_SECTIONS[section]
        if key is None:
            entities[(section, None)] = value
            continue
        if not isinstance(value, list):
            raise ValueError(f"proof plan section {section!r} must be a table list")
        for entry in value:
            name = entry.get(key) if isinstance(entry, dict) else None
            if not isinstance(name, str) or not name:
                raise ValueError(f"{section}: every entry needs a string {key!r}")
            if (section, name) in entities:
                raise ValueError(f"{section}: duplicate {key} {name!r}")
            entities[(section, name)] = entry
    return entities


def changed_plan_entities(
    before: Mapping[str, Any] | None, after: Mapping[str, Any]
) -> frozenset[PlanEntity]:
    """The entries an edit added, removed or changed. A table section whose
    entries changed order changed as a whole: rule order is gate order.
    Without a prior plan every entry is new."""
    new = plan_entities(after)
    if before is None:
        return frozenset(new)
    old = plan_entities(before)
    changed = {
        entity
        for entity in old.keys() | new.keys()
        if old.get(entity) != new.get(entity)
    }

    def order(entities: Mapping[PlanEntity, object], section: str) -> list[str]:
        return [name for kind, name in entities if kind == section and name is not None]

    for section, key in PLAN_SECTIONS.items():
        if key is None:
            continue
        kept = set(order(old, section)) & set(order(new, section))
        if [n for n in order(old, section) if n in kept] != [
            n for n in order(new, section) if n in kept
        ]:
            changed.add((section, None))
    return frozenset(changed)


class PlanClaims:
    """Resolve local rules' `plan_entities` against one plan, read once.

    `section` claims every entry of a section and `section:name` one entry; a
    toolchain policy also claims the policies it depends on, since a change to
    any of them changes what it admits.
    """

    def __init__(self, data: Mapping[str, Any], *, root: Path = ROOT) -> None:
        self._data = data
        self._root = root
        # Read on the first claim of a named entry: most rules claim none.
        self._entities: dict[PlanEntity, object] | None = None
        self._policies: tuple[ToolchainPolicy, ...] | None = None

    def claims(
        self, rule: Mapping[str, Any]
    ) -> tuple[list[str], frozenset[PlanEntity]]:
        """A rule's claims as (errors, the entities a change to which selects it)."""
        name = rule.get("name", "local rule")
        raw = rule.get("plan_entities", [])
        if not isinstance(raw, list) or not all(isinstance(item, str) for item in raw):
            return [f"{name}: plan_entities must be a list of str"], frozenset()
        errors: list[str] = []
        claims: set[PlanEntity] = set()
        for item in raw:
            section, separator, entry = item.partition(":")
            if section not in PLAN_SECTIONS:
                errors.append(
                    f"{name}: plan_entities names unknown section {section!r}"
                )
                continue
            if not separator:
                claims.add((section, None))
                continue
            if self._entities is None:
                self._entities = plan_entities(self._data)
            if PLAN_SECTIONS[section] is None or (section, entry) not in self._entities:
                errors.append(f"{name}: plan_entities names no entry {item!r}")
                continue
            claims.add((section, entry))
            if section == "toolchain_policy":
                if self._policies is None:
                    self._policies = load_toolchain_policies(
                        self._data, root=self._root
                    )
                for dependency in toolchain_closure(self._policies, (entry,)):
                    claims.add((section, dependency))
        return errors, frozenset(claims)


def plan_claim_matches(
    claims: Collection[PlanEntity], changed: Iterable[PlanEntity]
) -> bool:
    """Whether a change to any of ``changed`` reaches one of ``claims``: the
    same entry, a claim of its whole section, or a change to a whole section
    that holds a claimed entry."""
    sections = {section for section, _entry in claims}
    return any(
        entity in claims
        or (entity[0], None) in claims
        or (entity[1] is None and entity[0] in sections)
        for entity in changed
    )


def plan_policy_errors(data: Mapping[str, Any]) -> list[str]:
    """The committed plan's selection invariants (HF-457): every section is
    claimed whole by some rule, so an edit to it selects a consumer, and no
    rule globs the plan file, which would select it for every edit."""
    from tools.dirty_tree_policy import is_ignored

    try:
        plan_entities(data)
    except ValueError as exc:
        return [str(exc)]
    resolver = PlanClaims(data, root=ROOT)
    errors: list[str] = []
    claimed: set[str] = set()
    for rule in data.get("rule", []):
        rule_errors, claims = resolver.claims(rule)
        errors.extend(rule_errors)
        claimed.update(section for section, entry in claims if entry is None)
        if is_ignored(PLAN_PATH, tuple(rule.get("globs", []))):
            errors.append(
                f"{rule.get('name')}: claim the plan entries the rule reads with "
                f"plan_entities; a glob of {PLAN_PATH} selects it for any edit"
            )
    if unclaimed := sorted(set(PLAN_SECTIONS) - claimed):
        errors.append(
            "no local rule claims these proof plan sections, so an edit to them "
            "selects no consumer: " + ", ".join(unclaimed)
        )
    return errors


_LOCAL_PYTEST_PREFIX = ("python", "-m", "pytest", "-q")
# The selection options a `pytest_select` entry may name, by its table key.
_LOCAL_PYTEST_SELECTORS = {"k": "-k", "m": "-m"}


def _pytest_target_parts(target: str) -> tuple[str, ...]:
    path, separator, nodes = target.rstrip("/").partition("::")
    parts = PurePosixPath(path).parts
    if not separator:
        return parts
    names, parameter, value = nodes.partition("[")
    return (
        *parts,
        *("::" + name for name in names.split("::")),
        *(("[" + value,) if parameter else ()),
    )


@dataclass(slots=True)
class _PytestMemoryNode:
    children: dict[str, _PytestMemoryNode] = dataclass_field(default_factory=dict)
    owned_gb: float | None = None
    maximum_gb: float = 0.0


class PytestMemoryUnits:
    """Derived command-target envelopes, indexed once without test collection.

    Whole-file/directory owners cover descendants; a node owner covers only
    that node. A broader request includes its known descendants and the ad hoc
    default for uncovered members. Selectors keep their declared file envelope;
    no finer memory measurement is inferred from a -k or -m expression.
    """

    def __init__(self, commands: Iterable[Mapping[str, Any]]) -> None:
        self._root = _PytestMemoryNode()
        for command in commands:
            # A row declares its targets and measured unit as data (HF-179);
            # nothing reads them back from argv, and no test is collected.
            unit = command.get("pytest_worker_memory_gb")
            if unit is None:
                continue
            if isinstance(unit, bool) or not isinstance(unit, (int, float)):
                raise ValueError("pytest_worker_memory_gb must be a number")
            unit = float(unit)
            size = unit * 1024**3
            if not math.isfinite(size) or size < 1:
                raise ValueError(
                    "pytest_worker_memory_gb must describe a positive finite byte count"
                )
            for target in command.get("pytest", ()):
                node = self._root
                node.maximum_gb = max(node.maximum_gb, unit)
                for component in _pytest_target_parts(target):
                    child = node.children.get(component)
                    if child is None:
                        child = node.children[component] = _PytestMemoryNode()
                    node = child
                    node.maximum_gb = max(node.maximum_gb, unit)
                node.owned_gb = max(node.owned_gb or 0.0, unit)

    def memory_gb(self, targets: Iterable[str]) -> float:
        maximum = 0.0
        default = host_capacity.DEFAULT_PYTEST_WORKER_MEMORY_GB
        for target in targets:
            node = self._root
            owned = node.owned_gb
            for component in _pytest_target_parts(target):
                child = node.children.get(component)
                if child is None:
                    maximum = max(maximum, default if owned is None else owned)
                    break
                node = child
                if node.owned_gb is not None:
                    owned = max(owned or 0.0, node.owned_gb)
            else:
                maximum = max(
                    maximum, default if owned is None else owned, node.maximum_gb
                )
        return maximum or default


def local_pytest_argv(
    targets: Iterable[str],
    select: tuple[str, ...] = (),
    *,
    memory_units: PytestMemoryUnits,
) -> tuple[str, ...]:
    """The one pytest invocation for local gate test targets.

    Integration merges every selected rule's targets into one run, so the
    interpreter, guard and collection costs are paid once. ``-n auto`` sizes
    the workers from the host using the largest owning target envelope. A
    selection (``-k`` or ``-m``) applies
    to every target of its run, so each one runs on its own.
    """
    targets = tuple(targets)
    unit = memory_units.memory_gb(targets)
    return (
        *_LOCAL_PYTEST_PREFIX,
        "-n",
        "auto",
        f"{PYTEST_WORKER_MEMORY_OPTION}={unit}",
        *select,
        *targets,
    )


def local_pytest_collection_argv(command: tuple[str, ...]) -> tuple[str, ...]:
    """Collect a canonical selected target projection under its declared options.

    Only the executor's worker count changes. Pytest itself validates node IDs,
    parameter IDs and imports; collection never establishes a test result.
    """
    prefix = (*_LOCAL_PYTEST_PREFIX, "-n", "auto")
    if command[: len(prefix)] != prefix:
        raise ValueError("collection requires a canonical local pytest argv")
    return (*_LOCAL_PYTEST_PREFIX, "-n", "0", *command[len(prefix) :], "--collect-only")


def local_pytest_command(
    targets: Iterable[str],
    select: tuple[str, ...] = (),
    *,
    memory_units: PytestMemoryUnits,
) -> str:
    """Display/projection of the argv executed directly by the local driver."""
    return shlex.join(local_pytest_argv(targets, select, memory_units=memory_units))


def local_pytest_selections(
    rule: Mapping[str, Any],
) -> list[tuple[tuple[str, ...], tuple[str, ...]]]:
    """A rule's `pytest_select` entries as (selection argv, targets)."""
    selections = []
    for entry in rule.get("pytest_select", []):
        (key,) = set(entry) & set(_LOCAL_PYTEST_SELECTORS)
        selections.append(
            ((_LOCAL_PYTEST_SELECTORS[key], entry[key]), tuple(entry["targets"]))
        )
    return selections


def local_pytest_select_errors(
    name: str, entries: object, *, root: Path = ROOT
) -> list[str]:
    """Each `pytest_select` entry names one selection and its test targets."""
    if not isinstance(entries, list):
        return [f"{name}: pytest_select must be a list of tables"]
    errors = []
    for index, entry in enumerate(entries):
        label = f"{name}: pytest_select[{index}]"
        if not isinstance(entry, dict):
            errors.append(f"{label} must be a table")
            continue
        keys = set(entry) & set(_LOCAL_PYTEST_SELECTORS)
        unknown = set(entry) - set(_LOCAL_PYTEST_SELECTORS) - {"targets"}
        if len(keys) != 1 or unknown:
            errors.append(f"{label} must hold exactly one of k or m, and targets")
            continue
        (key,) = keys
        if not isinstance(entry[key], str) or not entry[key].strip():
            errors.append(f"{label}.{key} must be a non-empty expression")
        errors.extend(
            local_pytest_target_errors(label, entry.get("targets"), root=root)
        )
    return errors


def local_rule_field_error(rule: Mapping[str, Any]) -> str | None:
    """Reject policy fields that neither the executor nor its projection consumes."""
    unknown = sorted(set(rule) - _LOCAL_RULE_FIELDS)
    if unknown:
        return (
            f"{rule.get('name', 'local rule')}: unknown local rule fields: "
            + ", ".join(unknown)
        )
    return None


def local_gate_commands(
    data: Mapping[str, Any],
    *,
    memory_units: PytestMemoryUnits,
) -> dict[str, tuple[str, tuple[str, ...]]]:
    """The existing generated local IDs and their declared prerequisites."""
    commands = {}

    def add_shell(identifier: str, command: str, tools: Iterable[str]) -> None:
        if error := local_gate_template_error(command):
            raise ValueError(f"{identifier}: {error}")
        commands[identifier] = command, tuple(tools)

    for index, command in enumerate(data.get("always", [])):
        add_shell(f"local.always.{index}", command, data.get("always_toolchains", []))
    for rule in data.get("rule", []):
        for index, command in enumerate(rule.get("gates", [])):
            add_shell(
                f"local.{rule['name']}.{index}", command, rule.get("toolchains", [])
            )
        if rule.get("pytest"):
            commands[f"local.{rule['name']}.pytest"] = (
                local_pytest_command(rule["pytest"], memory_units=memory_units),
                tuple(rule.get("toolchains", [])),
            )
        for index, (select, targets) in enumerate(local_pytest_selections(rule)):
            commands[f"local.{rule['name']}.pytest-select-{index}"] = (
                local_pytest_command(targets, select, memory_units=memory_units),
                tuple(rule.get("toolchains", [])),
            )
    return commands


def local_gate_template_error(command: str) -> str | None:
    """Admit the one closed integration binding as a commit or commit range."""
    for token in _ARGV_PLACEHOLDER.findall(command):
        if token != INTEGRATION_UPSTREAM_COMMIT_PLACEHOLDER:
            return f"unknown local gate placeholder {token!r}"
    remainder = command.replace(INTEGRATION_UPSTREAM_COMMIT_PLACEHOLDER, "")
    if "{integration_" in remainder or "integration_upstream_commit}" in remainder:
        return "malformed integration upstream placeholder"
    if INTEGRATION_UPSTREAM_COMMIT_PLACEHOLDER not in command:
        return None
    try:
        arguments = shlex.split(command)
    except ValueError as exc:
        return f"invalid local gate template: {exc}"
    for argument in arguments:
        if INTEGRATION_UPSTREAM_COMMIT_PLACEHOLDER in argument and argument not in {
            INTEGRATION_UPSTREAM_COMMIT_PLACEHOLDER,
            INTEGRATION_UPSTREAM_COMMIT_PLACEHOLDER + "..HEAD",
        }:
            return "integration upstream placeholder must be a whole commit or BASE..HEAD argument"
    return None


def bind_local_gate_command(
    command: str | tuple[str, ...], *, upstream_commit: str
) -> str | tuple[str, ...]:
    """Bind an execution copy; selection and prerequisite IDs keep raw policy."""
    require_git_object_id(upstream_commit, label="integration gate upstream")
    # Structured pytest selections contain literal argv, not shell templates.
    if not isinstance(command, str):
        return command
    if error := local_gate_template_error(command):
        raise ValueError(error)
    return command.replace(INTEGRATION_UPSTREAM_COMMIT_PLACEHOLDER, upstream_commit)


def local_pytest_target_errors(
    name: str, targets: object, *, root: Path = ROOT
) -> list[str]:
    """A gate's pytest targets are existing test paths or node IDs, never options.

    Options belong to the merged invocation; a target-specific option would
    silently apply to every other rule's tests in that run.
    """
    if not isinstance(targets, list) or not all(
        isinstance(item, str) and item for item in targets
    ):
        return [f"{name}: pytest must be a list of test paths"]
    errors = []
    tests_root = (root / "tests").resolve()
    listings: dict[Path, frozenset[str]] = {}
    for target in targets:
        path = target.split("::", 1)[0]
        candidate = (root / path).resolve()
        if not path.startswith("tests/") or not candidate.is_relative_to(tests_root):
            errors.append(f"{name}: pytest target {target!r} must be a tests/ path")
        elif (
            error := pytest_target_spelling_error(target, root=root, listings=listings)
        ) is not None:
            errors.append(f"{name}: {error}")
        elif not candidate.is_file():
            errors.append(f"{name}: pytest target {target!r} does not exist")
    return errors


@dataclass(frozen=True, slots=True)
class ResourcePolicy:
    name: str
    max_parallel: int


@dataclass(frozen=True, slots=True)
class TimeoutEnvelope:
    projected_makespan_seconds: int
    critical_path_seconds: int
    resource_capacity_floor_seconds: dict[str, int]


@dataclass(frozen=True, slots=True)
class Selection:
    changed_paths: tuple[str, ...]
    selected: tuple[ProofFamily, ...]
    reasons: dict[str, tuple[str, ...]]
    fail_closed_reason: str | None = None


_REPOSITORY_SOURCE_ROOTS = ("examples", "formal", "src", "tests", "tools")


def argv_repository_paths(argv: Iterable[object]) -> tuple[str, ...]:
    """The repository paths that an argv names, read from their spelling.

    A token names a path when its text before any ``::`` is a source root or
    lies under one. This reads the argv of any tool, so it cannot tell an
    operand from an option value; a pytest row declares its test targets in
    ``pytest`` instead. Plan validation (each path must exist) and the test
    inputs of commands that do not run pytest read this rule.
    """
    prefixes = tuple(f"{root}/" for root in _REPOSITORY_SOURCE_ROOTS)
    paths: dict[str, None] = {}
    for part in argv:
        if not isinstance(part, str):
            continue
        path = part.split("::", 1)[0].rstrip("/")
        if path in _REPOSITORY_SOURCE_ROOTS or path.startswith(prefixes):
            paths[path] = None
    return tuple(paths)


def _resolve_ci_command_test_inputs(
    families: tuple[ProofFamily, ...], commands: tuple[ProofCommand, ...]
) -> None:
    """Project validated command targets into their change-selected owners.

    A pytest command's inputs are its declared ``pytest`` targets (HF-179), and
    a node id selects its file. A command that does not run pytest declares no
    targets; the ``tests`` paths its argv names (``argv_repository_paths``) are
    its test inputs. A literal target owns itself and its descendants, whether
    it is a file or directory. The existing ``path/**`` matcher expresses that
    without stat calls, collection, extension guesses or glob interpretation
    of a target's spelling. Scheduled families have no changed-path owner.
    Resolve once at load time; selection and generated projections read the
    same family data. Work is linear in declared argv, inputs and path parts.
    """
    targets: dict[str, dict[str, None]] = {family.name: {} for family in families}
    for command in commands:
        owner = targets.get(command.family)
        if owner is None:
            continue
        if command.pytest_targets:
            owner.update(
                dict.fromkeys(
                    target.split("::", 1)[0] for target in command.pytest_targets
                )
            )
            continue
        for path in argv_repository_paths(command.argv):
            if path == "tests" or path.startswith("tests/"):
                owner[path] = None

    for family in families:
        owned = targets[family.name]
        # Exact copies of command targets are not another source of ownership.
        inputs = [pattern for pattern in family.inputs if pattern not in owned]
        subtrees = {
            pattern[:-3].rstrip("/") for pattern in inputs if pattern.endswith("/**")
        }
        for target in owned:
            parents = tuple(str(parent) for parent in PurePosixPath(target).parents)
            if target in subtrees or any(
                parent in subtrees or parent in owned for parent in parents
            ):
                continue
            inputs.append(target + "/**")
        family.data["inputs"] = inputs


@dataclass(frozen=True, slots=True)
class ProofPlan:
    path: Path
    authority_inputs: tuple[str, ...]
    receipt_schema: str
    matrix_cells: tuple[MatrixCell, ...]
    families: tuple[ProofFamily, ...]
    scheduled_families: tuple[ScheduledFamily, ...]
    commands: tuple[ProofCommand, ...]
    toolchain_policies: tuple[ToolchainPolicy, ...]
    named_lanes: tuple[NamedLane, ...]
    executor_max_workers: int
    inventory_hash_workers: int
    resource_policies: tuple[ResourcePolicy, ...]
    local_rules: tuple[dict[str, Any], ...]
    always: tuple[str, ...]
    always_toolchains: tuple[str, ...] = ()

    def toolchain_closure(self, names: Iterable[str]) -> tuple[str, ...]:
        """Expand declared toolchains through the canonical dependency graph."""
        return toolchain_closure(self.toolchain_policies, names)

    def required_toolchains(self, command: ProofCommand) -> tuple[str, ...]:
        """Return the static process/tool identity closure for one command.

        Queue admission adds contextual prerequisites that need the command's
        Cargo context, such as the declared ``build-python`` generator.
        """

        return self.toolchain_closure(command.toolchains)

    def family_cells(self, family_name: str) -> tuple[str, ...]:
        """Return the family's matrix cells in first-declaration order."""
        return tuple(
            dict.fromkeys(
                str(command.data["cell"])
                for command in self.commands
                if command.family == family_name
            )
        )

    def timeout_envelope(
        self, family_name: str, *, matrix_cell: str | None = None
    ) -> TimeoutEnvelope:
        """Project command deadlines, excluding setup and finalization work.

        A `github-matrix` job runs one cell, so its envelope is per cell.
        Each job separately reserves its declared operational allowance.
        """
        commands = tuple(
            command
            for command in self.commands
            if command.family == family_name
            and (matrix_cell is None or command.data["cell"] == matrix_cell)
        )
        if not commands:
            suffix = "" if matrix_cell is None else f" in matrix cell {matrix_cell!r}"
            raise ValueError(f"{family_name}: selected family has no commands{suffix}")
        command_by_id = {command.id: command for command in commands}
        command_index = {command.id: index for index, command in enumerate(commands)}
        resource_limits = {
            policy.name: policy.max_parallel for policy in self.resource_policies
        }
        remaining_dependencies: dict[str, int] = {}
        dependents: dict[str, list[str]] = {command.id: [] for command in commands}
        critical_paths: dict[str, int] = {}
        for command in commands:
            dependencies = tuple(
                dependency
                for dependency in command.dependencies
                if dependency in command_by_id
            )
            remaining_dependencies[command.id] = len(dependencies)
            for dependency in dependencies:
                dependents[dependency].append(command.id)

        ready_by_resource: dict[str, list[tuple[int, str]]] = {
            name: [] for name in resource_limits
        }
        for command in commands:
            if remaining_dependencies[command.id] == 0:
                resource = str(command.data["resource_class"])
                heapq.heappush(
                    ready_by_resource[resource],
                    (command_index[command.id], command.id),
                )
        active_by_resource = {name: 0 for name in resource_limits}
        active: list[tuple[int, int, str, str]] = []
        completed: set[str] = set()
        now = 0
        while len(completed) < len(commands):
            while len(active) < self.executor_max_workers:
                available_resources = tuple(
                    resource
                    for resource, ready in ready_by_resource.items()
                    if ready
                    and active_by_resource[resource] < resource_limits[resource]
                )
                if not available_resources:
                    break
                resource = min(
                    available_resources,
                    key=lambda name: ready_by_resource[name][0][0],
                )
                _, command_id = heapq.heappop(ready_by_resource[resource])
                command = command_by_id[command_id]
                finish = now + int(command.data["timeout_seconds"])
                heapq.heappush(
                    active,
                    (finish, command_index[command_id], command_id, resource),
                )
                active_by_resource[resource] += 1
            if not active:
                blocked = sorted(set(command_by_id) - completed)
                raise ValueError(
                    f"{family_name}: timeout projection dependency deadlock: {blocked!r}"
                )
            now = active[0][0]
            finishing: list[tuple[int, int, str, str]] = []
            while active and active[0][0] == now:
                finishing.append(heapq.heappop(active))
            for _, _, command_id, resource in finishing:
                command = command_by_id[command_id]
                active_by_resource[resource] -= 1
                completed.add(command_id)
                parent_paths = [
                    critical_paths[dependency]
                    for dependency in command.dependencies
                    if dependency in critical_paths
                ]
                critical_paths[command_id] = int(command.data["timeout_seconds"]) + max(
                    parent_paths, default=0
                )
                for dependent in dependents[command_id]:
                    remaining_dependencies[dependent] -= 1
                    if remaining_dependencies[dependent] == 0:
                        dependent_command = command_by_id[dependent]
                        dependent_resource = str(
                            dependent_command.data["resource_class"]
                        )
                        heapq.heappush(
                            ready_by_resource[dependent_resource],
                            (command_index[dependent], dependent),
                        )
        resource_capacity_floor_seconds = {
            resource: (
                sum(
                    int(command.data["timeout_seconds"])
                    for command in commands
                    if command.data["resource_class"] == resource
                )
                + limit
                - 1
            )
            // limit
            for resource, limit in resource_limits.items()
            if any(command.data["resource_class"] == resource for command in commands)
        }
        return TimeoutEnvelope(
            projected_makespan_seconds=now,
            critical_path_seconds=max(critical_paths.values(), default=0),
            resource_capacity_floor_seconds=resource_capacity_floor_seconds,
        )

    @classmethod
    def load(cls, path: Path = DEFAULT_MANIFEST) -> "ProofPlan":
        data = tomllib.loads(path.read_text(encoding="utf-8"))
        if data.get("schema") != "molt.proof-plan.v6":
            raise ValueError(f"{path}: expected schema molt.proof-plan.v6")
        families = tuple(
            ProofFamily(str(entry.get("name", "")), dict(entry))
            for entry in data.get("ci_family", [])
        )
        cargo_budgets = load_ci_cargo_policy().execution_budgets
        commands: list[ProofCommand] = []
        for entry in data.get("command", []):
            command_data = dict(entry)
            budget_class = command_data.get("timeout_budget")
            if budget_class is not None:
                if "timeout_seconds" in command_data:
                    raise ValueError(
                        f"{command_data.get('id', '<unknown>')}: declare timeout_budget "
                        "or timeout_seconds, not both"
                    )
                command_data["timeout_seconds"] = cargo_budgets.timeout_seconds(
                    str(budget_class)
                )
            commands.append(ProofCommand(str(command_data.get("id", "")), command_data))
        plan = cls(
            path=path,
            authority_inputs=tuple(data.get("authority_inputs", [])),
            receipt_schema=str(data.get("receipt_schema", "")),
            matrix_cells=tuple(
                MatrixCell(str(entry.get("id", "")), dict(entry))
                for entry in data.get("matrix_cell", [])
            ),
            families=families,
            scheduled_families=tuple(
                ScheduledFamily(str(entry.get("name", "")), dict(entry))
                for entry in data.get("scheduled_family", [])
            ),
            commands=tuple(commands),
            named_lanes=tuple(
                NamedLane(str(entry.get("id", "")), dict(entry))
                for entry in data.get("named_lane", [])
            ),
            toolchain_policies=load_toolchain_policies(data, root=path.parent.parent),
            executor_max_workers=int(data.get("executor_max_workers", 0)),
            inventory_hash_workers=int(data.get("inventory_hash_workers", 0)),
            resource_policies=tuple(
                ResourcePolicy(
                    str(entry.get("name", "")), int(entry.get("max_parallel", 0))
                )
                for entry in data.get("resource_policy", [])
            ),
            local_rules=tuple(dict(entry) for entry in data.get("rule", [])),
            always=tuple(data.get("always", [])),
            always_toolchains=tuple(data.get("always_toolchains", [])),
        )
        errors = plan.validate() + plan_policy_errors(data)
        if errors:
            raise ValueError("invalid proof plan:\n- " + "\n- ".join(errors))
        _resolve_ci_command_test_inputs(plan.families, plan.commands)
        return plan

    def named_lane(self, lane_id: str) -> NamedLane:
        for lane in self.named_lanes:
            if lane.id == lane_id:
                return lane
        raise KeyError(f"proof plan has no named lane {lane_id!r}")

    def _validate_named_lanes(
        self, errors: list[str], listings: dict[Path, frozenset[str]]
    ) -> None:
        policy_names = {policy.name for policy in self.toolchain_policies}
        command_argvs = {
            tuple(str(value) for value in command.argv) for command in self.commands
        }
        seen_ids: set[str] = set()
        seen_argvs: set[tuple[str, ...]] = set()
        for lane in self.named_lanes:
            if re.fullmatch(r"[a-z0-9]+(?:[._-][a-z0-9]+)*", lane.id) is None:
                errors.append(f"named lane id is not canonical: {lane.id!r}")
            if lane.id in seen_ids:
                errors.append(f"{lane.id}: named lane ids must be unique")
            seen_ids.add(lane.id)
            for field in REQUIRED_NAMED_LANE_FIELDS:
                if field not in lane.data:
                    errors.append(f"{lane.id}: missing {field}")
            extra = (
                set(lane.data)
                - set(REQUIRED_NAMED_LANE_FIELDS)
                - {
                    "id",
                    "cargo_output_lifetime",
                    "cargo_native_units",
                    "cargo_build_python",
                    "differential_cases",
                    "pytest",
                }
            )
            if extra:
                errors.append(f"{lane.id}: unknown named lane fields {sorted(extra)!r}")
            errors.extend(
                pytest_declaration_errors(lane.id, lane.data, listings=listings)
            )
            if "pytest" in lane.data and "differential_cases" in lane.data:
                errors.append(
                    f"{lane.id}: a named lane declares pytest targets or "
                    "differential cases, not both"
                )
            try:
                cargo_native_units(lane.data)
                cargo_build_python(lane.data)
            except ValueError as exc:
                errors.append(f"{lane.id}: {exc}")
            if lane.data.get("cache_domain") not in {"none", "cargo-workspace"}:
                errors.append(f"{lane.id}: invalid Cargo cache domain")
            cases = lane.data.get("differential_cases")
            if cases is not None and (
                not isinstance(cases, list)
                or not cases
                or any(
                    not isinstance(case, str)
                    or not case
                    or case.startswith("-")
                    or Path(case).is_absolute()
                    or not case.endswith(".py")
                    for case in cases
                )
                or len(set(cases)) != len(cases)
                or lane.command_prefix.count("tests/molt_diff.py") != 1
            ):
                errors.append(
                    f"{lane.id}: differential_cases requires unique relative Python "
                    "files and the tests/molt_diff.py command"
                )
            argv = lane.argv
            # Named recipes currently own Python payloads, whose downstream
            # Cargo consumers are not statically closed by this declaration.
            if lane.data.get("cargo_output_lifetime", "retain") != "retain":
                errors.append(
                    f"{lane.id}: Python named lanes must retain Cargo outputs"
                )
            raw_argv = lane.data.get("argv")
            if (
                not isinstance(raw_argv, list)
                or not argv
                or any(not isinstance(value, str) or not value for value in raw_argv)
            ):
                errors.append(f"{lane.id}: argv must be a non-empty list of strings")
            if argv in seen_argvs:
                errors.append(f"{lane.id}: named lane argv duplicates another lane")
            seen_argvs.add(argv)
            if argv in command_argvs:
                errors.append(
                    f"{lane.id}: named lane argv duplicates a CI proof command"
                )
            toolchains = lane.toolchains
            if not toolchains or not isinstance(lane.data.get("toolchains"), list):
                errors.append(f"{lane.id}: toolchains must be a non-empty list")
            for name in toolchains:
                if name not in policy_names:
                    errors.append(f"{lane.id}: unknown toolchain {name!r}")
            if "python" not in toolchains:
                errors.append(
                    f"{lane.id}: named lanes are Python payloads; declare python"
                )
            timeout = lane.data.get("timeout_seconds")
            if (
                not isinstance(timeout, int)
                or isinstance(timeout, bool)
                or timeout <= 0
            ):
                errors.append(f"{lane.id}: timeout_seconds must be a positive integer")
            for key in ("resource_family", "contention_key", "description"):
                value = lane.data.get(key)
                if not isinstance(value, str) or not value:
                    errors.append(f"{lane.id}: {key} must be a non-empty string")

    def validate(self) -> list[str]:
        errors: list[str] = []
        if self.receipt_schema != proof_receipts.RECEIPT_SCHEMA:
            errors.append(f"receipt_schema must be {proof_receipts.RECEIPT_SCHEMA!r}")
        # One directory listing per checkout directory for every declared target.
        listings: dict[Path, frozenset[str]] = {}
        self._validate_named_lanes(errors, listings)
        if self.executor_max_workers <= 0:
            errors.append("executor_max_workers must be positive")
        if not 1 <= self.inventory_hash_workers <= 32:
            errors.append("inventory_hash_workers must be between 1 and 32")
        resource_names = [policy.name for policy in self.resource_policies]
        if not resource_names or any(not name for name in resource_names):
            errors.append("resource policies must have non-empty names")
        if len(resource_names) != len(set(resource_names)):
            errors.append("resource policy names must be unique")
        for policy in self.resource_policies:
            if policy.max_parallel <= 0:
                errors.append(f"{policy.name}: max_parallel must be positive")
            elif policy.max_parallel > self.executor_max_workers:
                errors.append(
                    f"{policy.name}: max_parallel exceeds executor_max_workers"
                )
        authority_inputs = list(self.authority_inputs)
        if not authority_inputs:
            errors.append("authority_inputs must declare the executable closure")
        if len(authority_inputs) != len(set(authority_inputs)):
            errors.append("authority_inputs must be unique")
        for relative in authority_inputs:
            normalized = _normalize_path(relative)
            if (
                normalized != relative
                or Path(relative).is_absolute()
                or ".." in Path(relative).parts
            ):
                errors.append(f"authority input is not canonical: {relative!r}")
                continue
            if not (ROOT / relative).is_file():
                errors.append(f"authority input does not exist: {relative!r}")
        try:
            manifest_relative = self.path.resolve().relative_to(ROOT).as_posix()
        except ValueError:
            manifest_relative = ""
        if manifest_relative and manifest_relative not in authority_inputs:
            errors.append("authority_inputs must include the proof-plan manifest")

        policy_names = [policy.name for policy in self.toolchain_policies]
        if not policy_names or any(not name for name in policy_names):
            errors.append("toolchain policies must have non-empty names")
        if len(policy_names) != len(set(policy_names)):
            errors.append("toolchain policy names must be unique")
        dependency_graph: dict[str, tuple[str, ...]] = {}
        for policy in self.toolchain_policies:
            if policy.identity_kind == "group":
                if any(
                    field in policy.data
                    for field in ("executable", "identity_provider", "version_args")
                ):
                    errors.append(
                        f"{policy.name}: a selection group must not declare an image provider"
                    )
                try:
                    toolchain_closure(self.toolchain_policies, (policy.name,))
                except ValueError as exc:
                    errors.append(str(exc))
                continue
            if "identity_kind" in policy.data:
                errors.append(
                    f"{policy.name}: toolchain policy must not declare identity_kind; "
                    "identity_provider is the target-derived discriminator"
                )
            for field in REQUIRED_TOOLCHAIN_FIELDS:
                if field not in policy.data:
                    errors.append(f"{policy.name}: missing toolchain field {field}")
            projection = policy.data.get("projection")
            if projection is not None and projection not in TOOLCHAIN_PROJECTIONS:
                errors.append(
                    f"{policy.name}: projection must be one of "
                    f"{sorted(TOOLCHAIN_PROJECTIONS)!r}"
                )
            if projection in RELEASE_PROJECTIONS and (
                _tool_release_kinds(policy.root).get(str(policy.data.get("executable")))
                != projection
            ):
                errors.append(
                    f"{policy.name}: a {projection} projection needs its executable "
                    f"in config/tool_releases.toml as a "
                    f"{'runtime' if projection == 'runtime-tool-release' else 'native'} tool"
                )
            identity_kind = policy.identity_kind
            if identity_kind == "executable":
                executable = policy.data.get("executable")
                if not isinstance(executable, str) or not executable:
                    errors.append(
                        f"{policy.name}: executable identity requires a non-empty executable"
                    )
                args = policy.data.get("version_args")
                if (
                    not isinstance(args, list)
                    or not args
                    or not all(isinstance(item, str) and item for item in args)
                ):
                    errors.append(
                        f"{policy.name}: executable identity requires non-empty version_args"
                    )
                setup_value = policy.data.get("setup_value")
                if not isinstance(setup_value, str) or not setup_value:
                    errors.append(
                        f"{policy.name}: executable identity requires non-empty setup_value"
                    )
            else:
                for field in ("executable", "version_args", "setup_value"):
                    if field in policy.data:
                        errors.append(
                            f"{policy.name}: target-derived identity must not declare {field}"
                        )
                provider = policy.data.get("identity_provider")
                if provider not in TARGET_DERIVED_IDENTITY_PROVIDERS:
                    errors.append(
                        f"{policy.name}: target-derived identity_provider must be one of "
                        f"{sorted(TARGET_DERIVED_IDENTITY_PROVIDERS)!r}"
                    )
            selector = policy.data.get("compiler_tool_selector")
            if selector is not None and (
                identity_kind != "executable"
                or {
                    "native-strip": "llvm-strip",
                    "wasm-opt": "wasm-opt",
                    "llvm-sdk": "llvm-config",
                    "build-python": "build-python",
                }.get(selector)
                != policy.data.get("executable")
                or policy.data.get("wasi_sdk_tool") is not None
            ):
                errors.append(f"{policy.name}: invalid compiler_tool_selector")
            sdk_role = policy.data.get("wasi_sdk_tool")
            if sdk_role is not None and (
                identity_kind != "executable"
                or sdk_role not in {"clang", "clang++"}
                or policy.data.get("executable") != sdk_role
            ):
                errors.append(
                    f"{policy.name}: wasi_sdk_tool requires the matching selected SDK compiler executable"
                )
            probe_cwd = policy.data.get("probe_cwd", ".")
            if not isinstance(probe_cwd, str) or not probe_cwd:
                errors.append(f"{policy.name}: probe_cwd must be a non-empty string")
            else:
                normalized_probe_cwd = _normalize_path(probe_cwd)
                probe_path = Path(normalized_probe_cwd)
                if (
                    normalized_probe_cwd != probe_cwd
                    or probe_path.is_absolute()
                    or ".." in probe_path.parts
                ):
                    errors.append(
                        f"{policy.name}: probe_cwd must be a canonical "
                        f"repository-relative directory: {probe_cwd!r}"
                    )
                else:
                    try:
                        resolved_probe_cwd = (ROOT / probe_path).resolve(strict=True)
                        resolved_probe_cwd.relative_to(ROOT.resolve())
                    except (OSError, ValueError):
                        errors.append(
                            f"{policy.name}: probe_cwd must resolve inside the "
                            f"repository: {probe_cwd!r}"
                        )
                    else:
                        if not resolved_probe_cwd.is_dir():
                            errors.append(
                                f"{policy.name}: probe_cwd is not a directory: "
                                f"{probe_cwd!r}"
                            )
            try:
                python_role = policy.python_identity_role
            except ValueError as exc:
                errors.append(str(exc))
                python_role = None
            if (
                python_role == "environment"
                and policy.data.get("executable") != "{python}"
            ):
                errors.append(
                    f"{policy.name}: Python environment role requires the command interpreter"
                )
            if python_role == "runtime" and (
                not policy.data.get("selection_only")
                or not policy.data.get("version_from_content_path")
                or (
                    policy.data.get("compiler_tool_selector") != "build-python"
                    and (
                        not isinstance(policy.data.get("content_path_command"), list)
                        or not isinstance(policy.data.get("python_version"), str)
                        or re.fullmatch(r"3\.[0-9]+", policy.data["python_version"])
                        is None
                    )
                )
            ):
                errors.append(
                    f"{policy.name}: Python runtime role requires an explicit typed isolated selection"
                )
            content_path_command = policy.data.get("content_path_command")
            selected_version = policy.data.get("version_from_content_path", False)
            if type(selected_version) is not bool or (
                selected_version
                and content_path_command is None
                and selector != "build-python"
            ):
                errors.append(
                    f"{policy.name}: selected version requires a typed content resolver"
                )
            package = policy.data.get("node_package")
            explicit_selection = policy.data.get("selection_only", False)
            # Selection controls bare-launcher inference, not image identity.
            # Ordinary executables already resolve/hash their selected PATH image;
            # runtime views and selected versions keep their own resolver checks.
            if type(explicit_selection) is not bool:
                errors.append(f"{policy.name}: selection_only must be a boolean")
            if package is not None and (
                not isinstance(package, str)
                or not package
                or policy.data.get("node_package_mode", "require")
                not in {"require", "import"}
            ):
                errors.append(f"{policy.name}: invalid node package selection")
            elif package is None and "node_package_mode" in policy.data:
                errors.append(f"{policy.name}: node package mode requires node_package")
            if content_path_command is not None and (
                not isinstance(content_path_command, list)
                or not content_path_command
                or not all(
                    isinstance(item, str) and item for item in content_path_command
                )
            ):
                errors.append(
                    f"{policy.name}: content_path_command must be a non-empty string list"
                )
            process_image_probes = policy.data.get("process_image_probes", [])
            if (
                not isinstance(process_image_probes, list)
                or not all(
                    isinstance(probe, list)
                    and probe
                    and all(isinstance(item, str) and item for item in probe)
                    for probe in process_image_probes
                )
                or len({tuple(probe) for probe in process_image_probes})
                != len(process_image_probes)
            ):
                errors.append(
                    f"{policy.name}: process_image_probes must be unique "
                    "non-empty string lists"
                )
            dependencies = policy.data.get("dependencies", [])
            if (
                not isinstance(dependencies, list)
                or not all(isinstance(item, str) and item for item in dependencies)
                or len(dependencies) != len(set(dependencies))
            ):
                errors.append(
                    f"{policy.name}: dependencies must be a unique string list"
                )
                dependency_graph[policy.name] = ()
            else:
                dependency_graph[policy.name] = tuple(dependencies)
                for dependency in dependencies:
                    if dependency not in policy_names:
                        errors.append(
                            f"{policy.name}: unknown toolchain dependency {dependency!r}"
                        )
            linker_process_helpers = policy.data.get("linker_process_helpers")
            if linker_process_helpers is not None:
                if not isinstance(linker_process_helpers, dict):
                    errors.append(
                        f"{policy.name}: linker_process_helpers must be a table"
                    )
                else:
                    for linker, helpers in linker_process_helpers.items():
                        if (
                            not isinstance(linker, str)
                            or not linker
                            or Path(linker).name != linker
                        ):
                            errors.append(
                                f"{policy.name}: linker helper key must be a basename"
                            )
                            continue
                        if (
                            not isinstance(helpers, list)
                            or not helpers
                            or not all(
                                isinstance(helper, str)
                                and helper
                                and Path(helper).name == helper
                                for helper in helpers
                            )
                            or len({helper.casefold() for helper in helpers})
                            != len(helpers)
                        ):
                            errors.append(
                                f"{policy.name}: linker helpers for {linker!r} "
                                "must be unique basenames"
                            )
            linker_build_tools = policy.data.get("linker_build_tools")
            if linker_build_tools is not None:
                if not isinstance(linker_build_tools, dict):
                    errors.append(f"{policy.name}: linker_build_tools must be a table")
                else:
                    for linker, tools in linker_build_tools.items():
                        if (
                            not isinstance(linker, str)
                            or not linker
                            or Path(linker).name != linker
                            or not isinstance(tools, dict)
                            or not tools
                        ):
                            errors.append(
                                f"{policy.name}: linker build-tool entry must map a "
                                "linker basename to a non-empty table"
                            )
                            continue
                        folded_tools: set[str] = set()
                        for tool, role in tools.items():
                            if (
                                not isinstance(tool, str)
                                or not tool
                                or Path(tool).name != tool
                                or tool.casefold() in folded_tools
                                or not isinstance(role, str)
                                or re.fullmatch(r"rust-build-[a-z][a-z-]*", role)
                                is None
                            ):
                                errors.append(
                                    f"{policy.name}: linker build tools for {linker!r} "
                                    "must map unique basenames to typed rust-build roles"
                                )
                                break
                            folded_tools.add(tool.casefold())
            fingerprint_domain = policy.data.get("fingerprint_domain")
            if fingerprint_domain is not None and (
                not isinstance(fingerprint_domain, str) or not fingerprint_domain
            ):
                errors.append(
                    f"{policy.name}: fingerprint_domain must be a non-empty string"
                )
            pattern = policy.data.get("version_pattern")
            if not isinstance(pattern, str) or not pattern:
                errors.append(f"{policy.name}: version_pattern must be non-empty")
            else:
                try:
                    re.compile(pattern)
                except re.error as exc:
                    errors.append(f"{policy.name}: invalid version pattern: {exc}")
                setup_value = policy.data.get("setup_value")
                # An exact pin and the identity pattern move together.
                if (
                    isinstance(setup_value, str)
                    and re.fullmatch(r"\d+(?:\.\d+)+", setup_value)
                    and re.escape(setup_value) not in pattern
                ):
                    errors.append(
                        f"{policy.name}: version_pattern {pattern!r} does not "
                        f"pin setup_value {setup_value!r}"
                    )
            evidence = policy.data.get("setup_evidence")
            if not isinstance(evidence, list) or not evidence:
                errors.append(f"{policy.name}: setup_evidence must be non-empty")
            elif all(isinstance(item, str) for item in evidence):
                for item in evidence:
                    if "::" not in item:
                        errors.append(
                            f"{policy.name}: setup evidence must be PATH::TOKEN"
                        )
                        continue
                    relative, token = item.split("::", 1)
                    evidence_path = ROOT / relative
                    if not evidence_path.is_file():
                        errors.append(
                            f"{policy.name}: setup evidence file missing: {relative}"
                        )
                    elif token not in evidence_path.read_text(encoding="utf-8"):
                        errors.append(
                            f"{policy.name}: setup evidence token missing from {relative}"
                        )
        visited_dependencies: set[str] = set()
        visiting_dependencies: set[str] = set()

        def visit_toolchain_dependencies(name: str) -> None:
            if name in visiting_dependencies:
                errors.append(f"toolchain dependency cycle includes {name!r}")
                return
            if name in visited_dependencies:
                return
            visiting_dependencies.add(name)
            for dependency in dependency_graph.get(name, ()):
                if dependency in dependency_graph:
                    visit_toolchain_dependencies(dependency)
            visiting_dependencies.remove(name)
            visited_dependencies.add(name)

        for policy_name in policy_names:
            visit_toolchain_dependencies(policy_name)
        cell_ids = [cell.id for cell in self.matrix_cells]
        if not cell_ids or any(not cell_id for cell_id in cell_ids):
            errors.append("matrix_cell IDs must be non-empty")
        if len(cell_ids) != len(set(cell_ids)):
            errors.append("matrix_cell IDs must be unique")
        for cell in self.matrix_cells:
            for field in (
                "runner",
                "os",
                "arch",
                "python",
                "backend",
                "target",
                "profile",
            ):
                if not isinstance(cell.data.get(field), str) or not cell.data[field]:
                    errors.append(f"{cell.id}: matrix cell missing non-empty {field}")
        names = [family.name for family in self.families]
        if not names:
            errors.append("at least one [[ci_family]] is required")
        if len(names) != len(set(names)):
            errors.append("ci_family names must be unique")
        for family in (*self.families, *self.scheduled_families):
            models_job = isinstance(family, ScheduledFamily) or family.data.get(
                "executor"
            ) in {"github-job", "github-matrix"}
            reserve = family.data.get("job_reserve_seconds")
            if models_job:
                if type(reserve) is not int or reserve <= 0:
                    errors.append(
                        f"{family.name}: job_reserve_seconds must be a positive integer"
                    )
            elif "job_reserve_seconds" in family.data:
                errors.append(
                    f"{family.name}: job_reserve_seconds requires a modeled job"
                )
            tiers = family.data.get("tiers")
            if not isinstance(tiers, list) or not set(tiers) <= set(PROOF_TIERS):
                errors.append(
                    f"{family.name}: tiers {tiers!r} must come from "
                    f"{list(PROOF_TIERS)!r}"
                )
        for family in self.families:
            if not family.name:
                errors.append("ci_family.name must be non-empty")
            for field in REQUIRED_FAMILY_FIELDS:
                if field not in family.data:
                    errors.append(f"{family.name}: missing {field}")
            executor = family.data.get("executor")
            if executor not in {"github-job", "github-matrix", "github-workflow"}:
                errors.append(f"{family.name}: unknown executor {executor!r}")
            if family.data.get("resource_class") not in set(resource_names):
                errors.append(
                    f"{family.name}: unknown resource class "
                    f"{family.data.get('resource_class')!r}"
                )
            if not family.data.get("required"):
                errors.append(
                    f"{family.name}: selected proof families must be required"
                )
            workflow = ROOT / str(family.data.get("workflow", ""))
            if not workflow.is_file():
                errors.append(f"{family.name}: workflow does not exist: {workflow}")
            elif family.data.get("executor") in {"github-job", "github-matrix"}:
                job = str(family.data.get("job", ""))
                workflow_text = workflow.read_text(encoding="utf-8")
                block = _workflow_job_block(workflow_text, job)
                if block is None:
                    errors.append(f"{family.name}: workflow job {job!r} is missing")
                else:
                    invocation = f"--run-family {family.name} --receipt"
                    if invocation not in block:
                        errors.append(
                            f"{family.name}: workflow job does not execute {invocation!r}"
                        )
                    for receipt_token in (
                        "actions/upload-artifact@",
                        "if-no-files-found: error",
                    ):
                        if receipt_token not in block:
                            errors.append(
                                f"{family.name}: workflow job does not enforce "
                                f"receipt upload token {receipt_token!r}"
                            )
                    if "continue-on-error" in block:
                        errors.append(
                            f"{family.name}: claimed correctness job may not continue-on-error"
                        )
                    timeout = f"timeout-minutes: {family.data['timeout_minutes']}"
                    if timeout not in block:
                        errors.append(
                            f"{family.name}: workflow job does not enforce {timeout!r}"
                        )
                    if family.data.get("executor") == "github-matrix":
                        # Each matrix job consumes only its own family's cells.
                        for token in (
                            "matrix: ${{ fromJSON(needs.classify-changes.outputs."
                            f"{family_matrix_output(family.name)}) }}}}",
                            "runs-on: ${{ matrix.runner }}",
                            '--matrix-cell "${{ matrix.cell }}"',
                        ):
                            if token not in block:
                                errors.append(
                                    f"{family.name}: matrix workflow job does not "
                                    f"contain {token!r}"
                                )
                    else:
                        runners = {
                            cell.data.get("runner")
                            for cell in self.matrix_cells
                            if cell.id in self.family_cells(family.name)
                        }
                        if len(runners) > 1:
                            errors.append(
                                f"{family.name}: a github-job runs every command on "
                                f"one runner, but its cells name {sorted(runners)!r}; "
                                "use executor github-matrix"
                            )
            admission_workflow_name = str(family.data.get("admission_workflow", ""))
            admission_workflow = ROOT / admission_workflow_name
            admission_job = str(family.data.get("admission_job", ""))
            admission_needs = family.data.get("admission_needs")
            if admission_workflow_name != ".github/workflows/ci.yml":
                errors.append(
                    f"{family.name}: admission must join the canonical CI verdict "
                    "workflow"
                )
            if re.fullmatch(r"[A-Za-z0-9_-]+", admission_job) is None:
                errors.append(
                    f"{family.name}: admission_job must be a literal job name"
                )
            if not isinstance(admission_needs, list) or not all(
                isinstance(dependency, str) and dependency
                for dependency in admission_needs
            ):
                errors.append(
                    f"{family.name}: admission_needs must be a list of job names"
                )
            elif len(admission_needs) != len(set(admission_needs)):
                errors.append(f"{family.name}: admission_needs must be unique")
            if not admission_workflow.is_file():
                errors.append(
                    f"{family.name}: admission workflow does not exist: "
                    f"{admission_workflow}"
                )
            else:
                admission_text = admission_workflow.read_text(encoding="utf-8")
                if isinstance(admission_needs, list):
                    for dependency in admission_needs:
                        if _workflow_job_block(admission_text, dependency) is None:
                            errors.append(
                                f"{family.name}: admission dependency "
                                f"{dependency!r} is not a workflow job"
                            )
                admission_block = _workflow_job_block(admission_text, admission_job)
                if admission_block is None:
                    errors.append(
                        f"{family.name}: admission job {admission_job!r} is missing"
                    )
                else:
                    try:
                        actual_admission_needs = _workflow_job_needs(admission_block)
                    except ValueError as exc:
                        errors.append(f"{family.name}: {exc}")
                    else:
                        expected_admission_needs = (
                            tuple(admission_needs)
                            if isinstance(admission_needs, list)
                            else ()
                        )
                        if actual_admission_needs != expected_admission_needs:
                            errors.append(
                                f"{family.name}: admission job {admission_job!r} "
                                f"needs {list(actual_admission_needs)!r}; authority "
                                f"requires {list(expected_admission_needs)!r}"
                            )
                    if "continue-on-error" in admission_block:
                        errors.append(
                            f"{family.name}: claimed correctness admission may not "
                            "continue-on-error"
                        )
        scheduled_names = [family.name for family in self.scheduled_families]
        if len(scheduled_names) != len(set(scheduled_names)):
            errors.append("scheduled_family names must be unique")
        overlap = set(names).intersection(scheduled_names)
        if overlap:
            errors.append(
                f"CI and scheduled families must be disjoint: {sorted(overlap)!r}"
            )
        for family in self.scheduled_families:
            if re.fullmatch(r"[a-z0-9]+(?:_[a-z0-9]+)*", family.name) is None:
                errors.append(f"{family.name}: scheduled family name is not canonical")
            for field in REQUIRED_SCHEDULED_FAMILY_FIELDS:
                if field not in family.data:
                    errors.append(f"{family.name}: missing {field}")
            if family.data.get("resource_class") not in set(resource_names):
                errors.append(
                    f"{family.name}: unknown resource class "
                    f"{family.data.get('resource_class')!r}"
                )
            tiers = family.data.get("tiers")
            if tiers != [SCHEDULED_TIER]:
                errors.append(
                    f"{family.name}: scheduled family tiers must be "
                    f"[{SCHEDULED_TIER!r}]"
                )
            timeout_minutes = family.data.get("timeout_minutes")
            if not isinstance(timeout_minutes, int) or timeout_minutes <= 0:
                errors.append(
                    f"{family.name}: timeout_minutes must be a positive integer"
                )
            sharded_program = family.data.get("sharded_program")
            shard_count = family.data.get("shard_count")
            shard_timeout = family.data.get("shard_timeout_minutes")
            shard_overhead = family.data.get("shard_overhead_seconds")
            if sharded_program is None:
                if shard_count is not None:
                    errors.append(
                        f"{family.name}: shard_count requires sharded_program"
                    )
                if shard_timeout is not None or shard_overhead is not None:
                    errors.append(
                        f"{family.name}: shard budgets require sharded_program"
                    )
            elif (
                not isinstance(shard_timeout, int)
                or isinstance(shard_timeout, bool)
                or shard_timeout <= 0
                or not isinstance(shard_overhead, int)
                or isinstance(shard_overhead, bool)
                or not 0 < shard_overhead < shard_timeout * 60
            ):
                errors.append(
                    f"{family.name}: shard_timeout_minutes and shard_overhead_seconds "
                    "must leave a positive shard deadline"
                )
            elif (
                sharded_program not in {"conformance", "differential", "regrtest"}
                or not isinstance(shard_count, int)
                or isinstance(shard_count, bool)
                or shard_count <= 1
            ):
                errors.append(f"{family.name}: invalid sharded program/count contract")
            item_timeout = family.data.get("item_timeout_seconds")
            if item_timeout is not None and (
                sharded_program is None
                or not isinstance(item_timeout, int)
                or isinstance(item_timeout, bool)
                or not isinstance(shard_timeout, int)
                or not isinstance(shard_overhead, int)
                or not 0 < item_timeout < shard_timeout * 60 - shard_overhead
            ):
                errors.append(
                    f"{family.name}: item_timeout_seconds must be a positive "
                    "per-item bound inside the shard deadline"
                )
            workflow = ROOT / str(family.data.get("workflow", ""))
            if not workflow.is_file():
                errors.append(f"{family.name}: workflow does not exist: {workflow}")
                continue
            job = str(family.data.get("job", ""))
            workflow_text = workflow.read_text(encoding="utf-8")
            block = _workflow_job_block(workflow_text, job)
            if block is None:
                errors.append(f"{family.name}: workflow job {job!r} is missing")
                continue
            invocation = f"--run-family {family.name} --receipt"
            if invocation not in block:
                errors.append(
                    f"{family.name}: workflow job does not execute {invocation!r}"
                )
            for token in ("actions/upload-artifact@", "if-no-files-found: error"):
                if token not in block:
                    errors.append(
                        f"{family.name}: workflow job does not enforce {token!r}"
                    )
            if "continue-on-error" in block:
                errors.append(
                    f"{family.name}: scheduled correctness job may not continue-on-error"
                )
            declared_needs = family.data.get("needs")
            if declared_needs is not None:
                if not isinstance(declared_needs, list) or not all(
                    isinstance(need, str) for need in declared_needs
                ):
                    errors.append(f"{family.name}: needs must be a list of job names")
                else:
                    try:
                        actual_needs = _workflow_job_needs(block)
                    except ValueError as exc:
                        errors.append(f"{family.name}: {exc}")
                    else:
                        if actual_needs != tuple(declared_needs):
                            errors.append(
                                f"{family.name}: workflow needs "
                                f"{list(actual_needs)!r}; authority requires "
                                f"{declared_needs!r}"
                            )
            timeout = f"timeout-minutes: {timeout_minutes}"
            if timeout not in block:
                errors.append(
                    f"{family.name}: workflow job does not enforce {timeout!r}"
                )
            if isinstance(sharded_program, str) and isinstance(shard_count, int):
                shard_job = f"{sharded_program}-shard"
                shard_block = _workflow_job_block(workflow_text, shard_job)
                if shard_block is None:
                    errors.append(f"{family.name}: shard job {shard_job!r} is missing")
                else:
                    for token in (
                        "needs: nightly-prepare",
                        f"timeout-minutes: {shard_timeout}",
                        f"max-parallel: {shard_count}",
                        f"outputs.{sharded_program}_matrix",
                        "tools/nightly_runtime_bundle.py verify-extract",
                        'MOLT_STDLIB_PROFILE: "full"',
                        "tools/nightly_sharding.py run-shard",
                        f"--program {sharded_program}",
                        "--shard '${{ matrix.shard }}'",
                        "if: always()",
                        "if-no-files-found: error",
                    ):
                        if token not in shard_block:
                            errors.append(f"{family.name}: shard job omits {token!r}")
                    if (
                        sharded_program == "differential"
                        and 'MOLT_DIFF_STDLIB_PROFILE: "full"' not in shard_block
                    ):
                        errors.append(
                            f"{family.name}: differential shards must consume the "
                            "shared full runtime profile"
                        )
                    if "continue-on-error" in shard_block:
                        errors.append(
                            f"{family.name}: shard job may not continue-on-error"
                        )
                try:
                    aggregate_needs = _workflow_job_needs(block)
                except ValueError as exc:
                    errors.append(f"{family.name}: {exc}")
                else:
                    expected_needs = ("nightly-prepare", shard_job)
                    if aggregate_needs != expected_needs:
                        errors.append(
                            f"{family.name}: aggregate needs {list(aggregate_needs)!r}; "
                            f"authority requires {list(expected_needs)!r}"
                        )
                if "if: always()" not in block:
                    errors.append(
                        f"{family.name}: aggregate must run after failed shards"
                    )
        known = set(names)
        dependency_graph: dict[str, tuple[str, ...]] = {}
        for family in self.families:
            dependencies = family.data.get("dependencies", [])
            if not isinstance(dependencies, list) or not all(
                isinstance(dependency, str) for dependency in dependencies
            ):
                errors.append(f"{family.name}: dependencies must be a list of names")
                continue
            dependency_graph[family.name] = tuple(dependencies)
            unknown = set(dependencies) - known
            if unknown:
                errors.append(
                    f"{family.name}: unknown dependencies {sorted(unknown)!r}"
                )
        visiting: list[str] = []
        visited: set[str] = set()

        def visit(name: str) -> None:
            if name in visited:
                return
            if name in visiting:
                start = visiting.index(name)
                cycle = (*visiting[start:], name)
                errors.append(f"dependency cycle: {' -> '.join(cycle)}")
                return
            visiting.append(name)
            for dependency in dependency_graph.get(name, ()):
                if dependency in dependency_graph:
                    visit(dependency)
            visiting.pop()
            visited.add(name)

        for name in names:
            visit(name)

        command_ids = [command.id for command in self.commands]
        if not command_ids or any(not command_id for command_id in command_ids):
            errors.append("command IDs must be non-empty")
        if len(command_ids) != len(set(command_ids)):
            errors.append("command IDs must be unique")
        known_commands = set(command_ids)
        known_cells = set(cell_ids)
        known_toolchains = set(policy_names)
        toolchain_identity_kinds = {
            policy.name: policy.identity_kind for policy in self.toolchain_policies
        }
        known_resources = set(resource_names)
        execution_families = {
            **{family.name: family.data for family in self.families},
            **{family.name: family.data for family in self.scheduled_families},
        }
        commands_by_family: dict[str, int] = {name: 0 for name in execution_families}
        referenced_cells: set[str] = set()
        command_shapes: set[tuple[str, tuple[str, ...], str]] = set()
        command_graph: dict[str, tuple[str, ...]] = {}
        evidence_owners: dict[str, str] = {}
        for command in self.commands:
            if re.fullmatch(r"[a-z0-9]+(?:[.-][a-z0-9]+)*", command.id) is None:
                errors.append(f"{command.id}: command ID is not canonical")
            for field in REQUIRED_COMMAND_FIELDS:
                if field not in command.data:
                    errors.append(f"{command.id}: missing {field}")
            if command.family not in execution_families:
                errors.append(f"{command.id}: unknown family {command.family!r}")
            else:
                commands_by_family[command.family] += 1
            resource_class = command.data.get("resource_class")
            if resource_class not in known_resources:
                errors.append(
                    f"{command.id}: unknown resource class {resource_class!r}"
                )
            if command.data.get("cell") not in known_cells:
                errors.append(
                    f"{command.id}: unknown matrix cell {command.data.get('cell')!r}"
                )
            else:
                referenced_cells.add(str(command.data["cell"]))
            argv = command.data.get("argv")
            if (
                not isinstance(argv, list)
                or not argv
                or not all(isinstance(part, str) and part for part in argv)
            ):
                errors.append(f"{command.id}: argv must be a non-empty string list")
            else:
                for part in argv:
                    if part == PROOF_SOURCE_COMMIT_PLACEHOLDER:
                        continue
                    if PROOF_SOURCE_COMMIT_PLACEHOLDER in part:
                        errors.append(
                            f"{command.id}: {PROOF_SOURCE_COMMIT_PLACEHOLDER!r} "
                            f"must be a whole argv element: {part!r}"
                        )
                    elif _ARGV_PLACEHOLDER.fullmatch(part):
                        errors.append(
                            f"{command.id}: unknown argv placeholder {part!r}"
                        )
                if command.binds_source_commit:
                    # A source-bound command attests one commit. A developer
                    # checkout need not be clean, and an unretained receipt
                    # attests nothing, so it runs only in CI tiers and keeps
                    # its output as declared evidence.
                    if "pre-push" in (command.data.get("tiers") or ()):
                        errors.append(
                            f"{command.id}: a source-bound command cannot run "
                            "in the pre-push tier"
                        )
                    if not command.data.get("evidence_outputs"):
                        errors.append(
                            f"{command.id}: a source-bound command must declare "
                            "evidence_outputs"
                        )
            try:
                cargo_native_units(command.data)
                cargo_build_python(command.data)
            except ValueError as exc:
                errors.append(f"{command.id}: {exc}")
            errors.extend(
                pytest_declaration_errors(command.id, command.data, listings=listings)
            )
            toolchains = command.data.get("toolchains")
            if (
                not isinstance(toolchains, list)
                or not toolchains
                or not all(isinstance(name, str) and name for name in toolchains)
            ):
                errors.append(f"{command.id}: toolchains must be a non-empty list")
            elif len(toolchains) != len(set(toolchains)):
                errors.append(f"{command.id}: toolchains must be unique")
            elif unknown_toolchains := set(toolchains) - known_toolchains:
                errors.append(
                    f"{command.id}: unknown toolchains {sorted(unknown_toolchains)!r}"
                )
            else:
                target_derived_toolchains = sorted(
                    name
                    for name in toolchains
                    if toolchain_identity_kinds[name] == "target-derived"
                )
                if target_derived_toolchains:
                    errors.append(
                        f"{command.id}: static command cannot use target-derived "
                        f"toolchains {target_derived_toolchains!r}; typed target "
                        "context is required"
                    )
            timeout_env = command.data.get("timeout_env", [])
            if not isinstance(timeout_env, list) or not all(
                isinstance(name, str) and name for name in timeout_env
            ):
                errors.append(f"{command.id}: timeout_env must be a string list")
            elif len(timeout_env) != len(set(timeout_env)):
                errors.append(f"{command.id}: timeout_env must be unique")
            environment = command.data.get("env", {})
            if not isinstance(environment, dict) or not all(
                isinstance(name, str) and name and isinstance(value, str)
                for name, value in environment.items()
            ):
                errors.append(f"{command.id}: env must be a string map")
            elif PROOF_COMMAND_TIMEOUT_ENV in environment:
                errors.append(
                    f"{command.id}: env cannot override {PROOF_COMMAND_TIMEOUT_ENV}"
                )
            evidence_outputs = command.data.get("evidence_outputs", [])
            if not isinstance(evidence_outputs, list) or not all(
                isinstance(relative, str) and relative for relative in evidence_outputs
            ):
                errors.append(f"{command.id}: evidence_outputs must be a string list")
            elif len(evidence_outputs) != len(set(evidence_outputs)):
                errors.append(f"{command.id}: evidence_outputs must be unique")
            else:
                for relative in evidence_outputs:
                    try:
                        portable_relative_path(relative)
                        canonical = relative.startswith(
                            ("proof-results/", "proof-receipts/evidence/")
                        )
                    except ValueError:
                        canonical = False
                    if not canonical:
                        errors.append(
                            f"{command.id}: evidence output is not a canonical "
                            f"ephemeral path: {relative!r}"
                        )
                        continue
                    for owned_path, owner in evidence_owners.items():
                        if (
                            relative == owned_path
                            or relative.startswith(f"{owned_path}/")
                            or owned_path.startswith(f"{relative}/")
                        ):
                            errors.append(
                                f"{command.id}: evidence output {relative!r} "
                                f"overlaps {owner}:{owned_path}"
                            )
                    evidence_owners[relative] = command.id
            timeout = command.data.get("timeout_seconds")
            if not isinstance(timeout, int) or timeout <= 0:
                errors.append(f"{command.id}: timeout_seconds must be positive")
            elif command.family in execution_families:
                family_data = execution_families[command.family]
                if timeout > int(family_data["timeout_minutes"]) * 60:
                    errors.append(
                        f"{command.id}: command timeout exceeds family job timeout"
                    )
                tiers = command.data.get("tiers")
                if not isinstance(tiers, list) or not tiers:
                    errors.append(f"{command.id}: tiers must be a non-empty list")
                elif not set(tiers).issubset(set(family_data["tiers"])):
                    errors.append(f"{command.id}: command tiers escape family tiers")
            timeout_budget = command.data.get("timeout_budget")
            is_compiler_build = resource_class == "compiler-build-resource"
            if is_compiler_build and not isinstance(timeout_budget, str):
                errors.append(
                    f"{command.id}: compiler-build-resource requires timeout_budget"
                )
            elif timeout_budget is not None:
                if not isinstance(timeout_budget, str) or not timeout_budget:
                    errors.append(f"{command.id}: timeout_budget must be non-empty")
                if "cargo" not in command.toolchains:
                    errors.append(
                        f"{command.id}: timeout_budget requires the cargo toolchain"
                    )
            if isinstance(argv, list) and all(isinstance(part, str) for part in argv):
                for candidate in argv_repository_paths(argv):
                    if not (ROOT / candidate).exists():
                        errors.append(
                            f"{command.id}: repository input does not exist: {candidate!r}"
                        )
                shape = (
                    command.family,
                    command.argv,
                    str(command.data.get("cwd", ".")),
                    str(command.data.get("cell", "")),
                )
                if shape in command_shapes:
                    errors.append(f"{command.id}: duplicate executable command shape")
                command_shapes.add(shape)
            dependencies = command.data.get("dependencies")
            if not isinstance(dependencies, list) or not all(
                isinstance(dependency, str) for dependency in dependencies
            ):
                errors.append(f"{command.id}: dependencies must be a list of IDs")
                continue
            command_graph[command.id] = tuple(dependencies)
            unknown = set(dependencies) - known_commands
            if unknown:
                errors.append(
                    f"{command.id}: unknown command dependencies {sorted(unknown)!r}"
                )
        for family, count in commands_by_family.items():
            if count == 0:
                errors.append(f"{family}: selected family has no executable commands")
        # Every tier a family claims gates at least one of its commands in each
        # matrix cell; otherwise that event would run an empty, green job.
        for name, family_data in execution_families.items():
            members = [command for command in self.commands if command.family == name]
            cells = (
                sorted({str(command.data.get("cell")) for command in members})
                if family_data.get("executor") == "github-matrix"
                else [None]
            )
            for tier in family_data.get("tiers") or ():
                for cell in cells:
                    if not any(
                        tier in (command.data.get("tiers") or ())
                        and (cell is None or command.data.get("cell") == cell)
                        for command in members
                    ):
                        where = "" if cell is None else f" in matrix cell {cell!r}"
                        errors.append(f"{name}: tier {tier!r} gates no command{where}")
        # Declared evidence that never leaves the runner attests nothing: the
        # owning job must transport every output as a workflow artifact.
        scheduled_names = {family.name for family in self.scheduled_families}
        for family in (*self.families, *self.scheduled_families):
            outputs = tuple(
                output
                for command in self.commands
                if command.family == family.name
                for output in command.evidence_outputs
            )
            if not outputs:
                if family.name in scheduled_names:
                    errors.append(
                        f"{family.name}: scheduled family has no declared "
                        "evidence outputs"
                    )
                continue
            if "workflow" not in family.data or "job" not in family.data:
                errors.append(
                    f"{family.name}: family with evidence outputs has no "
                    "workflow job to transport them"
                )
                continue
            workflow = ROOT / str(family.data["workflow"])
            if not workflow.is_file():
                continue
            block = _workflow_job_block(
                workflow.read_text(encoding="utf-8"),
                str(family.data["job"]),
            )
            if block is None:
                continue
            for output in outputs:
                if output not in block:
                    errors.append(
                        f"{family.name}: workflow artifact transport omits {output!r}"
                    )
        used_resources = {
            str(command.data.get("resource_class", "")) for command in self.commands
        }
        for unused in sorted(known_resources - used_resources):
            errors.append(f"{unused}: resource policy has no executable command")
        for cell_id in sorted(known_cells - referenced_cells):
            errors.append(f"{cell_id}: matrix cell has no executable command")

        visiting_commands: list[str] = []
        visited_commands: set[str] = set()

        def visit_command(command_id: str) -> None:
            if command_id in visited_commands:
                return
            if command_id in visiting_commands:
                start = visiting_commands.index(command_id)
                cycle = (*visiting_commands[start:], command_id)
                errors.append(f"command dependency cycle: {' -> '.join(cycle)}")
                return
            visiting_commands.append(command_id)
            command = next(
                (item for item in self.commands if item.id == command_id), None
            )
            for dependency in command_graph.get(command_id, ()):
                dependency_command = next(
                    (item for item in self.commands if item.id == dependency), None
                )
                if (
                    command is not None
                    and dependency_command is not None
                    and command.family != dependency_command.family
                ):
                    errors.append(
                        f"{command_id}: command dependency {dependency!r} crosses families"
                    )
                if (
                    command is not None
                    and dependency_command is not None
                    and command.family == dependency_command.family
                ):
                    family = next(
                        (item for item in self.families if item.name == command.family),
                        None,
                    )
                    if (
                        family is not None
                        and family.data.get("executor") == "github-matrix"
                        and command.data.get("cell")
                        != dependency_command.data.get("cell")
                    ):
                        errors.append(
                            f"{command_id}: matrix command dependency {dependency!r} "
                            "crosses runner cells"
                        )
                visit_command(dependency)
            visiting_commands.pop()
            visited_commands.add(command_id)

        for command_id in command_ids:
            visit_command(command_id)

        if not any(error.startswith("command dependency cycle:") for error in errors):
            for family in self.families:
                executor = family.data.get("executor")
                if executor not in {"github-job", "github-matrix"}:
                    continue
                # One github-job runs every command; a matrix job runs one cell.
                cells = (
                    (None,)
                    if executor == "github-job"
                    else self.family_cells(family.name)
                )
                for cell in cells:
                    try:
                        envelope = self.timeout_envelope(family.name, matrix_cell=cell)
                    except (KeyError, TypeError, ValueError) as exc:
                        errors.append(str(exc))
                        continue
                    job_budget = int(family.data["timeout_minutes"]) * 60
                    reserve = family.data.get("job_reserve_seconds")
                    if type(reserve) is not int or reserve <= 0:
                        continue  # The owning field validation reports this error.
                    if envelope.projected_makespan_seconds + reserve > job_budget:
                        scope = "" if cell is None else f" in matrix cell {cell}"
                        errors.append(
                            f"{family.name}: projected resource-aware timeout "
                            f"envelope {envelope.projected_makespan_seconds}s"
                            f"{scope} plus job reserve {reserve}s exceeds "
                            f"GitHub job budget {job_budget}s"
                        )
            for family in self.scheduled_families:
                try:
                    envelope = self.timeout_envelope(family.name)
                except (KeyError, TypeError, ValueError) as exc:
                    errors.append(str(exc))
                    continue
                job_budget = int(family.data["timeout_minutes"]) * 60
                reserve = family.data.get("job_reserve_seconds")
                if type(reserve) is not int or reserve <= 0:
                    continue  # The owning field validation reports this error.
                if envelope.projected_makespan_seconds + reserve > job_budget:
                    errors.append(
                        f"{family.name}: projected resource-aware timeout envelope "
                        f"{envelope.projected_makespan_seconds}s plus job reserve "
                        f"{reserve}s exceeds scheduled job budget {job_budget}s"
                    )
        for family in self.families:
            if family.data.get("executor") != "github-workflow":
                continue
            workflow = ROOT / str(family.data["workflow"])
            if not workflow.is_file():
                continue
            text = workflow.read_text(encoding="utf-8")
            family_commands = tuple(
                command for command in self.commands if command.family == family.name
            )
            dependency_ids = {
                dependency
                for command in family_commands
                for dependency in command.dependencies
            }
            terminal_commands = tuple(
                command
                for command in family_commands
                if command.id not in dependency_ids
            )
            for command in terminal_commands:
                invocation = f"--run-command {command.id} --receipt"
                if invocation not in text:
                    errors.append(
                        f"{family.name}: workflow does not execute {invocation!r}"
                    )
            expected_receipts = len(terminal_commands)
            for token in ("actions/upload-artifact@", "if-no-files-found: error"):
                if text.count(token) < expected_receipts:
                    errors.append(
                        f"{family.name}: workflow has fewer {token!r} receipt "
                        "uploads than executable commands"
                    )
            if "continue-on-error" in text:
                errors.append(
                    f"{family.name}: claimed correctness workflow may not continue-on-error"
                )
        verdict_workflow = ROOT / ".github" / "workflows" / "ci.yml"
        if verdict_workflow.is_file():
            verdict_text = verdict_workflow.read_text(encoding="utf-8")
            verdict_block = _workflow_job_block(verdict_text, "proof-plan-verdict")
            for token in (
                "actions/download-artifact@",
                "--verify-selected",
                "--receipt-dir proof-receipts",
            ):
                if verdict_block is None or token not in verdict_block:
                    errors.append(
                        f"proof-plan-verdict: missing executable input {token!r}"
                    )
            if verdict_block is not None and "--result" in verdict_block:
                errors.append(
                    "proof-plan-verdict: legacy synthetic job results are forbidden"
                )
            if verdict_block is not None:
                try:
                    actual_verdict_needs = _workflow_job_needs(verdict_block)
                except ValueError as exc:
                    errors.append(f"proof-plan-verdict: {exc}")
                else:
                    expected_verdict_needs = {
                        "classify-changes",
                        *(
                            str(family.data["admission_job"])
                            for family in self.families
                        ),
                    }
                    if len(actual_verdict_needs) != len(set(actual_verdict_needs)):
                        errors.append("proof-plan-verdict: needs must be unique")
                    if set(actual_verdict_needs) != expected_verdict_needs:
                        errors.append(
                            "proof-plan-verdict: needs must be the exact conjunction "
                            f"of classifier and family admissions; got "
                            f"{sorted(actual_verdict_needs)!r}, expected "
                            f"{sorted(expected_verdict_needs)!r}"
                        )
        local_names: set[str] = set()
        for index, command in enumerate(self.always):
            if error := local_gate_template_error(command):
                errors.append(f"local.always.{index}: {error}")
        try:
            toolchain_closure(self.toolchain_policies, self.always_toolchains)
        except ValueError as exc:
            errors.append(f"local always: {exc}")
        for rule in self.local_rules:
            if field_error := local_rule_field_error(rule):
                errors.append(field_error)
            name = rule.get("name")
            if not isinstance(name, str) or not name:
                errors.append("local [[rule]] missing non-empty name")
                continue
            if name in local_names:
                errors.append(f"duplicate local rule {name!r}")
            local_names.add(name)
            tools = rule.get("toolchains")
            if not isinstance(tools, list) or not all(
                isinstance(item, str) and item for item in tools
            ):
                errors.append(f"{name}: toolchains must be an explicit list of names")
            else:
                try:
                    toolchain_closure(self.toolchain_policies, tools)
                except ValueError as exc:
                    errors.append(f"{name}: {exc}")
            for field in ("globs", "gates"):
                value = rule.get(field)
                if not isinstance(value, list) or not all(
                    isinstance(item, str) for item in value
                ):
                    errors.append(f"{name}: {field} must be a list of strings")
                elif field == "gates":
                    for index, command in enumerate(value):
                        if error := local_gate_template_error(command):
                            errors.append(f"local.{name}.{index}: {error}")
            errors.extend(local_pytest_target_errors(name, rule.get("pytest", [])))
            errors.extend(
                local_pytest_select_errors(name, rule.get("pytest_select", []))
            )
        return errors

    def all_selected(self, *, reason: str, fail_closed: bool = True) -> Selection:
        return Selection(
            changed_paths=(),
            selected=self.families,
            reasons={family.name: (reason,) for family in self.families},
            fail_closed_reason=reason if fail_closed else None,
        )

    def select(self, paths: list[str] | tuple[str, ...]) -> Selection:
        normalized = tuple(_normalize_path(path) for path in paths if path.strip())
        authority_matches = tuple(
            path
            for path in normalized
            if any(_matches(path, pattern) for pattern in self.authority_inputs)
        )
        direct_reasons: dict[str, tuple[str, ...]] = {}
        for family in self.families:
            matched = tuple(
                path
                for path in normalized
                if any(_matches(path, pattern) for pattern in family.inputs)
            )
            family_reasons = tuple(dict.fromkeys((*authority_matches, *matched)))
            if family_reasons:
                direct_reasons[family.name] = family_reasons

        selected_names = set(direct_reasons)
        dependency_reasons: dict[str, list[str]] = {}
        pending = [
            family.name for family in self.families if family.name in selected_names
        ]
        family_by_name = {family.name: family for family in self.families}
        for name in pending:
            for dependency in family_by_name[name].data["dependencies"]:
                reason = f"dependency:{name}"
                if dependency not in selected_names:
                    dependency_reasons.setdefault(dependency, []).append(reason)
                    selected_names.add(dependency)
                    pending.append(dependency)
                elif dependency not in direct_reasons:
                    dependency_reasons.setdefault(dependency, [])
                    if reason not in dependency_reasons[dependency]:
                        dependency_reasons[dependency].append(reason)

        selected = tuple(
            family for family in self.families if family.name in selected_names
        )
        reasons = {
            family.name: tuple(
                (
                    *direct_reasons.get(family.name, ()),
                    *dependency_reasons.get(family.name, ()),
                )
            )
            for family in selected
        }
        return Selection(normalized, selected, reasons)


def _normalize_path(path: str) -> str:
    return path.replace("\\", "/").removeprefix("./")


def _toolchain_probe_cwd(policy: ToolchainPolicy) -> tuple[str, Path]:
    relative = str(policy.data.get("probe_cwd", "."))
    return relative, (policy.root / relative).resolve(strict=True)


def _workflow_job_block(text: str, job: str) -> str | None:
    match = re.search(
        rf"(?ms)^  {re.escape(job)}:\n.*?(?=^  [A-Za-z0-9_-]+:\n|\Z)", text
    )
    return match.group(0) if match is not None else None


def _workflow_job_needs(block: str) -> tuple[str, ...]:
    """Read one job's literal ``needs`` edge list without a YAML dependency.

    The changed-path classifier runs before repository toolchain setup, so proof
    authority validation must stay in the Python standard library.  CI admission
    edges are intentionally restricted to literal scalar, inline-list, or block-
    list job names; expressions and mappings are not valid dependency authority.
    """

    declarations: list[tuple[int, str]] = []
    lines = block.splitlines()
    for index, line in enumerate(lines):
        match = re.fullmatch(r"    needs:\s*(.*?)\s*", line)
        if match is not None:
            declarations.append((index, match.group(1)))
    if len(declarations) > 1:
        raise ValueError("admission job declares needs more than once")
    if not declarations:
        return ()
    index, value = declarations[0]
    if value:
        if value.startswith("[") and value.endswith("]"):
            items = tuple(part.strip().strip("'\"") for part in value[1:-1].split(","))
            if any(not item for item in items):
                raise ValueError("admission job has an empty inline needs item")
            return items
        if value.startswith(("{", "${{")):
            raise ValueError("admission job needs must be literal job names")
        return (value.strip("'\""),)
    items: list[str] = []
    for line in lines[index + 1 :]:
        match = re.fullmatch(r"      -\s+([A-Za-z0-9_-]+)\s*", line)
        if match is not None:
            items.append(match.group(1))
            continue
        if line.startswith("      ") and not line.strip():
            continue
        break
    if not items:
        raise ValueError("admission job needs block is empty or non-literal")
    return tuple(items)


def _matches(path: str, pattern: str) -> bool:
    normalized = _normalize_path(pattern)
    if normalized.endswith("/**"):
        prefix = normalized[:-3].rstrip("/")
        return path == prefix or path.startswith(prefix + "/")
    return fnmatch.fnmatchcase(path, normalized)


def _run_git(args: list[str]) -> str:
    return subprocess.check_output(
        ["git", *args], cwd=ROOT, text=True, stderr=subprocess.STDOUT, encoding="utf-8"
    )


def _source_commit() -> str:
    try:
        actual = _run_git(["rev-parse", "--verify", "HEAD^{commit}"]).strip()
    except (OSError, subprocess.CalledProcessError) as exc:
        raise ValueError(f"cannot resolve proof checkout HEAD: {exc}") from exc
    commit = require_git_object_id(
        actual,
        label="proof checkout commit",
    )
    declared = os.environ.get("GITHUB_SHA", "").strip()
    if declared:
        require_git_object_id(declared, label="declared CI source commit")
        if declared != commit:
            raise ValueError(
                "declared CI source commit does not match actual checkout HEAD"
            )
    return commit


def _source_identity() -> dict[str, str]:
    """Bind immutable Git objects; this is not a continuous worktree monitor."""
    commit = _source_commit()
    try:
        actual = _run_git(["rev-parse", "--verify", f"{commit}^{{tree}}"]).strip()
    except (OSError, subprocess.CalledProcessError) as exc:
        raise ValueError(f"cannot resolve proof checkout tree: {exc}") from exc
    tree = require_git_object_id(
        actual,
        label="proof checkout tree",
    )
    if _source_commit() != commit:
        raise ValueError("checkout HEAD changed while capturing proof candidate")
    return {"commit": commit, "tree": tree}


def resolved_command_argv(command: ProofCommand) -> tuple[str, ...]:
    """The argv the executor launches, with the source-commit placeholder bound."""
    if not command.binds_source_commit:
        return command.argv
    commit = _source_commit()
    return tuple(
        commit if part == PROOF_SOURCE_COMMIT_PLACEHOLDER else part
        for part in command.argv
    )


def _source_tree_changes() -> tuple[str, ...]:
    """Return the Git status entries that keep proof inputs from being commit-backed.

    A commit SHA alone does not attest staged, unstaged, or untracked source.
    Receipts therefore fail closed unless Git reports an entirely clean tree
    when execution begins and after every command. The probe splits Git's raw
    NUL-separated output, so unusual path encodings cannot hide an entry; the
    decoded entries only name the change in diagnostics.
    """
    try:
        status = subprocess.check_output(
            [
                "git",
                *clean_checkout_status_arguments(null_terminated=True),
            ],
            cwd=ROOT,
        )
    except (OSError, subprocess.CalledProcessError) as exc:
        raise ValueError(f"cannot observe proof checkout cleanliness: {exc}") from exc
    return tuple(
        entry.decode("utf-8", "backslashreplace")
        for entry in status.split(b"\0")
        if entry
    )


def _diff_paths(base: str, head: str, *, three_dot: bool = False) -> list[str]:
    return changed_paths(base, head, run_git=_run_git, three_dot=three_dot)


def _pull_request_paths(base_ref: str) -> list[str]:
    if not base_ref:
        raise RuntimeError("GITHUB_BASE_REF is not set")
    remote_ref = f"origin/{base_ref}"
    try:
        _run_git(["rev-parse", "--verify", remote_ref])
    except subprocess.CalledProcessError:
        _run_git(
            [
                "fetch",
                "--no-tags",
                "--prune",
                "origin",
                f"+{base_ref}:refs/remotes/{remote_ref}",
            ]
        )
    return _diff_paths(remote_ref, "HEAD", three_dot=True)


def _event_payload(path: str) -> dict[str, Any]:
    if not path:
        return {}
    value = json.loads(Path(path).read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise RuntimeError("GitHub event payload is not an object")
    return value


# Each command's `tiers` names the CI events it gates. A pull request runs the
# fast `pr` tier; pushes to main and the merge queue run `main`; every
# scheduled workflow runs `scheduled`, whatever its cron cadence. `pre-push` is
# selected explicitly. A scheduled family always runs its scheduled tier, so a
# manual dispatch of a scheduled workflow proves what the schedule proves.
# Dependencies always run with their dependents.
PROOF_TIER_ENV = "MOLT_PROOF_TIER"
SCHEDULED_TIER = "scheduled"
PROOF_TIERS = ("pre-push", "pr", "main", SCHEDULED_TIER)
_EVENT_TIERS = {
    "pull_request": "pr",
    "pull_request_target": "pr",
    "push": "main",
    "merge_group": "main",
    "workflow_dispatch": "main",
    "workflow_call": "main",
    "schedule": SCHEDULED_TIER,
}


def active_tier(environ: Mapping[str, str] | None = None) -> str | None:
    """The tier this process gates, or None to run every tier (local use)."""
    source = os.environ if environ is None else environ
    explicit = source.get(PROOF_TIER_ENV, "").strip()
    if explicit:
        if explicit not in PROOF_TIERS:
            raise ValueError(
                f"{PROOF_TIER_ENV}={explicit!r} is not one of {list(PROOF_TIERS)!r}"
            )
        return explicit
    return _EVENT_TIERS.get(source.get("GITHUB_EVENT_NAME", "").strip())


def _command_in_tier(command: ProofCommand, tier: str | None) -> bool:
    return tier is None or tier in command.data["tiers"]


def selection_for_event(
    plan: ProofPlan,
    *,
    event_name: str,
    base_ref: str,
    event_path: str,
    before: str,
    after: str,
) -> Selection:
    try:
        payload = _event_payload(event_path)
        if event_name in {"pull_request", "pull_request_target"}:
            return plan.select(_pull_request_paths(base_ref))
        if event_name == "push":
            if bool(payload.get("forced")):
                raise RuntimeError("forced push has no trustworthy incremental base")
            base = before or str(payload.get("before", ""))
            head = after or str(payload.get("after", "")) or "HEAD"
            return plan.select(_diff_paths(base, head))
        if event_name in {
            "merge_group",
            "schedule",
            "workflow_dispatch",
            "workflow_call",
        }:
            return plan.all_selected(
                reason=f"{event_name}: full proof plan", fail_closed=False
            )
        raise RuntimeError(f"unsupported or missing event {event_name!r}")
    except Exception as exc:
        return plan.all_selected(reason=f"fail-closed event selection: {exc}")


def family_outputs(
    plan: ProofPlan, selection: Selection, *, tier: str | None = None
) -> dict[str, str]:
    tiered = tuple(
        command for command in plan.commands if _command_in_tier(command, tier)
    )
    # A family with nothing to run in this tier never starts a runner.
    selected = {
        family.name
        for family in selection.selected
        if any(command.family == family.name for command in tiered)
    }
    outputs = {
        family.name: "true" if family.name in selected else "false"
        for family in plan.families
    }
    topology = [
        {
            "name": family.name,
            **{
                key: family.data[key]
                for key in (
                    "executor",
                    "workflow",
                    "job",
                    "admission_workflow",
                    "admission_job",
                    "admission_needs",
                    "dependencies",
                    "required",
                    "timeout_minutes",
                    "resource_class",
                )
            },
            **(
                {"job_reserve_seconds": family.data["job_reserve_seconds"]}
                if family.data["executor"] in {"github-job", "github-matrix"}
                else {}
            ),
            "command_ids": [
                command.id for command in tiered if command.family == family.name
            ],
            "matrix_cells": list(
                dict.fromkeys(
                    str(command.data["cell"])
                    for command in tiered
                    if command.family == family.name
                )
            ),
            "selected_by": list(selection.reasons.get(family.name, ())),
        }
        for family in selection.selected
        if family.name in selected
    ]
    # Every matrix family owns its own output: a matrix job must never receive
    # another family's runner cells.
    matrices: dict[str, list[dict[str, Any]]] = {
        family.name: []
        for family in plan.families
        if family.data["executor"] == "github-matrix"
    }
    for cell in plan.matrix_cells:
        command_ids = [
            command.id
            for command in tiered
            if command.family in matrices
            and command.family in selected
            and command.data["cell"] == cell.id
        ]
        if not command_ids:
            continue
        families = {
            plan_command.family
            for plan_command in plan.commands
            if plan_command.id in command_ids
        }
        if len(families) != 1:
            raise ValueError(f"{cell.id}: executable matrix cell spans families")
        family_name = families.pop()
        matrices[family_name].append(
            {
                "family": family_name,
                "cell": cell.id,
                **{
                    key: cell.data[key]
                    for key in (
                        "runner",
                        "os",
                        "arch",
                        "python",
                        "backend",
                        "target",
                        "profile",
                    )
                },
                "command_ids": command_ids,
                "toolchains": list(
                    plan.toolchain_closure(
                        name
                        for command in _topological_commands(
                            plan, family=family_name, matrix_cell=cell.id, tier=tier
                        )
                        for name in command.toolchains
                    )
                ),
                "selected_by": list(selection.reasons.get(family_name, ())),
            }
        )
    outputs["topology"] = json.dumps({"include": topology}, separators=(",", ":"))
    for family_name, matrix in matrices.items():
        outputs[family_matrix_output(family_name)] = json.dumps(
            {"include": matrix}, separators=(",", ":")
        )
    outputs["selected"] = json.dumps(sorted(selected), separators=(",", ":"))
    outputs["changed_paths"] = json.dumps(
        selection.changed_paths, separators=(",", ":")
    )
    return outputs


def _authority_sha256(
    plan: ProofPlan,
    overrides: Mapping[str, bytes] | None = None,
) -> str:
    """Hash canonical path + content for the complete declared authority closure."""
    override_map = {} if overrides is None else dict(overrides)
    unknown = set(override_map) - set(plan.authority_inputs)
    if unknown:
        raise ValueError(f"unknown authority input overrides: {sorted(unknown)!r}")
    digest = hashlib.sha256(b"molt.proof-authority.v2\0")
    for relative in sorted(plan.authority_inputs):
        content = override_map.get(relative)
        if content is None:
            content = (ROOT / relative).read_bytes()
        try:
            canonical_content = (
                content.decode("utf-8")
                .replace("\r\n", "\n")
                .replace("\r", "\n")
                .encode("utf-8")
            )
        except UnicodeDecodeError as exc:
            raise ValueError(
                f"authority input must be canonical UTF-8 text: {relative}"
            ) from exc
        encoded_path = relative.encode("utf-8")
        digest.update(len(encoded_path).to_bytes(8, "big"))
        digest.update(encoded_path)
        digest.update(len(canonical_content).to_bytes(8, "big"))
        digest.update(canonical_content)
    return digest.hexdigest()


def _normalized_os() -> str:
    value = platform.system().lower()
    return {"darwin": "macos", "windows": "windows"}.get(value, value)


def _normalized_arch() -> str:
    value = platform.machine().lower()
    return {"amd64": "x86_64", "x64": "x86_64", "arm64": "aarch64"}.get(value, value)


def host_matrix_cell(plan: ProofPlan, family_name: str) -> str:
    """Select a matrix family's cell for this host when none is named.

    CI passes `--matrix-cell` explicitly. A local `--run-family` runs only the
    cell whose OS and architecture match the host; its receipt binds both.
    """
    family_cells = plan.family_cells(family_name)
    cells = [cell for cell in plan.matrix_cells if cell.id in family_cells]
    host = (_normalized_os(), _normalized_arch())
    matching = [
        cell.id for cell in cells if (cell.data["os"], cell.data["arch"]) == host
    ]
    if len(matching) == 1:
        return matching[0]
    listed = ", ".join(
        f"{cell.id} ({cell.data['os']}/{cell.data['arch']})" for cell in cells
    )
    raise ValueError(
        f"{family_name} runs one job per matrix cell and no unique cell matches "
        f"host {host[0]}/{host[1]}; pass --matrix-cell with one of: {listed}"
    )


def _toolchain_fingerprint_digest(identity: Mapping[str, Any]) -> str:
    material = "\0".join(
        identity[name]
        for name in (
            "path",
            "launcher_path",
            "launcher_sha256",
            "content_path",
            "executable_sha256",
            "version",
            "probe_cwd",
        )
    )
    if "wasi_sdk_sha256" in identity:
        material += "\0" + identity["wasi_sdk_sha256"]
    if "llvm_sdk_sha256" in identity:
        material += "\0" + identity["llvm_sdk_sha256"]
    if "python_runtime_sha256" in identity:
        material += "\0" + identity["python_runtime_sha256"]
    return hashlib.sha256(material.encode()).hexdigest()


def compiler_tool_path(
    policy: ToolchainPolicy, *, environment: Mapping[str, str], cwd: Path
) -> Path | None:
    """Use the compiler's selector for a declared post-link executable.

    Proof admission must capture the tool the compiler will execute, including
    managed installations and explicit optimizer pins, rather than a PATH decoy.
    """
    selector = policy.data.get("compiler_tool_selector")
    if selector is None:
        return None
    if selector == "native-strip":
        from molt.llvm_wasi_tools import llvm_tool_candidates

        # As for llvm-sdk below: the policy's checkout pins the WASI SDK and
        # owns the managed tool state, not the driver running admission.
        candidates = llvm_tool_candidates(
            "strip", environment=environment, source_root=policy.root
        )
        selected = candidates[0] if candidates else None
    elif selector == "build-python":
        from molt.python_runtime_admission import resolve_build_python

        selected = resolve_build_python(environment, cwd=cwd)
    elif selector == "llvm-sdk":
        from molt.llvm_toolchain import discover_llvm_toolchain

        # The policy's checkout pins the SDK, not the driver running admission
        # (under the landing controller they differ).
        discovery = discover_llvm_toolchain(
            policy.root, environ=dict(environment), cwd=cwd
        )
        selected = discovery.llvm_config if discovery is not None else None
    elif selector == "wasm-opt":
        from molt.wasm_optimizer_identity import find_wasm_opt

        selected = find_wasm_opt(environment=environment, source_root=policy.root)
    else:
        raise ValueError(f"{policy.name}: unknown compiler tool selector {selector!r}")
    if selected is None:
        raise ValueError(f"{policy.name}: selected compiler helper is unavailable")
    return Path(selected).absolute()


def capture_selected_python_runtime(
    policy: ToolchainPolicy,
    executable: Path,
    *,
    environment: Mapping[str, str],
    admission: BuildPythonAdmission | None = None,
) -> dict[str, Any]:
    """Use the existing isolated runtime owner for this selected prerequisite."""
    from molt.python_runtime_admission import BuildPythonAdmission

    if policy.python_identity_role != "runtime":
        raise ValueError(f"{policy.name}: selected Python runtime role is required")
    if admission is None:
        with BuildPythonAdmission(with_custody=True) as owner:
            return capture_selected_python_runtime(
                policy, executable, environment=environment, admission=owner
            )
    captured = admission.capture({**environment, "MOLT_BUILD_PYTHON": str(executable)})
    capture = captured["capture"]
    return {
        "launch_argv": [str(executable), "-B", "-I", "-S"],
        "startup_selection": captured["startup_selection"],
        "runtime": capture["identity"],
        "file_custody": capture["file_custody"],
        "node_custody": capture["node_custody"],
        "inventory_profile": capture["inventory_profile"],
    }


def validate_selected_python_runtime(
    policy: ToolchainPolicy, selected: Mapping[str, Any]
) -> None:
    """A runtime role always requires the canonical complete custody envelope."""
    from molt.python_environment_identity import (
        PYTHON_CAPTURE_SCHEMA,
        validate_python_capture,
    )
    from molt.python_runtime_identity import (
        PYTHON_RUNTIME_IDENTITY_SCHEMA,
        PYTHON_RUNTIME_SELECTION_SCHEMA,
    )

    if set(selected) != {
        "launch_argv",
        "startup_selection",
        "runtime",
        "file_custody",
        "node_custody",
        "inventory_profile",
    }:
        raise ValueError(f"{policy.name}: incomplete selected Python runtime custody")
    launch = selected["launch_argv"]
    startup = selected["startup_selection"]
    runtime = selected["runtime"]
    if (
        not isinstance(launch, list)
        or len(launch) != 4
        or not isinstance(launch[0], str)
        or not Path(launch[0]).is_absolute()
        or launch[1:] != ["-B", "-I", "-S"]
        or not isinstance(startup, dict)
        or set(startup)
        != {
            "schema",
            "platform",
            "selected_executable",
            "base_executable",
            "prefix",
            "base_prefix",
            "runtime_library",
            "base_paths",
            "import_paths",
            "import_directories",
            "import_archives",
            "startup_configuration",
            "native",
        }
        or startup.get("schema") != PYTHON_RUNTIME_SELECTION_SCHEMA
        or not isinstance(startup.get("selected_executable"), dict)
        or startup["selected_executable"].get("lexical") != launch[0]
        or startup["selected_executable"].get("present") is not True
        or not isinstance(runtime, dict)
        or runtime.get("schema") != PYTHON_RUNTIME_IDENTITY_SCHEMA
        or startup.get("platform")
        != {
            key: runtime.get(key)
            for key in (
                "implementation",
                "version",
                "cache_tag",
                "soabi",
                "abi_flags",
                "multiarch",
                "py_debug",
                "gil_disabled",
                "operating_system",
                "architecture",
                "pointer_bits",
                "byteorder",
            )
        }
    ):
        raise ValueError(f"{policy.name}: selected Python isolated launch is invalid")
    validate_python_capture(
        {
            "schema": PYTHON_CAPTURE_SCHEMA,
            "identity": runtime,
            **{
                key: selected[key]
                for key in ("file_custody", "node_custody", "inventory_profile")
            },
        }
    )
    if (
        re.search(
            str(policy.data["version_pattern"]), "Python " + str(runtime["version"])
        )
        is None
    ):
        raise ValueError(
            f"{policy.name}: captured Python runtime violates version policy"
        )


def selected_python_runtime_material(selected: Mapping[str, Any]) -> dict[str, Any]:
    """Capture timings are observations, never semantic identity inputs."""
    return {key: value for key, value in selected.items() if key != "inventory_profile"}


def validate_selected_python_fingerprint(
    policy: ToolchainPolicy, identity: Mapping[str, Any]
) -> None:
    if not isinstance(identity, Mapping):
        raise ValueError(f"{policy.name}: selected Python fingerprint is not an object")
    selected = identity.get("python_runtime")
    if not isinstance(selected, Mapping):
        raise ValueError(
            f"{policy.name}: complete selected Python fingerprint is required"
        )
    validate_selected_python_runtime(policy, selected)
    if selected["launch_argv"][0] != identity.get("content_path"):
        raise ValueError(
            f"{policy.name}: fingerprint launch differs from admitted runtime"
        )
    expected = hashlib.sha256(
        json.dumps(
            selected_python_runtime_material(selected),
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
    ).hexdigest()
    if identity.get("python_runtime_sha256") != expected:
        raise ValueError(f"{policy.name}: Python runtime fingerprint digest is invalid")


def _find_tool(requested: str, environment: Mapping[str, str], cwd: Path) -> str | None:
    """The executable ``requested`` names under ``environment`` alone."""
    found = find_executable(requested, environment=environment, cwd=cwd)
    return None if found is None else str(found)


_RUNTIME_COMMANDS: dict[tuple[str, ...], Any] = {}


def _runtime_command(
    tool: str,
    root: Path,
    environ: Mapping[str, str],
    cwd: Path | None,
    *,
    provision: bool = False,
) -> Any:
    """A runtime tool release's pinned command, verified once per installation.

    Discovery re-hashes the package tree and its runtime (about 0.17 s for
    npm's 1,942 files and Node), and one admission needs the command to
    provision, project and identify the tool. The key binds the tool's and
    its runtime's pins and host-asset digests, and the identity of each
    installation's attestation, which provisioning replaces with the tree:
    a long-lived runner whose batches move either pin, or re-provision
    either tree, verifies again instead of serving the old command.
    """
    from molt.dx import DxConfigError, canonical_toolchain_root
    from molt.tool_releases import (
        TOOL_ATTESTATION_FILENAME,
        ToolReleaseError,
        host_asset,
        load_tool_releases,
        pinned_command,
        tool_prefix,
        tool_release,
    )

    manifest = load_tool_releases(root)
    release = manifest.get(tool) or tool_release(tool, root)
    try:
        toolchain_root = canonical_toolchain_root(
            root, environ, require_exists=False, cwd=cwd
        )
    except DxConfigError as exc:
        raise ToolReleaseError(str(exc)) from exc
    releases = [release]
    if release.runtime is not None:
        releases.append(
            manifest.get(release.runtime) or tool_release(release.runtime, root)
        )

    def key() -> tuple[object, ...]:
        parts: list[object] = [str(root), str(toolchain_root)]
        for selected in releases:
            attestation = (
                tool_prefix(toolchain_root, selected) / TOOL_ATTESTATION_FILENAME
            )
            try:
                stat = attestation.stat()
                installed = (stat.st_ino, stat.st_mtime_ns, stat.st_size)
            except OSError:
                installed = None
            parts.append(
                (
                    selected.name,
                    selected.version,
                    host_asset(selected).sha256,
                    installed,
                )
            )
        return tuple(parts)

    command = _RUNTIME_COMMANDS.get(key())
    if command is None:
        command = pinned_command(
            tool, root, environ=environ, cwd=cwd, provision=provision
        )
        # Keyed after provisioning, which may have just written the trees;
        # never under an absent installation (a tree retired meanwhile),
        # which a later call must refuse or provision again.
        stored = key()
        if all(part[3] is not None for part in stored[2:]):
            _RUNTIME_COMMANDS[stored] = command
    return command


def _provision_runtime_tools(
    policies: Iterable[ToolchainPolicy],
    names: tuple[str, ...],
    *,
    root: Path,
    environment: Mapping[str, str],
) -> str | None:
    """Provision each selected runtime tool release (npm) before admission.

    It runs on its pinned runtime through ``molt.tool_releases``, which
    provisions it on use, so admission provisions it too, as it provisions
    a checkout input from its lock, instead of refusing until a person does.
    """
    from molt.tool_releases import ToolReleaseError

    selected = set(names)
    errors = []
    for policy in policies:
        if policy.name in selected and policy.projection == "runtime-tool-release":
            try:
                _runtime_command(
                    str(policy.data["executable"]),
                    root,
                    environment,
                    root,
                    provision=True,
                )
            except ToolReleaseError as exc:
                errors.append(f"{policy.name}: {exc}")
    return "; ".join(errors) or None


def _runtime_tool_fingerprint(
    policy: ToolchainPolicy, *, environment: Mapping[str, str]
) -> dict[str, Any] | None:
    """Identify a runtime tool release by the installation consumers run.

    It runs on its pinned runtime through ``molt.tool_releases``, never from
    PATH, so the identity binds the runtime's executable, the digest of every
    file of the package, and the version the tool reports when run that way.
    """
    from molt.tool_releases import ToolReleaseError

    probe_cwd, probe_directory = _toolchain_probe_cwd(policy)
    try:
        command = _runtime_command(
            str(policy.data["executable"]), policy.root, environment, probe_directory
        )
    except ToolReleaseError:
        return None
    completed = toolchain_probe.run_toolchain_probe(
        policy.name,
        [*command.argv, *policy.data["version_args"]],
        stage="version query",
        cwd=probe_directory,
        environment=environment,
    )
    runtime = command.executable
    # Like every identity, `path` and `launcher_*` name the launched file (the
    # runtime); `content_path` names what it runs.
    identity = {
        "path": str(runtime.executable),
        "launcher_path": str(runtime.executable),
        "launcher_sha256": runtime.executable_sha256,
        "content_path": str(command.tool.executable),
        "version": completed.stdout.strip(),
        "version_pattern": str(policy.data["version_pattern"]),
        "probe_cwd": probe_cwd,
        "executable_sha256": command.tool.executable_sha256,
        "tree_sha256": command.tool.tree_sha256,
    }
    identity["identity_sha256"] = _toolchain_fingerprint_digest(identity)
    return identity


def _version_fingerprint(
    policy: ToolchainPolicy,
    *,
    environment: Mapping[str, str],
    python_admission: BuildPythonAdmission | None = None,
) -> dict[str, Any] | None:
    """Identify the tool ``policy`` selects under ``environment``.

    Every lookup reads ``environment`` (its PATH and PATHEXT through
    ``find_executable``), never the process environment, so a long-lived
    caller's earlier admissions cannot steer a later one.
    """
    if policy.identity_kind != "executable":
        raise ValueError(
            f"{policy.name}: {policy.identity_kind} toolchain identity "
            "requires target-context capture"
        )
    if policy.projection == "runtime-tool-release":
        return _runtime_tool_fingerprint(policy, environment=environment)
    executable = str(policy.data["executable"])
    requested = sys.executable if executable == "{python}" else executable
    probe_cwd, probe_directory = _toolchain_probe_cwd(policy)
    compiler_path = compiler_tool_path(
        policy, environment=environment, cwd=probe_directory
    )
    sdk_role = policy.data.get("wasi_sdk_tool")
    if compiler_path is not None:
        path = str(compiler_path)
    elif requested == "wasm-ld" or sdk_role is not None:
        from molt.llvm_toolchain import LlvmToolchainConfigError, resolve_wasi_sdk_tool

        try:
            path = str(
                resolve_wasi_sdk_tool(
                    policy.root, sdk_role or "wasm-ld", environ=dict(environment)
                )
            )
        except LlvmToolchainConfigError as exc:
            raise ValueError(
                f"{policy.name} toolchain selection failed: {exc}"
            ) from exc
    elif policy.name in {"rustc", "cargo"}:
        from molt.rust_toolchain import cargo_selected_value

        requested = (
            cargo_selected_value(
                {},
                {},
                environment,
                ("build", "rustc"),
                ("RUSTC", "CARGO_BUILD_RUSTC"),
                requested,
            )
            if policy.name == "rustc"
            else environment.get("CARGO", requested)
        )
        path = _find_tool(requested, environment, probe_directory)
    else:
        path = _find_tool(requested, environment, probe_directory)
    if path is None:
        return None
    command_path = Path(path).absolute()
    launcher_path = command_path.resolve()

    def content_hash(executable_path: Path) -> str:
        try:
            with executable_path.open("rb") as executable_file:
                return hashlib.file_digest(executable_file, "sha256").hexdigest()
        except OSError as exc:
            return f"unavailable:{type(exc).__name__}"

    sdk_closure = None
    if sdk_role is not None:
        from molt.llvm_toolchain import capture_wasi_sdk_selection
        from molt.wasi_sdk_identity import capture_wasi_sdk_tool_files

        sdk_closure = capture_wasi_sdk_selection(root=policy.root, env=environment)
        images = capture_wasi_sdk_tool_files(sdk_closure)
        launcher = next(
            (row for row in images if row["path"] == str(command_path)), None
        )
        if launcher is None:
            raise ValueError(
                "WASI compiler selection differs from captured SDK helpers"
            )
        launcher_sha256 = str(launcher["sha256"])
    else:
        launcher_sha256 = content_hash(launcher_path)
    content_path = (
        command_path if policy.python_identity_role == "runtime" else launcher_path
    )
    content_path_command = policy.data.get("content_path_command")
    if policy.name in {"rustc", "cargo"}:
        from molt.rust_toolchain import resolve_rustup_proxy

        content_path = resolve_rustup_proxy(
            command_path, role=policy.name, root=probe_directory, env=environment
        )
    elif isinstance(content_path_command, list):
        resolved = toolchain_probe.run_toolchain_probe(
            policy.name,
            content_path_command,
            stage="content resolver",
            cwd=probe_directory,
            environment=environment,
        )
        try:
            content_path = toolchain_probe.resolve_single_file_path(
                resolved.stdout,
                probe_cwd=probe_directory,
                preserve_entrypoint=policy.python_identity_role == "runtime",
            )
        except (OSError, ValueError) as exc:
            raise toolchain_probe.probe_failure(
                policy.name,
                content_path_command,
                probe_directory,
                f"invalid resolved path ({type(exc).__name__}): {exc}",
                stage="content resolver",
                stdout=resolved.stdout,
                stderr=resolved.stderr,
            ) from exc
    executable_sha256 = (
        launcher_sha256 if content_path == launcher_path else content_hash(content_path)
    )
    version_argv = [
        str(content_path)
        if policy.name in {"rustc", "cargo"}
        or policy.data.get("version_from_content_path")
        else path,
        *policy.data["version_args"],
    ]
    completed = toolchain_probe.run_toolchain_probe(
        policy.name,
        version_argv,
        stage="version query",
        cwd=probe_directory,
        environment=environment,
    )
    version = completed.stdout.strip()
    sdk_digest = ""
    if sdk_closure is not None:
        sdk_digest = hashlib.sha256(
            json.dumps(
                sdk_closure,
                sort_keys=True,
                separators=(",", ":"),
            ).encode()
        ).hexdigest()
    identity = {
        "path": str(command_path),
        "launcher_path": str(launcher_path),
        "launcher_sha256": launcher_sha256,
        "content_path": str(content_path),
        "version": version,
        "version_pattern": str(policy.data["version_pattern"]),
        "probe_cwd": probe_cwd,
        "executable_sha256": executable_sha256,
    }
    if sdk_closure is not None:
        identity["wasi_sdk_sha256"] = sdk_digest
    if policy.data.get("compiler_tool_selector") == "llvm-sdk":
        from molt.llvm_toolchain import llvm_verification_selection

        selected = llvm_verification_selection(
            root=policy.root, cwd=probe_directory, env=environment
        )
        images = [
            {
                "role": role,
                "path": path,
                "content_path": str(Path(path).resolve(strict=True)),
                "sha256": content_hash(Path(path)),
            }
            for role, path in {**selected["tools"], **selected["selectors"]}.items()
        ]
        identity["llvm_sdk_sha256"] = hashlib.sha256(
            json.dumps(
                {"selection": selected, "images": images},
                sort_keys=True,
                separators=(",", ":"),
            ).encode()
        ).hexdigest()
    if policy.python_identity_role == "runtime":
        selected = capture_selected_python_runtime(
            policy, content_path, environment=environment, admission=python_admission
        )
        validate_selected_python_runtime(policy, selected)
        identity["python_runtime"] = selected
        identity["version"] = "Python " + str(selected["runtime"]["version"])
        identity["python_runtime_sha256"] = hashlib.sha256(
            json.dumps(
                selected_python_runtime_material(selected),
                sort_keys=True,
                separators=(",", ":"),
            ).encode()
        ).hexdigest()
    identity["identity_sha256"] = _toolchain_fingerprint_digest(
        {key: value for key, value in identity.items() if key != "python_runtime"}
    )
    return identity


@dataclass(frozen=True, slots=True)
class ToolchainProjection:
    """The variables that put the installed providers first, and the install
    command of each selected provider that is not installed."""

    delta: dict[str, str]
    missing: tuple[str, ...]


def project_toolchains(
    policies: Iterable[ToolchainPolicy],
    names: tuple[str, ...],
    *,
    environ: Mapping[str, str],
    root: Path = ROOT,
    cwd: Path | None = None,
) -> ToolchainProjection:
    """Project declared providers in the request context; never download."""

    from molt.dx import canonical_toolchain_root
    from molt.llvm_toolchain import (
        llvm_bootstrap_command,
        llvm_toolchain_environment_delta,
        required_llvm_backend_pin,
        selected_wasi_sdk_installation,
        verify_available_llvm_toolchain,
        wasm_toolchain_environment_delta,
    )
    from molt.tool_releases import ToolReleaseError, discover_pinned_tool
    from molt.wasm_optimizer_identity import managed_wasm_opt

    policies = {policy.name: policy for policy in policies}
    kinds: dict[str, list[str]] = {}
    for name in names:
        projection = policies[name].projection
        if projection is not None:
            kinds.setdefault(projection, []).append(name)

    env = dict(environ)
    delta: dict[str, str] = {}
    missing: list[str] = []

    def apply(values: dict[str, str]) -> None:
        delta.update(values)
        env.update(values)

    if "llvm-sdk" in kinds:
        verification = verify_available_llvm_toolchain(root, environ=env, cwd=cwd)
        if verification is None:
            pin = required_llvm_backend_pin(root)
            command = "" if pin is None else f": {llvm_bootstrap_command(pin)}"
            missing.append(f"LLVM SDK ({', '.join(kinds['llvm-sdk'])}){command}")
        else:
            apply(llvm_toolchain_environment_delta(root, verification, environ=env))
    if "wasi-sdk" in kinds:
        installation = selected_wasi_sdk_installation(root, environ=env, cwd=cwd)
        if installation is None:
            missing.append(
                f"wasi-sdk ({', '.join(kinds['wasi-sdk'])}): "
                "python3 tools/provision_wasi_sdk.py"
            )
        else:
            apply(
                wasm_toolchain_environment_delta(
                    installation, environ=env, rust_target="wasm32-wasip1"
                )
            )
    for name in kinds.get("runtime-tool-release", ()):
        # It runs through molt.tool_releases on its runtime, never PATH.
        tool = str(policies[name].data["executable"])
        try:
            _runtime_command(tool, root, env, cwd)
        except ToolReleaseError:
            missing.append(f"{tool}: python3 -m molt.tool_releases provision {tool}")
    for name in kinds.get("tool-release", ()):
        tool = str(policies[name].data["executable"])
        discovery = discover_pinned_tool(tool, root, environ=env, cwd=cwd)
        if discovery is None:
            missing.append(f"{tool}: python3 -m molt.tool_releases provision {tool}")
            continue
        directory = str(discovery.executable.parent)
        rest = [part for part in env.get("PATH", "").split(os.pathsep) if part]
        apply(
            {"PATH": os.pathsep.join([directory, *(p for p in rest if p != directory)])}
        )
    if "binaryen" in kinds:
        wasm_opt = managed_wasm_opt(environment=env)
        if wasm_opt is None:
            from molt.binaryen_toolchain import binaryen_host_asset

            install = (
                canonical_toolchain_root(root, env, require_exists=False, cwd=cwd)
                / "toolchains"
                / binaryen_host_asset(root).archive_root
            )
            missing.append(
                f"wasm-opt: python3 tools/provision_binaryen.py --output {install}"
            )
        else:
            apply({"MOLT_WASM_OPT": wasm_opt})
    return ToolchainProjection(delta=delta, missing=tuple(missing))


def _not_provisioned(missing: tuple[str, ...], *, attempted: bool = False) -> str:
    """The refusal for unprovisioned tools; ``attempted`` when admission tried."""
    why = (
        "admission's provisioning failed (see above)"
        if attempted
        else "this command never downloads"
    )
    return f"not provisioned ({why}); install with:\n  " + "\n  ".join(missing)


@dataclass(frozen=True, slots=True)
class AdmittedToolchains:
    """The admitted identities and the variables the admission projected."""

    fingerprints: dict[str, dict[str, Any]]
    environment: dict[str, str]


def admit_toolchains(
    policies: Iterable[ToolchainPolicy],
    names: tuple[str, ...],
    *,
    environ: Mapping[str, str],
    owners: Mapping[str, Iterable[str]] | None = None,
    python_admissions: Mapping[str, BuildPythonAdmission] | None = None,
    root: Path,
) -> AdmittedToolchains:
    """Put the installed pinned providers first in ``environ``, then admit each
    selected tool by its version contract, as CI's setup actions do.

    Every executor admits its toolchains here, so no shell needs projecting
    by hand. Admission never changes the process environment: its probes read
    ``environ`` plus the projection of the whole selection, and the result
    carries that projection. A long-lived process (the landing queue runner)
    therefore admits batch after batch from the same launch environment. Each
    command then runs under its own declared closure's projection of the same
    baseline, so no command inherits another command's compiler or target
    selection.
    A tool the contract admits on PATH still passes when its pinned provider
    is not installed; a refusal names what to install. A selected tool that a
    checkout input supplies (``checkout_input``) is provisioned from its lock
    first, as ``uv run`` syncs before it runs (`molt.checkout_provisioning`),
    and a selected runtime tool release (npm) is provisioned the same way.
    """
    policies = tuple(policies)
    names = toolchain_closure(policies, names)
    runtime_error = _provision_runtime_tools(
        policies, names, root=root, environment=environ
    )
    projection = project_toolchains(policies, names, environ=environ, root=root)
    environment = MappingProxyType({**environ, **projection.delta})
    provisioning_error = _provision_checkout_inputs(
        policies, names, root=root, environment=environment
    )
    if runtime_error is not None:
        provisioning_error = "; ".join(
            error for error in (runtime_error, provisioning_error) if error
        )
    try:
        fingerprints = toolchain_fingerprints(
            policies,
            names,
            environment=environment,
            owners=owners,
            python_admissions=python_admissions,
        )
    except ValueError as exc:
        details = [str(exc)]
        if provisioning_error is not None:
            details.append(provisioning_error)
        if projection.missing:
            details.append(
                _not_provisioned(
                    projection.missing, attempted=provisioning_error is not None
                )
            )
        if len(details) == 1:
            raise
        raise ValueError("; ".join(details)) from exc
    if provisioning_error is not None:
        raise ValueError(f"toolchain contract violation: {provisioning_error}")
    return AdmittedToolchains(fingerprints=fingerprints, environment=projection.delta)


def _provision_checkout_inputs(
    policies: Iterable[ToolchainPolicy],
    names: tuple[str, ...],
    *,
    root: Path,
    environment: Mapping[str, str],
) -> str | None:
    """Ensure each selected checkout input under ``environment``; the failure
    text, or None."""
    selected = set(names)
    inputs = tuple(
        dict.fromkeys(
            policy.data["checkout_input"]
            for policy in policies
            if policy.name in selected and "checkout_input" in policy.data
        )
    )
    if not inputs:
        return None
    from molt import checkout_provisioning

    try:
        checkout_provisioning.ensure_all(
            root,
            environment,
            names=inputs,
            guard_source_root=_COMMANDS.repo_root,
        )
    except checkout_provisioning.ProvisioningError as exc:
        return f"checkout input provisioning failed: {exc}"
    return None


def toolchain_fingerprints(
    toolchain_policies: Iterable[ToolchainPolicy],
    names: tuple[str, ...],
    *,
    environment: Mapping[str, str],
    owners: Mapping[str, Iterable[str]] | None = None,
    python_admissions: Mapping[str, BuildPythonAdmission] | None = None,
) -> dict[str, dict[str, Any]]:
    """Admit all selected images under ``environment`` before work, preserving
    every probe failure."""
    policies = {policy.name: policy for policy in toolchain_policies}
    names = toolchain_closure(policies.values(), names)
    fingerprints: dict[str, dict[str, Any]] = {}
    errors: dict[str, str] = {}
    # Only groups expand here. Expanding each executable's transitive closure
    # would repeatedly traverse long dependency chains and retain quadratic
    # edge sets. The selected closure above has already validated the graph.
    group_images: dict[str, tuple[str, ...]] = {}

    def images(name: str) -> tuple[str, ...]:
        policy = policies[name]
        if policy.identity_kind != "group":
            return (name,)
        if name not in group_images:
            group_images[name] = tuple(
                dict.fromkeys(
                    image
                    for member in (
                        *policy.data.get("dependencies", []),
                        *policy.data["members"],
                    )
                    for image in images(member)
                )
            )
        return group_images[name]

    dependencies = {
        name: tuple(
            dict.fromkeys(
                image
                for dependency in policies[name].data.get("dependencies", [])
                for image in images(dependency)
            )
        )
        for name in names
    }
    remaining = {name: len(dependencies[name]) for name in names}
    dependents: dict[str, list[str]] = {name: [] for name in names}
    for name, required in dependencies.items():
        for dependency in required:
            dependents[dependency].append(name)
    ready = [name for name in names if not remaining[name]]

    def fingerprint_domain(domain_names: list[str]):
        # Provisioning-domain launchers (notably rustup) remain serial. One
        # failing probe must not suppress siblings or independent domains.
        results = {}
        for name in domain_names:
            try:
                results[name] = (
                    _version_fingerprint(
                        policies[name],
                        environment=environment,
                        python_admission=python_admissions[name],
                    )
                    if python_admissions is not None and name in python_admissions
                    else _version_fingerprint(policies[name], environment=environment),
                    None,
                )
            except Exception as exc:
                detail = "".join(traceback.format_exception_only(exc)).strip()
                results[name] = (None, f"probe raised {detail}")
        return results

    with ThreadPoolExecutor(max_workers=min(4, max(1, len(names)))) as executor:
        while ready:
            domains: dict[str, list[str]] = {}
            for name in ready:
                blocked = [
                    dependency
                    for dependency in dependencies[name]
                    if dependency in errors
                ]
                if blocked:
                    errors[name] = "blocked by failed prerequisites: " + ", ".join(
                        blocked
                    )
                    continue
                policy = policies[name]
                domain = str(policy.data.get("fingerprint_domain", f"toolchain:{name}"))
                domains.setdefault(domain, []).append(name)
            results = {}
            if domains:
                futures = [
                    executor.submit(fingerprint_domain, domain)
                    for domain in domains.values()
                ]
                for future in futures:
                    results.update(future.result())
            for name in ready:
                if name in errors:
                    continue
                policy = policies[name]
                fingerprint, error = results[name]
                if error is not None:
                    errors[name] = error
                elif fingerprint is None:
                    errors[name] = f"executable {policy.data['executable']!r} not found"
                elif (
                    re.search(
                        str(policy.data["version_pattern"]), fingerprint["version"]
                    )
                    is None
                ):
                    errors[name] = (
                        f"version {fingerprint['version']!r} violates "
                        f"{policy.data['version_pattern']!r}"
                    )
                else:
                    for field in ("launcher_sha256", "executable_sha256"):
                        digest = fingerprint[field]
                        if len(digest) != 64 or any(
                            c not in "0123456789abcdef" for c in digest.lower()
                        ):
                            errors[name] = f"{field} is unavailable"
                            break
                    else:
                        if policy.python_identity_role == "runtime":
                            try:
                                validate_selected_python_fingerprint(
                                    policy, fingerprint
                                )
                            except (KeyError, TypeError, ValueError) as exc:
                                errors[name] = "".join(
                                    traceback.format_exception_only(exc)
                                ).strip()
                                continue
                        fingerprints[name] = fingerprint
            next_ready = []
            for name in ready:
                for dependent in dependents[name]:
                    remaining[dependent] -= 1
                    if not remaining[dependent]:
                        next_ready.append(dependent)
            ready = next_ready
    if errors:
        diagnostics = []
        for name in names:
            if name not in errors:
                continue
            policy = policies[name]
            owner = ", ".join(owners.get(name, ())) if owners else ""
            setup = policy.data.get("setup_command")
            suffix = f"; setup: {setup}" if setup else ""
            diagnostics.append(
                f"{name}"
                + (f" [{owner}]" if owner else "")
                + f": {errors[name]}{suffix}"
            )
        raise ValueError("toolchain contract violation: " + "; ".join(diagnostics))
    return fingerprints


def _cache_disposition(command: ProofCommand) -> str:
    domain = str(command.data["cache_domain"])
    if domain in {"none", "network"}:
        return "not-applicable"
    # Directory existence is not cache-hit evidence: a failed or partial build
    # also leaves directories behind. Until a command supplies counted restore
    # or rebuilt-byte telemetry, receipts must remain explicitly unobserved.
    return "unknown"


def _required_toolchains(plan: ProofPlan, command: ProofCommand) -> tuple[str, ...]:
    return plan.required_toolchains(command)


def _topological_commands(
    plan: ProofPlan,
    *,
    family: str | None = None,
    command_id: str | None = None,
    matrix_cell: str | None = None,
    tier: str | None = None,
) -> tuple[ProofCommand, ...]:
    by_id = {command.id: command for command in plan.commands}
    if family is not None:
        members = {
            command.id
            for command in plan.commands
            if command.family == family
            and (matrix_cell is None or command.data["cell"] == matrix_cell)
        }
        if not members:
            suffix = "" if matrix_cell is None else f" in matrix cell {matrix_cell!r}"
            raise ValueError(f"unknown or empty proof family {family!r}{suffix}")
        if tier is not None and family in {
            scheduled.name for scheduled in plan.scheduled_families
        }:
            tier = SCHEDULED_TIER
        # Tier filtering selects roots only; their dependencies still run.
        roots = {member for member in members if _command_in_tier(by_id[member], tier)}
        if not roots:
            suffix = "" if matrix_cell is None else f" in matrix cell {matrix_cell!r}"
            raise ValueError(
                f"proof family {family!r} has no commands in tier {tier!r}{suffix}"
            )
    elif command_id is not None:
        if command_id not in by_id:
            raise ValueError(f"unknown proof command {command_id!r}")
        roots = {command_id}
    else:
        raise ValueError("one of family or command_id is required")
    ordered: list[ProofCommand] = []
    emitted: set[str] = set()

    def emit(current: str) -> None:
        if current in emitted:
            return
        if current not in by_id:
            raise ValueError(f"unknown proof dependency {current!r}")
        for dependency in by_id[current].dependencies:
            emit(dependency)
        ordered.append(by_id[current])
        emitted.add(current)

    for current in (command.id for command in plan.commands if command.id in roots):
        emit(current)
    return tuple(ordered)


def _command_memory_share(
    plan: ProofPlan, commands: Iterable[ProofCommand]
) -> int | None:
    """The memory each command may claim while the executor runs others beside it.

    Count the selected resource classes within their existing parallel limits,
    capped by ``executor_max_workers``. This static upper bound preserves room
    for every possible concurrent command without dividing serialized compiler
    work by unused executor slots. Dependencies may reduce actual concurrency;
    this does not redistribute memory as commands finish.
    """
    selected = Counter(str(command.data["resource_class"]) for command in commands)
    if not selected:
        return None
    capacity = host_capacity.memory_capacity(os.environ)
    _total, available = capacity.bounded_bytes
    if available is None:
        return None
    resource_slots = sum(
        min(selected[policy.name], policy.max_parallel)
        for policy in plan.resource_policies
    )
    concurrency = max(1, min(plan.executor_max_workers, resource_slots))
    usable = max(0, available - host_capacity.headroom_bytes(capacity))
    return max(1, usable // concurrency)


def _command_environment(
    plan: ProofPlan,
    command: ProofCommand,
    timeout: int,
    *,
    environment: Mapping[str, str],
    memory_share: int | None,
) -> tuple[ProcessEnvironment, tuple[str, ...]]:
    """Resolve one command environment through shared Cargo invariants."""
    child_env = ProcessEnvironmentBuilder(environment)
    if memory_share is not None:
        child_env[host_capacity.MEMORY_SLICE_ENV] = str(memory_share)
    existing_pythonpath = child_env.get("PYTHONPATH", "")
    child_env["PYTHONPATH"] = os.pathsep.join(
        part for part in (str(ROOT / "src"), existing_pythonpath) if part
    )
    for name in command.data.get("timeout_env", []):
        child_env[str(name)] = str(timeout)
    child_env.update(
        {str(name): str(value) for name, value in command.data.get("env", {}).items()}
    )
    child_env[PROOF_COMMAND_TIMEOUT_ENV] = str(timeout)
    if "cargo" not in command.toolchains:
        return ProcessEnvironment(child_env), ()
    return normalize_cargo_environment(child_env)


def _admitted_linker_helpers(plan: ProofPlan) -> dict[str, frozenset[str]]:
    """Declared linker helper basename -> the linker basenames it may outlive.

    Toolchain capture already binds these helpers (MSVC's link.exe leaves
    vctip.exe uploading telemetry after it exits) with a terminate-on-root-exit
    disposition; Job closure honors the same declaration and nothing else.
    """
    helpers: dict[str, set[str]] = {}
    for policy in plan.toolchain_policies:
        for linker, names in (policy.data.get("linker_process_helpers") or {}).items():
            for name in names:
                helpers.setdefault(str(name).casefold(), set()).add(str(linker))
    return {name: frozenset(linkers) for name, linkers in helpers.items()}


def _admitted_job_survivor(
    process: object, admitted_linker_helpers: Mapping[str, Collection[str]]
) -> bool:
    """A Job survivor is admitted only as a declared helper beside its linker."""
    if not isinstance(process, dict) or not isinstance(process.get("image"), str):
        return False
    image = Path(process["image"])
    linkers = admitted_linker_helpers.get(image.name.casefold(), ())
    return image.is_absolute() and any(
        image.with_name(linker).is_file() for linker in linkers
    )


def _guarded_failure_scope(
    metrics: Mapping[str, Any],
    *,
    metrics_valid: bool,
    returncode: int,
    cancelled: bool,
    admitted_linker_helpers: Mapping[str, Collection[str]] | None = None,
) -> tuple[str, str | None]:
    """Classify the existing guard's evidence, never infer safe timeout from 124."""
    if not metrics_valid:
        return "global", "guard outcome unavailable or inconsistent"
    if metrics.get("infrastructure_failure") is not None:
        return "global", "guard infrastructure or ownership failure"
    if metrics.get("descendants_closed") is not True:
        return "global", "guard descendant closure is uncertain"
    if metrics.get("memory_violation") is not None:
        return "global", "unsafe memory pressure"
    if metrics.get("guard_signal") is not None:
        return "global", "guard interrupted by signal"
    reports = metrics.get("termination_reports", [])
    if not isinstance(reports, list) or any(
        not isinstance(report, dict)
        or not isinstance(report.get("remaining_pids"), list)
        or not isinstance(report.get("remaining_pgids"), list)
        or report.get("remaining_pids")
        or report.get("remaining_pgids")
        for report in reports
    ):
        return "global", "guard descendant closure is uncertain"
    cleanup = metrics.get("windows_job_cleanup")
    if cleanup is not None and (
        not isinstance(cleanup, dict)
        or cleanup.get("completed") is not True
        or not isinstance(cleanup.get("remaining_processes"), list)
        or not all(
            _admitted_job_survivor(process, admitted_linker_helpers or {})
            for process in cleanup["remaining_processes"]
        )
    ):
        return "global", "guard job closure is uncertain"
    quarantine = metrics.get("cargo_incremental_quarantine")
    if quarantine is not None and (
        not isinstance(quarantine, dict)
        or quarantine.get("ownership_status") not in {"quarantined", "not_required"}
        or not isinstance(quarantine.get("errors"), list)
        or quarantine.get("errors")
        or quarantine.get("interruption_inventory_complete") is not True
        or quarantine.get("interruption_inventory_error") is not None
        or (
            quarantine.get("ownership_status") == "not_required"
            and (
                quarantine.get("recovery_observations") != []
                or quarantine.get("moved_paths") != []
            )
        )
    ):
        return "global", "Cargo quarantine ownership or recovery is unresolved"
    if cancelled:
        from tools.memory_guard import GUARD_RETURN_CODE

        if metrics.get("cancelled") is not True or returncode != GUARD_RETURN_CODE:
            return "global", "executor cancellation lacks matching terminal outcome"
        return "global", "executor cancellation"
    if returncode == 124 or metrics.get("timed_out") is True:
        closed = bool(reports) or (
            isinstance(cleanup, dict) and cleanup.get("completed") is True
        )
        if returncode != 124 or metrics.get("timed_out") is not True or not closed:
            return "global", "timeout lacks confirmed guard closure"
        return "partition", "command deadline with confirmed guard closure"
    if metrics.get("exit_signal") is not None:
        return "global", "command ended by signal or host exception"
    return ("none", None) if returncode == 0 else ("partition", "command failed")


def _run_command(
    plan: ProofPlan,
    command: ProofCommand,
    cancel_event: threading.Event | None = None,
    *,
    environment: Mapping[str, str],
    memory_share: int | None,
) -> dict[str, Any]:
    relative_cwd = str(command.data.get("cwd", "."))
    cache = _cache_disposition(command)
    started_at = dt.datetime.now(dt.UTC).isoformat()
    timeout = int(command.data["timeout_seconds"])
    child_env, applied_environment_policies = _command_environment(
        plan, command, timeout, environment=environment, memory_share=memory_share
    )
    if cancel_event is not None and cancel_event.is_set():
        return {
            **proof_receipts.command_record(command),
            "started_at": started_at,
            "duration_seconds": 0.0,
            "peak_rss_bytes": None,
            "cache_disposition": cache,
            "status": "cancelled",
            "returncode": 130,
            "guard_metrics_schema": None,
            "cancelled_by_global_stop": True,
            "failure_scope": "global",
            "failure_reason": "executor cancellation before launch",
            "termination_escalated": False,
            "environment_policies_applied": list(applied_environment_policies),
            "evidence_outputs": [],
        }
    try:
        proof_receipts.clear_evidence_outputs(ROOT, command)
    except OSError as exc:
        return {
            **proof_receipts.command_record(command),
            "started_at": started_at,
            "duration_seconds": 0.0,
            "peak_rss_bytes": None,
            "cache_disposition": cache,
            "status": "failure",
            "returncode": 2,
            "guard_metrics_schema": None,
            "evidence_outputs": [],
            "evidence_error": f"cannot clear stale evidence: {exc}",
            "failure_scope": "partition",
            "failure_reason": "evidence publication failed before launch",
            "environment_policies_applied": list(applied_environment_policies),
        }
    process = None
    cancelled = False
    custody_error: Exception | None = None
    cancellation_started: float | None = None
    cancellation_elapsed: float | None = None
    try:
        process = _COMMANDS.start_guarded(
            resolved_command_argv(command),
            cwd=ROOT / relative_cwd,
            env=child_env,
            timeout=timeout,
            harness=True,
        )
        while process.poll() is None:
            if cancel_event is not None and cancel_event.is_set():
                cancelled = True
                cancellation_started = time.monotonic()
                process.cancel_and_wait()
                cancellation_elapsed = time.monotonic() - cancellation_started
                break
            try:
                # Poll only the owned launcher; terminal admission belongs to wait.
                process.process.wait(timeout=0.05)
            except subprocess.TimeoutExpired:
                pass
        if not process.terminal:
            process.wait(timeout=0)
    except Exception as exc:
        launch_error = process is None
        process = getattr(exc, "guard_command", process)
        if process is None:
            raise
        exc.guard_command = process
        if (
            process.returncode is None
            and cancellation_started is None
            and not launch_error
        ):
            # Observer failure does not abandon a running command. Preserve the
            # original error even if the owner's bounded observation also fails.
            cancellation_started = time.monotonic()
            try:
                process.cancel_and_wait()
            except Exception as cleanup_error:
                exc.cleanup_error = cleanup_error
                exc.add_note(f"guard cancellation unresolved: {cleanup_error}")
        custody_error = exc
        if cancellation_started is not None:
            cancellation_elapsed = time.monotonic() - cancellation_started
    completed_returncode = None if process is None else process.returncode
    try:
        metrics = json.loads(process.summary_path.read_text(encoding="utf-8"))
        if not isinstance(metrics, dict):
            metrics = {}
    except (OSError, json.JSONDecodeError):
        metrics = {}
    # Keep the canonical summary: an unresolved live owner may still replace it.
    metrics_valid = (
        custody_error is None
        and process is not None
        and process.terminal
        and metrics.get("schema") == "molt.guarded-command-metrics.v1"
        and metrics.get("returncode") == completed_returncode
        and isinstance(metrics.get("duration_seconds"), (int, float))
        and isinstance(metrics.get("peak_tree_rss_bytes"), int)
    )
    returncode = int(completed_returncode) if metrics_valid else 2
    status = (
        "cancelled"
        if cancelled and metrics_valid and metrics.get("cancelled") is True
        else "timeout"
        if returncode == 124
        else "success"
        if returncode == 0 and metrics_valid
        else "failure"
    )
    failure_scope, failure_reason = _guarded_failure_scope(
        metrics,
        metrics_valid=metrics_valid,
        returncode=returncode,
        cancelled=cancelled,
        admitted_linker_helpers=_admitted_linker_helpers(plan),
    )
    if failure_reason == "executor cancellation":
        # Raw guard cancellation remains 137. Only its admitted terminal
        # outcome becomes the executor's public cancellation code.
        status, returncode = "cancelled", 130
    elif failure_scope == "global" and (
        cancelled or status in {"success", "cancelled"}
    ):
        status, returncode = "failure", 2
    evidence_outputs: list[dict[str, Any]] = []
    evidence_error: str | None = None
    for relative in command.evidence_outputs:
        try:
            evidence_outputs.append(
                proof_receipts.snapshot_evidence_output(ROOT, relative)
            )
        except (OSError, ValueError) as exc:
            if status == "success":
                status = "failure"
                returncode = 2
                evidence_error = str(exc)
                failure_scope, failure_reason = (
                    "partition",
                    "declared evidence unavailable",
                )
            break
    record = {
        **proof_receipts.command_record(command),
        "started_at": started_at,
        "duration_seconds": (
            round(float(metrics["duration_seconds"]), 6) if metrics_valid else None
        ),
        "peak_rss_bytes": metrics.get("peak_tree_rss_bytes") if metrics_valid else None,
        "worker_peaks": metrics.get("worker_peaks") if metrics_valid else None,
        "peak_job_commit_bytes": (
            metrics.get("peak_job_commit_bytes") if metrics_valid else None
        ),
        "windows_job_cleanup": (
            metrics.get("windows_job_cleanup") if metrics_valid else None
        ),
        "cache_disposition": cache,
        "status": status,
        "returncode": returncode,
        "guard_metrics_schema": metrics.get("schema"),
        "cancelled_by_global_stop": cancelled,
        "failure_scope": failure_scope,
        "failure_reason": failure_reason,
        "guard_outcome": {
            name: metrics.get(name)
            for name in (
                "cancelled",
                "descendants_closed",
                "temporary_artifacts",
                "timed_out",
                "memory_violation",
                "guard_signal",
                "exit_signal",
                "infrastructure_failure",
                "termination_reports",
                "cargo_incremental_quarantine",
            )
        },
        "termination_escalated": False,
        "guard_returncode": completed_returncode,
        "guard_custody": {
            "launch_id": process.launch_id,
            "launch_pid": process.pid,
            "guard_pid": process.guard_pid,
            "startup_path": str(process.startup_path),
            "summary_path": str(process.summary_path),
            "cancellation_path": str(process.cancellation_path),
            "evidence_path": str(process.evidence_path),
            "terminal": process.terminal,
            "cancellation_observation_seconds": cancellation_elapsed,
            "cancellation_observation_budget_seconds": (
                GUARD_CANCELLATION_OBSERVATION_SECONDS
                if cancellation_started is not None
                else None
            ),
            "observation_error": None
            if custody_error is None
            else {
                "type": type(custody_error).__name__,
                "message": str(custody_error),
                "closure": "confirmed" if process.terminal else "unresolved",
                "cleanup_error": (
                    None
                    if getattr(custody_error, "cleanup_error", None) is None
                    else {
                        "type": type(custody_error.cleanup_error).__name__,
                        "message": str(custody_error.cleanup_error),
                    }
                ),
            },
        },
        "environment_policies_applied": list(applied_environment_policies),
        "evidence_outputs": evidence_outputs,
    }
    if evidence_error is not None:
        record["evidence_error"] = evidence_error
    if custody_error is not None:
        custody_error.proof_record = record
        raise custody_error
    return record


def execute_commands(
    plan: ProofPlan,
    commands: Iterable[ProofCommand],
    receipt_path: Path,
) -> int:
    commands = tuple(commands)
    baseline = MappingProxyType(dict(os.environ))
    memory_share = _command_memory_share(plan, commands)
    closures = {command.id: plan.required_toolchains(command) for command in commands}
    environments: dict[tuple[str, ...], Mapping[str, str]] = {}

    def admit(
        policies: Iterable[ToolchainPolicy], names: tuple[str, ...]
    ) -> dict[str, dict[str, Any]]:
        policies = tuple(policies)
        fingerprints = admit_toolchains(
            policies, names, environ=baseline, root=plan.path.parent.parent
        ).fingerprints
        # Admission reports every selected prerequisite before any execution.
        # Its union projection serves only its own probes, never a child's
        # target policy. Reuse one immutable projection per declared closure.
        for closure in dict.fromkeys(closures.values()):
            if closure not in environments:
                projection = project_toolchains(
                    policies, closure, environ=baseline, root=plan.path.parent.parent
                )
                environments[closure] = MappingProxyType(
                    {**baseline, **projection.delta}
                )
        return fingerprints

    return _execute_commands(
        plan,
        commands,
        receipt_path,
        _source_tree_changes=_source_tree_changes,
        toolchain_fingerprints=admit,
        _authority_sha256=_authority_sha256,
        _source_identity=_source_identity,
        _normalized_os=_normalized_os,
        _normalized_arch=_normalized_arch,
        _required_toolchains=lambda command: closures[command.id],
        _run_command=lambda plan, command, event: _run_command(
            plan,
            command,
            event,
            environment=environments[closures[command.id]],
            memory_share=memory_share,
        ),
        _cache_disposition=_cache_disposition,
    )


def replay_recent_commits(plan: ProofPlan, count: int) -> dict[str, Any]:
    return _proof_plan_cli.replay_recent_commits(
        plan,
        count,
        run_git=_run_git,
        diff_paths=_diff_paths,
    )


write_github_outputs = _proof_plan_cli.write_github_outputs


def main(argv: list[str] | None = None) -> int:
    return _proof_plan_cli.main(sys.modules[__name__], argv)


if __name__ == "__main__":
    raise SystemExit(main())
