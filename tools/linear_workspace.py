from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any


LINEAR_ENDPOINT = "https://api.linear.app/graphql"


@dataclass(frozen=True, slots=True)
class DesiredIssue:
    title: str
    description: str
    priority: int | None


@dataclass(frozen=True, slots=True)
class SyncPlan:
    creates: tuple[DesiredIssue, ...]
    updates: tuple[tuple[str, DesiredIssue], ...]
    duplicate_updates: tuple[str, ...]
    existing_skipped: int


def _api_key() -> str:
    token = os.environ.get("LINEAR_API_KEY", "").strip()
    if not token:
        raise RuntimeError("LINEAR_API_KEY is required")
    return token


def graphql(query: str, variables: dict[str, Any] | None = None) -> dict[str, Any]:
    payload = json.dumps({"query": query, "variables": variables or {}}).encode("utf-8")
    req = urllib.request.Request(
        LINEAR_ENDPOINT,
        method="POST",
        data=payload,
        headers={
            "Authorization": _api_key(),
            "Content-Type": "application/json",
            "User-Agent": "molt-linear-harness/1",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=30.0) as resp:
            body = resp.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        text = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"linear HTTP {exc.code}: {text}") from exc
    except urllib.error.URLError as exc:
        raise RuntimeError(f"linear request failed: {exc}") from exc

    parsed = json.loads(body)
    if parsed.get("errors"):
        raise RuntimeError(json.dumps(parsed["errors"], indent=2))
    data = parsed.get("data")
    if not isinstance(data, dict):
        raise RuntimeError("linear response missing data")
    return data


QUERY_VIEWER = """
query Viewer {
  viewer {
    id
    email
    organization { id name urlKey }
    teams(first: 50) { nodes { id name key } }
  }
}
""".strip()

QUERY_PROJECTS = """
query Projects($first: Int!, $after: String) {
  projects(first: $first, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes {
      id
      name
      slugId
      state
      teams { nodes { id name key } }
    }
  }
}
""".strip()

QUERY_PROJECTS_FALLBACK = """
query ProjectsFallback($first: Int!, $after: String) {
  projects(first: $first, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes { id name slugId state }
  }
}
""".strip()

QUERY_STATES = """
query States($teamId: ID!) {
  workflowStates(filter: { team: { id: { eq: $teamId } } }, first: 250) {
    nodes { id name type position }
  }
}
""".strip()

QUERY_ISSUES_BY_PROJECT = """
query IssuesByProject($teamId: ID!, $projectId: ID!, $first: Int!, $after: String) {
  issues(
    filter: {
      team: { id: { eq: $teamId } }
      project: { id: { eq: $projectId } }
    }
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
      state { id name type }
      project { id name slugId }
      priority
      createdAt
      updatedAt
      url
    }
  }
}
""".strip()

QUERY_ISSUES_BY_TEAM = """
query IssuesByTeam($teamId: ID!, $first: Int!, $after: String) {
  issues(
    filter: {
      team: { id: { eq: $teamId } }
    }
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
      state { id name type }
      project { id name slugId }
      priority
      createdAt
      updatedAt
      url
    }
  }
}
""".strip()

QUERY_ISSUE_BY_ID = """
query IssueById($id: String!) {
  issue(id: $id) {
    id
    identifier
    title
    description
    state { id name type }
    project { id name slugId }
    priority
    createdAt
    updatedAt
    url
  }
}
""".strip()

QUERY_ISSUE_COMMENTS = """
query IssueComments($id: String!, $first: Int!, $after: String) {
  issue(id: $id) {
    id
    identifier
    comments(first: $first, after: $after) {
      pageInfo { hasNextPage endCursor }
      nodes {
        id
        body
        createdAt
        updatedAt
      }
    }
  }
}
""".strip()

MUTATION_ISSUE_CREATE = """
mutation IssueCreate($input: IssueCreateInput!) {
  issueCreate(input: $input) {
    success
    issue { id identifier title url }
  }
}
""".strip()

