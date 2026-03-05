from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

import tools.linear_workspace as linear_workspace  # noqa: E402

try:  # pragma: no cover - optional dependency
    from pydantic import BaseModel, Field, ValidationError  # type: ignore
except Exception:  # pragma: no cover - optional dependency
    BaseModel = None  # type: ignore[assignment]
    Field = None  # type: ignore[assignment]
    ValidationError = Exception  # type: ignore[assignment]

try:  # pragma: no cover - optional dependency
    import dspy  # type: ignore
except Exception:  # pragma: no cover - optional dependency
    dspy = None


SEED_HEADER = "Auto-seeded from Molt roadmap/status TODO contracts."
REQUIRED_METADATA_KEYS = ("area", "milestone", "owner", "priority", "source", "status")
DEFAULT_SOURCE = "legacy-seed-backfill"
MANAGED_LABEL_PREFIXES = ("role:", "area:", "risk:", "formal:")
ACTIVE_FLOW_STATES = {"in progress", "in review"}

QUERY_ISSUES_WITH_LABEL_IDS = """
query IssuesByTeamWithLabels($teamId: ID!, $first: Int!, $after: String) {
  issues(
    filter: { team: { id: { eq: $teamId } } }
    first: $first
    after: $after
    orderBy: createdAt
  ) {
    pageInfo { hasNextPage endCursor }
    nodes {
      id
      identifier
      title
      description
      priority
      createdAt
      state { id name type }
      project { id name }
      labels { nodes { id name } }
    }
  }
}
""".strip()

QUERY_LABELS_GLOBAL = """
query LabelsGlobal {
  issueLabels(first: 250) {
    nodes { id name color description }
  }
}
""".strip()

MUTATION_LABEL_CREATE = """
mutation IssueLabelCreate($input: IssueLabelCreateInput!) {
  issueLabelCreate(input: $input) {
    success
    issueLabel { id name color description }
  }
}
""".strip()

MUTATION_ISSUE_UPDATE = """
mutation IssueUpdate($id: String!, $input: IssueUpdateInput!) {
  issueUpdate(id: $id, input: $input) {
    success
    issue {
      id
      identifier
      title
      priority
      state { id name type }
      labels { nodes { id name } }
    }
  }
}
""".strip()

AREA_FROM_PROJECT = {
    "compiler & frontend": "compiler",
    "runtime & intrinsics": "runtime",
    "wasm parity": "wasm",
    "performance & benchmarking": "performance",
    "testing & differential": "testing",
    "tooling & devex": "tooling",
    "security & supply chain": "security",
    "offload & data ecosystem": "offload",
}
PROJECT_FROM_AREA = {
    "compiler": "Compiler & Frontend",
    "runtime": "Runtime & Intrinsics",
    "stdlib": "Runtime & Intrinsics",
    "wasm": "WASM Parity",
    "performance": "Performance & Benchmarking",
    "testing": "Testing & Differential",
    "tooling": "Tooling & DevEx",
    "security": "Security & Supply Chain",
    "offload": "Offload & Data Ecosystem",
    "c-api": "Runtime & Intrinsics",
}

AREA_ROLE_MAP = {
    "compiler": "formalizer",
    "runtime": "formalizer",
    "wasm": "formalizer",
    "security": "formalizer",
    "stdlib": "formalizer",
    "c-api": "formalizer",
    "testing": "reviewer",
    "tooling": "executor",
    "offload": "executor",
    "performance": "executor",
}

FORMAL_REQUIRED_AREAS = {"compiler", "runtime", "wasm", "security", "stdlib", "c-api"}
FORMAL_SUITE_MODES = ("off", "inventory", "lean", "quint", "all")

LABEL_TAXONOMY = (
    ("role:executor", "#4EA7FC", "Default execution role for implementation."),
    ("role:triage", "#8E8E93", "Triage/planning role for backlog curation."),
    ("role:formalizer", "#F2994A", "Formal methods + high-risk verification role."),
    ("role:reviewer", "#27AE60", "Review/validation role for evidence and QA."),
    ("area:compiler", "#4EA7FC", "Compiler/frontend lane."),
    ("area:runtime", "#EB5757", "Runtime/intrinsics lane."),
    ("area:stdlib", "#BB6BD9", "Stdlib compatibility lane."),
    ("area:wasm", "#F2C94C", "WASM parity lane."),
    ("area:tooling", "#56CCF2", "Tooling and developer experience lane."),
    ("area:testing", "#6FCF97", "Testing and differential parity lane."),
    ("area:security", "#E63946", "Security and supply-chain lane."),
    ("area:offload", "#2D9CDB", "Offload/data ecosystem lane."),
    ("area:performance", "#F2994A", "Performance/benchmarking lane."),
    ("area:c-api", "#9B51E0", "C-API compatibility lane."),
    ("risk:blocker", "#D7263D", "Urgent blocker risk."),
    ("risk:high", "#F2994A", "High risk requiring tight verification."),
    ("risk:medium", "#F2C94C", "Medium risk workstream."),
    ("formal:required", "#8E44AD", "Formalization suite execution is required."),
    ("formal:verified", "#27AE60", "Formalization suite passed for this issue."),
)


