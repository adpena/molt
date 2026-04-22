#!/usr/bin/env python3
"""Benchmark performance dashboard for the Molt project.

Reads JSON benchmark result files and generates markdown or HTML reports
comparing against a baseline.

Usage:
    python3 tools/bench_dashboard.py bench/results/full_native_*.json \\
        --baseline bench/baseline.json --format markdown

    python3 tools/bench_dashboard.py bench/results/full_native_*.json \\
        --baseline bench/baseline.json --format html --output bench_report.html
"""

import argparse
import json
import statistics
import sys
from html import escape as html_escape
from pathlib import Path


def load_benchmarks(path: Path) -> dict:
    """Load benchmark results from a JSON file, returning the benchmarks dict."""
    with open(path) as f:
        data = json.load(f)
    return data.get("benchmarks", data)


def merge_results(paths: list[Path]) -> dict:
    """Load and merge benchmark results from multiple files.

    If the same benchmark appears in multiple files, the last one wins.
    """
    merged = {}
    for p in sorted(paths):
        merged.update(load_benchmarks(p))
    return merged


def status_label(entry: dict) -> str:
    """Return pass/fail/skip status for a benchmark entry."""
    if not entry.get("molt_ok"):
        return "FAIL"
    if entry.get("molt_time_s") is None or entry.get("molt_time_s") == 0:
        return "SKIP"
    return "PASS"


def speedup_value(entry: dict) -> float | None:
    """Extract the molt speedup value, normalizing various schemas."""
    # Try molt_speedup first (baseline.json style: cpython/molt)
    s = entry.get("molt_speedup")
    if s is not None and s != 0:
        return float(s)
    # Fall back to computing from times
    cpython = entry.get("cpython_time_s")
    molt = entry.get("molt_time_s")
    if cpython and molt and molt > 0:
        return cpython / molt
    return None


def ratio_value(entry: dict) -> float | None:
    """Extract the molt/cpython ratio (>1 means slower than CPython)."""
    r = entry.get("molt_cpython_ratio")
    if r is not None:
        return float(r)
    cpython = entry.get("cpython_time_s")
    molt = entry.get("molt_time_s")
    if cpython and molt and cpython > 0:
        return molt / cpython
    return None


def fmt_time(t) -> str:
    if t is None or t == 0:
        return "-"
    if t >= 1.0:
        return f"{t:.3f}s"
    return f"{t * 1000:.1f}ms"


def fmt_speedup(s) -> str:
    if s is None:
        return "-"
    return f"{s:.2f}x"


def fmt_pct(val) -> str:
    if val is None:
        return "-"
    sign = "+" if val >= 0 else ""
    return f"{sign}{val:.1f}%"


# ---------------------------------------------------------------------------
# Report data collection
# ---------------------------------------------------------------------------

def build_rows(results: dict) -> list[dict]:
    """Build sorted list of row dicts for the report table."""
    rows = []
    for name in sorted(results):
        entry = results[name]
        st = status_label(entry)
        cpython_t = entry.get("cpython_time_s")
        molt_t = entry.get("molt_time_s")
        sp = speedup_value(entry)
        rows.append({
            "name": name,
            "cpython_time": cpython_t,
            "molt_time": molt_t if molt_t and molt_t != 0 else None,
            "speedup": sp,
            "status": st,
        })
    return rows


def compute_summary(rows: list[dict]) -> dict:
    """Compute summary statistics from rows."""
    speedups = [r["speedup"] for r in rows if r["speedup"] is not None]
    passing = [r for r in rows if r["status"] == "PASS"]
    faster = [s for s in speedups if s > 1.0]
    slower = [s for s in speedups if s < 1.0]

    return {
        "total": len(rows),
        "pass_count": len(passing),
        "fail_count": sum(1 for r in rows if r["status"] == "FAIL"),
        "skip_count": sum(1 for r in rows if r["status"] == "SKIP"),
        "pass_rate": len(passing) / len(rows) * 100 if rows else 0,
        "median_speedup": statistics.median(speedups) if speedups else None,
        "mean_speedup": statistics.mean(speedups) if speedups else None,
        "faster_count": len(faster),
        "slower_count": len(slower),
        "best": max(speedups) if speedups else None,
        "worst": min(speedups) if speedups else None,
    }