MUTATION_COMMENT_CREATE = """
mutation CommentCreate($input: CommentCreateInput!) {
  commentCreate(input: $input) {
    success
    comment {
      id
      body
      createdAt
      updatedAt
    }
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
      state { id name type }
      priority
      updatedAt
    }
  }
}
""".strip()


def cmd_whoami(_: argparse.Namespace) -> int:
    data = graphql(QUERY_VIEWER)
    print(json.dumps(data["viewer"], indent=2, sort_keys=True))
    return 0


def _resolve_team_id(team_ref: str) -> str:
    data = graphql(QUERY_VIEWER)
    teams = data["viewer"]["teams"]["nodes"]
    target = team_ref.strip().lower()
    for team in teams:
        if str(team.get("id", "")).lower() == target:
            return str(team["id"])
        if str(team.get("key", "")).lower() == target:
            return str(team["id"])
        if str(team.get("name", "")).lower() == target:
            return str(team["id"])
    raise RuntimeError(f"team not found: {team_ref}")


def cmd_list_projects(args: argparse.Namespace) -> int:
    team_id = _resolve_team_id(args.team)
    projects = _fetch_projects(team_id)
    print(json.dumps(projects, indent=2, sort_keys=True))
    return 0


def cmd_list_states(args: argparse.Namespace) -> int:
    team_id = _resolve_team_id(args.team)
    data = graphql(QUERY_STATES, {"teamId": team_id})
    print(json.dumps(data["workflowStates"]["nodes"], indent=2, sort_keys=True))
    return 0


def _resolve_project_id(team_id: str, project_ref: str | None) -> str | None:
    if not project_ref:
        return None
    projects = _fetch_projects(team_id)
    target = project_ref.strip().lower()
    for proj in projects:
        if str(proj.get("id", "")).lower() == target:
            return str(proj["id"])
        if str(proj.get("slugId", "")).lower() == target:
            return str(proj["id"])
        if str(proj.get("name", "")).lower() == target:
            return str(proj["id"])
    raise RuntimeError(f"project not found: {project_ref}")


def _fetch_projects(team_id: str) -> list[dict[str, Any]]:
    def _paginate(query: str) -> list[dict[str, Any]]:
        rows: list[dict[str, Any]] = []
        after: str | None = None
        while True:
            data = graphql(query, {"first": 100, "after": after})
            block = data["projects"]
            rows.extend(block["nodes"])
            if not block["pageInfo"]["hasNextPage"]:
                break
            after = block["pageInfo"]["endCursor"]
            if not after:
                break
        return rows

    try:
        rows = _paginate(QUERY_PROJECTS)
        filtered: list[dict[str, Any]] = []
        for row in rows:
            teams_obj = row.get("teams")
            nodes = teams_obj.get("nodes") if isinstance(teams_obj, dict) else None
            if not isinstance(nodes, list):
                filtered.append(row)
                continue
            team_ids = {
                str(team.get("id", "")).strip()
                for team in nodes
                if isinstance(team, dict)
            }
            if team_id in team_ids:
                filtered.append(row)
        return filtered
    except RuntimeError:
        # Fallback for schema variants without project.team/project.teams fields.
        return _paginate(QUERY_PROJECTS_FALLBACK)


def _fetch_issues(team_id: str, project_id: str | None) -> list[dict[str, Any]]:
    issues: list[dict[str, Any]] = []
    after: str | None = None
    while True:
        if project_id:
            data = graphql(
                QUERY_ISSUES_BY_PROJECT,
                {
                    "teamId": team_id,
                    "projectId": project_id,
                    "first": 100,
                    "after": after,
                },
            )
        else:
            data = graphql(
                QUERY_ISSUES_BY_TEAM,
                {
                    "teamId": team_id,
                    "first": 100,
                    "after": after,
                },
            )

        block = data["issues"]
        issues.extend(block["nodes"])
        if not block["pageInfo"]["hasNextPage"]:
            break
        after = block["pageInfo"]["endCursor"]
        if not after:
            break
    return issues