if BaseModel is not None:

    class RouteDecision(BaseModel):  # type: ignore[misc]
        role: str = Field(min_length=3, max_length=32)
        formal_required: bool = False
        rationale: str = Field(min_length=3, max_length=400)
        extra_labels: list[str] = Field(default_factory=list)


else:

    @dataclass(frozen=True, slots=True)
    class RouteDecision:
        role: str
        formal_required: bool
        rationale: str
        extra_labels: list[str]


def _title_to_priority(title: str) -> str | None:
    match = re.match(r"^\[(P[0-4])\]\[[^\]]+\]\s+", title.strip())
    if not match:
        return None
    return match.group(1)


def sanitize_issue_title(raw: str) -> str:
    text = raw.replace("\\n", " ").replace("\n", " ").strip()
    text = text.replace('\\"', '"').replace("\\'", "'")
    text = re.sub(r"\s+", " ", text)
    if " (TODO(" in text:
        text = text.split(" (TODO(", 1)[0].rstrip()
    text = re.sub(r"\s*\|\s*$", "", text)
    text = re.sub(r'"\s*$', "", text)
    text = re.sub(r",\s*$", "", text)
    text = re.sub(r"\.\)\s*$", ")", text)
    text = re.sub(r"\)\)\s*$", ")", text)
    text = re.sub(r"\.\s*$", "", text)
    return re.sub(r"\s+", " ", text).strip()


def canonicalize_title(title: str) -> str:
    normalized = sanitize_issue_title(title).lower()
    normalized = re.sub(r"[^a-z0-9]+", " ", normalized)
    return re.sub(r"\s+", " ", normalized).strip()


def _extract_metadata_block(description: str) -> dict[str, str]:
    metadata: dict[str, str] = {}
    for raw in description.splitlines():
        match = re.match(r"^\s*[-*]\s*([a-z_]+)\s*:\s*(.+?)\s*$", raw)
        if not match:
            continue
        key = match.group(1).strip().lower()
        value = match.group(2).strip()
        if key and value:
            metadata[key] = value
    return metadata


def _extract_seed_line(description: str, prefix: str) -> str | None:
    needle = f"{prefix}:"
    for raw in description.splitlines():
        if raw.strip().lower().startswith(needle.lower()):
            value = raw.split(":", 1)[1].strip()
            return value or None
    return None


def _description_without_metadata_block(description: str) -> str:
    if "\n---\n" in description:
        base = description.split("\n---\n", 1)[0]
        return base.rstrip()
    return description.rstrip()


def _compose_metadata_block(metadata: dict[str, str]) -> str:
    lines = [f"* {key}: {metadata[key]}" for key in REQUIRED_METADATA_KEYS]
    return "\n".join(lines)


def _compose_description_with_metadata(base: str, metadata: dict[str, str]) -> str:
    return f"{base.rstrip()}\n\n---\n\n{_compose_metadata_block(metadata)}".rstrip()


def _manifest_items_from_index(index_path: Path) -> list[dict[str, Any]]:
    payload = json.loads(index_path.read_text(encoding="utf-8"))
    if not isinstance(payload, list):
        raise RuntimeError("manifest index must be a list")
    items: list[dict[str, Any]] = []
    for row in payload:
        if not isinstance(row, dict):
            continue
        manifest_rel = str(row.get("path") or "").strip()
        if not manifest_rel:
            continue
        manifest_path = Path(manifest_rel)
        if not manifest_path.exists():
            continue
        entries_raw = json.loads(manifest_path.read_text(encoding="utf-8"))
        if not isinstance(entries_raw, list):
            continue
        for entry in entries_raw:
            if isinstance(entry, dict):
                items.append(entry)
    return items


