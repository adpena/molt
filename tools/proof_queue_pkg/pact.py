"""Queue-native Pact and version-parity named-lane specifications."""

from __future__ import annotations

import argparse
import contextlib
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
from typing import Mapping, NotRequired, Sequence, TypedDict

from molt.browser_asset_closure import wasm_loader_asset_scope_paths
from molt.cli.source_extension_set_registry import (
    SourceExtensionSet,
    SourceExtensionVariant,
    load_source_extension_registry,
    source_extension_set,
    source_extension_set_expected_identity,
)
from molt.cli.source_build_environment import (
    LockedSourceBuildEnvironment,
    source_build_environment,
)
from molt.cli.source_extension_target import (
    SourceExtensionTargetPlan,
    resolve_source_extension_target_plan,
)
from molt.target_python import _parse_target_python_version
from molt.cli.source_extension_set_validation import (
    validate_source_extension_set_seal,
)
from molt.cli.source_package_seal import (
    SourcePackageSealVerificationError,
)
from molt.dx import DxConfigError, _reject_onedrive, checkout_custody
from tools import proof_plan
from molt.scientific_stack_versions import (
    CONFIG_ENV as SCIENTIFIC_STACK_CONFIG_ENV,
)
from molt.scientific_stack_versions import (
    PACT_WITNESS_DEPENDENCY_GROUP,
    ScientificStackVersion,
    resolve_scientific_stack,
    scientific_witness_seal_root,
    scientific_witness_variant,
)
from tools.proof_queue_pkg import command_admission, policy, runner, state


class NamedProofSpec(TypedDict):
    logical_id: str
    reason: str
    command: list[str]
    resource_family: str
    contention_key: str
    scopes: list[str]
    env_overrides: dict[str, str]
    locked_env: NotRequired[tuple[str, ...]]
    prepared_named_lane: NotRequired[str]
    cargo_output_lifetime: NotRequired[str]
    cargo_output_root: NotRequired[str | None]
    notes: list[str]
    timeout: float


def _scientific_extension_set_seal_validation(
    root: Path,
    extension_set: SourceExtensionSet,
    stack: ScientificStackVersion | None = None,
) -> tuple[list[str], Path | None]:
    selected = resolve_scientific_stack() if stack is None else stack
    try:
        validated = validate_source_extension_set_seal(
            root,
            extension_set,
            variant=scientific_witness_variant(stack=selected),
            registry=selected.source_extension_registry,
        )
    except (SourcePackageSealVerificationError, ValueError) as exc:
        return [str(exc)], None
    return [], validated.payload_root


def _scientific_extension_set_seal_problems(
    root: Path,
    extension_set: SourceExtensionSet,
    stack: ScientificStackVersion | None = None,
) -> list[str]:
    return _scientific_extension_set_seal_validation(root, extension_set, stack)[0]


def _pact_witness_extension_roots(repo_root: Path = state.ROOT) -> list[Path]:
    del repo_root
    stack = resolve_scientific_stack()
    variant = scientific_witness_variant(stack=stack)
    roots: list[Path] = []
    for package, display_name in (("numpy", "NumPy"), ("scipy", "SciPy")):
        extension_set = stack.extension_set(package, "pact-witness")
        durable_root = scientific_witness_seal_root(
            package,
            variant=variant,
            stack=stack,
        )
        problems, verified_payload_root = (
            _scientific_extension_set_seal_validation(
                durable_root, extension_set, stack
            )
            if durable_root.exists()
            else (["canonical root does not exist"], None)
        )
        if problems:
            raise ValueError(
                f"canonical {display_name} witness seal is absent or incomplete; "
                f"expected {durable_root} with exactly the configured extension set: "
                + "; ".join(problems)
            )
        assert verified_payload_root is not None
        roots.append(verified_payload_root)
    return roots