def cmd_list_issues(args: argparse.Namespace) -> int:
    team_id = _resolve_team_id(args.team)
    project_id = _resolve_project_id(team_id, args.project)

    issues = _fetch_issues(team_id, project_id)
    if args.active_only:
        active = {"unstarted", "started"}
        issues = [
            i
            for i in issues
            if str(i.get("state", {}).get("type", "")).lower() in active
        ]

    print(json.dumps(issues, indent=2, sort_keys=True))
    return 0


def _build_issue_description(item: dict[str, Any]) -> str:
    parts = []
    body = str(item.get("description", "")).strip()
    if body:
        parts.append(body)
    metadata: dict[str, Any] = item.get("metadata") or {}
    if metadata:
        parts.append("\n\n---\n")
        for key in sorted(metadata):
            parts.append(f"- {key}: {metadata[key]}")
    return "\n".join(parts).strip()


def _issue_priority(item: dict[str, Any]) -> int | None:
    priority = item.get("priority")
    return priority if isinstance(priority, int) else None


def _load_manifest(path: Path) -> list[dict[str, Any]]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, list):
        raise RuntimeError("manifest must be a JSON list")
    entries: list[dict[str, Any]] = []
    for index, item in enumerate(payload, start=1):
        if not isinstance(item, dict):
            raise RuntimeError(f"manifest item {index} must be an object")
        title = str(item.get("title", "")).strip()
        if not title:
            raise RuntimeError(f"manifest item {index} missing title")
        entries.append(item)
    return entries


def _title_key(title: str) -> str:
    return " ".join(title.strip().lower().split())


def _to_desired(item: dict[str, Any]) -> DesiredIssue:
    return DesiredIssue(
        title=str(item.get("title", "")).strip(),
        description=_build_issue_description(item),
        priority=_issue_priority(item),
    )


def _issue_sort_key(issue: dict[str, Any]) -> tuple[str, str]:
    created = str(issue.get("createdAt") or "")
    issue_id = str(issue.get("id") or "")
    return (created, issue_id)


def _normalize_existing_text(value: Any) -> str:
    return str(value or "").strip()


def _build_sync_plan(
    *,
    desired: list[DesiredIssue],
    existing: list[dict[str, Any]],
    update_existing: bool,
    close_duplicates: bool,
    duplicate_state_id: str | None,
) -> SyncPlan:
    by_key: dict[str, list[dict[str, Any]]] = {}
    for row in existing:
        key = _title_key(str(row.get("title") or ""))
        if not key:
            continue
        by_key.setdefault(key, []).append(row)

    creates: list[DesiredIssue] = []
    updates: list[tuple[str, DesiredIssue]] = []
    existing_skipped = 0
    desired_keys: set[str] = set()

    for want in desired:
        key = _title_key(want.title)
        desired_keys.add(key)
        matches = sorted(by_key.get(key, []), key=_issue_sort_key)
        if not matches:
            creates.append(want)
            continue

        canonical = matches[0]
        existing_skipped += 1
        if update_existing:
            existing_desc = _normalize_existing_text(canonical.get("description"))
            existing_priority = canonical.get("priority")
            if not isinstance(existing_priority, int):
                existing_priority = None
            if existing_desc != want.description or existing_priority != want.priority:
                updates.append((str(canonical["id"]), want))

    duplicate_updates: list[str] = []
    if close_duplicates and duplicate_state_id:
        for key, rows in by_key.items():
            if key not in desired_keys:
                continue
            if len(rows) < 2:
                continue
            sorted_rows = sorted(rows, key=_issue_sort_key)
            for duplicate in sorted_rows[1:]:
                state_type = str(
                    (duplicate.get("state") or {}).get("type") or ""
                ).lower()
                if state_type in {"completed", "canceled"}:
                    continue
                duplicate_updates.append(str(duplicate["id"]))

    return SyncPlan(
        creates=tuple(creates),
        updates=tuple(updates),
        duplicate_updates=tuple(duplicate_updates),
        existing_skipped=existing_skipped,
    )


