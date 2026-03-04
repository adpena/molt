from __future__ import annotations

import json
import urllib.error
import urllib.request
from dataclasses import dataclass
from datetime import datetime
from typing import Any

from .config import normalize_state_name
from .errors import TrackerError
from .models import BlockerRef, Issue, TrackerConfig


_CANDIDATE_QUERY = """
query CandidateIssues($projectSlugs: [String!], $states: [String!], $after: String) {
  issues(
    filter: {
      project: { slugId: { in: $projectSlugs } }
      state: { name: { in: $states } }
    }
    first: 50
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
      branchName
      url
      createdAt
      updatedAt
      state { name }
      labels { nodes { name } }
      inverseRelations {
        nodes {
          type
          issue {
            id
            identifier
            state { name }
          }
        }
      }
    }
  }
}
""".strip()


_TERMINAL_QUERY = """
query TerminalIssues($projectSlugs: [String!], $states: [String!], $after: String) {
  issues(
    filter: {
      project: { slugId: { in: $projectSlugs } }
      state: { name: { in: $states } }
    }
    first: 50
    after: $after
    orderBy: updatedAt
  ) {
    pageInfo { hasNextPage endCursor }
    nodes {
      id
      identifier
      title
      state { name }
    }
  }
}
""".strip()


_STATE_REFRESH_QUERY = """
query IssueStates($ids: [ID!]!) {
  issues(filter: { id: { in: $ids } }, first: 250) {
    nodes {
      id
      identifier
      title
      description
      priority
      branchName
      url
      createdAt
      updatedAt
      state { name }
      labels { nodes { name } }
      inverseRelations {
        nodes {
          type
          issue {
            id
            identifier
            state { name }
          }
        }
      }
    }
  }
}
""".strip()


@dataclass(frozen=True, slots=True)
class GraphQLResult:
    success: bool
    payload: dict[str, Any]


class LinearTrackerClient:
    def __init__(self, config: TrackerConfig, timeout_seconds: float = 30.0) -> None:
        self._config = config
        self._timeout_seconds = timeout_seconds

    def fetch_candidate_issues(self) -> list[Issue]:
        return self._fetch_paginated(
            query=_CANDIDATE_QUERY,
            states=list(self._config.active_states),
        )

    def fetch_issues_by_states(self, state_names: list[str]) -> list[Issue]:
        if not state_names:
            return []
        return self._fetch_paginated(query=_TERMINAL_QUERY, states=state_names)

    def fetch_issue_states_by_ids(self, issue_ids: list[str]) -> dict[str, Issue]:
        if not issue_ids:
            return {}
        body = self._graphql(_STATE_REFRESH_QUERY, {"ids": issue_ids})
        data = body.get("data", {})
        nodes = data.get("issues", {}).get("nodes", [])
        issues: dict[str, Issue] = {}
        for node in nodes:
            issue = _normalize_issue(node)
            issues[issue.id] = issue
        return issues

    def execute_raw_graphql(
        self, query: str, variables: dict[str, Any] | None = None
    ) -> GraphQLResult:
        query_text = query.strip()
        if not query_text:
            return GraphQLResult(
                success=False, payload={"error": "query must be non-empty"}
            )
        if _count_graphql_operations(query_text) != 1:
            return GraphQLResult(
                success=False,
                payload={"error": "query must contain exactly one operation"},
            )

        try:
            body = self._graphql(query_text, variables or {})
        except TrackerError as exc:
            return GraphQLResult(success=False, payload={"error": str(exc)})

        if body.get("errors"):
            return GraphQLResult(success=False, payload=body)
        return GraphQLResult(success=True, payload=body)

    def _fetch_paginated(self, query: str, states: list[str]) -> list[Issue]:
        after: str | None = None
        seen: dict[str, Issue] = {}
        while True:
            body = self._graphql(
                query,
                {
                    "projectSlugs": list(self._config.project_slugs),
                    "states": states,
                    "after": after,
                },
            )
            data = body.get("data", {})
            issues_obj = data.get("issues", {})
            nodes = issues_obj.get("nodes", [])
            page_info = issues_obj.get("pageInfo", {})

            for node in nodes:
                issue = _normalize_issue(node)
                seen[issue.id] = issue

            has_next_page = bool(page_info.get("hasNextPage"))
            if not has_next_page:
                break

            end_cursor = page_info.get("endCursor")
            if not isinstance(end_cursor, str) or not end_cursor:
                raise TrackerError("linear_missing_end_cursor")
            after = end_cursor
        return list(seen.values())

    def _graphql(self, query: str, variables: dict[str, Any]) -> dict[str, Any]:
        if not self._config.api_key:
            raise TrackerError("missing_tracker_api_key")

        payload = json.dumps({"query": query, "variables": variables}).encode("utf-8")
        request = urllib.request.Request(
            self._config.endpoint,
            method="POST",
            data=payload,
            headers={
                "Content-Type": "application/json",
                "Authorization": self._config.api_key,
                "User-Agent": "molt-symphony/1",
            },
        )
        try:
            with urllib.request.urlopen(
                request, timeout=self._timeout_seconds
            ) as response:
                status = response.status
                body_raw = response.read().decode("utf-8")
        except urllib.error.HTTPError as exc:
            raise TrackerError(f"linear_api_status status={exc.code}") from exc
        except urllib.error.URLError as exc:
            raise TrackerError(f"linear_api_request {exc}") from exc

        if status != 200:
            raise TrackerError(f"linear_api_status status={status}")

        try:
            body = json.loads(body_raw)
        except json.JSONDecodeError as exc:
            raise TrackerError("linear_unknown_payload") from exc

        if not isinstance(body, dict):
            raise TrackerError("linear_unknown_payload")
        if body.get("errors"):
            raise TrackerError("linear_graphql_errors")
        return body