def _pact_witness_env_overrides(repo_root: Path = state.ROOT) -> dict[str, str]:
    # Force UTF-8 across the ENTIRE witness process tree (the parent tool + every
    # spawned build/gate subprocess). On Windows the default cp1252 stdio codec
    # raises UnicodeEncodeError on any non-cp1252 char in a relayed subprocess
    # capture (e.g. a gate's em-dash decoded to U+FFFD), which once aborted an
    # otherwise-SUCCESSFUL witness build after ~20 min. PYTHONUTF8=1 makes stdio
    # and the default file encoding UTF-8 tree-wide — the single-primitive fix for
    # this recurring encoding bug class. Set unconditionally (independent of the
    # native-root delta below) so the guarantee holds on every witness path.
    env: dict[str, str] = {
        "PYTHONUTF8": "1",
        "PYTHONIOENCODING": "utf-8",
        "MOLT_MODULE_ROOTS": "",
        "MOLT_EXTERNAL_STATIC_PACKAGES": "",
    }
    roots = _pact_witness_extension_roots(repo_root)
    if roots:
        env["MOLT_MODULE_ROOTS"] = os.pathsep.join(str(root) for root in roots)
        env["MOLT_EXTERNAL_STATIC_PACKAGES"] = "numpy scipy"
    return env


_PACT_WITNESS_ACCEPTANCE_LOGICAL_ID = "pact-witness-acceptance"

_PACT_WITNESS_ACCEPTANCE_LOCKED_ENV = (
    "MOLT_MODULE_ROOTS",
    "MOLT_EXTERNAL_STATIC_PACKAGES",
    "MOLT_WITNESS_EXPECTED_REPO_ROOT",
    "MOLT_WITNESS_EXPECTED_GIT_HEAD",
    SCIENTIFIC_STACK_CONFIG_ENV,
    "MOLT_EXT_ROOT",
    "MOLT_EXTERNAL_ARTIFACT_ROOTS",
    "PYTHONUTF8",
    "PYTHONIOENCODING",
)


_SOURCE_EXTENSION_PRODUCER_LOGICAL_ID = "source-extension-produce"
_SOURCE_EXTENSION_PRODUCER_LOCKED_ENV = (
    "PATH",
    "VIRTUAL_ENV",
    "PYTHONUTF8",
    "PYTHONIOENCODING",
)


@dataclass(frozen=True, slots=True)
class _SourceExtensionProducerPlan:
    repo_root: Path
    source_root: Path
    build_root: Path
    extension_set: SourceExtensionSet
    variant: SourceExtensionVariant
    target_plan: SourceExtensionTargetPlan
    environment: LockedSourceBuildEnvironment


def _source_extension_producer_plan(
    *,
    package: str,
    package_version: str,
    module_set: str,
    python_version: str,
    source: str,
    build_root: str,
    target: str,
    abi_tier: str,
    repo_root: Path,
) -> _SourceExtensionProducerPlan:
    """Resolve one immutable producer address without mutating source or tools."""

    from molt.cli.source_extension_producer import resolve_source_extension_destination

    root = repo_root.resolve(strict=True)
    source_root = Path(source).expanduser().resolve(strict=True)
    resolved_build_root = Path(build_root).expanduser().resolve(strict=False)
    try:
        _reject_onedrive(source_root, "source-extension source root")
        _reject_onedrive(resolved_build_root, "source-extension build root")
    except DxConfigError as exc:
        raise SystemExit(str(exc)) from exc
    registry = load_source_extension_registry()
    extension_set = source_extension_set(
        package,
        package_version,
        module_set,
        registry=registry,
    )
    target_plan = resolve_source_extension_target_plan(target)
    variant = SourceExtensionVariant(
        target_python=_parse_target_python_version(python_version),
        abi_tier=abi_tier,
        target_triple=target_plan.target_triple,
    )
    try:
        source_extension_set_expected_identity(
            extension_set, variant=variant, registry=registry
        )
    except ValueError as exc:
        raise SystemExit(
            "source-extension producer tuple is outside the registered verified "
            f"subset: {variant.cpython}/{variant.abi_tier}/{variant.target_triple}: "
            f"{exc}"
        ) from exc
    resolve_source_extension_destination(
        extension_set,
        variant=variant,
        source_root=source_root,
        build_root=resolved_build_root,
        registry=registry,
    )
    environment = source_build_environment(root, extension_set.build_dependency_group)
    return _SourceExtensionProducerPlan(
        repo_root=root,
        source_root=source_root,
        build_root=resolved_build_root,
        extension_set=extension_set,
        variant=variant,
        target_plan=target_plan,
        environment=environment,
    )