async def _run_limited(
    semaphore: asyncio.Semaphore,
    operation: str,
    variables: dict[str, Any],
) -> tuple[str, dict[str, Any]]:
    async with semaphore:
        data = await asyncio.to_thread(graphql, operation, variables)
        return (operation, data)


async def _execute_sync_plan(
    *,
    plan: SyncPlan,
    team_id: str,
    project_id: str | None,
    duplicate_state_id: str | None,
    concurrency: int,
) -> dict[str, Any]:
    semaphore = asyncio.Semaphore(max(1, concurrency))
    tasks: list[asyncio.Task[tuple[str, dict[str, Any]]]] = []

    for item in plan.creates:
        payload: dict[str, Any] = {
            "teamId": team_id,
            "title": item.title,
            "description": item.description,
        }
        if project_id:
            payload["projectId"] = project_id
        if item.priority is not None:
            payload["priority"] = item.priority
        tasks.append(
            asyncio.create_task(
                _run_limited(
                    semaphore,
                    MUTATION_ISSUE_CREATE,
                    {"input": payload},
                )
            )
        )

    for issue_id, item in plan.updates:
        input_payload: dict[str, Any] = {
            "description": item.description,
        }
        if item.priority is not None:
            input_payload["priority"] = item.priority
        tasks.append(
            asyncio.create_task(
                _run_limited(
                    semaphore,
                    MUTATION_ISSUE_UPDATE,
                    {
                        "id": issue_id,
                        "input": input_payload,
                    },
                )
            )
        )

    if duplicate_state_id:
        for issue_id in plan.duplicate_updates:
            tasks.append(
                asyncio.create_task(
                    _run_limited(
                        semaphore,
                        MUTATION_ISSUE_UPDATE,
                        {
                            "id": issue_id,
                            "input": {"stateId": duplicate_state_id},
                        },
                    )
                )
            )

    created: list[dict[str, Any]] = []
    updated: list[dict[str, Any]] = []
    duplicate_closed: list[str] = []
    errors: list[str] = []

    results = await asyncio.gather(*tasks, return_exceptions=True)
    for result in results:
        if isinstance(result, BaseException):
            errors.append(str(result))
            continue

        operation, payload = result
        if operation == MUTATION_ISSUE_CREATE:
            block = payload.get("issueCreate") or {}
            if block.get("success"):
                issue = block.get("issue")
                if isinstance(issue, dict):
                    created.append(issue)
            else:
                errors.append("issueCreate returned success=false")
            continue

        block = payload.get("issueUpdate") or {}
        issue = block.get("issue") or {}
        if not block.get("success"):
            errors.append("issueUpdate returned success=false")
            continue

        state_type = str((issue.get("state") or {}).get("type") or "").lower()
        issue_id = str(issue.get("id") or "")
        if state_type in {"completed", "canceled"} and issue_id:
            duplicate_closed.append(issue_id)
        elif isinstance(issue, dict):
            updated.append(issue)

    return {
        "created": created,
        "updated": updated,
        "duplicate_closed_issue_ids": duplicate_closed,
        "errors": errors,
    }


def _state_id_by_name(team_id: str, state_name: str) -> str:
    data = graphql(QUERY_STATES, {"teamId": team_id})
    target = state_name.strip().lower()
    for row in data["workflowStates"]["nodes"]:
        if str(row.get("name") or "").strip().lower() == target:
            return str(row["id"])
    raise RuntimeError(f"workflow state not found: {state_name}")


def _resolve_issue_id(team_id: str, issue_ref: str) -> str:
    target = issue_ref.strip()
    if not target:
        raise RuntimeError("issue reference cannot be empty")

    try:
        data = graphql(QUERY_ISSUE_BY_ID, {"id": target})
        issue = data.get("issue")
        if isinstance(issue, dict):
            issue_id = str(issue.get("id") or "").strip()
            if issue_id:
                return issue_id
    except RuntimeError:
        pass

    normalized = target.lower()
    issues = _fetch_issues(team_id, None)
    for issue in issues:
        issue_id = str(issue.get("id") or "").strip()
        identifier = str(issue.get("identifier") or "").strip()
        if issue_id.lower() == normalized or identifier.lower() == normalized:
            return issue_id
    raise RuntimeError(f"issue not found in team: {issue_ref}")