def _build_manifest_lookup(index_path: Path) -> dict[str, dict[str, str]]:
    lookup: dict[str, dict[str, str]] = {}
    for item in _manifest_items_from_index(index_path):
        title = str(item.get("title") or "")
        metadata_raw = item.get("metadata")
        if not title or not isinstance(metadata_raw, dict):
            continue
        metadata: dict[str, str] = {
            str(key).lower(): str(value).strip()
            for key, value in metadata_raw.items()
            if value is not None and str(value).strip()
        }
        key = canonicalize_title(title)
        if key and metadata:
            lookup[key] = metadata
    return lookup


def _fetch_issues_for_hygiene(team_id: str) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    after: str | None = None
    while True:
        payload = linear_workspace.graphql(
            QUERY_ISSUES_WITH_LABEL_IDS,
            {"teamId": team_id, "first": 100, "after": after},
        )
        issues_obj = payload["issues"]
        nodes = issues_obj["nodes"]
        rows.extend(nodes)
        page_info = issues_obj["pageInfo"]
        if not page_info["hasNextPage"]:
            break
        after = page_info["endCursor"]
        if not after:
            break
    return rows


def _infer_seed_metadata(
    *,
    issue: dict[str, Any],
    manifest_lookup: dict[str, dict[str, str]],
) -> dict[str, str] | None:
    description = str(issue.get("description") or "")
    if SEED_HEADER not in description:
        return None

    existing = _extract_metadata_block(description)
    title = sanitize_issue_title(str(issue.get("title") or ""))
    canonical_key = canonicalize_title(title)
    manifest_meta = manifest_lookup.get(canonical_key, {})

    area = (
        existing.get("area")
        or _extract_seed_line(description, "Area")
        or manifest_meta.get("area")
        or "unknown"
    )
    owner = (
        existing.get("owner")
        or _extract_seed_line(description, "Owner lane")
        or manifest_meta.get("owner")
        or "unknown"
    )
    milestone = (
        existing.get("milestone")
        or _extract_seed_line(description, "Milestone")
        or manifest_meta.get("milestone")
        or "unknown"
    )
    priority = (
        existing.get("priority")
        or _title_to_priority(title)
        or manifest_meta.get("priority")
        or "P2"
    )
    status = (
        existing.get("status")
        or _extract_seed_line(description, "Status tag")
        or manifest_meta.get("status")
        or "partial"
    )
    source = existing.get("source") or manifest_meta.get("source") or DEFAULT_SOURCE

    result = {
        "area": area,
        "milestone": milestone,
        "owner": owner,
        "priority": priority,
        "source": source,
        "status": status,
    }
    missing = [key for key in REQUIRED_METADATA_KEYS if not result.get(key)]
    if missing:
        return None
    return result


def _update_issue(issue_id: str, input_payload: dict[str, Any]) -> bool:
    response = linear_workspace.graphql(
        MUTATION_ISSUE_UPDATE, {"id": issue_id, "input": input_payload}
    )
    block = response.get("issueUpdate") or {}
    return bool(block.get("success"))


def _ensure_label_taxonomy(team_id: str, *, apply: bool) -> dict[str, Any]:
    existing = linear_workspace.graphql(QUERY_LABELS_GLOBAL)["issueLabels"]["nodes"]
    existing_by_name = {
        str(row.get("name", "")).strip().lower(): row for row in existing
    }
    created: list[str] = []
    missing: list[dict[str, str]] = []

    for name, color, description in LABEL_TAXONOMY:
        normalized = name.lower()
        if normalized in existing_by_name:
            continue
        item = {"name": name, "color": color, "description": description}
        if not apply:
            missing.append(item)
            continue
        out = linear_workspace.graphql(
            MUTATION_LABEL_CREATE,
            {
                "input": {
                    "name": name,
                    "color": color,
                    "description": description,
                    "teamId": team_id,
                }
            },
        )
        block = out.get("issueLabelCreate") or {}
        if not block.get("success"):
            raise RuntimeError(f"failed creating label: {name}")
        created.append(name)
    return {
        "existing_count": len(existing),
        "created": created,
        "missing_if_dry_run": missing,
    }