def _prepare_source_extension_producer(plan: _SourceExtensionProducerPlan) -> None:
    """Perform setup mutations before the guarded proof snapshots its inputs."""

    from molt.cli.source_extension_producer import (
        prepare_source_extension_prerequisites,
        resolve_source_extension_destination,
    )

    resolve_source_extension_destination(
        plan.extension_set,
        variant=plan.variant,
        source_root=plan.source_root,
        build_root=plan.build_root,
    )
    prepare_source_extension_prerequisites(
        plan.extension_set,
        plan.source_root,
        repo_root=plan.repo_root,
        planned_environment=plan.environment,
    )


def _source_extension_producer_spec_from_plan(
    plan: _SourceExtensionProducerPlan,
    *,
    expected_identity_sha256: str | None,
    expected_candidate_identity_sha256: str | None,
    timeout: float | None,
    json_output: bool = True,
) -> NamedProofSpec:
    from molt.cli.source_extension_invocation import SourceExtensionSetInvocation
    from molt.cli.source_extension_producer import _locked_console_tool_path

    package = plan.extension_set.package
    package_version = plan.extension_set.package_version
    module_set = plan.extension_set.name
    source_root = plan.source_root
    resolved_build_root = plan.build_root
    target_plan = plan.target_plan
    environment = plan.environment
    invocation = SourceExtensionSetInvocation(
        command="produce-set",
        prepared=True,
        package=package,
        package_version=package_version,
        module_set=module_set,
        python_version=plan.variant.cpython,
        source=str(source_root),
        build_root=str(resolved_build_root),
        # A frozen triple must not turn host CC/CXX selection into the explicit
        # cross-target MOLT_CROSS_CC/Zig lane. The envelope records both facts.
        target=(
            target_plan.requested
            if target_plan.compiler_target_triple is None
            else target_plan.target_triple
        ),
        abi_tier=plan.variant.abi_tier,
        json_output=json_output,
        expected_identity_sha256=expected_identity_sha256,
        expected_candidate_identity_sha256=expected_candidate_identity_sha256,
    )
    if expected_candidate_identity_sha256 is not None:
        expected = source_extension_set_expected_identity(
            plan.extension_set, variant=plan.variant
        )
        if expected_candidate_identity_sha256 != expected:
            raise ValueError(
                "source-extension expected candidate identity differs from the registered target cell"
            )
    command = list(invocation.module_argv(str(environment.python_executable)))
    env_overrides = {
        "PATH": _locked_console_tool_path(
            environment.python_executable.parent, os.environ.get("PATH")
        ),
        "VIRTUAL_ENV": str(environment.root.resolve()),
        "PYTHONUTF8": "1",
        "PYTHONIOENCODING": "utf-8",
    }
    return {
        "logical_id": (
            f"{_SOURCE_EXTENSION_PRODUCER_LOGICAL_ID}-{state._slug(package)}-"
            f"{state._slug(target_plan.target_triple)}"
        ),
        "reason": (
            f"Produce the registered {package} {package_version} {module_set} "
            f"source-extension set for {target_plan.target_triple} from its "
            "pre-attested locked Python environment."
        ),
        "command": command,
        "resource_family": "compiler-build-resource",
        "contention_key": (
            f"source-extension-{state._slug(package)}-"
            f"{state._slug(target_plan.target_triple)}"
        ),
        "scopes": [
            str(source_root),
            str(resolved_build_root),
            str(environment.root.resolve()),
        ],
        "env_overrides": env_overrides,
        "locked_env": _SOURCE_EXTENSION_PRODUCER_LOCKED_ENV,
        "notes": [
            "Environment provisioning is a separate setup mutation; the queued "
            "proof starts directly from the exact attested interpreter and may "
            "not provision or restart Python.",
        ],
        "timeout": timeout if timeout is not None else 3600.0,
    }