def _resolve_issue(team_id: str, issue_ref: str) -> dict[str, Any]:
    issue_id = _resolve_issue_id(team_id, issue_ref)
    data = graphql(QUERY_ISSUE_BY_ID, {"id": issue_id})
    issue = data.get("issue")
    if isinstance(issue, dict):
        return issue
    for row in _fetch_issues(team_id, None):
        row_id = str(row.get("id") or "").strip()
        if row_id == issue_id:
            return row
    raise RuntimeError(f"issue resolution failed: {issue_ref}")


def _sync_manifest(
    *,
    team: str,
    project: str | None,
    manifest_path: Path,
    dry_run: bool,
    update_existing: bool,
    close_duplicates: bool,
    duplicate_state: str,
    concurrency: int,
) -> dict[str, Any]:
    team_id = _resolve_team_id(team)
    project_id = _resolve_project_id(team_id, project)

    manifest = _load_manifest(manifest_path)
    desired = [_to_desired(item) for item in manifest]
    existing = _fetch_issues(team_id, project_id)

    duplicate_state_id: str | None = None
    if close_duplicates:
        duplicate_state_id = _state_id_by_name(team_id, duplicate_state)

    plan = _build_sync_plan(
        desired=desired,
        existing=existing,
        update_existing=update_existing,
        close_duplicates=close_duplicates,
        duplicate_state_id=duplicate_state_id,
    )

    summary: dict[str, Any] = {
        "team": team,
        "project": project,
        "manifest": str(manifest_path),
        "manifest_count": len(manifest),
        "existing_count": len(existing),
        "planned": {
            "create": len(plan.creates),
            "update": len(plan.updates),
            "close_duplicates": len(plan.duplicate_updates),
            "existing_skipped": plan.existing_skipped,
        },
        "dry_run": dry_run,
    }

    if dry_run:
        summary["sample"] = {
            "create_titles": [item.title for item in plan.creates[:10]],
            "update_issue_ids": [issue_id for issue_id, _ in plan.updates[:10]],
            "close_duplicate_issue_ids": list(plan.duplicate_updates[:10]),
        }
        return summary

    outcome = asyncio.run(
        _execute_sync_plan(
            plan=plan,
            team_id=team_id,
            project_id=project_id,
            duplicate_state_id=duplicate_state_id,
            concurrency=concurrency,
        )
    )

    summary["result"] = {
        "created_count": len(outcome["created"]),
        "updated_count": len(outcome["updated"]),
        "duplicate_closed_count": len(outcome["duplicate_closed_issue_ids"]),
        "error_count": len(outcome["errors"]),
        "created": outcome["created"],
        "updated": outcome["updated"],
        "duplicate_closed_issue_ids": outcome["duplicate_closed_issue_ids"],
        "errors": outcome["errors"],
    }
    return summary


def cmd_create_issue(args: argparse.Namespace) -> int:
    team_id = _resolve_team_id(args.team)
    project_id = _resolve_project_id(team_id, args.project)
    input_payload: dict[str, Any] = {
        "teamId": team_id,
        "title": args.title,
        "description": args.description or "",
    }
    if project_id:
        input_payload["projectId"] = project_id
    if args.priority is not None:
        input_payload["priority"] = args.priority

    data = graphql(MUTATION_ISSUE_CREATE, {"input": input_payload})
    result = data["issueCreate"]
    if not result.get("success"):
        raise RuntimeError("issueCreate returned success=false")
    print(json.dumps(result["issue"], indent=2, sort_keys=True))
    return 0


def cmd_get_issue(args: argparse.Namespace) -> int:
    team_id = _resolve_team_id(args.team)
    issue = _resolve_issue(team_id, args.issue)
    print(json.dumps(issue, indent=2, sort_keys=True))
    return 0