def _build_area_label(*, issue: dict[str, Any], metadata: dict[str, str]) -> str:
    area = str(metadata.get("area") or "").strip().lower()
    if not area:
        project_name = (
            str(((issue.get("project") or {}).get("name") or "")).strip().lower()
        )
        mapped = AREA_FROM_PROJECT.get(project_name)
        if mapped:
            area = mapped

    if "compiler" in area:
        return "area:compiler"
    if "wasm" in area:
        return "area:wasm"
    if "security" in area:
        return "area:security"
    if "tool" in area:
        return "area:tooling"
    if "test" in area:
        return "area:testing"
    if "offload" in area or "accel" in area or "data" in area or "django" in area:
        return "area:offload"
    if "perf" in area or "bench" in area:
        return "area:performance"
    if "stdlib" in area:
        return "area:stdlib"
    if "c-api" in area or "c api" in area:
        return "area:c-api"
    if "runtime" in area or "async" in area:
        return "area:runtime"
    return "area:runtime"


def _infer_area_hint(issue: dict[str, Any], metadata: dict[str, str]) -> str:
    area = str(metadata.get("area") or "").strip()
    if area:
        return area
    title = str(issue.get("title") or "")
    if ":" in title:
        return title.split(":", 1)[0].strip()
    return ""


def _project_name_for_issue(issue: dict[str, Any], metadata: dict[str, str]) -> str:
    area_hint = _infer_area_hint(issue, metadata).lower()
    if area_hint in {"formal", "proof", "verification"}:
        return "Testing & Differential"
    if area_hint in {"symphony", "agent", "orchestration"}:
        return "Tooling & DevEx"
    if area_hint in {"moltlib"}:
        return "Offload & Data Ecosystem"
    area_label = _build_area_label(issue=issue, metadata={"area": area_hint})
    area_key = area_label.removeprefix("area:")
    return PROJECT_FROM_AREA.get(area_key, "Runtime & Intrinsics")


def _risk_label(priority: int | None) -> str:
    if priority == 1:
        return "risk:blocker"
    if priority == 2:
        return "risk:high"
    return "risk:medium"


def _heuristic_route_decision(
    *,
    issue: dict[str, Any],
    area_label: str,
) -> RouteDecision:
    state_name = str(((issue.get("state") or {}).get("name") or "")).strip().lower()
    area_key = area_label.removeprefix("area:")
    role = AREA_ROLE_MAP.get(area_key, "executor")
    if state_name == "backlog":
        role = "triage"
    formal_required = area_key in FORMAL_REQUIRED_AREAS and (
        issue.get("priority") in {1, 2}
    )
    rationale = f"heuristic area={area_key} state={state_name or 'unknown'}"
    return RouteDecision(
        role=role,
        formal_required=formal_required,
        rationale=rationale,
        extra_labels=[],
    )


def _dspy_route_decision(
    *,
    issue: dict[str, Any],
    fallback: RouteDecision,
) -> RouteDecision:
    if dspy is None or BaseModel is None:
        return fallback
    if not os.environ.get("MOLT_SYMPHONY_DSPY_ENABLE"):
        return fallback

    model = os.environ.get("MOLT_SYMPHONY_DSPY_MODEL", "").strip()
    api_key = os.environ.get("OPENAI_API_KEY", "").strip()
    if not model or not api_key:
        return fallback

    try:
        if getattr(dspy.settings, "lm", None) is None:  # pragma: no cover
            lm = dspy.LM(model=model, api_key=api_key)
            dspy.configure(lm=lm)

        class RoutingSignature(dspy.Signature):  # type: ignore[misc]
            issue_title = dspy.InputField()
            issue_description = dspy.InputField()
            issue_priority = dspy.InputField()
            issue_state = dspy.InputField()
            role = dspy.OutputField(
                desc="one of executor, triage, formalizer, reviewer"
            )
            formal_required = dspy.OutputField(desc="true or false")
            extra_labels = dspy.OutputField(
                desc="comma-separated labels or empty string"
            )
            rationale = dspy.OutputField(desc="short rationale")

        predictor = dspy.Predict(RoutingSignature)
        result = predictor(
            issue_title=str(issue.get("title") or ""),
            issue_description=str(issue.get("description") or "")[:2400],
            issue_priority=str(issue.get("priority") or "none"),
            issue_state=str(((issue.get("state") or {}).get("name") or "")),
        )
        payload = {
            "role": str(getattr(result, "role", fallback.role) or fallback.role)
            .strip()
            .lower(),
            "formal_required": str(
                getattr(result, "formal_required", fallback.formal_required)
            )
            .strip()
            .lower()
            in {"true", "1", "yes"},
            "rationale": str(
                getattr(result, "rationale", fallback.rationale) or ""
            ).strip()[:400]
            or fallback.rationale,
            "extra_labels": [
                item.strip()
                for item in str(getattr(result, "extra_labels", "") or "").split(",")
                if item.strip()
            ],
        }
        if BaseModel is not None:
            return RouteDecision.model_validate(payload)  # type: ignore[attr-defined]
        return RouteDecision(**payload)  # type: ignore[arg-type]
    except Exception:
        return fallback


