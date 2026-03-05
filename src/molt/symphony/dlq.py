from __future__ import annotations

import hashlib
import json
from collections import Counter
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any


def _utc_now() -> str:
    return datetime.now(UTC).isoformat().replace("+00:00", "Z")


def dead_letter_fingerprint(*, kind: str, name: str, command: list[str]) -> str:
    payload = json.dumps(
        {"kind": kind, "name": name, "command": command},
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()[:16]


@dataclass(slots=True)
class DeadLetterQueue:
    path: Path

    def append(self, row: dict[str, Any]) -> dict[str, Any]:
        payload = dict(row)
        payload.setdefault("recorded_at", _utc_now())
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self.path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(payload, ensure_ascii=True, sort_keys=True) + "\n")
        return payload

    def load(self, *, limit: int = 200) -> list[dict[str, Any]]:
        if not self.path.exists():
            return []
        rows: list[dict[str, Any]] = []
        with self.path.open("r", encoding="utf-8") as handle:
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

    def summary(self, *, limit: int = 200) -> dict[str, Any]:
        rows = self.load(limit=limit)
        by_kind = Counter(str(row.get("kind") or "unknown") for row in rows)
        by_name = Counter(str(row.get("name") or "unknown") for row in rows)
        by_fingerprint = Counter(
            str(row.get("fingerprint") or "unknown") for row in rows
        )
        latest = rows[-1] if rows else None
        return {
            "path": str(self.path),
            "count": len(rows),
            "by_kind": dict(by_kind),
            "by_name": dict(by_name.most_common(20)),
            "by_fingerprint": dict(by_fingerprint.most_common(20)),
            "latest": latest,
            "health": self.health_summary(limit=limit),
        }

    def append_replay_result(
        self,
        *,
        target_fingerprint: str,
        command: list[str],
        returncode: int,
        dry_run: bool = False,
    ) -> dict[str, Any]:
        return self.append(
            {
                "kind": "dlq_replay_attempt",
                "target_fingerprint": target_fingerprint,
                "command": list(command),
                "returncode": int(returncode),
                "status": "dry_run"
                if dry_run
                else ("success" if int(returncode) == 0 else "fail"),
            }
        )

    def health_summary(self, *, limit: int = 200) -> dict[str, Any]:
        rows = self.load(limit=limit)
        open_failures: dict[str, dict[str, Any]] = {}
        replay_success_count = 0
        replay_failure_count = 0
        replay_dry_run_count = 0
        recurring_failures: Counter[str] = Counter()

        for row in rows:
            kind = str(row.get("kind") or "")
            if kind == "recursive_loop_step_failure":
                fingerprint = str(row.get("fingerprint") or "").strip()
                if not fingerprint:
                    continue
                recurring_failures[fingerprint] += 1
                open_failures[fingerprint] = {
                    "fingerprint": fingerprint,
                    "name": str(row.get("name") or ""),
                    "phase": str(row.get("phase") or ""),
                    "count": recurring_failures[fingerprint],
                    "last_seen": str(row.get("recorded_at") or ""),
                    "command": list(row.get("command") or []),
                }
                continue

            if kind != "dlq_replay_attempt":
                continue
            target = str(row.get("target_fingerprint") or "").strip()
            status = str(row.get("status") or "").strip().lower()
            if status == "success":
                replay_success_count += 1
                open_failures.pop(target, None)
            elif status == "dry_run":
                replay_dry_run_count += 1
            elif status:
                replay_failure_count += 1

        recurring_open = {
            fingerprint: int(payload.get("count") or 0)
            for fingerprint, payload in open_failures.items()
            if int(payload.get("count") or 0) >= 2
        }
        latest_open = list(open_failures.values())[-1] if open_failures else None
        return {
            "open_failure_count": len(open_failures),
            "open_failures": list(open_failures.values())[-20:],
            "recurring_open_fingerprints": recurring_open,
            "replay_success_count": replay_success_count,
            "replay_failure_count": replay_failure_count,
            "replay_dry_run_count": replay_dry_run_count,
            "latest_open_failure": latest_open,
        }

    def replay_candidates(self, *, limit: int = 200) -> list[dict[str, Any]]:
        health = self.health_summary(limit=limit)
        open_failures = health.get("open_failures")
        if not isinstance(open_failures, list):
            return []
        candidates = [row for row in open_failures if isinstance(row, dict)]
        candidates.sort(
            key=lambda row: (
                -int(row.get("count") or 0),
                str(row.get("last_seen") or ""),
                str(row.get("name") or ""),
            )
        )
        return candidates

    def recommended_replay_target(self, *, limit: int = 200) -> dict[str, Any] | None:
        candidates = self.replay_candidates(limit=limit)
        return candidates[0] if candidates else None
