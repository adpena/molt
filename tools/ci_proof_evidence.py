"""Retrieve CI proof coverage through the existing archive and receipt owners.

This module transports evidence. The proof plan still selects the work, and
proof_receipts still decides whether that work passed for the exact source.
"""

from __future__ import annotations

from collections.abc import Callable, Mapping
from dataclasses import asdict, dataclass
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
from types import ModuleType
from typing import Any, Literal
import zipfile

from molt.artifact_publication import atomic_write_json
from molt.dx import scratch_dir
from tools.command_execution import CommandExecutor


@dataclass(frozen=True, slots=True)
class Verification:
    status: Literal["verified", "pending", "failed", "unavailable"]
    errors: tuple[str, ...] = ()
    evidence_dir: str | None = None
    receipt_artifacts: int = 0
    scope: str = "source-bound proof coverage; receipts may precede this attempt"


def _run_identity(run: Mapping[str, Any]) -> tuple[object, ...]:
    return tuple(
        run.get(key)
        for key in ("databaseId", "headSha", "attempt", "event", "status", "conclusion")
    )


def _selected_artifacts(
    pages: object, run: Mapping[str, Any]
) -> tuple[dict[str, Any], tuple[dict[str, Any], ...]]:
    if not isinstance(pages, list) or not pages:
        raise ValueError("artifact inventory is missing")
    artifacts: list[dict[str, Any]] = []
    selection_name = f"proof-selection-{run['attempt']}"
    for page in pages:
        if not isinstance(page, dict) or not isinstance(page.get("artifacts"), list):
            raise ValueError("malformed artifact inventory page")
        for artifact in page["artifacts"]:
            if not isinstance(artifact, dict) or not isinstance(
                artifact.get("name"), str
            ):
                raise ValueError("malformed artifact inventory entry")
            if artifact["name"] != selection_name and not artifact["name"].startswith(
                "proof-receipt-"
            ):
                continue
            workflow = artifact.get("workflow_run")
            if (
                not isinstance(workflow, dict)
                or workflow.get("id") != run["databaseId"]
                or workflow.get("head_sha") != run["headSha"]
            ):
                raise ValueError("artifact belongs to another run or source")
            if artifact.get("expired") is not False:
                raise ValueError(f"artifact expired: {artifact['name']}")
            if type(artifact.get("id")) is not int or artifact["id"] <= 0:
                raise ValueError("artifact ID must be a positive integer")
            if (
                re.fullmatch(r"sha256:[0-9a-f]{64}", str(artifact.get("digest")))
                is None
            ):
                raise ValueError(
                    f"artifact has no SHA-256 identity: {artifact['name']}"
                )
            artifacts.append(artifact)
    if len({item["id"] for item in artifacts}) != len(artifacts):
        raise ValueError("duplicate artifact IDs")
    if len({item["name"] for item in artifacts}) != len(artifacts):
        raise ValueError("ambiguous artifact names; no latest-artifact inference")
    selections = [item for item in artifacts if item["name"] == selection_name]
    if len(selections) != 1:
        raise ValueError(
            f"required artifact {selection_name} is unavailable; this run does not "
            "export its current-attempt selection. Older runs and partial reruns "
            "without a current classifier cannot be verified automatically"
        )
    receipts = tuple(
        item for item in artifacts if item["name"].startswith("proof-receipt-")
    )
    return selections[0], receipts


