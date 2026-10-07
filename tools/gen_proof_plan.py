#!/usr/bin/env python3
"""Generate and verify projections of the canonical Molt proof plan."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
from pathlib import Path

from proof_plan import DEFAULT_MANIFEST, ProofPlan, _authority_sha256
from generator_io import generator_main, write_generated_texts
from molt.cargo_execution_policy import (
    PROOF_COMMAND_TIMEOUT_ENV,
    load_ci_cargo_policy,
)


ROOT = Path(__file__).resolve().parents[1]
JSON_OUTPUT = ROOT / ".github" / "proof-plan.generated.json"
DOC_OUTPUT = ROOT / "docs" / "agent" / "PROOF_PLAN.generated.md"

# The staged interpreter has no site imports or ambient import roots. Project
# modules compile only their captured source, even if a staged cache or native
# extension could otherwise take precedence over the matching .py file.
_STAGED_SOURCE_BOOTSTRAP = """
import importlib.machinery
from pathlib import Path
import sys
_staged_root = Path(sys.argv[1]).resolve(strict=True)
class _StagedSourceLoader(importlib.machinery.SourceFileLoader):
    def get_code(self, fullname):
        filename = self.get_filename(fullname)
        return self.source_to_code(self.get_data(filename), filename)
def _staged_source_hook(path):
    try:
        Path(path).resolve(strict=True).relative_to(_staged_root)
    except (OSError, ValueError):
        raise ImportError
    return importlib.machinery.FileFinder(
        path, (_StagedSourceLoader, importlib.machinery.SOURCE_SUFFIXES)
    )