def cmd_update_issue(args: argparse.Namespace) -> int:
    team_id = _resolve_team_id(args.team)
    issue_id = _resolve_issue_id(team_id, args.issue)
    project_id = _resolve_project_id(team_id, args.project) if args.project else None
    state_id = _state_id_by_name(team_id, args.state) if args.state else None

    input_payload: dict[str, Any] = {}
    if args.title is not None:
        input_payload["title"] = args.title
    if args.description is not None:
        input_payload["description"] = args.description
    if args.priority is not None:
        input_payload["priority"] = args.priority
    if project_id is not None:
        input_payload["projectId"] = project_id
    if state_id is not None:
        input_payload["stateId"] = state_id

    if not input_payload:
        raise RuntimeError(
            "no update fields provided; set at least one of --title/--description/--priority/--project/--state"
        )

    data = graphql(MUTATION_ISSUE_UPDATE, {"id": issue_id, "input": input_payload})
    result = data["issueUpdate"]
    if not result.get("success"):
        raise RuntimeError("issueUpdate returned success=false")
    print(json.dumps(result["issue"], indent=2, sort_keys=True))
    return 0


def cmd_list_comments(args: argparse.Namespace) -> int:
    team_id = _resolve_team_id(args.team)
    issue_id = _resolve_issue_id(team_id, args.issue)
    first = max(1, min(int(args.page_size), 250))
    remaining = max(1, int(args.limit))

    comments: list[dict[str, Any]] = []
    after: str | None = None
    while True:
        data = graphql(
            QUERY_ISSUE_COMMENTS,
            {
                "id": issue_id,
                "first": min(first, remaining),
                "after": after,
            },
        )
        issue = data.get("issue")
        if not isinstance(issue, dict):
            raise RuntimeError("issue not found when listing comments")
        block = issue.get("comments")
        if not isinstance(block, dict):
            raise RuntimeError("issue comments payload missing")

        nodes = block.get("nodes")
        if isinstance(nodes, list):
            for node in nodes:
                if isinstance(node, dict):
                    comments.append(node)
                    remaining -= 1
                    if remaining <= 0:
                        break
        if remaining <= 0:
            break

        page_info = block.get("pageInfo") or {}
        has_next = bool(page_info.get("hasNextPage"))
        if not has_next:
            break
        end_cursor = page_info.get("endCursor")
        if not isinstance(end_cursor, str) or not end_cursor:
            break
        after = end_cursor

    print(json.dumps(comments, indent=2, sort_keys=True))
    return 0


def cmd_comment_issue(args: argparse.Namespace) -> int:
    team_id = _resolve_team_id(args.team)
    issue_id = _resolve_issue_id(team_id, args.issue)
    body = str(args.body or "").strip()
    if not body:
        raise RuntimeError("--body cannot be empty")

    data = graphql(
        MUTATION_COMMENT_CREATE,
        {
            "input": {
                "issueId": issue_id,
                "body": body,
            }
        },
    )
    result = data["commentCreate"]
    if not result.get("success"):
        raise RuntimeError("commentCreate returned success=false")
    print(json.dumps(result["comment"], indent=2, sort_keys=True))
    return 0


def cmd_bulk_create(args: argparse.Namespace) -> int:
    manifest = _load_manifest(Path(args.manifest))

    team_id = _resolve_team_id(args.team)
    project_id = _resolve_project_id(team_id, args.project)

    created: list[dict[str, Any]] = []
    for index, item in enumerate(manifest, start=1):
        title = str(item.get("title", "")).strip()
        input_payload: dict[str, Any] = {
            "teamId": team_id,
            "title": title,
            "description": _build_issue_description(item),
        }
        if project_id:
            input_payload["projectId"] = project_id
        priority = item.get("priority")
        if isinstance(priority, int):
            input_payload["priority"] = priority

        data = graphql(MUTATION_ISSUE_CREATE, {"input": input_payload})
        result = data["issueCreate"]
        if not result.get("success"):
            raise RuntimeError(f"issueCreate failed for item {index}: {title}")
        issue = result["issue"]
        created.append(issue)
        print(
            f"[{index}/{len(manifest)}] created {issue['identifier']}: {issue['title']}"
        )

    print(json.dumps(created, indent=2, sort_keys=True))
    return 0