def _source_extension_producer_spec(
    *,
    package: str,
    package_version: str,
    module_set: str,
    python_version: str,
    source: str,
    build_root: str,
    target: str,
    abi_tier: str,
    expected_identity_sha256: str | None,
    expected_candidate_identity_sha256: str | None,
    timeout: float | None,
    repo_root: Path,
) -> NamedProofSpec:
    plan = _source_extension_producer_plan(
        package=package,
        package_version=package_version,
        module_set=module_set,
        python_version=python_version,
        source=source,
        build_root=build_root,
        target=target,
        abi_tier=abi_tier,
        repo_root=repo_root,
    )
    return _source_extension_producer_spec_from_plan(
        plan,
        expected_identity_sha256=expected_identity_sha256,
        expected_candidate_identity_sha256=expected_candidate_identity_sha256,
        timeout=timeout,
    )


def _pact_canonical_input_environment(repo_root: Path) -> dict[str, str]:
    """Resolve named Pact input custody without consulting ambient overrides."""
    root = repo_root.resolve()
    config_path = root / "config" / "scientific_stack_versions.toml"
    if not config_path.is_file():
        raise SystemExit(
            f"named Pact proof is missing canonical stack config {config_path}"
        )
    # Named seals are immutable inputs, not build capacity. Their identity is
    # anchored directly to durable Molt custody; volume labels, ambient output
    # variables, free-space thresholds, and fallback selection are irrelevant.
    canonical_artifact_root = str(checkout_custody(root, os.environ).custody_root)
    return {
        SCIENTIFIC_STACK_CONFIG_ENV: str(config_path.resolve()),
        "MOLT_EXT_ROOT": canonical_artifact_root,
        "MOLT_EXTERNAL_ARTIFACT_ROOTS": canonical_artifact_root,
    }


@contextlib.contextmanager
def _temporary_environment(overrides: Mapping[str, str]):
    previous = {name: os.environ.get(name) for name in overrides}
    try:
        os.environ.update(overrides)
        yield
    finally:
        for name, value in previous.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


def named_lane_argv(lane_id: str) -> list[str]:
    """The registered argv is the child-admission authority; never rebuild it here."""
    return list(proof_plan.ProofPlan.load().named_lane(lane_id).argv)


def _named_lane_spec(
    lane_id: str, timeout: float | None = None, repo_root: Path = state.ROOT
) -> dict[str, object]:
    del repo_root
    lane = proof_plan.ProofPlan.load().named_lane(lane_id)
    return {
        "logical_id": lane_id.replace(".", "-"),
        "reason": str(lane.data["description"]),
        "command": list(lane.argv),
        "cargo_output_lifetime": command_admission.parse_cargo_output_lifetime(
            lane.data.get("cargo_output_lifetime", "retain")
        ),
        "resource_family": str(lane.data["resource_family"]),
        "contention_key": str(lane.data["contention_key"]),
        "scopes": ["tools/proof_plan.toml"],
        "env_overrides": {},
        "notes": [
            f"named lane {lane_id}: argv and toolchain closure come from "
            "tools/proof_plan.toml; outputs go to the run's scratch root."
        ],
        "timeout": timeout
        if timeout is not None
        else float(lane.data["timeout_seconds"]),
    }


def _cmd_named_lane(args: argparse.Namespace) -> int:
    # A lane with a dedicated aperture (input custody, locked environment
    # names, provenance pins) runs through that aperture whichever entry point
    # names it: one lane id, one spec, so a generic submission can never strip
    # the custody the lane's tool fails closed without.
    dedicated = _DEDICATED_NAMED_LANE_HANDLERS.get(args.lane_id)
    if dedicated is not None:
        return dedicated(args)
    return _run_named_spec(
        args, _named_lane_spec(args.lane_id, args.timeout, state._repo_root(args))
    )


