from __future__ import annotations

import argparse
import csv
import json
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

from molt.symphony.paths import default_molt_ext_root, symphony_metrics_dir


def _parse_iso(value: str) -> datetime:
    raw = value.strip()
    return datetime.fromisoformat(raw.replace("Z", "+00:00")).astimezone(UTC)


def _to_int(value: Any) -> int | None:
    if value is None:
        return None
    if isinstance(value, int):
        return value
    text = str(value).strip()
    if not text:
        return None
    try:
        return int(text)
    except Exception:
        return None


def _load_rows(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8", newline="") as handle:
        reader = csv.DictReader(handle)
        for row in reader:
            captured_at = str(row.get("captured_at") or "").strip()
            if not captured_at:
                continue
            parsed: dict[str, Any] = {
                "captured_at": captured_at,
                "_dt": _parse_iso(captured_at),
            }
            for key, value in row.items():
                if key == "captured_at":
                    continue
                parsed[key] = value
            rows.append(parsed)
    rows.sort(key=lambda item: item["_dt"])
    return rows


def _delta(current: int | None, baseline: int | None) -> int | None:
    if current is None or baseline is None:
        return None
    return current - baseline


def _delta_ratio(current: int | None, baseline: int | None) -> float | None:
    if current is None or baseline is None or baseline <= 0:
        return None
    return (current - baseline) / baseline


def _summary_payload(rows: list[dict[str, Any]], *, days: int) -> dict[str, Any]:
    if not rows:
        raise RuntimeError("timeseries is empty")
    latest = rows[-1]
    latest_dt = latest["_dt"]
    window_start = latest_dt - timedelta(days=max(1, days))
    window_rows = [row for row in rows if row["_dt"] >= window_start]
    baseline = (
        window_rows[0]
        if len(window_rows) >= 2
        else (rows[-2] if len(rows) >= 2 else latest)
    )

    metrics = (
        "harness_score",
        "linear_issue_count",
        "linear_project_count",
        "linear_label_count",
        "durable_jsonl_size",
        "durable_duckdb_size",
        "durable_parquet_size",
    )
    deltas: dict[str, Any] = {}
    for metric in metrics:
        curr = _to_int(latest.get(metric))
        base = _to_int(baseline.get(metric))
        deltas[metric] = {
            "baseline": base,
            "latest": curr,
            "delta": _delta(curr, base),
            "delta_ratio": _delta_ratio(curr, base),
        }

    return {
        "window_days": max(1, days),
        "rows_total": len(rows),
        "rows_in_window": len(window_rows),
        "baseline_captured_at": baseline["captured_at"],
        "latest_captured_at": latest["captured_at"],
        "baseline_status": baseline.get("readiness_overall_status"),
        "latest_status": latest.get("readiness_overall_status"),
        "baseline_formal_mode": baseline.get("formal_suite_mode"),
        "latest_formal_mode": latest.get("formal_suite_mode"),
        "deltas": deltas,
    }


def _as_markdown(summary: dict[str, Any]) -> str:
    lines = [
        "# Symphony Harness 7-Day Trend",
        "",
        f"- Window days: `{summary['window_days']}`",
        f"- Baseline: `{summary['baseline_captured_at']}`",
        f"- Latest: `{summary['latest_captured_at']}`",
        f"- Readiness status: `{summary['baseline_status']}` -> `{summary['latest_status']}`",
        f"- Formal mode: `{summary['baseline_formal_mode']}` -> `{summary['latest_formal_mode']}`",
        "",
        "## Metric Deltas",
    ]
    deltas = summary.get("deltas")
    if isinstance(deltas, dict):
        for metric, payload in deltas.items():
            if not isinstance(payload, dict):
                continue
            delta = payload.get("delta")
            ratio = payload.get("delta_ratio")
            ratio_txt = "n/a" if ratio is None else f"{float(ratio) * 100:.2f}%"
            lines.append(
                f"- `{metric}`: `{payload.get('baseline')}` -> `{payload.get('latest')}` "
                f"(delta `{delta}`, ratio `{ratio_txt}`)"
            )
    lines.append("")
    return "\n".join(lines)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Summarize rolling harness timeseries deltas for Symphony readiness."
    )
    parser.add_argument(
        "--ext-root",
        default=str(default_molt_ext_root()),
        help="Build external root; metrics default to Symphony log root when --csv is unset.",
    )
    parser.add_argument(
        "--csv",
        default=None,
        help="Override harness_timeseries.csv path.",
    )
    parser.add_argument(
        "--days",
        type=int,
        default=7,
        help="Rolling window size in days for baseline selection.",
    )
    parser.add_argument(
        "--json-out",
        default=None,
        help="Optional path to write machine-readable summary JSON.",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    csv_path = (
        Path(str(args.csv)).expanduser().resolve()
        if args.csv
        else (symphony_metrics_dir() / "harness_timeseries.csv")
    )
    if not csv_path.exists():
        raise RuntimeError(f"missing harness timeseries: {csv_path}")

    rows = _load_rows(csv_path)
    summary = _summary_payload(rows, days=int(args.days))
    if args.json_out:
        out_path = Path(str(args.json_out)).expanduser().resolve()
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(
            json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
    print(_as_markdown(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