def _label_name_to_ids() -> dict[str, str]:
    labels = linear_workspace.graphql(QUERY_LABELS_GLOBAL)["issueLabels"]["nodes"]
    result: dict[str, str] = {}
    for row in labels:
        name = str(row.get("name") or "").strip().lower()
        label_id = str(row.get("id") or "").strip()
        if name and label_id:
            result[name] = label_id
    return result


def _infer_metadata_for_routing(
    issue: dict[str, Any], manifest_lookup: dict[str, dict[str, str]]
) -> dict[str, str]:
    description = str(issue.get("description") or "")
    metadata = _extract_metadata_block(description)
    if metadata:
        return metadata
    inferred = _infer_seed_metadata(issue=issue, manifest_lookup=manifest_lookup)
    return inferred or {}


def _run_formal_suite(mode: str) -> dict[str, Any]:
    mode_key = mode.strip().lower()
    if mode_key not in FORMAL_SUITE_MODES:
        raise RuntimeError(f"invalid formal suite mode: {mode}")

    command = [
        "uv",
        "run",
        "--python",
        "3.12",
        "python3",
        "tools/check_formal_methods.py",
        "--json-only",
    ]
    if mode_key == "inventory":
        command.append("--inventory")
    elif mode_key == "lean":
        command.append("--lean")
    elif mode_key == "quint":
        command.append("--quint")

    proc = subprocess.run(command, check=False, capture_output=True, text=True)
    report: dict[str, Any] | None = None
    stdout = (proc.stdout or "").strip()
    if stdout:
        try:
            parsed = json.loads(stdout)
            if isinstance(parsed, dict):
                report = parsed
        except Exception:
            report = None

    status = "pass"
    if report is None:
        status = "fail"
    elif not bool(report.get("ok")):
        checks = report.get("checks")
        quint = checks.get("quint") if isinstance(checks, dict) else None
        diagnostics = quint.get("diagnostics") if isinstance(quint, dict) else None
        runtime_mismatch = bool(
            isinstance(diagnostics, dict)
            and diagnostics.get("runtime_mismatch_detected")
        )
        status = "warn" if runtime_mismatch else "fail"

    return {
        "mode": mode_key,
        "status": status,
        "command": command,
        "returncode": int(proc.returncode),
        "stdout": stdout[-2000:],
        "stderr": (proc.stderr or "").strip()[-2000:],
        "report": report,
    }


def cmd_fix_manifests(args: argparse.Namespace) -> int:
    index_path = Path(args.index).resolve()
    payload = json.loads(index_path.read_text(encoding="utf-8"))
    if not isinstance(payload, list):
        raise RuntimeError("manifest index must be a list")

    changed_files: list[str] = []
    changed_entries: list[dict[str, Any]] = []
    for row in payload:
        if not isinstance(row, dict):
            continue
        rel = str(row.get("path") or "").strip()
        if not rel:
            continue
        manifest_path = Path(rel).resolve()
        if not manifest_path.exists():
            continue
        entries = json.loads(manifest_path.read_text(encoding="utf-8"))
        if not isinstance(entries, list):
            continue
        local_changed = False
        for idx, entry in enumerate(entries, start=1):
            if not isinstance(entry, dict):
                continue
            old = str(entry.get("title") or "")
            new = sanitize_issue_title(old)
            if new == old:
                continue
            entry["title"] = new
            local_changed = True
            changed_entries.append(
                {
                    "path": str(manifest_path),
                    "index": idx,
                    "old": old,
                    "new": new,
                }
            )
        if local_changed:
            changed_files.append(str(manifest_path))
            if args.apply:
                manifest_path.write_text(
                    json.dumps(entries, indent=2, ensure_ascii=True) + "\n",
                    encoding="utf-8",
                )

    output = {
        "index": str(index_path),
        "apply": bool(args.apply),
        "changed_file_count": len(changed_files),
        "changed_files": changed_files,
        "changed_entry_count": len(changed_entries),
        "changed_entries": changed_entries,
    }
    print(json.dumps(output, indent=2, sort_keys=True))
    return 0


