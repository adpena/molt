# MOLT_META: stdout=pyperformance
"""Purpose: run a pyperformance smoke subset with timing-insensitive diff output."""

from pathlib import Path
import sys


REPO_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO_ROOT / "tools"))

import pyperformance_adapter  # noqa: E402


SUITE_ROOT = Path(__file__).resolve().parent / "fixtures" / "pyperformance_smoke"
catalog = pyperformance_adapter.catalog_suite(SUITE_ROOT)
smoke_available = tuple(catalog["smoke_available"])

print("smoke_available=" + ",".join(smoke_available))
payload = pyperformance_adapter.run_subset(
    SUITE_ROOT,
    benchmarks=smoke_available,
    rounds=1,
)
for row in payload["results"]:
    benchmark = str(row["benchmark"])
    elapsed_s = float(row["elapsed_s"])
    fingerprint = str(row["result_fingerprint"])
    print(f"benchmark={benchmark} elapsed_s={elapsed_s:.9f} fingerprint={fingerprint}")

print(f"total_elapsed_s={float(payload['total_elapsed_s']):.9f}")