def _pact_witness_acceptance_spec(
    timeout: float | None = None, repo_root: Path = state.ROOT
) -> NamedProofSpec:
    canonical_inputs = _pact_canonical_input_environment(repo_root)
    with _temporary_environment(canonical_inputs):
        git_snapshot = state._git_snapshot(repo_root)
        expected_head = git_snapshot.get("head")
        if not isinstance(expected_head, str) or not expected_head:
            raise SystemExit(
                "pact-witness-acceptance requires a git worktree with a resolvable HEAD"
            )
        env_overrides = _pact_witness_env_overrides(repo_root)
    env_overrides.update(canonical_inputs)
    env_overrides.update(
        {
            "MOLT_WITNESS_EXPECTED_REPO_ROOT": str(repo_root.resolve()),
            "MOLT_WITNESS_EXPECTED_GIT_HEAD": expected_head,
        }
    )
    return {
        "logical_id": _PACT_WITNESS_ACCEPTANCE_LOGICAL_ID,
        "reason": (
            "Run the Pact Kernel A browser/WASM witness acceptance aperture "
            "through queue custody."
        ),
        "command": named_lane_argv("pact.witness.acceptance"),
        "prepared_named_lane": "pact.witness.acceptance",
        "resource_family": "wasm-browser",
        "contention_key": "wasm:pact-witness",
        "scopes": [
            "collab/pact/pact_witness_kernel/make_fixture.py",
            "collab/pact/pact_witness_kernel/field_solve.py",
            "collab/pact/pact_witness_kernel/check_parity.py",
            "wasm/run_wasm.js",
            "tools/pact_witness_acceptance.py",
            "config/scientific_stack_versions.toml",
            "pyproject.toml",
            "uv.lock",
            *wasm_loader_asset_scope_paths(),
        ],
        "env_overrides": env_overrides,
        "locked_env": _PACT_WITNESS_ACCEPTANCE_LOCKED_ENV,
        "notes": [
            "Named Pact acceptance requires the version-keyed durable NumPy "
            "and canonical scientific extension seals, builds field_solve.py, "
            "regenerates the fixture/reference oracle in the run directory, "
            "runs the WASM artifact to produce candidate_outputs.npz, and "
            "executes check_parity.py; --env remains available for diagnostics "
            "but cannot override the named lane's input and identity custody."
        ],
        "timeout": timeout if timeout is not None else 1800.0,
    }


def _pact_witness_oracle_spec(timeout: float | None = None) -> NamedProofSpec:
    return {
        "logical_id": "pact-witness-oracle-parity",
        "reason": (
            "Regenerate the Pact Kernel A fixture/reference pair and prove the "
            "check_parity.py oracle under queue custody."
        ),
        "command": named_lane_argv("pact.witness.oracle"),
        "prepared_named_lane": "pact.witness.oracle",
        "resource_family": "wasm-browser",
        "contention_key": "wasm:pact-witness",
        "scopes": [
            "collab/pact/pact_witness_kernel/make_fixture.py",
            "collab/pact/pact_witness_kernel/field_solve.py",
            "collab/pact/pact_witness_kernel/check_parity.py",
            "tools/pact_witness_oracle.py",
            "pyproject.toml",
            "uv.lock",
        ],
        "env_overrides": {},
        "notes": [],
        "timeout": timeout if timeout is not None else 900.0,
    }


_R6_TARGET_VERSION_PARITY_FILES = (
    "tests/differential/stdlib/sys_metadata_intrinsics.py",
    "tests/differential/stdlib/sys_stat_version_gate.py",
    "tests/differential/stdlib/stat_api_surface_versioned.py",
    "tests/differential/stdlib/queue_shutdown_version_gate.py",
    "tests/differential/stdlib/removed_stdlib_modules_version_gate.py",
)