def _normalize_issue(node: dict[str, Any]) -> Issue:
    issue_id = str(node.get("id") or "").strip()
    identifier = str(node.get("identifier") or "").strip()
    title = str(node.get("title") or "").strip()
    state_name = str(((node.get("state") or {}).get("name") or "")).strip()

    if not issue_id or not identifier or not title or not state_name:
        raise TrackerError("linear_unknown_payload missing issue identity fields")

    raw_priority = node.get("priority")
    priority: int | None
    if isinstance(raw_priority, int):
        priority = raw_priority
    else:
        try:
            if raw_priority is None:
                raise ValueError("priority missing")
            priority = int(str(raw_priority))
        except (TypeError, ValueError):
            priority = None

    labels_node = (node.get("labels") or {}).get("nodes") or []
    labels = tuple(
        sorted(
            {
                str(label.get("name", "")).strip().lower()
                for label in labels_node
                if str(label.get("name", "")).strip()
            }
        )
    )

    blockers: list[BlockerRef] = []
    inverse_relations = (node.get("inverseRelations") or {}).get("nodes") or []
    for rel in inverse_relations:
        rel_type = str(rel.get("type") or "").strip().lower()
        if rel_type != "blocks":
            continue
        rel_issue = rel.get("issue") or {}
        blocker_state = (rel_issue.get("state") or {}).get("name") or None
        blockers.append(
            BlockerRef(
                id=_optional_text(rel_issue.get("id")),
                identifier=_optional_text(rel_issue.get("identifier")),
                state=_optional_text(blocker_state),
            )
        )

    created_at = _parse_iso8601(_optional_text(node.get("createdAt")))
    updated_at = _parse_iso8601(_optional_text(node.get("updatedAt")))

    return Issue(
        id=issue_id,
        identifier=identifier,
        title=title,
        description=_optional_text(node.get("description")),
        priority=priority,
        state=state_name,
        branch_name=_optional_text(node.get("branchName")),
        url=_optional_text(node.get("url")),
        labels=labels,
        blocked_by=tuple(blockers),
        created_at=created_at,
        updated_at=updated_at,
    )


def _optional_text(value: Any) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    return text or None


def _parse_iso8601(value: str | None) -> datetime | None:
    if not value:
        return None
    normalized = value.replace("Z", "+00:00")
    try:
        return datetime.fromisoformat(normalized)
    except ValueError:
        return None


def _count_graphql_operations(query: str) -> int:
    tokens = ["query ", "mutation ", "subscription "]
    lowered = query.lower()
    count = sum(lowered.count(token) for token in tokens)
    if count == 0 and "{" in query:
        return 1
    return count


def blocker_allows_todo(issue: Issue, terminal_states: set[str]) -> bool:
    if normalize_state_name(issue.state) != "todo":
        return True
    for blocker in issue.blocked_by:
        blocker_state = normalize_state_name(blocker.state or "")
        if blocker_state and blocker_state not in terminal_states:
            return False
    return True
