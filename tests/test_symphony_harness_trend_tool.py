from __future__ import annotations

from pathlib import Path

import tools.symphony_harness_trend as harness_trend


def test_summary_payload_uses_window_baseline() -> None:
    rows = [
        {
            "captured_at": "2026-03-01T08:30:00Z",
            "_dt": harness_trend._parse_iso("2026-03-01T08:30:00Z"),
            "readiness_overall_status": "pass",
            "formal_suite_mode": "all",
            "harness_score": "100",
            "linear_issue_count": "200",
            "linear_project_count": "8",
            "linear_label_count": "19",
            "durable_jsonl_size": "500",
            "durable_duckdb_size": "50",
            "durable_parquet_size": "5",
        },
        {
            "captured_at": "2026-03-05T08:30:00Z",
            "_dt": harness_trend._parse_iso("2026-03-05T08:30:00Z"),
            "readiness_overall_status": "warn",
            "formal_suite_mode": "inventory",
            "harness_score": "100",
            "linear_issue_count": "211",
            "linear_project_count": "8",
            "linear_label_count": "19",
            "durable_jsonl_size": "750",
            "durable_duckdb_size": "75",
            "durable_parquet_size": "8",
        },
    ]
    summary = harness_trend._summary_payload(rows, days=7)
    deltas = summary["deltas"]
    assert deltas["linear_issue_count"]["delta"] == 11
    assert deltas["durable_jsonl_size"]["delta"] == 250
    assert summary["baseline_status"] == "pass"
    assert summary["latest_status"] == "warn"


def test_load_rows_and_markdown(tmp_path: Path) -> None:
    csv_path = tmp_path / "harness_timeseries.csv"
    csv_path.write_text(
        (
            "captured_at,readiness_overall_status,harness_score,harness_target,"
            "linear_issue_count,linear_project_count,linear_label_count,"
            "linear_active_execution_flow,formal_suite_status,formal_suite_mode,"
            "durable_status,durable_jsonl_size,durable_duckdb_size,durable_parquet_size\n"
            "2026-03-04T08:30:00Z,pass,100,90,210,8,19,True,pass,all,pass,700,70,7\n"
            "2026-03-05T08:30:00Z,pass,100,90,211,8,19,True,pass,all,pass,750,75,8\n"
        ),
        encoding="utf-8",
    )
    rows = harness_trend._load_rows(csv_path)
    summary = harness_trend._summary_payload(rows, days=7)
    md = harness_trend._as_markdown(summary)
    assert "Symphony Harness 7-Day Trend" in md
    assert "`linear_issue_count`" in md
    assert "`durable_jsonl_size`" in md