def _normalize_r6_target_version_fixtures(
    requested: Sequence[str] | None,
) -> list[str]:
    if not requested:
        return list(_R6_TARGET_VERSION_PARITY_FILES)
    by_alias: dict[str, str] = {}
    for fixture in _R6_TARGET_VERSION_PARITY_FILES:
        path = Path(fixture)
        aliases = {
            fixture,
            fixture.replace("\\", "/"),
            path.name,
            path.stem,
        }
        for alias in aliases:
            by_alias[alias.lower()] = fixture
    selected: list[str] = []
    for raw in requested:
        normalized = raw.replace("\\", "/").lower()
        fixture = by_alias.get(normalized)
        if fixture is None:
            allowed = ", ".join(
                Path(item).name for item in _R6_TARGET_VERSION_PARITY_FILES
            )
            raise SystemExit(
                f"unknown R6 target-version fixture {raw!r}; choose one of: {allowed}"
            )
        if fixture not in selected:
            selected.append(fixture)
    return selected


def _r6_target_version_fixture_suffix(fixtures: Sequence[str]) -> str:
    if tuple(fixtures) == _R6_TARGET_VERSION_PARITY_FILES:
        return ""
    stems = [state._slug(Path(fixture).stem) for fixture in fixtures]
    suffix = "-".join(stems)
    if len(suffix) <= 96:
        return suffix
    digest = hashlib.sha256("|".join(fixtures).encode("utf-8")).hexdigest()[:10]
    return f"{stems[0]}-plus-{len(stems) - 1}-{digest}"


def _r6_target_version_parity_spec(
    python_version: str,
    timeout: float | None = None,
    fixtures: Sequence[str] | None = None,
) -> NamedProofSpec:
    normalized_version = python_version.strip()
    if not normalized_version:
        raise SystemExit("--python-version must not be empty")
    target_tag = "py" + "".join(normalized_version.split(".")[:2])
    selected_fixtures = _normalize_r6_target_version_fixtures(fixtures)
    fixture_suffix = _r6_target_version_fixture_suffix(selected_fixtures)
    logical_id = f"r6-target-version-parity-{target_tag}"
    if fixture_suffix:
        logical_id = f"{logical_id}-{fixture_suffix}"
    return {
        "logical_id": logical_id,
        "reason": (
            "Run the R6 target-version parity shard through queue custody with "
            "the differential harness and TargetPythonVersion command authority."
        ),
        "command": policy._uv_active_python_command(
            "tests/molt_diff.py",
            "--jobs",
            "1",
            "--python-version",
            normalized_version,
            "--build-profile",
            "dev",
            "--fail-fast",
            *selected_fixtures,
        ),
        "resource_family": "python",
        "contention_key": f"python:r6-target-version-{target_tag}",
        "scopes": [
            "src/molt/python_interpreter.py",
            "tests/molt_diff.py",
            "src/molt/target_python.py",
            "src/molt/stdlib/sys.py",
            "src/molt/stdlib/stat.py",
            "src/molt/stdlib/queue.py",
            *selected_fixtures,
        ],
        "env_overrides": {},
        "notes": [
            "Named R6 parity lane runs sys metadata plus stdlib version-gated "
            "stat, queue shutdown, and PEP 594 removed-module fixtures with "
            "serial fail-fast differential custody; missing target interpreters "
            "fail closed through src/molt/python_interpreter.py.",
            "Selected R6 fixtures: " + ", ".join(selected_fixtures),
        ],
        "timeout": timeout if timeout is not None else 900.0,
    }


def _native_molt_run_spec(
    entry: str,
    *,
    script_args: Sequence[str] | None = None,
    timeout: float | None = None,
    repo_root: Path = state.ROOT,
) -> NamedProofSpec:
    root = repo_root.resolve()
    entry_path = Path(entry)
    if not entry_path.is_absolute():
        entry_path = root / entry_path
    entry_path = entry_path.resolve()
    try:
        rel_entry = entry_path.relative_to(root)
    except ValueError as exc:
        raise SystemExit(
            f"native Molt run entry must live under repo root {root}: {entry_path}"
        ) from exc
    if not entry_path.is_file():
        raise SystemExit(f"native Molt run entry does not exist: {entry_path}")
    entry_scope = rel_entry.as_posix()
    arg_list = list(script_args or [])
    if arg_list[:1] == ["--"]:
        arg_list = arg_list[1:]
    entry_slug = state._slug(entry_scope)
    digest = hashlib.sha256(entry_scope.encode("utf-8")).hexdigest()[:10]
    return {
        "logical_id": f"native-molt-run-{entry_slug}-{digest}",
        "reason": (
            "Run a native Molt entrypoint through proof-queue custody instead "
            "of a foreground Codex shell compile."
        ),
        "command": policy._uv_active_python_command(
            "-m",
            "molt.cli",
            "run",
            entry_scope,
            *arg_list,
        ),
        "resource_family": "python-native",
        "contention_key": f"python:native-molt-run:{entry_slug}",
        "scopes": [entry_scope],
        "env_overrides": {},
        "notes": [
            "Named native Molt run lane prevents compile-heavy `molt run` probes "
            "from occupying the foreground Codex control plane; use --detach "
            "and `proof_queue.py run --jobs N --detach` for cross-platform "
            "bounded worker fanout.",
            "Native Molt entry: " + entry_scope,
        ],
        "timeout": timeout if timeout is not None else 900.0,
    }