sys.path_hooks.insert(0, _staged_source_hook)
sys.path[:0] = [str(_staged_root / 'tools'), str(_staged_root / 'src'), str(_staged_root)]
_staged_script = _staged_root / 'tools' / 'gen_proof_plan.py'
sys.argv = [str(_staged_script), *sys.argv[2:]]
__file__ = str(_staged_script)
__spec__ = None
exec(compile(_staged_script.read_bytes(), __file__, 'exec', dont_inherit=True), globals())
"""


def _index_projection(*, manifest: Path, check: bool) -> int:
    from molt.compiler_distribution import (
        MAX_SOURCE_BYTES,
        MAX_SOURCE_FILES,
        verify_source_inventory,
    )
    from molt.artifact_publication import is_publication_lock_file
    from molt.temporary_artifacts import OwnedTemporaryDirectory
    from molt.toolchain_identity import capture_stable_regular_file, resolve_executable
    from tools.command_execution import CommandExecutor
    from tools.release.git_source_snapshot import (
        capture_git_index_source_snapshot,
        fenced_git_index,
        materialize_git_source_snapshot,
    )

    environment = dict(os.environ)
    git = resolve_executable("git", environment=environment, label="staged source Git")
    try:
        relative_manifest = manifest.absolute().relative_to(ROOT)
    except ValueError as exc:
        raise ValueError(
            "staged proof-plan manifest must be inside its repository"
        ) from exc
    snapshot = capture_git_index_source_snapshot(
        ROOT,
        git=git,
        environment=environment,
        required_markers=frozenset(
            {"tools/gen_proof_plan.py", relative_manifest.as_posix()}
        ),
        max_files=MAX_SOURCE_FILES,
        max_bytes=MAX_SOURCE_BYTES,
    )
    output_paths = (JSON_OUTPUT.relative_to(ROOT), DOC_OUTPUT.relative_to(ROOT))
    commands = CommandExecutor.for_file(__file__)
    with OwnedTemporaryDirectory(prefix="molt-staged-proof-") as temporary:
        output_root = Path(temporary) / "projections"
        staged_root = materialize_git_source_snapshot(
            snapshot,
            Path(temporary) / "source",
            repo_root=ROOT,
            git=git,
            environment=environment,
        )
        argv = [
            sys.executable,
            "-I",
            "-S",
            "-B",
            "-c",
            _STAGED_SOURCE_BOOTSTRAP,
            str(staged_root),
            "--manifest",
            str(staged_root / relative_manifest),
        ]
        if check:
            argv.append("--check")
        else:
            argv.extend(("--write", "--output-root", str(output_root)))
        completed = commands.run(
            argv,
            cwd=staged_root,
            env=environment,
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=120,
        )
        if completed.stdout:
            print(completed.stdout, end="")
        if completed.stderr:
            print(completed.stderr, file=sys.stderr, end="")
        if completed.returncode:
            return completed.returncode
        snapshot.verify(staged_root)
        if check:
            snapshot.verify_index(repo_root=ROOT, git=git, environment=environment)
            return 0
        # Output is separate from the captured source. Admit exactly the two
        # projections and verified persistent metadata from their publication
        # authority; every other file or source mutation rejects publication.
        outputs = {}
        records = []
        for relative in output_paths:
            identity, data = capture_stable_regular_file(
                output_root / relative,
                label="staged proof-plan output",
                max_bytes=16 * 1024 * 1024,
            )
            outputs[relative] = data.decode("utf-8")
            records.append(
                {
                    "path": relative.as_posix(),
                    "mode": 0o100644,
                    "size": identity.size,
                    "sha256": identity.sha256,
                }
            )
        for parent in {relative.parent for relative in output_paths}:
            for path in (output_root / parent).iterdir():
                if is_publication_lock_file(path):
                    records.append(
                        {
                            "path": path.relative_to(output_root).as_posix(),
                            "mode": 0o100644,
                            "size": 0,
                            "sha256": hashlib.sha256(b"").hexdigest(),
                        }
                    )
        verify_source_inventory(output_root, tuple(records))
        with fenced_git_index(
            snapshot, repo_root=ROOT, git=git, environment=environment
        ):
            write_generated_texts(
                {ROOT / relative: content for relative, content in outputs.items()}
            )
    return 0


def _envelope_record(
    plan: ProofPlan, family_name: str, budget: int, **scope: str | None
) -> dict[str, object]:
    envelope = plan.timeout_envelope(family_name, **scope)
    return {
        "budget_seconds": budget,
        "projected_makespan_seconds": envelope.projected_makespan_seconds,
        "critical_path_seconds": envelope.critical_path_seconds,
        "resource_capacity_floor_seconds": envelope.resource_capacity_floor_seconds,
        "headroom_seconds": budget - envelope.projected_makespan_seconds,
    }


def _timeout_envelope_projection(plan: ProofPlan) -> dict[str, dict[str, object]]:
    return {
        family.name: _envelope_record(
            plan, family.name, int(family.data["timeout_minutes"]) * 60
        )
        for family in plan.families
        if family.data["executor"] == "github-job"
    }


def _matrix_timeout_envelope_projection(
    plan: ProofPlan,
) -> dict[str, dict[str, dict[str, object]]]:
    """One envelope per runner cell: each matrix job runs exactly one cell."""
    return {
        family.name: {
            cell: _envelope_record(
                plan,
                family.name,
                int(family.data["timeout_minutes"]) * 60,
                matrix_cell=cell,
            )
            for cell in plan.family_cells(family.name)
        }
        for family in plan.families
        if family.data["executor"] == "github-matrix"
    }


def _scheduled_timeout_envelope_projection(
    plan: ProofPlan,
) -> dict[str, dict[str, object]]:
    return {
        family.name: _envelope_record(
            plan, family.name, int(family.data["timeout_minutes"]) * 60
        )
        for family in plan.scheduled_families
    }


def _json_projection(plan: ProofPlan) -> str:
    shared_cargo_policy = load_ci_cargo_policy()
    cargo_policy = shared_cargo_policy.execution_budgets
    local_commands = {
        **{
            f"local.always.{index}": command
            for index, command in enumerate(plan.always)
        },
        **{
            f"local.{rule['name']}.{index}": command
            for rule in plan.local_rules
            for index, command in enumerate(rule.get("gates", []))
        },
    }
    local_rules = [
        {
            **rule,
            "command_ids": [
                f"local.{rule['name']}.{index}"
                for index, _command in enumerate(rule.get("gates", []))
            ],
        }
        for rule in plan.local_rules
    ]
    payload = {
        "schema": "molt.proof-plan-projection.v6",
        "authority": str(plan.path.relative_to(ROOT)).replace("\\", "/"),
        "authority_inputs": list(plan.authority_inputs),
        "authority_sha256": _authority_sha256(plan),
        "receipt_schema": plan.receipt_schema,
        "executor": {
            "max_workers": plan.executor_max_workers,
            "inventory_hash_workers": plan.inventory_hash_workers,
            "resource_policies": [
                {"name": policy.name, "max_parallel": policy.max_parallel}
                for policy in plan.resource_policies
            ],
            "github_job_timeout_envelopes": _timeout_envelope_projection(plan),
            "github_matrix_timeout_envelopes": (
                _matrix_timeout_envelope_projection(plan)
            ),
            "scheduled_job_timeout_envelopes": (
                _scheduled_timeout_envelope_projection(plan)
            ),
        },
        "ci_families": [family.data for family in plan.families],
        "scheduled_families": [family.data for family in plan.scheduled_families],
        "matrix_cells": [cell.data for cell in plan.matrix_cells],
        "commands": [command.data for command in plan.commands],
        "toolchain_policies": [policy.data for policy in plan.toolchain_policies],
        "cargo_environment_policy": {
            "wrapper_environment_names": list(
                shared_cargo_policy.environment.wrapper_environment_names
            ),
            "incident_run_id": shared_cargo_policy.environment.incident_run_id,
            "incident_job_id": shared_cargo_policy.environment.incident_job_id,
            "incident_commit": shared_cargo_policy.environment.incident_commit,
            "incident_command": shared_cargo_policy.environment.incident_command,
        },
        "cargo_execution_policy": {
            "owning_proof_timeout_environment": PROOF_COMMAND_TIMEOUT_ENV,
            "timeout_seconds_by_class": dict(cargo_policy.timeout_seconds_by_class),
            "observed_cold_timeout_seconds": (
                cargo_policy.observed_cold_timeout_seconds
            ),
            "minimum_cold_headroom_multiplier": (
                cargo_policy.minimum_cold_headroom_multiplier
            ),
            "measurement_run_id": cargo_policy.measurement_run_id,
            "measurement_job_id": cargo_policy.measurement_job_id,
            "measurement_commit": cargo_policy.measurement_commit,
            "measurement_command": cargo_policy.measurement_command,
        },
        "local": {
            "always": list(plan.always),
            "always_command_ids": [
                f"local.always.{index}" for index, _command in enumerate(plan.always)
            ],
            "commands": local_commands,
            "rules": local_rules,
        },
    }
    return json.dumps(payload, indent=2, sort_keys=True) + "\n"


def _markdown_projection(plan: ProofPlan) -> str:
    shared_cargo_policy = load_ci_cargo_policy()
    cargo_policy = shared_cargo_policy.execution_budgets
    unique_local_commands = {
        command for rule in plan.local_rules for command in rule.get("gates", [])
    } | set(plan.always)
    lines = [
        "# Generated proof plan",
        "",
        "> Generated by `tools/gen_proof_plan.py` from "
        "`tools/proof_plan.toml`; do not edit.",
        "",
        "## Generation and partial commits",
        "",
        "`uv run python tools/gen_proof_plan.py` projects the working source. "
        "For a partial commit, stage the complete changed authority family, run "
        "`uv run python tools/gen_proof_plan.py --from-index`, then stage both "
        "generated outputs. The pre-commit freshness hook uses "
        "`--check --from-index`.",
        "",
        "Index mode executes the captured staged generator with its staged "
        "manifest, policy and source imports. It preserves unstaged work and "
        "the caller's index; private Git plumbing disables repository hooks "
        "and filesystem monitors. Generated outputs publish as one recoverable "
        "family after the captured source is verified and the selected Git "
        "index is revalidated under its exclusive lock. A competing index "
        "writer, indirect output, source mutation or undeclared output rejects "
        "publication. The lock excludes cooperative Git writers; it does not "
        "establish kernel-level byte-use or process execution identity.",
        "",
        "## Authority compression",
        "",
        "| Metric | Before report 001 | Current |",
        "|---|---:|---:|",
        "| Hand-maintained path-to-proof authorities | 4 | 1 |",
        f"| CI selection families | 5 | {len(plan.families)} |",
        f"| Hashed executable authority inputs | 1 | {len(plan.authority_inputs)} |",
        f"| Local path rules | 35 | {len(plan.local_rules)} |",
        f"| Unique local commands | 73 | {len(unique_local_commands)} |",
        "| Handwritten Python classifier rule tables | 5 | 0 |",
        "",
        "## CI families",
        "",
        "Every selected family expands to stable command IDs. Each command binds "
        "an exact OS/architecture/Python/backend/target/profile cell, timeout, "
        "resource class, cache domain, and DAG parents. CI admission requires "
        "receipts whose canonical LF-normalized authority-closure digest, source "
        "commit and immutable Git tree, command, cell, execution partition, duration, peak RSS, cache "
        "disposition, and version-constrained toolchain identities validate.",
        "",
        "Proof-family selection parents and GitHub admission edges are distinct "
        "authorities. A family may depend on another family only when it consumes "
        "that family's data or control result. Independent admissions depend only "
        "on the changed-path classifier, so a selected sibling failure cannot mask "
        "their execution; the Proof Plan Verdict remains the sole conjunction.",
        "",
        "The canonical executor admits dependency-ready commands in manifest "
        "order, bounds global fanout at "
        f"{plan.executor_max_workers}, and enforces these per-resource limits:",
        f"Installed Python custody uses {plan.inventory_hash_workers} bounded "
        "hash workers over deterministic coarse file batches; prelaunch and "
        "postcompletion both read the complete admitted byte inventory.",
        "",
        "| Resource | Max parallel commands |",
        "|---|---:|",
        *[
            f"| `{policy.name}` | {policy.max_parallel} |"
            for policy in plan.resource_policies
        ],
        "",
        "GitHub job budgets are validated against a deterministic worst-case "
        "DAG schedule in which every admitted command consumes its full declared "
        "timeout. The projection accounts for dependencies, the global worker "
        "ceiling, and per-resource capacity. A `github-matrix` job runs one "
        "cell, so its budget binds each cell's schedule separately.",
        "",
        "| Family | Tiers | Required | Executor | Timeout | Projected | Headroom | Resource | Selection parents | Admission | Inputs |",
        "|---|---|---:|---|---:|---:|---:|---|---|---|---:|",
    ]
    for family in plan.families:
        data = family.data
        if data["executor"] == "github-job":
            projected = plan.timeout_envelope(family.name).projected_makespan_seconds
            projected_cell = f"{projected} s"
            headroom_cell = f"{int(data['timeout_minutes']) * 60 - projected} s"
        elif data["executor"] == "github-matrix":
            # The budget binds each cell's job; report the tightest cell.
            projected = max(
                plan.timeout_envelope(
                    family.name, matrix_cell=cell
                ).projected_makespan_seconds
                for cell in plan.family_cells(family.name)
            )
            projected_cell = f"{projected} s per cell"
            headroom_cell = f"{int(data['timeout_minutes']) * 60 - projected} s"
        else:
            projected_cell = "n/a"
            headroom_cell = "n/a"
        lines.append(
            f"| `{family.name}` | {', '.join(data['tiers'])} | "
            f"{'yes' if data['required'] else 'no'} | `{data['executor']}` | "
            f"{data['timeout_minutes']} min | {projected_cell} | {headroom_cell} | "
            f"`{data['resource_class']}` | "
            f"{', '.join(f'`{name}`' for name in data['dependencies']) or 'none'} | "
            f"`{data['admission_job']}` needs "
            f"{', '.join(f'`{name}`' for name in data['admission_needs']) or 'none'} | "
            f"{len(data['inputs'])} |"
        )
    lines.extend(
        [
            "",
            "## Scheduled families",
            "",
            "Scheduled workflows consume the same typed command DAG and receipt "
            "executor without entering changed-path CI admission.",
            "",
            "| Family | Workflow job | Timeout | Projected | Headroom | Resource | Commands |",
            "|---|---|---:|---:|---:|---|---:|",
        ]
    )
    for family in plan.scheduled_families:
        data = family.data
        envelope = plan.timeout_envelope(family.name)
        budget = int(data["timeout_minutes"]) * 60
        command_count = sum(command.family == family.name for command in plan.commands)
        lines.append(
            f"| `{family.name}` | `{data['job']}` | {budget} s | "
            f"{envelope.projected_makespan_seconds} s | "
            f"{budget - envelope.projected_makespan_seconds} s | "
            f"`{data['resource_class']}` | {command_count} |"
        )
    lines.extend(
        [
            "",
            "## Matrix cells",
            "",
            "`github-matrix` families project these cells directly into the "
            "workflow strategy. The runner is therefore generated policy, not "
            "a second handwritten OS list. Each matrix family has its own "
            "classifier output, `<family>_matrix`, so a matrix job never "
            "receives another family's cells.",
            "",
            "| Cell | Runner | OS | Architecture | Python | Backend | Target | Profile |",
            "|---|---|---|---|---|---|---|---|",
            *[
                f"| `{cell.id}` | `{cell.data['runner']}` | `{cell.data['os']}` | "
                f"`{cell.data['arch']}` | `{cell.data['python']}` | "
                f"`{cell.data['backend']}` | `{cell.data['target']}` | "
                f"`{cell.data['profile']}` |"
                for cell in plan.matrix_cells
            ],
            "",
            "## Toolchain contracts",
            "",
            "Executable identities bind resolved path, version text, and the "
            "repository-relative probe working directory. Target-derived identities "
            "use their declared provider to capture the selected target's tool family "
            "and input custody; they have no single setup executable or probe cwd. "
            "Both kinds must satisfy their declared version authority.",
            "",
            "| Toolchain | Identity kind | Provider | Required version | Probe cwd | Setup value | Setup evidence |",
            "|---|---|---|---|---|---|---:|",
        ]
    )
    for policy in plan.toolchain_policies:
        data = policy.data
        if policy.identity_kind == "target-derived":
            provider = f"`{data['identity_provider']}`"
            probe_cwd = setup_value = setup_evidence = "—"
        else:
            provider = "—"
            probe_cwd = f"`{data.get('probe_cwd', '.')}`"
            setup_value = f"`{data['setup_value']}`"
            setup_evidence = str(len(data["setup_evidence"]))
        lines.append(
            f"| `{policy.name}` | `{policy.identity_kind}` | {provider} | "
            f"`{data['version_pattern']}` | {probe_cwd} | {setup_value} | "
            f"{setup_evidence} |"
        )
    lines.extend(
        [
            "",
            "## Cargo execution contracts",
            "",
            "Cargo wrapper and incremental policy is applied at the canonical "
            "subprocess boundary, including metadata and toolchain probes. Compiler "
            "build partitions select a named timeout budget derived from the shared "
            "receipt-calibrated CI Cargo policy. Nested native/WASM test guards "
            f"inherit that envelope through `{PROOF_COMMAND_TIMEOUT_ENV}` so a "
            "private default cannot terminate progressing work first.",
            "",
            "The wrapper conflict was reconfirmed by native CI run "
            f"`{shared_cargo_policy.environment.incident_run_id}` job "
            f"`{shared_cargo_policy.environment.incident_job_id}` at commit "
            f"`{shared_cargo_policy.environment.incident_commit}`.",
            "",
            "| Budget | Timeout |",
            "|---|---:|",
        ]
    )
    for name, timeout in cargo_policy.timeout_seconds_by_class.items():
        lines.append(f"| `{name}` | {timeout} s |")
    lines.extend(
        [
            "",
            "## Executable partitions",
            "",
            "| Command ID | Family | Cell | Budget | Timeout | Resource | Parents |",
            "|---|---|---|---|---:|---|---:|",
        ]
    )
    for command in plan.commands:
        data = command.data
        lines.append(
            f"| `{command.id}` | `{data['family']}` | `{data['cell']}` | "
            f"`{data.get('timeout_budget', 'explicit')}` | "
            f"{data['timeout_seconds']} s | `{data['resource_class']}` | "
            f"{len(data['dependencies'])} |"
        )
    lines.extend(
        [
            "",
            "## Local integration families",
            "",
            "| Rule | Input globs | Commands | Fresh toolchain |",
            "|---|---:|---:|---:|",
        ]
    )
    for rule in plan.local_rules:
        lines.append(
            f"| `{rule['name']}` | {len(rule['globs'])} | {len(rule['gates'])} | "
            f"{'yes' if rule.get('require_fresh_toolchain', False) else 'no'} |"
        )
    lines.extend(
        [
            "",
            "## Selection contract",
            "",
            "Pull requests use the merge-base diff. Pushes use the event's "
            "`before..after` identities. Forced pushes, null SHAs, missing refs, "
            "and unknown events fail closed to the full plan. Merge-group, "
            "scheduled, and manual runs intentionally select the full plan. The "
            "topology projection "
            "records why every family was selected; each selected `github-matrix` "
            "family's executable matrix expands into its exact runner cells.",
            "",
        ]
    )
    return "\n".join(lines)


def generated_outputs(
    manifest: Path = DEFAULT_MANIFEST, output_root: Path = ROOT
) -> dict[Path, str]:
    """Each output path mapped to its exact generated text."""
    plan = ProofPlan.load(manifest)
    return {
        output_root / JSON_OUTPUT.relative_to(ROOT): _json_projection(plan),
        output_root / DOC_OUTPUT.relative_to(ROOT): _markdown_projection(plan),
    }


def main(argv: list[str] | None = None) -> int:
    # The pre-commit hook projects the git index (--from-index), which reruns
    # the staged copy of this file against its staged manifest and publishes
    # into a scratch root; those options are this generator's own.
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--from-index", action="store_true")
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--output-root", type=Path, default=ROOT)
    staged, rest = parser.parse_known_args(argv)
    if staged.from_index:
        if staged.output_root != ROOT:
            parser.error("--from-index owns publication into its repository")
        if rest not in (["--check"], ["--write"]):
            parser.error("--from-index takes exactly one of --check or --write")
        try:
            return _index_projection(
                manifest=staged.manifest, check=rest == ["--check"]
            )
        except (OSError, UnicodeError, ValueError) as exc:
            print(f"proof-plan staged projection rejected: {exc}", file=sys.stderr)
            return 2
    return generator_main(
        lambda: generated_outputs(staged.manifest, staged.output_root),
        rest,
        description=__doc__,
    )


if __name__ == "__main__":
    raise SystemExit(main())
