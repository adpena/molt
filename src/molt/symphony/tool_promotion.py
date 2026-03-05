from __future__ import annotations

import json
import re
from collections import Counter
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any


def _utc_now() -> str:
    return datetime.now(UTC).isoformat().replace("+00:00", "Z")


def _slugify(value: str) -> str:
    normalized = re.sub(r"[^a-z0-9]+", "-", value.strip().lower())
    normalized = normalized.strip("-")
    return normalized or "candidate"


def _candidate_id(command: str) -> str:
    return f"command-macro-{_slugify(command)}"


def _recommended_surface(command: str) -> str:
    normalized = command.strip().lower()
    if "tools/" in normalized and ".py" in normalized:
        return "tool_or_wrapper"
    if "symphony_" in normalized:
        return "hook_or_command_macro"
    return "command_macro"


def _candidate_title(command: str) -> str:
    compact = re.sub(r"\s+", " ", command.strip())
    if len(compact) > 72:
        compact = compact[:69].rstrip() + "..."
    return f"Promote recurring Symphony action: {compact}"


@dataclass(slots=True)
class ToolPromotionStore:
    events_path: Path
    distillations_dir: Path

    def record(self, row: dict[str, Any]) -> dict[str, Any]:
        payload = dict(row)
        payload.setdefault("recorded_at", _utc_now())
        self.events_path.parent.mkdir(parents=True, exist_ok=True)
        with self.events_path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(payload, ensure_ascii=True, sort_keys=True) + "\n")
        return payload

    def load(self, *, limit: int = 200) -> list[dict[str, Any]]:
        if not self.events_path.exists():
            return []
        rows: list[dict[str, Any]] = []
        with self.events_path.open("r", encoding="utf-8") as handle:
            for raw in handle:
                text = raw.strip()
                if not text:
                    continue
                try:
                    parsed = json.loads(text)
                except Exception:
                    continue
                if isinstance(parsed, dict):
                    rows.append(parsed)
        if limit > 0:
            return rows[-limit:]
        return rows

    def generate_manifests(
        self,
        *,
        distillation: dict[str, Any],
        limit: int = 10,
    ) -> dict[str, Any]:
        ready_candidates = distillation.get("ready_candidates")
        candidates = (
            [row for row in ready_candidates if isinstance(row, dict)]
            if isinstance(ready_candidates, list)
            else []
        )
        manifests_dir = self.distillations_dir.parent / "manifests"
        manifests_dir.mkdir(parents=True, exist_ok=True)

        manifests: list[dict[str, Any]] = []
        for candidate in candidates[: max(int(limit), 0)]:
            candidate_id = str(candidate.get("candidate_id") or "").strip()
            command = str(candidate.get("command") or "").strip()
            if not candidate_id or not command:
                continue
            manifest = {
                "manifest_version": 1,
                "kind": "tool_promotion_manifest",
                "generated_at": _utc_now(),
                "candidate_id": candidate_id,
                "title": _candidate_title(command),
                "recommended_surface": _recommended_surface(command),
                "command": command,
                "rationale": str(candidate.get("rationale") or "").strip(),
                "evidence": {
                    "success_count": int(candidate.get("success_count") or 0),
                    "first_seen": candidate.get("first_seen"),
                    "last_seen": candidate.get("last_seen"),
                    "supporting_tools": dict(candidate.get("supporting_tools") or {}),
                },
                "source_distillation_path": distillation.get("path"),
                "review_checklist": [
                    "Confirm the command solves a recurring operational need.",
                    "Decide whether the abstraction belongs in a tool, hook, or workflow wrapper.",
                    "Add deterministic tests before promoting into the active Symphony tool surface.",
                ],
            }
            path = manifests_dir / f"{candidate_id}.json"
            path.write_text(
                json.dumps(manifest, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            manifests.append(
                {
                    "candidate_id": candidate_id,
                    "path": str(path),
                    "title": manifest["title"],
                    "recommended_surface": manifest["recommended_surface"],
                }
            )

        return {
            "generated_at": _utc_now(),
            "manifest_count": len(manifests),
            "manifests_dir": str(manifests_dir),
            "manifests": manifests,
        }

    def distill_candidates(
        self,
        *,
        taste_rows: list[dict[str, Any]],
        limit: int = 200,
        min_success_count: int = 3,
    ) -> dict[str, Any]:
        rows = taste_rows[-limit:] if limit > 0 else list(taste_rows)
        success_counts: Counter[str] = Counter()
        tool_support: dict[str, Counter[str]] = {}
        first_seen: dict[str, str] = {}
        last_seen: dict[str, str] = {}

        for row in rows:
            recorded_at = str(row.get("recorded_at") or row.get("generated_at") or "")
            tools_used = [str(tool) for tool in (row.get("tools_used") or []) if tool]
            for action in row.get("successful_actions") or []:
                command = str(action or "").strip()
                if not command:
                    continue
                success_counts[command] += 1
                if recorded_at and command not in first_seen:
                    first_seen[command] = recorded_at
                if recorded_at:
                    last_seen[command] = recorded_at
                support = tool_support.setdefault(command, Counter())
                for tool in tools_used:
                    support[tool] += 1

        candidates: list[dict[str, Any]] = []
        for command, count in success_counts.most_common():
            candidate = {
                "candidate_id": _candidate_id(command),
                "kind": "command_macro",
                "command": command,
                "success_count": count,
                "first_seen": first_seen.get(command),
                "last_seen": last_seen.get(command),
                "supporting_tools": dict(
                    tool_support.get(command, Counter()).most_common(10)
                ),
                "ready": count >= max(1, int(min_success_count)),
                "rationale": (
                    "Recurring successful action observed in Symphony taste memory; "
                    "candidate for explicit tool or hook extraction."
                ),
            }
            candidates.append(candidate)

        ready_candidates = [row for row in candidates if bool(row.get("ready"))]
        payload = {
            "generated_at": _utc_now(),
            "samples": len(rows),
            "min_success_count": max(1, int(min_success_count)),
            "candidate_count": len(candidates),
            "ready_candidate_count": len(ready_candidates),
            "top_candidates": candidates[:20],
            "ready_candidates": ready_candidates[:20],
        }
        self.distillations_dir.mkdir(parents=True, exist_ok=True)
        stamp = payload["generated_at"].replace(":", "").replace("-", "")
        out = self.distillations_dir / f"tool_promotion_{stamp}.json"
        payload["path"] = str(out)
        payload["manifest_batch"] = self.generate_manifests(
            distillation=payload,
            limit=10,
        )
        out.write_text(
            json.dumps(payload, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        return payload