def _run_named_spec(args: argparse.Namespace, spec: NamedProofSpec) -> int:
    output_root = getattr(args, "cargo_output_root", None)
    if output_root is None:
        output_root = spec.get("cargo_output_root")
    lifetime = getattr(args, "cargo_output_lifetime", None)
    if lifetime is None:
        lifetime = spec.get("cargo_output_lifetime", "retain")
    if lifetime != "retain" or output_root is not None:
        command_admission.envelope_for_command(
            spec["command"],
            cargo_output_lifetime=lifetime,
            cargo_output_root=output_root,
        )
    prepared_lane = spec.get("prepared_named_lane")
    if prepared_lane is not None:
        from molt.cli.source_extension_producer import _locked_console_tool_path

        locked = tuple(
            dict.fromkeys(
                (
                    *spec.get("locked_env", ()),
                    *_SOURCE_EXTENSION_PRODUCER_LOCKED_ENV,
                )
            )
        )
        # Refuse redirection before any setup mutation, then resolve/provision
        # using the same typed environment authority as source producers.
        policy._named_spec_user_env_overrides(
            spec["logical_id"],
            policy._named_spec_locked_env(spec["logical_id"], locked),
            args.env,
        )
        with _temporary_environment(spec["env_overrides"]):
            environment = source_build_environment(
                state._repo_root(args),
                PACT_WITNESS_DEPENDENCY_GROUP,
                provision=not args.print_spec,
            )
        spec = {
            **spec,
            "command": command_admission.prepared_named_lane_command(
                prepared_lane, environment.python_executable
            ),
            "scopes": [*spec["scopes"], str(environment.root.resolve())],
            "locked_env": locked,
            "env_overrides": {
                **spec["env_overrides"],
                "PATH": _locked_console_tool_path(
                    environment.python_executable.parent, os.environ.get("PATH")
                ),
                "VIRTUAL_ENV": str(environment.root.resolve()),
                "PYTHONUTF8": "1",
                "PYTHONIOENCODING": "utf-8",
            },
        }
    env_overrides = policy._named_spec_env_overrides(spec, args.env)
    initial_notes = list(spec["notes"])
    initial_notes.extend(getattr(args, "note", []) or [])
    runnable: NamedProofSpec = {
        **spec,
        "cargo_output_lifetime": lifetime,
        **({"cargo_output_root": output_root} if output_root is not None else {}),
        "env_overrides": env_overrides,
    }
    if args.print_spec:
        print(json.dumps(runnable, indent=2, sort_keys=True))
        return 0
    queue_only = getattr(args, "queue_only", False)
    if queue_only or getattr(args, "detach", False):
        rc, run_id = runner._queue_one(
            args,
            logical_id=runnable["logical_id"],
            reason=runnable["reason"],
            command=list(runnable["command"]),
            cargo_output_lifetime=lifetime,
            cargo_output_root=output_root,
            resource_family=runnable["resource_family"],
            contention_key=runnable["contention_key"],
            scopes=list(runnable["scopes"]),
            env_overrides=dict(runnable["env_overrides"]),
            initial_notes=initial_notes,
            depends_on=getattr(args, "depends_on", []) or [],
            edge_kind=getattr(args, "edge_kind", state.DEFAULT_EDGE_KIND),
            edge_note=getattr(args, "edge_note", None),
        )
        if queue_only or rc != 0 or run_id is None:
            return rc
        with contextlib.closing(state._connect(state._db_path(args))) as conn:
            dispatch = runner._dispatch_detached_runner(
                args,
                conn,
                run_id=run_id,
                timeout=runnable["timeout"],
            )
        if dispatch is None:
            return 0
        pid, runner_log = dispatch
        print(f"detached {run_id} runner_pid={pid}")
        print(f"runner_log: {runner_log}")
        return 0
    return runner._run_one(
        args,
        logical_id=runnable["logical_id"],
        reason=runnable["reason"],
        command=list(runnable["command"]),
        cargo_output_lifetime=lifetime,
        cargo_output_root=output_root,
        resource_family=runnable["resource_family"],
        contention_key=runnable["contention_key"],
        scopes=list(runnable["scopes"]),
        env_overrides=dict(runnable["env_overrides"]),
        timeout=runnable["timeout"],
        initial_notes=initial_notes,
        depends_on=getattr(args, "depends_on", []) or [],
        edge_kind=getattr(args, "edge_kind", state.DEFAULT_EDGE_KIND),
        edge_note=getattr(args, "edge_note", None),
    )