def compute_baseline_diff(rows: list[dict], baseline: dict) -> list[dict]:
    """Compare current rows against baseline, returning diff entries."""
    diffs = []
    baseline_rows = {name: entry for name, entry in baseline.items()}

    for row in rows:
        name = row["name"]
        cur_speedup = row["speedup"]
        if name not in baseline_rows:
            diffs.append({"name": name, "tag": "NEW", "pct_change": None,
                          "cur_speedup": cur_speedup, "base_speedup": None})
            continue

        base_entry = baseline_rows[name]
        base_speedup = speedup_value(base_entry)

        if cur_speedup is None or base_speedup is None:
            continue

        if base_speedup == 0:
            continue

        pct = (cur_speedup - base_speedup) / base_speedup * 100

        if pct > 10:
            tag = "IMPROVED"
        elif pct < -10:
            tag = "REGRESSED"
        else:
            tag = "STABLE"

        diffs.append({
            "name": name,
            "tag": tag,
            "pct_change": pct,
            "cur_speedup": cur_speedup,
            "base_speedup": base_speedup,
        })

    # Check for removed benchmarks
    current_names = {r["name"] for r in rows}
    for name in sorted(baseline_rows):
        if name not in current_names:
            diffs.append({"name": name, "tag": "REMOVED", "pct_change": None,
                          "cur_speedup": None,
                          "base_speedup": speedup_value(baseline_rows[name])})

    return diffs


# ---------------------------------------------------------------------------
# Markdown output
# ---------------------------------------------------------------------------

def render_markdown(rows: list[dict], summary: dict,
                    diffs: list[dict] | None) -> str:
    lines = []
    lines.append("# Molt Benchmark Dashboard\n")

    # Summary
    lines.append("## Summary\n")
    lines.append("| Metric | Value |")
    lines.append("|--------|-------|")
    lines.append(f"| Total benchmarks | {summary['total']} |")
    lines.append(f"| Pass / Fail / Skip | {summary['pass_count']} / {summary['fail_count']} / {summary['skip_count']} |")
    lines.append(f"| Pass rate | {summary['pass_rate']:.1f}% |")
    lines.append(f"| Median speedup | {fmt_speedup(summary['median_speedup'])} |")
    lines.append(f"| Mean speedup | {fmt_speedup(summary['mean_speedup'])} |")
    lines.append(f"| Faster than CPython | {summary['faster_count']} |")
    lines.append(f"| Slower than CPython | {summary['slower_count']} |")
    lines.append(f"| Best speedup | {fmt_speedup(summary['best'])} |")
    lines.append(f"| Worst speedup | {fmt_speedup(summary['worst'])} |")
    lines.append("")

    # Main table
    lines.append("## Benchmark Results\n")
    lines.append("| Benchmark | CPython | Molt | Speedup | Status |")
    lines.append("|-----------|---------|------|---------|--------|")
    for r in rows:
        marker = ""
        if r["speedup"] is not None:
            if r["speedup"] > 1.0:
                marker = " :arrow_up:"
            elif r["speedup"] < 1.0:
                marker = " :arrow_down:"
        lines.append(
            f"| {r['name']} "
            f"| {fmt_time(r['cpython_time'])} "
            f"| {fmt_time(r['molt_time'])} "
            f"| {fmt_speedup(r['speedup'])}{marker} "
            f"| {r['status']} |"
        )
    lines.append("")

    # Baseline comparison
    if diffs:
        regressions = [d for d in diffs if d["tag"] == "REGRESSED"]
        improvements = [d for d in diffs if d["tag"] == "IMPROVED"]
        new_benches = [d for d in diffs if d["tag"] == "NEW"]
        removed = [d for d in diffs if d["tag"] == "REMOVED"]

        lines.append("## Baseline Comparison\n")

        if regressions:
            lines.append("### Regressions (>10% slower)\n")
            lines.append("| Benchmark | Baseline | Current | Change |")
            lines.append("|-----------|----------|---------|--------|")
            for d in sorted(regressions, key=lambda x: x["pct_change"]):
                lines.append(
                    f"| {d['name']} "
                    f"| {fmt_speedup(d['base_speedup'])} "
                    f"| {fmt_speedup(d['cur_speedup'])} "
                    f"| {fmt_pct(d['pct_change'])} |"
                )
            lines.append("")

        if improvements:
            lines.append("### Improvements (>10% faster)\n")
            lines.append("| Benchmark | Baseline | Current | Change |")
            lines.append("|-----------|----------|---------|--------|")
            for d in sorted(improvements, key=lambda x: -x["pct_change"]):
                lines.append(
                    f"| {d['name']} "
                    f"| {fmt_speedup(d['base_speedup'])} "
                    f"| {fmt_speedup(d['cur_speedup'])} "
                    f"| {fmt_pct(d['pct_change'])} |"
                )
            lines.append("")

        if new_benches:
            lines.append("### New Benchmarks\n")
            for d in new_benches:
                lines.append(f"- {d['name']} ({fmt_speedup(d['cur_speedup'])})")
            lines.append("")

        if removed:
            lines.append("### Removed Benchmarks\n")
            for d in removed:
                lines.append(f"- {d['name']}")
            lines.append("")

        stable_count = sum(1 for d in diffs if d["tag"] == "STABLE")
        lines.append(
            f"**Summary:** {len(improvements)} improved, "
            f"{len(regressions)} regressed, "
            f"{stable_count} stable, "
            f"{len(new_benches)} new, "
            f"{len(removed)} removed\n"
        )

    return "\n".join(lines)