def cmd_fix_issues(args: argparse.Namespace) -> int:
    team_id = linear_workspace._resolve_team_id(args.team)
    manifest_lookup = _build_manifest_lookup(Path(args.index).resolve())
    issues = _fetch_issues_for_hygiene(team_id)
    changes: list[dict[str, Any]] = []
    updated = 0

    for issue in issues:
        issue_id = str(issue.get("id") or "")
        identifier = str(issue.get("identifier") or "")
        title = str(issue.get("title") or "")
        description = str(issue.get("description") or "")
        new_title = sanitize_issue_title(title)
        metadata = _infer_seed_metadata(issue=issue, manifest_lookup=manifest_lookup)
        update_input: dict[str, Any] = {}

        if new_title != title:
            update_input["title"] = new_title

        if metadata is not None:
            existing_block = _extract_metadata_block(description)
            missing = [
                key for key in REQUIRED_METADATA_KEYS if not existing_block.get(key)
            ]
            if missing:
                base = _description_without_metadata_block(description)
                update_input["description"] = _compose_description_with_metadata(
                    base, metadata
                )

        if not update_input:
            continue
        changes.append(
            {
                "id": issue_id,
                "identifier": identifier,
                "update_input": update_input,
            }
        )

    if args.apply:
        for row in changes:
            if _update_issue(row["id"], row["update_input"]):
                updated += 1

    result = {
        "team": args.team,
        "apply": bool(args.apply),
        "planned_updates": len(changes),
        "updated": updated if args.apply else 0,
        "changes": changes,
    }
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


def cmd_ensure_labels(args: argparse.Namespace) -> int:
    team_id = linear_workspace._resolve_team_id(args.team)
    result = _ensure_label_taxonomy(team_id, apply=bool(args.apply))
    result["team"] = args.team
    result["apply"] = bool(args.apply)
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


def cmd_ensure_projects(args: argparse.Namespace) -> int:
    team_id = linear_workspace._resolve_team_id(args.team)
    projects = linear_workspace._fetch_projects(team_id)
    project_name_to_id = {
        str(project.get("name") or "").strip().lower(): str(
            project.get("id") or ""
        ).strip()
        for project in projects
        if str(project.get("name") or "").strip()
        and str(project.get("id") or "").strip()
    }
    manifest_lookup = _build_manifest_lookup(Path(args.index).resolve())
    issues = _fetch_issues_for_hygiene(team_id)

    planned: list[dict[str, Any]] = []
    updated = 0
    skipped: list[dict[str, str]] = []

    for issue in issues:
        project = issue.get("project")
        if isinstance(project, dict) and str(project.get("id") or "").strip():
            continue
        metadata = _infer_metadata_for_routing(issue, manifest_lookup)
        project_name = _project_name_for_issue(issue, metadata)
        project_id = project_name_to_id.get(project_name.lower())
        if not project_id:
            skipped.append(
                {
                    "identifier": str(issue.get("identifier") or ""),
                    "reason": f"missing_project_named:{project_name}",
                }
            )
            continue
        planned.append(
            {
                "id": str(issue.get("id") or ""),
                "identifier": str(issue.get("identifier") or ""),
                "project_name": project_name,
                "input": {"projectId": project_id},
            }
        )

    if args.apply:
        for row in planned:
            if _update_issue(row["id"], row["input"]):
                updated += 1

    print(
        json.dumps(
            {
                "team": args.team,
                "apply": bool(args.apply),
                "planned_updates": len(planned),
                "updated": updated if args.apply else 0,
                "skipped": skipped,
                "changes": planned,
            },
            indent=2,
            sort_keys=True,
        )
    )
    return 0


