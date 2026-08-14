"""Queue-native Pact and version-parity named-lane specifications."""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
from typing import Mapping, Sequence

from molt.browser_asset_closure import wasm_loader_asset_scope_paths
from molt.cli.source_extension_set_registry import (
    SourceExtensionSet,
)
from molt.cli.source_extension_set_validation import (
    validate_source_extension_set_seal,
)
from molt.cli.source_package_seal import (
    SourcePackageSealVerificationError,
)
from molt.dx import checkout_custody
from molt.scientific_stack_versions import (
    CONFIG_ENV as SCIENTIFIC_STACK_CONFIG_ENV,
)
from molt.scientific_stack_versions import (
    ScientificStackVersion,
    resolve_scientific_stack,
    scientific_witness_seal_root,
    scientific_witness_variant,
)
from tools.proof_queue_pkg import policy, runner, state


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

_PACT_WITNESS_REQUIREMENTS = "config/proof_requirements/pact_witness.txt"

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


def _pact_witness_acceptance_spec(
    timeout: float | None = None, repo_root: Path = state.ROOT
) -> dict[str, object]:
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
        "command": policy._uv_active_python_command(
            "tools/pact_witness_acceptance.py",
            "--out-dir",
            "tmp/pact_witness_acceptance_queue",
            with_requirements=_PACT_WITNESS_REQUIREMENTS,
        ),
        "resource_family": "wasm-browser",
        "contention_key": "wasm:pact-witness",
        "scopes": [
            "collab/pact/pact_witness_kernel/make_fixture.py",
            "collab/pact/pact_witness_kernel/field_solve.py",
            "collab/pact/pact_witness_kernel/check_parity.py",
            "wasm/run_wasm.js",
            "tools/pact_witness_acceptance.py",
            "config/scientific_stack_versions.toml",
            _PACT_WITNESS_REQUIREMENTS,
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


def _pact_witness_oracle_spec(timeout: float | None = None) -> dict[str, object]:
    return {
        "logical_id": "pact-witness-oracle-parity",
        "reason": (
            "Regenerate the Pact Kernel A fixture/reference pair and prove the "
            "check_parity.py oracle under queue custody."
        ),
        "command": policy._uv_active_python_command(
            "tools/pact_witness_oracle.py",
            with_requirements=_PACT_WITNESS_REQUIREMENTS,
        ),
        "resource_family": "wasm-browser",
        "contention_key": "wasm:pact-witness",
        "scopes": [
            "collab/pact/pact_witness_kernel/make_fixture.py",
            "collab/pact/pact_witness_kernel/field_solve.py",
            "collab/pact/pact_witness_kernel/check_parity.py",
            "tools/pact_witness_oracle.py",
            _PACT_WITNESS_REQUIREMENTS,
        ],
        "env_overrides": {},
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
) -> dict[str, object]:
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
) -> dict[str, object]:
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


def _run_named_spec(args: argparse.Namespace, spec: dict[str, object]) -> int:
    env_overrides = policy._named_spec_env_overrides(spec, args.env)
    initial_notes = state._notes_from_raw(spec.get("note"))
    initial_notes.extend(state._notes_from_raw(spec.get("notes")))
    initial_notes.extend(getattr(args, "note", []) or [])
    runnable = {
        **spec,
        "env_overrides": env_overrides,
    }
    if args.print_spec:
        print(json.dumps(runnable, indent=2, sort_keys=True))
        return 0
    if getattr(args, "queue_only", False):
        rc, _run_id = runner._queue_one(
            args,
            logical_id=str(runnable["logical_id"]),
            reason=str(runnable["reason"]),
            command=list(runnable["command"]),
            resource_family=str(runnable["resource_family"]),
            contention_key=str(runnable["contention_key"]),
            scopes=list(runnable["scopes"]),
            env_overrides=dict(runnable["env_overrides"]),
            initial_notes=initial_notes,
            depends_on=getattr(args, "depends_on", []) or [],
            edge_kind=getattr(args, "edge_kind", state.DEFAULT_EDGE_KIND),
            edge_note=getattr(args, "edge_note", None),
        )
        return rc
    if getattr(args, "detach", False):
        rc, run_id = runner._queue_one(
            args,
            logical_id=str(runnable["logical_id"]),
            reason=str(runnable["reason"]),
            command=list(runnable["command"]),
            resource_family=str(runnable["resource_family"]),
            contention_key=str(runnable["contention_key"]),
            scopes=list(runnable["scopes"]),
            env_overrides=dict(runnable["env_overrides"]),
            initial_notes=initial_notes,
            depends_on=getattr(args, "depends_on", []) or [],
            edge_kind=getattr(args, "edge_kind", state.DEFAULT_EDGE_KIND),
            edge_note=getattr(args, "edge_note", None),
        )
        if rc != 0 or run_id is None:
            return rc
        conn = state._connect(state._db_path(args))
        dispatch = runner._dispatch_detached_runner(
            args,
            conn,
            run_id=run_id,
            timeout=float(runnable["timeout"]),
        )
        if dispatch is None:
            return 0
        pid, runner_log = dispatch
        print(f"detached {run_id} runner_pid={pid}")
        print(f"runner_log: {runner_log}")
        return 0
    return runner._run_one(
        args,
        logical_id=str(runnable["logical_id"]),
        reason=str(runnable["reason"]),
        command=list(runnable["command"]),
        resource_family=str(runnable["resource_family"]),
        contention_key=str(runnable["contention_key"]),
        scopes=list(runnable["scopes"]),
        env_overrides=dict(runnable["env_overrides"]),
        timeout=float(runnable["timeout"]),
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
        _PACT_WITNESS_ACCEPTANCE_LOCKED_ENV,
        args.env,
    )
    return _run_named_spec(
        args, _pact_witness_acceptance_spec(args.timeout, state._repo_root(args))
    )


def _cmd_pact_witness_oracle(args: argparse.Namespace) -> int:
    return _run_named_spec(args, _pact_witness_oracle_spec(args.timeout))


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
