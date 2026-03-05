"""Symphony hotspot: _normalize_issue extracted for Molt compilation.

This module contains the hot path from symphony/linear.py — the function
that normalizes raw Linear GraphQL JSON into typed Issue objects.
Called 50-250 times per orchestrator tick.

Molt compilation target: pure Python, no dynamic features, frozen dataclasses,
only str/int/dict/list/tuple/bool types.
"""

from dataclasses import dataclass


@dataclass(frozen=True)
class BlockerRef:
    id: str
    identifier: str
    state: str


@dataclass(frozen=True)
class Issue:
    id: str
    identifier: str
    title: str
    description: str
    priority: int
    state: str
    branch_name: str
    url: str
    labels: tuple[str, ...]
    blocked_by: tuple[BlockerRef, ...]
    created_at: str
    updated_at: str


def optional_text(value: str) -> str:
    if value == "":
        return ""
    text = value.strip()
    if text == "":
        return ""
    return text


def normalize_issue(
    issue_id: str,
    identifier: str,
    title: str,
    state_name: str,
    raw_priority: int,
    label_names: list[str],
    blocker_ids: list[str],
    blocker_identifiers: list[str],
    blocker_states: list[str],
    description: str,
    branch_name: str,
    url: str,
    created_at: str,
    updated_at: str,
) -> Issue:
    """Normalize raw Linear fields into an Issue.

    Pre-flattened from the dict-heavy _normalize_issue for Molt compilation:
    all dict navigation happens in the caller, this function does the
    CPU-intensive string normalization and dataclass construction.
    """
    clean_id = issue_id.strip()
    clean_identifier = identifier.strip()
    clean_title = title.strip()
    clean_state = state_name.strip()

    # Label normalization: strip, lowercase, dedup, sort
    seen: dict[str, bool] = {}
    clean_labels: list[str] = []
    for name in label_names:
        stripped = name.strip()
        if stripped == "":
            continue
        lowered = stripped.lower()
        if lowered not in seen:
            seen[lowered] = True
            clean_labels.append(lowered)
    clean_labels.sort()

    # Blocker construction
    blockers: list[BlockerRef] = []
    i = 0
    while i < len(blocker_ids):
        blockers.append(
            BlockerRef(
                id=optional_text(blocker_ids[i]),
                identifier=optional_text(blocker_identifiers[i]),
                state=optional_text(blocker_states[i]),
            )
        )
        i += 1

    return Issue(
        id=clean_id,
        identifier=clean_identifier,
        title=clean_title,
        description=optional_text(description),
        priority=raw_priority,
        state=clean_state,
        branch_name=optional_text(branch_name),
        url=optional_text(url),
        labels=tuple(clean_labels),
        blocked_by=tuple(blockers),
        created_at=optional_text(created_at),
        updated_at=optional_text(updated_at),
    )


# --- Benchmark driver ---


def make_test_payload(
    index: int,
) -> tuple[
    str,
    str,
    str,
    str,
    int,
    list[str],
    list[str],
    list[str],
    list[str],
    str,
    str,
    str,
    str,
    str,
]:
    """Generate a realistic Linear issue payload for benchmarking."""
    issue_id = "id-" + str(index) + "-abcdef1234567890"
    identifier = "MOL-" + str(index)
    title = (
        "  fix: resolve edge case in string normalization for module "
        + str(index)
        + "  "
    )
    state_name = "  In Progress  "
    raw_priority = (index % 4) + 1
    label_names = [
        "  area:runtime  ",
        " owner:compiler ",
        "  P1  ",
        "  milestone:M2  ",
        " area:runtime ",  # duplicate
    ]
    blocker_ids = ["blocker-id-" + str(index)]
    blocker_identifiers = ["MOL-" + str(index + 100)]
    blocker_states = ["  Todo  "]
    description = (
        "This is a detailed description for issue "
        + str(index)
        + " with various edge cases."
    )
    branch_name = "  feat/mol-" + str(index) + "-fix  "
    url = "https://linear.app/moltlang/issue/MOL-" + str(index)
    created_at = "2026-03-04T12:00:00.000Z"
    updated_at = "2026-03-05T08:30:00.000Z"
    return (
        issue_id,
        identifier,
        title,
        state_name,
        raw_priority,
        label_names,
        blocker_ids,
        blocker_identifiers,
        blocker_states,
        description,
        branch_name,
        url,
        created_at,
        updated_at,
    )


def clock_ns() -> int:
    """Monotonic clock in nanoseconds. Uses time.perf_counter_ns if available."""
    import time

    return time.perf_counter_ns()


def run_benchmark(iterations: int) -> None:
    """Run normalize_issue N times and print results with internal timing."""
    # Pre-generate all payloads to exclude from timing
    payloads: list[
        tuple[
            str,
            str,
            str,
            str,
            int,
            list[str],
            list[str],
            list[str],
            list[str],
            str,
            str,
            str,
            str,
            str,
        ]
    ] = []
    i = 0
    while i < 200:
        payloads.append(make_test_payload(i))
        i += 1

    # Warm up
    warmup_payload = payloads[0]
    wi = 0
    while wi < 100:
        normalize_issue(
            warmup_payload[0],
            warmup_payload[1],
            warmup_payload[2],
            warmup_payload[3],
            warmup_payload[4],
            warmup_payload[5],
            warmup_payload[6],
            warmup_payload[7],
            warmup_payload[8],
            warmup_payload[9],
            warmup_payload[10],
            warmup_payload[11],
            warmup_payload[12],
            warmup_payload[13],
        )
        wi += 1

    # Timed run
    start = clock_ns()
    results: list[Issue] = []
    i = 0
    while i < iterations:
        payload = payloads[i % 200]
        issue = normalize_issue(
            payload[0],
            payload[1],
            payload[2],
            payload[3],
            payload[4],
            payload[5],
            payload[6],
            payload[7],
            payload[8],
            payload[9],
            payload[10],
            payload[11],
            payload[12],
            payload[13],
        )
        results.append(issue)
        i += 1
    end = clock_ns()

    elapsed_ms = (end - start) / 1000000.0
    per_call_us = (end - start) / 1000.0 / iterations

    # Print summary to prevent dead-code elimination
    print("Processed " + str(len(results)) + " issues")
    print("Last issue: " + results[-1].identifier + " state=" + results[-1].state)
    print("Labels: " + str(results[-1].labels))
    print("INTERNAL_TIME_MS=" + str(int(elapsed_ms)))
    print("PER_CALL_US=" + str(int(per_call_us * 100) / 100.0))


run_benchmark(50000)