# ---------------------------------------------------------------------------
# HTML output
# ---------------------------------------------------------------------------

def speedup_color(s: float | None) -> str:
    if s is None:
        return "#888"
    if s >= 2.0:
        return "#1a7f37"
    if s >= 1.0:
        return "#2da44e"
    if s >= 0.5:
        return "#d1242f"
    return "#a40e26"


def status_color(st: str) -> str:
    return {"PASS": "#2da44e", "FAIL": "#d1242f", "SKIP": "#888"}.get(st, "#888")


def diff_color(tag: str) -> str:
    return {
        "IMPROVED": "#2da44e",
        "REGRESSED": "#d1242f",
        "STABLE": "#888",
        "NEW": "#0969da",
        "REMOVED": "#888",
    }.get(tag, "#888")


def render_html(rows: list[dict], summary: dict,
                diffs: list[dict] | None) -> str:
    h = []
    h.append("<!DOCTYPE html>")
    h.append("<html lang='en'><head><meta charset='utf-8'>")
    h.append("<title>Molt Benchmark Dashboard</title>")
    h.append("<style>")
    h.append("""
body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
       max-width: 1100px; margin: 2em auto; padding: 0 1em;
       color: #1f2328; background: #fff; }
h1,h2,h3 { margin-top: 1.5em; }
table { border-collapse: collapse; width: 100%; margin: 1em 0; }
th, td { padding: 6px 12px; border: 1px solid #d0d7de; text-align: left; }
th { background: #f6f8fa; font-weight: 600; }
tr:hover { background: #f6f8fa; }
.chip { display: inline-block; padding: 2px 8px; border-radius: 12px;
        color: #fff; font-size: 0.85em; font-weight: 600; }
.summary-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
                gap: 12px; margin: 1em 0; }
.summary-card { background: #f6f8fa; border: 1px solid #d0d7de;
                border-radius: 8px; padding: 12px 16px; }
.summary-card .label { font-size: 0.85em; color: #656d76; }
.summary-card .value { font-size: 1.4em; font-weight: 700; }
""")
    h.append("</style></head><body>")
    h.append("<h1>Molt Benchmark Dashboard</h1>")

    # Summary cards
    h.append("<h2>Summary</h2>")
    h.append('<div class="summary-grid">')
    cards = [
        ("Total", str(summary["total"])),
        ("Pass Rate", f"{summary['pass_rate']:.1f}%"),
        ("Median Speedup", fmt_speedup(summary["median_speedup"])),
        ("Mean Speedup", fmt_speedup(summary["mean_speedup"])),
        ("Faster than CPython", str(summary["faster_count"])),
        ("Slower than CPython", str(summary["slower_count"])),
        ("Best", fmt_speedup(summary["best"])),
        ("Worst", fmt_speedup(summary["worst"])),
    ]
    for label, val in cards:
        h.append(f'<div class="summary-card"><div class="label">{html_escape(label)}</div>'
                 f'<div class="value">{html_escape(val)}</div></div>')
    h.append("</div>")

    # Main table
    h.append("<h2>Benchmark Results</h2>")
    h.append("<table><thead><tr>")
    h.append("<th>Benchmark</th><th>CPython</th><th>Molt</th>"
             "<th>Speedup</th><th>Status</th>")
    h.append("</tr></thead><tbody>")
    for r in rows:
        sc = speedup_color(r["speedup"])
        stc = status_color(r["status"])
        h.append("<tr>")
        h.append(f"<td>{html_escape(r['name'])}</td>")
        h.append(f"<td>{html_escape(fmt_time(r['cpython_time']))}</td>")
        h.append(f"<td>{html_escape(fmt_time(r['molt_time']))}</td>")
        h.append(f'<td style="color:{sc};font-weight:600">'
                 f'{html_escape(fmt_speedup(r["speedup"]))}</td>')
        h.append(f'<td><span class="chip" style="background:{stc}">'
                 f'{html_escape(r["status"])}</span></td>')
        h.append("</tr>")
    h.append("</tbody></table>")

    # Baseline diff
    if diffs:
        regressions = [d for d in diffs if d["tag"] == "REGRESSED"]
        improvements = [d for d in diffs if d["tag"] == "IMPROVED"]
        new_benches = [d for d in diffs if d["tag"] == "NEW"]
        removed = [d for d in diffs if d["tag"] == "REMOVED"]
        stable_count = sum(1 for d in diffs if d["tag"] == "STABLE")

        h.append("<h2>Baseline Comparison</h2>")
        h.append(f"<p><strong>{len(improvements)}</strong> improved, "
                 f"<strong>{len(regressions)}</strong> regressed, "
                 f"<strong>{stable_count}</strong> stable, "
                 f"<strong>{len(new_benches)}</strong> new, "
                 f"<strong>{len(removed)}</strong> removed</p>")

        if regressions:
            h.append("<h3>Regressions (&gt;10% slower)</h3>")
            h.append("<table><thead><tr><th>Benchmark</th><th>Baseline</th>"
                     "<th>Current</th><th>Change</th></tr></thead><tbody>")
            for d in sorted(regressions, key=lambda x: x["pct_change"]):
                dc = diff_color(d["tag"])
                h.append(f"<tr><td>{html_escape(d['name'])}</td>"
                         f"<td>{html_escape(fmt_speedup(d['base_speedup']))}</td>"
                         f"<td>{html_escape(fmt_speedup(d['cur_speedup']))}</td>"
                         f'<td style="color:{dc};font-weight:600">'
                         f'{html_escape(fmt_pct(d["pct_change"]))}</td></tr>')
            h.append("</tbody></table>")

        if improvements:
            h.append("<h3>Improvements (&gt;10% faster)</h3>")
            h.append("<table><thead><tr><th>Benchmark</th><th>Baseline</th>"
                     "<th>Current</th><th>Change</th></tr></thead><tbody>")
            for d in sorted(improvements, key=lambda x: -x["pct_change"]):
                dc = diff_color(d["tag"])
                h.append(f"<tr><td>{html_escape(d['name'])}</td>"
                         f"<td>{html_escape(fmt_speedup(d['base_speedup']))}</td>"
                         f"<td>{html_escape(fmt_speedup(d['cur_speedup']))}</td>"
                         f'<td style="color:{dc};font-weight:600">'
                         f'{html_escape(fmt_pct(d["pct_change"]))}</td></tr>')
            h.append("</tbody></table>")

        if new_benches:
            h.append("<h3>New Benchmarks</h3><ul>")
            for d in new_benches:
                h.append(f"<li>{html_escape(d['name'])} "
                         f"({html_escape(fmt_speedup(d['cur_speedup']))})</li>")
            h.append("</ul>")

        if removed:
            h.append("<h3>Removed Benchmarks</h3><ul>")
            for d in removed:
                h.append(f"<li>{html_escape(d['name'])}</li>")
            h.append("</ul>")

    h.append("</body></html>")
    return "\n".join(h)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(
        description="Molt benchmark performance dashboard")
    parser.add_argument("files", nargs="+", type=Path,
                        help="Benchmark result JSON files")
    parser.add_argument("--baseline", type=Path, default=None,
                        help="Baseline JSON file for comparison")
    parser.add_argument("--format", choices=["markdown", "html"],
                        default="markdown", dest="fmt",
                        help="Output format (default: markdown)")
    parser.add_argument("--output", "-o", type=Path, default=None,
                        help="Output file (default: stdout)")

    args = parser.parse_args()

    # Validate input files
    missing = [p for p in args.files if not p.exists()]
    if missing:
        print(f"Error: files not found: {', '.join(str(p) for p in missing)}",
              file=sys.stderr)
        sys.exit(1)

    results = merge_results(args.files)
    if not results:
        print("Error: no benchmark data found in input files", file=sys.stderr)
        sys.exit(1)

    rows = build_rows(results)
    summary = compute_summary(rows)

    diffs = None
    if args.baseline:
        if not args.baseline.exists():
            print(f"Error: baseline not found: {args.baseline}", file=sys.stderr)
            sys.exit(1)
        baseline = load_benchmarks(args.baseline)
        diffs = compute_baseline_diff(rows, baseline)

    if args.fmt == "markdown":
        output = render_markdown(rows, summary, diffs)
    else:
        output = render_html(rows, summary, diffs)

    if args.output:
        args.output.write_text(output, encoding="utf-8")
        print(f"Report written to {args.output}", file=sys.stderr)
    else:
        print(output)


if __name__ == "__main__":
    main()