def verify_run(
    run: Mapping[str, Any],
    *,
    repo_root: Path,
    commands: CommandExecutor,
    read_run: Callable[[int], dict[str, Any]],
    proof_api: ModuleType | None = None,
) -> Verification:
    """Observe once, retrieve immutable artifacts, and recheck before admission.

    A failed or unfinished CI run is never a successful coverage result. This
    operation neither starts jobs nor changes the caller's checkout.
    """
    if run.get("status") != "completed":
        return Verification("pending", ("CI has not reached a terminal result",))
    if run.get("conclusion") != "success":
        return Verification("failed", (f"CI conclusion is {run.get('conclusion')!r}",))
    evidence_dir: Path | None = None
    receipt_count = 0
    try:
        if (
            type(run.get("databaseId")) is not int
            or run["databaseId"] <= 0
            or type(run.get("attempt")) is not int
            or run["attempt"] <= 0
        ):
            raise ValueError("CI run/attempt identity is missing")
        # Lazy: ordinary failure triage has no proof-plan or archive startup.
        from tools import proof_plan_cli
        from tools.release.archive import extract_zip_strict

        if proof_api is None:
            from tools import proof_plan as proof_api

        source = proof_api._source_identity()
        # Pull requests test a merge checkout. The run head binds the artifact,
        # while the classifier records the exact source that was tested.
        if run.get("event") not in {"pull_request", "pull_request_target"} and source[
            "commit"
        ] != run.get("headSha"):
            raise ValueError(
                f"verification requires a clean checkout of {run.get('headSha')}; "
                f"this checkout is {source['commit']}. No checkout was changed"
            )
        if proof_api._source_tree_changes():
            raise ValueError("verification requires clean committed source")
        tier = proof_api.active_tier({"GITHUB_EVENT_NAME": str(run.get("event", ""))})
        if tier is None:
            raise ValueError("CI event has no declared proof tier")
        pages = json.loads(
            commands.check_output(
                [
                    "gh",
                    "api",
                    "--paginate",
                    "--slurp",
                    f"repos/{{owner}}/{{repo}}/actions/runs/{run['databaseId']}/artifacts?per_page=100",
                ],
                cwd=repo_root,
                text=True,
                encoding="utf-8",
                timeout=120,
            )
        )
        selection, receipts = _selected_artifacts(pages, run)
        receipt_count = len(receipts)
        parent = scratch_dir(repo_root, "ci-proof-evidence", os.environ)
        parent.mkdir(parents=True, exist_ok=True)
        evidence_dir = Path(
            tempfile.mkdtemp(
                prefix=f"{run['databaseId']}-{run['attempt']}-", dir=parent
            )
        )
        atomic_write_json(
            evidence_dir / "inventory.json",
            {
                "run": dict(run),
                "selection": selection,
                "receipts": receipts,
            },
        )
        receipt_root = evidence_dir / "receipts"
        receipt_root.mkdir()
        selection_path: Path | None = None
        plan = proof_api.ProofPlan.load(proof_api.DEFAULT_MANIFEST)
        context = {
            "run_id": run["databaseId"],
            "run_attempt": run["attempt"],
            "event_name": run["event"],
        }
        for artifact in (selection, *receipts):
            archive = evidence_dir / f"{artifact['id']}.zip"
            commands.run(
                [
                    "gh",
                    "api",
                    f"repos/{{owner}}/{{repo}}/actions/artifacts/{artifact['id']}/zip",
                ],
                cwd=repo_root,
                capture_output=True,
                stdout_capture_path=archive,
                stderr_capture_path=evidence_dir / f"{artifact['id']}.stderr",
                capture_tail_bytes=64 * 1024,
                text=False,
                timeout=300,
                check=True,
            )
            destination = (
                evidence_dir / "selection" if artifact is selection else receipt_root
            ) / str(artifact["id"])
            destination.parent.mkdir(parents=True, exist_ok=True)
            identity = extract_zip_strict(archive, destination)
            if f"sha256:{identity.sha256}" != artifact["digest"]:
                raise ValueError(
                    f"artifact bytes differ from GitHub digest: {artifact['id']}"
                )
            if artifact is selection:
                selection_path = destination / "selection.json"
                # Reject the wrong tested source or selection before fetching
                # large runtime/compiler evidence archives.
                proof_plan_cli.read_selection_file(
                    proof_api,
                    plan,
                    selection_path,
                    receipt_root,
                    tier=tier,
                    github_context=context,
                )
        assert selection_path is not None
        errors = proof_plan_cli.verify_selection_file(
            proof_api,
            plan,
            selection_path,
            receipt_root,
            tier=tier,
            github_context=context,
        )
        if _run_identity(read_run(run["databaseId"])) != _run_identity(run):
            raise ValueError(
                "CI run changed during retrieval; observe its current result"
            )
        if proof_api._source_tree_changes() or proof_api._source_identity() != source:
            raise ValueError("source changed during CI coverage observation")
        if errors:
            result = Verification(
                "failed", tuple(errors), str(evidence_dir), receipt_count
            )
        else:
            result = Verification(
                "verified",
                evidence_dir=str(evidence_dir),
                receipt_artifacts=receipt_count,
            )
    except (
        OSError,
        ValueError,
        RuntimeError,
        subprocess.SubprocessError,
        zipfile.BadZipFile,
    ) as exc:
        result = Verification(
            "unavailable",
            (str(exc),),
            None if evidence_dir is None else str(evidence_dir),
            receipt_count,
        )
    if evidence_dir is not None:
        atomic_write_json(evidence_dir / "verification.json", asdict(result))
    return result