def _cmd_pact_witness_acceptance(args: argparse.Namespace) -> int:
    # Admission must precede seal/config resolution so a forbidden override
    # cannot redirect spec construction or turn a policy refusal into a producer
    # traceback. _run_named_spec revalidates the completed spec before use.
    policy._named_spec_user_env_overrides(
        _PACT_WITNESS_ACCEPTANCE_LOGICAL_ID,
        policy._named_spec_locked_env(
            _PACT_WITNESS_ACCEPTANCE_LOGICAL_ID,
            _PACT_WITNESS_ACCEPTANCE_LOCKED_ENV,
        ),
        args.env,
    )
    return _run_named_spec(
        args, _pact_witness_acceptance_spec(args.timeout, state._repo_root(args))
    )


def _cmd_source_extension_produce(args: argparse.Namespace) -> int:
    policy._named_spec_user_env_overrides(
        _SOURCE_EXTENSION_PRODUCER_LOGICAL_ID,
        _SOURCE_EXTENSION_PRODUCER_LOCKED_ENV,
        args.env,
    )
    plan = _source_extension_producer_plan(
        package=args.package,
        package_version=args.package_version,
        module_set=args.module_set,
        python_version=args.python_version,
        source=args.source,
        build_root=args.build_root,
        target=args.target,
        abi_tier=args.abi_tier,
        repo_root=state._repo_root(args),
    )
    spec = _source_extension_producer_spec_from_plan(
        plan,
        expected_identity_sha256=args.expected_identity_sha256,
        expected_candidate_identity_sha256=args.expected_candidate_identity_sha256,
        timeout=args.timeout,
        json_output=args.json,
    )
    if not args.print_spec:
        _prepare_source_extension_producer(plan)
    return _run_named_spec(args, spec)


def _cmd_pact_witness_oracle(args: argparse.Namespace) -> int:
    return _run_named_spec(args, _pact_witness_oracle_spec(args.timeout))


_DEDICATED_NAMED_LANE_HANDLERS = {
    "pact.witness.acceptance": _cmd_pact_witness_acceptance,
    "pact.witness.oracle": _cmd_pact_witness_oracle,
}


def _cmd_r6_target_version_parity(args: argparse.Namespace) -> int:
    return _run_named_spec(
        args,
        _r6_target_version_parity_spec(
            args.python_version,
            args.timeout,
            args.fixture,
        ),
    )


def _cmd_native_molt_run(args: argparse.Namespace) -> int:
    return _run_named_spec(
        args,
        _native_molt_run_spec(
            args.entry,
            script_args=args.script_args,
            timeout=args.timeout,
            repo_root=state._repo_root(args),
        ),
    )
