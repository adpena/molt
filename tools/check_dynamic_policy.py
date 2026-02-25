#!/usr/bin/env python3
"""Static guardrails for Molt's dynamic-execution policy."""

from __future__ import annotations

import runpy
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MANIFEST_PATH = ROOT / "tools" / "stdlib_full_coverage_manifest.py"

REQUIRED_DOCS: tuple[str, ...] = (
    "docs/spec/areas/core/0000-vision.md",
    "docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md",
    "docs/spec/areas/testing/0007-testing.md",
    "docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md",
)

DOC_REQUIRED_SNIPPETS: dict[str, tuple[str, ...]] = {
    "docs/spec/areas/core/0000-vision.md": ("no monkeypatching", "no `eval/exec`"),
    "docs/spec/areas/core/0800_WHAT_MOLT_IS_WILLING_TO_BREAK.md": (
        "arbitrary monkeypatching at runtime",
        "reflection-heavy patterns",
    ),
    "docs/spec/areas/testing/0007-testing.md": (
        "Expected-Failure Policy For Too-Dynamic Cases",
        "No `exec`/`eval`",
    ),
    "docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md": (
        "Future Enablement Gate",
        "capability-gated",
    ),
}

RUNTIME_POLICY_MARKERS: dict[str, str] = {
    "runtime/molt-runtime/src/builtins/modules.rs": "dynamic-exec-policy",
    "runtime/molt-runtime/src/builtins/platform.rs": "dynamic-exec-policy",
}

RUNPY_POLICY_NOTE_DOCS: tuple[str, ...] = (
    "docs/spec/STATUS.md",
    "ROADMAP.md",
)

RUNPY_EMPTY_NOTE_REQUIRED_TOKENS: tuple[str, ...] = (
    "runpy",
    "dynamic-lane expected failures",
    "currently empty",
)

RUNPY_EMPTY_NOTE_REASON_TOKENS: tuple[str, ...] = (
    "supported lanes moved to intrinsic support",
    "moved to intrinsic support",
)


def _load_manifest() -> tuple[tuple[str, ...], tuple[str, ...]]:
    namespace = runpy.run_path(str(MANIFEST_PATH))
    doc_refs = namespace.get("TOO_DYNAMIC_POLICY_DOC_REFERENCES", ())
    expected_failures = namespace.get("TOO_DYNAMIC_EXPECTED_FAILURE_TESTS", ())
    if not isinstance(doc_refs, tuple):
        raise RuntimeError(
            "TOO_DYNAMIC_POLICY_DOC_REFERENCES must be a tuple[str, ...] in "
            "tools/stdlib_full_coverage_manifest.py"
        )
    if not isinstance(expected_failures, tuple):
        raise RuntimeError(
            "TOO_DYNAMIC_EXPECTED_FAILURE_TESTS must be a tuple[str, ...] in "
            "tools/stdlib_full_coverage_manifest.py"
        )
    return doc_refs, expected_failures


def _check_docs() -> list[str]:
    errors: list[str] = []
    for rel_path in REQUIRED_DOCS:
        doc_path = ROOT / rel_path
        if not doc_path.exists():
            errors.append(f"missing policy doc: {rel_path}")
            continue
        text = doc_path.read_text(encoding="utf-8")
        for snippet in DOC_REQUIRED_SNIPPETS.get(rel_path, ()):
            if snippet not in text:
                errors.append(f"policy doc missing snippet {snippet!r}: {rel_path}")
    return errors


def _check_manifest(
    doc_refs: tuple[str, ...], expected_failures: tuple[str, ...]
) -> list[str]:
    errors: list[str] = []
    missing_refs = sorted(set(REQUIRED_DOCS) - set(doc_refs))
    if missing_refs:
        errors.append(
            "TOO_DYNAMIC_POLICY_DOC_REFERENCES missing required docs: "
            + ", ".join(missing_refs)
        )
    has_exec = any("/exec" in path for path in expected_failures)
    has_eval = any("/eval" in path for path in expected_failures)
    if not has_exec:
        errors.append(
            "TOO_DYNAMIC_EXPECTED_FAILURE_TESTS must include at least one exec* case"
        )
    if not has_eval:
        errors.append(
            "TOO_DYNAMIC_EXPECTED_FAILURE_TESTS must include at least one eval* case"
        )
    return errors


def _check_runtime_policy_markers() -> list[str]:
    errors: list[str] = []
    for rel_path, marker in RUNTIME_POLICY_MARKERS.items():
        path = ROOT / rel_path
        if not path.exists():
            errors.append(f"missing runtime policy file: {rel_path}")
            continue
        text = path.read_text(encoding="utf-8")
        if marker not in text:
            errors.append(f"missing runtime policy marker {marker!r} in {rel_path}")
    return errors


def _is_runpy_expected_failure(path: str) -> bool:
    normalized = path.replace("\\", "/")
    filename = Path(normalized).name
    return (
        "/runpy_" in normalized
        or "/runpy/" in normalized
        or filename.startswith("runpy_")
    )


def _has_runpy_empty_lane_doc_note() -> bool:
    for rel_path in RUNPY_POLICY_NOTE_DOCS:
        doc_path = ROOT / rel_path
        if not doc_path.exists():
            continue
        text = doc_path.read_text(encoding="utf-8").lower()
        if not all(token in text for token in RUNPY_EMPTY_NOTE_REQUIRED_TOKENS):
            continue
        if any(token in text for token in RUNPY_EMPTY_NOTE_REASON_TOKENS):
            return True
    return False


def _check_runpy_policy_lanes(expected_failures: tuple[str, ...]) -> list[str]:
    errors: list[str] = []
    runpy_entries = sorted(
        path for path in expected_failures if _is_runpy_expected_failure(path)
    )
    if runpy_entries:
        for rel_path in runpy_entries:
            if not (ROOT / rel_path).exists():
                errors.append(f"runpy expected-failure path does not exist: {rel_path}")
        return errors

    if not _has_runpy_empty_lane_doc_note():
        errors.append(
            "runpy policy lane governance missing: add at least one runpy entry to "
            "TOO_DYNAMIC_EXPECTED_FAILURE_TESTS or add an explicit STATUS/ROADMAP "
            "note that runpy dynamic-lane expected failures are currently empty "
            "because supported lanes moved to intrinsic support"
        )
    return errors


def main() -> int:
    try:
        doc_refs, expected_failures = _load_manifest()
    except RuntimeError as exc:
        print(f"dynamic policy guard failed: {exc}")
        return 1

    errors = []
    errors.extend(_check_docs())
    errors.extend(_check_manifest(doc_refs, expected_failures))
    errors.extend(_check_runpy_policy_lanes(expected_failures))
    errors.extend(_check_runtime_policy_markers())
    if errors:
        print("dynamic policy guard violated:")
        for err in errors:
            print(f"- {err}")
        return 1
    print("dynamic policy guard: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
