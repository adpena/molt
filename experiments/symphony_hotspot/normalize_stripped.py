"""Stripped version: no dataclasses, just tuples and dicts.
Isolates whether the bottleneck is dataclass construction or string ops.
"""

import time


def optional_text(value: str) -> str:
    if value == "":
        return ""
    text = value.strip()
    if text == "":
        return ""
    return text


def normalize_issue_stripped(
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
) -> tuple[str, str, str, str, int, tuple[str, ...], str, str, str, str]:
    clean_id = issue_id.strip()
    clean_identifier = identifier.strip()
    clean_title = title.strip()
    clean_state = state_name.strip()

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

    return (
        clean_id,
        clean_identifier,
        clean_title,
        clean_state,
        raw_priority,
        tuple(clean_labels),
        optional_text(description),
        optional_text(branch_name),
        optional_text(url),
        optional_text(created_at),
    )


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
        " area:runtime ",
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


def run_benchmark(iterations: int) -> None:
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
    wi = 0
    while wi < 100:
        p = payloads[0]
        normalize_issue_stripped(
            p[0],
            p[1],
            p[2],
            p[3],
            p[4],
            p[5],
            p[6],
            p[7],
            p[8],
            p[9],
            p[10],
            p[11],
            p[12],
            p[13],
        )
        wi += 1

    start = time.perf_counter_ns()
    results: list[
        tuple[str, str, str, str, int, tuple[str, ...], str, str, str, str]
    ] = []
    i = 0
    while i < iterations:
        p = payloads[i % 200]
        result = normalize_issue_stripped(
            p[0],
            p[1],
            p[2],
            p[3],
            p[4],
            p[5],
            p[6],
            p[7],
            p[8],
            p[9],
            p[10],
            p[11],
            p[12],
            p[13],
        )
        results.append(result)
        i += 1
    end = time.perf_counter_ns()

    elapsed_ms = (end - start) / 1000000.0
    per_call_us = (end - start) / 1000.0 / iterations

    print("Processed " + str(len(results)) + " issues")
    print("Last: " + str(results[-1][1]) + " state=" + str(results[-1][3]))
    print("Labels: " + str(results[-1][5]))
    print("INTERNAL_TIME_MS=" + str(int(elapsed_ms)))
    print("PER_CALL_US=" + str(int(per_call_us * 100) / 100.0))


run_benchmark(50000)