def cmd_apply_routing(args: argparse.Namespace) -> int:
    team_id = linear_workspace._resolve_team_id(args.team)
    _ensure_label_taxonomy(team_id, apply=bool(args.apply))
    label_ids = _label_name_to_ids()
    manifest_lookup = _build_manifest_lookup(Path(args.index).resolve())
    issues = _fetch_issues_for_hygiene(team_id)
    target_states = {
        part.strip().lower() for part in str(args.states).split(",") if part.strip()
    }

    planned: list[dict[str, Any]] = []
    updated = 0
    formal_required_issue_ids: list[str] = []

    for issue in issues:
        state_name = str(((issue.get("state") or {}).get("name") or "")).strip().lower()
        if state_name not in target_states:
            continue

        metadata = _infer_metadata_for_routing(issue, manifest_lookup)
        area_label = _build_area_label(issue=issue, metadata=metadata)
        heuristic = _heuristic_route_decision(issue=issue, area_label=area_label)
        decision = _dspy_route_decision(issue=issue, fallback=heuristic)
        role_label = f"role:{decision.role}"
        risk_label = _risk_label(issue.get("priority"))

        desired_managed = {area_label, role_label, risk_label}
        if decision.formal_required:
            desired_managed.add("formal:required")
            formal_required_issue_ids.append(str(issue.get("identifier") or ""))
        for label in decision.extra_labels:
            normalized = label.strip().lower()
            if normalized:
                desired_managed.add(normalized)

        labels_node = (issue.get("labels") or {}).get("nodes") or []
        existing_names = {
            str(label.get("name") or "").strip().lower()
            for label in labels_node
            if str(label.get("name") or "").strip()
        }
        preserved = {
            name
            for name in existing_names
            if not any(name.startswith(prefix) for prefix in MANAGED_LABEL_PREFIXES)
        }
        final_names = sorted(preserved | desired_managed)

        missing_label_names = [name for name in final_names if name not in label_ids]
        if missing_label_names:
            continue

        existing_sorted = sorted(existing_names)
        if final_names == existing_sorted:
            continue

        input_payload = {"labelIds": [label_ids[name] for name in final_names]}
        planned.append(
            {
                "id": str(issue.get("id") or ""),
                "identifier": str(issue.get("identifier") or ""),
                "from": existing_sorted,
                "to": final_names,
                "decision": {
                    "role": decision.role,
                    "formal_required": decision.formal_required,
                    "rationale": decision.rationale,
                },
                "input": input_payload,
            }
        )

    if args.apply:
        for row in planned:
            if _update_issue(row["id"], row["input"]):
                updated += 1

    formal_suite_mode = str(getattr(args, "formal_suite", "off")).strip().lower()
    if (
        bool(getattr(args, "run_formal_inventory", False))
        and formal_suite_mode == "off"
    ):
        formal_suite_mode = "inventory"
    formal_suite: dict[str, Any] | None = None
    if formal_suite_mode != "off":
        formal_suite = _run_formal_suite(formal_suite_mode)

    result = {
        "team": args.team,
        "apply": bool(args.apply),
        "states": sorted(target_states),
        "dspy": {
            "env_enabled": bool(os.environ.get("MOLT_SYMPHONY_DSPY_ENABLE")),
            "module_available": dspy is not None,
            "pydantic_available": BaseModel is not None,
        },
        "planned_updates": len(planned),
        "updated": updated if args.apply else 0,
        "formal_required_issues": sorted(set(formal_required_issue_ids)),
        "formal_suite_mode": formal_suite_mode,
        "formal_suite": formal_suite,
        "formal_inventory": formal_suite if formal_suite_mode == "inventory" else None,
        "changes": planned,
    }
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


def cmd_ensure_active_flow(args: argparse.Namespace) -> int:
    team_id = linear_workspace._resolve_team_id(args.team)
    issues = _fetch_issues_for_hygiene(team_id)

    active = [
        issue
        for issue in issues
        if str(((issue.get("state") or {}).get("name") or "")).strip().lower()
        in ACTIVE_FLOW_STATES
    ]
    if active:
        print(
            json.dumps(
                {
                    "team": args.team,
                    "apply": bool(args.apply),
                    "changed": False,
                    "reason": "already_has_active_flow",
                    "active": [
                        str(issue.get("identifier") or "") for issue in active[:20]
                    ],
                },
                indent=2,
                sort_keys=True,
            )
        )
        return 0

    candidates = [
        issue
        for issue in issues
        if str(((issue.get("state") or {}).get("name") or "")).strip().lower() == "todo"
    ]
    candidates.sort(
        key=lambda issue: (
            int(issue.get("priority") or 5),
            str(issue.get("createdAt") or ""),
        )
    )
    chosen = candidates[0] if candidates else None
    if chosen is None:
        print(
            json.dumps(
                {
                    "team": args.team,
                    "apply": bool(args.apply),
                    "changed": False,
                    "reason": "no_todo_candidates",
                },
                indent=2,
                sort_keys=True,
            )
        )
        return 0

    in_progress_state_id = linear_workspace._state_id_by_name(team_id, "In Progress")
    planned = {
        "issue_id": str(chosen.get("id") or ""),
        "identifier": str(chosen.get("identifier") or ""),
        "title": str(chosen.get("title") or ""),
        "new_state": "In Progress",
    }
    changed = False
    if args.apply:
        changed = _update_issue(planned["issue_id"], {"stateId": in_progress_state_id})

    print(
        json.dumps(
            {
                "team": args.team,
                "apply": bool(args.apply),
                "changed": bool(changed),
                "planned": planned,
            },
            indent=2,
            sort_keys=True,
        )
    )
    return 0