def cmd_sync_manifest(args: argparse.Namespace) -> int:
    summary = _sync_manifest(
        team=args.team,
        project=args.project,
        manifest_path=Path(args.manifest).resolve(),
        dry_run=bool(args.dry_run),
        update_existing=bool(args.update_existing),
        close_duplicates=bool(args.close_duplicates),
        duplicate_state=args.duplicate_state,
        concurrency=max(1, int(args.concurrency)),
    )
    print(json.dumps(summary, indent=2, sort_keys=True))

    result = summary.get("result")
    if isinstance(result, dict) and int(result.get("error_count", 0)) > 0:
        return 1
    return 0


def cmd_sync_index(args: argparse.Namespace) -> int:
    index = json.loads(Path(args.index).resolve().read_text(encoding="utf-8"))
    if not isinstance(index, list):
        raise RuntimeError("index must be a JSON list")

    all_summaries: list[dict[str, Any]] = []
    total_errors = 0
    for row in index:
        if not isinstance(row, dict):
            raise RuntimeError("index row must be an object")
        project = str(row.get("project") or "").strip()
        rel_path = str(row.get("path") or "").strip()
        if not project or not rel_path:
            raise RuntimeError("index row must include project and path")
        manifest_path = Path(rel_path)
        summary = _sync_manifest(
            team=args.team,
            project=project,
            manifest_path=manifest_path.resolve(),
            dry_run=bool(args.dry_run),
            update_existing=bool(args.update_existing),
            close_duplicates=bool(args.close_duplicates),
            duplicate_state=args.duplicate_state,
            concurrency=max(1, int(args.concurrency)),
        )
        all_summaries.append(summary)

        result = summary.get("result")
        if isinstance(result, dict):
            total_errors += int(result.get("error_count", 0))

    output = {
        "index": str(Path(args.index).resolve()),
        "count": len(all_summaries),
        "dry_run": bool(args.dry_run),
        "total_errors": total_errors,
        "summaries": all_summaries,
    }
    print(json.dumps(output, indent=2, sort_keys=True))
    return 1 if total_errors > 0 else 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Linear workspace harness for Molt")
    sub = parser.add_subparsers(dest="cmd", required=True)

    whoami = sub.add_parser("whoami", help="Show viewer + teams")
    whoami.set_defaults(func=cmd_whoami)

    list_projects = sub.add_parser("list-projects", help="List projects for a team")
    list_projects.add_argument("--team", required=True, help="Team id/key/name")
    list_projects.set_defaults(func=cmd_list_projects)

    list_states = sub.add_parser("list-states", help="List workflow states for a team")
    list_states.add_argument("--team", required=True, help="Team id/key/name")
    list_states.set_defaults(func=cmd_list_states)

    list_issues = sub.add_parser("list-issues", help="List issues for a team/project")
    list_issues.add_argument("--team", required=True, help="Team id/key/name")
    list_issues.add_argument("--project", default=None, help="Project id/slug/name")
    list_issues.add_argument("--active-only", action="store_true")
    list_issues.set_defaults(func=cmd_list_issues)

    get_issue = sub.add_parser("get-issue", help="Fetch one issue by id/identifier")
    get_issue.add_argument("--team", required=True, help="Team id/key/name")
    get_issue.add_argument(
        "--issue", required=True, help="Issue id or identifier (for example MOL-123)"
    )
    get_issue.set_defaults(func=cmd_get_issue)

    create = sub.add_parser("create-issue", help="Create one issue")
    create.add_argument("--team", required=True, help="Team id/key/name")
    create.add_argument("--project", default=None, help="Project id/slug/name")
    create.add_argument("--title", required=True)
    create.add_argument("--description", default="")
    create.add_argument("--priority", type=int, default=None)
    create.set_defaults(func=cmd_create_issue)

    update = sub.add_parser("update-issue", help="Update one issue by id/identifier")
    update.add_argument("--team", required=True, help="Team id/key/name")
    update.add_argument(
        "--issue", required=True, help="Issue id or identifier (for example MOL-123)"
    )
    update.add_argument("--title", default=None)
    update.add_argument("--description", default=None)
    update.add_argument("--priority", type=int, default=None)
    update.add_argument("--project", default=None, help="Project id/slug/name")
    update.add_argument(
        "--state",
        default=None,
        help="Workflow state name (for example In Progress, Done, Canceled)",
    )
    update.set_defaults(func=cmd_update_issue)

    comments = sub.add_parser(
        "list-comments", help="List issue comments by id/identifier"
    )
    comments.add_argument("--team", required=True, help="Team id/key/name")
    comments.add_argument(
        "--issue", required=True, help="Issue id or identifier (for example MOL-123)"
    )
    comments.add_argument(
        "--limit", type=int, default=200, help="Maximum comments to return"
    )
    comments.add_argument(
        "--page-size", type=int, default=100, help="Page size for GraphQL pagination"
    )
    comments.set_defaults(func=cmd_list_comments)

    comment = sub.add_parser(
        "comment-issue", help="Create one comment on an issue by id/identifier"
    )
    comment.add_argument("--team", required=True, help="Team id/key/name")
    comment.add_argument(
        "--issue", required=True, help="Issue id or identifier (for example MOL-123)"
    )
    comment.add_argument("--body", required=True, help="Comment text")
    comment.set_defaults(func=cmd_comment_issue)

    bulk = sub.add_parser("bulk-create", help="Create issues from manifest JSON list")
    bulk.add_argument("--team", required=True, help="Team id/key/name")
    bulk.add_argument("--project", default=None, help="Project id/slug/name")
    bulk.add_argument("--manifest", required=True, help="Path to JSON list")
    bulk.set_defaults(func=cmd_bulk_create)

    sync_manifest = sub.add_parser(
        "sync-manifest",
        help=(
            "Idempotently sync one manifest into a team/project "
            "(create missing, optionally update existing and close duplicates)"
        ),
    )
    sync_manifest.add_argument("--team", required=True, help="Team id/key/name")
    sync_manifest.add_argument("--project", default=None, help="Project id/slug/name")
    sync_manifest.add_argument("--manifest", required=True, help="Path to JSON list")
    sync_manifest.add_argument("--dry-run", action="store_true")
    sync_manifest.add_argument("--update-existing", action="store_true")
    sync_manifest.add_argument("--close-duplicates", action="store_true")
    sync_manifest.add_argument(
        "--duplicate-state",
        default="Canceled",
        help="Workflow state name used when closing duplicates",
    )
    sync_manifest.add_argument(
        "--concurrency",
        type=int,
        default=2,
        help="Max concurrent Linear mutations",
    )
    sync_manifest.set_defaults(func=cmd_sync_manifest)

    sync_index = sub.add_parser(
        "sync-index",
        help="Sync all project manifests from an index JSON file",
    )
    sync_index.add_argument("--team", required=True, help="Team id/key/name")
    sync_index.add_argument(
        "--index",
        default="ops/linear/manifests/index.json",
        help="Path to manifest index JSON",
    )
    sync_index.add_argument("--dry-run", action="store_true")
    sync_index.add_argument("--update-existing", action="store_true")
    sync_index.add_argument("--close-duplicates", action="store_true")
    sync_index.add_argument(
        "--duplicate-state",
        default="Canceled",
        help="Workflow state name used when closing duplicates",
    )
    sync_index.add_argument(
        "--concurrency",
        type=int,
        default=2,
        help="Max concurrent Linear mutations per project sync",
    )
    sync_index.set_defaults(func=cmd_sync_index)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        return int(args.func(args))
    except Exception as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