def cmd_full_pass(args: argparse.Namespace) -> int:
    dry_ns = argparse.Namespace(index=args.index, apply=False)
    print("=== fix-manifests (plan) ===")
    cmd_fix_manifests(dry_ns)
    if args.apply:
        print("=== fix-manifests (apply) ===")
        cmd_fix_manifests(argparse.Namespace(index=args.index, apply=True))

    print("=== fix-issues (plan) ===")
    cmd_fix_issues(argparse.Namespace(team=args.team, index=args.index, apply=False))
    if args.apply:
        print("=== fix-issues (apply) ===")
        cmd_fix_issues(argparse.Namespace(team=args.team, index=args.index, apply=True))

    print("=== ensure-labels ===")
    cmd_ensure_labels(argparse.Namespace(team=args.team, apply=bool(args.apply)))

    print("=== ensure-projects ===")
    cmd_ensure_projects(
        argparse.Namespace(team=args.team, index=args.index, apply=bool(args.apply))
    )

    print("=== apply-routing ===")
    cmd_apply_routing(
        argparse.Namespace(
            team=args.team,
            index=args.index,
            states=args.states,
            apply=bool(args.apply),
            run_formal_inventory=bool(args.run_formal_inventory),
            formal_suite=str(getattr(args, "formal_suite", "off")),
        )
    )

    print("=== ensure-active-flow ===")
    cmd_ensure_active_flow(argparse.Namespace(team=args.team, apply=bool(args.apply)))
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Linear hygiene + routing utility for manifest cleanup, issue metadata "
            "backfill, taxonomy labels, and swarm role routing."
        )
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    fix_manifests = sub.add_parser("fix-manifests")
    fix_manifests.add_argument(
        "--index", default="ops/linear/manifests/index.json", help="Manifest index path"
    )
    fix_manifests.add_argument("--apply", action="store_true")
    fix_manifests.set_defaults(func=cmd_fix_manifests)

    fix_issues = sub.add_parser("fix-issues")
    fix_issues.add_argument("--team", default="Moltlang")
    fix_issues.add_argument("--index", default="ops/linear/manifests/index.json")
    fix_issues.add_argument("--apply", action="store_true")
    fix_issues.set_defaults(func=cmd_fix_issues)

    ensure_labels = sub.add_parser("ensure-labels")
    ensure_labels.add_argument("--team", default="Moltlang")
    ensure_labels.add_argument("--apply", action="store_true")
    ensure_labels.set_defaults(func=cmd_ensure_labels)

    ensure_projects = sub.add_parser("ensure-projects")
    ensure_projects.add_argument("--team", default="Moltlang")
    ensure_projects.add_argument("--index", default="ops/linear/manifests/index.json")
    ensure_projects.add_argument("--apply", action="store_true")
    ensure_projects.set_defaults(func=cmd_ensure_projects)

    apply_routing = sub.add_parser("apply-routing")
    apply_routing.add_argument("--team", default="Moltlang")
    apply_routing.add_argument("--index", default="ops/linear/manifests/index.json")
    apply_routing.add_argument("--states", default="Backlog,Todo,In Progress")
    apply_routing.add_argument("--apply", action="store_true")
    apply_routing.add_argument("--run-formal-inventory", action="store_true")
    apply_routing.add_argument(
        "--formal-suite",
        choices=list(FORMAL_SUITE_MODES),
        default="off",
        help="Formalization suite mode to run after routing.",
    )
    apply_routing.set_defaults(func=cmd_apply_routing)

    ensure_active = sub.add_parser("ensure-active-flow")
    ensure_active.add_argument("--team", default="Moltlang")
    ensure_active.add_argument("--apply", action="store_true")
    ensure_active.set_defaults(func=cmd_ensure_active_flow)

    full = sub.add_parser("full-pass")
    full.add_argument("--team", default="Moltlang")
    full.add_argument("--index", default="ops/linear/manifests/index.json")
    full.add_argument("--states", default="Backlog,Todo,In Progress")
    full.add_argument("--apply", action="store_true")
    full.add_argument("--run-formal-inventory", action="store_true")
    full.add_argument(
        "--formal-suite",
        choices=list(FORMAL_SUITE_MODES),
        default="off",
        help="Formalization suite mode to run during routing.",
    )
    full.set_defaults(func=cmd_full_pass)

    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        return int(args.func(args))
    except Exception as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
