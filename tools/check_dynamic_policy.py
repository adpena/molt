#!/usr/bin/env python3
"""Static guardrails for Molt's dynamic-execution policy."""

from __future__ import annotations

from pathlib import Path

from molt.verified_subset import load_verified_subset_policy

try:
    from tools.compat import test_policy
except ModuleNotFoundError:  # pragma: no cover - direct script import from tools/
    from compat import test_policy  # type: ignore[no-redef]

ROOT = Path(__file__).resolve().parents[1]
DYNAMIC_EXECUTION_SCOPE = "dynamic_execution_policy"

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
        "Verified-Subset Scope Policy For Too-Dynamic Cases",
        "No `exec`/`eval`",
    ),
    "docs/spec/areas/compat/contracts/dynamic_execution_policy_contract.md": (
        "Future Enablement Gate",
        "capability-gated",
    ),
}

RUNTIME_POLICY_EVIDENCE: dict[str, tuple[str, ...]] = {
    "runtime/molt-runtime/src/builtins/modules.rs": (
        'if trace_name == "exec" || trace_name == "eval"',
        "dynamic code execution is outside the verified subset",
    ),
    "runtime/molt-runtime/src/builtins/platform.rs": (
        "fn importlib_extension_exec_unavailable(",
        "has no compiler-emitted body in this binary",
    ),
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


def _load_scope_paths() -> tuple[str, ...]:
    try:
        policy = load_verified_subset_policy()
        paths = test_policy.verification_scope_paths(
            policy.suite_selectors,
            scope=DYNAMIC_EXECUTION_SCOPE,
            repo_root=ROOT,
        )
    except (OSError, ValueError) as exc:
        raise RuntimeError(str(exc)) from exc
    return tuple(sorted(paths))


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


def _check_scope(paths: tuple[str, ...]) -> list[str]:
    errors: list[str] = []
    has_exec = any("/exec" in path or "_exec_" in path for path in paths)
    has_eval = any("/eval" in path or "_eval_" in path for path in paths)
    if not has_exec:
        errors.append(
            "dynamic_execution_policy scope must include at least one exec case"
        )
    if not has_eval:
        errors.append(
            "dynamic_execution_policy scope must include at least one eval case"
        )
    for path in paths:
        metadata = test_policy.parse_metadata(ROOT / path)
        if not metadata.expect_molt_fail:
            errors.append(
                f"dynamic_execution_policy test must declare expect_fail=molt: {path}"
            )
        if metadata.expected_failure_reason != "too_dynamic_policy":
            errors.append(
                "dynamic_execution_policy test must declare "
                f"expect_fail_reason=too_dynamic_policy: {path}"
            )
    return errors


def _check_runtime_policy_evidence() -> list[str]:
    errors: list[str] = []
    for rel_path, required_snippets in RUNTIME_POLICY_EVIDENCE.items():
        path = ROOT / rel_path
        if not path.exists():
            errors.append(f"missing runtime policy file: {rel_path}")
            continue
        text = path.read_text(encoding="utf-8")
        for snippet in required_snippets:
            if snippet not in text:
                errors.append(
                    f"runtime policy evidence missing snippet {snippet!r}: {rel_path}"
                )
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


def _check_runpy_policy_lanes(scope_paths: tuple[str, ...]) -> list[str]:
    errors: list[str] = []
    runpy_entries = sorted(
        path for path in scope_paths if _is_runpy_expected_failure(path)
    )
    if runpy_entries:
        for rel_path in runpy_entries:
            if not (ROOT / rel_path).exists():
                errors.append(f"runpy expected-failure path does not exist: {rel_path}")
        return errors

    if not _has_runpy_empty_lane_doc_note():
        errors.append(
            "runpy policy lane governance missing: annotate at least one runpy test "
            "with verified_subset_scope=dynamic_execution_policy or add an explicit "
            "STATUS/ROADMAP note that runpy dynamic-lane expected failures are "
            "currently empty because supported lanes moved to intrinsic support"
        )
    return errors


def main() -> int:
    try:
        scope_paths = _load_scope_paths()
    except RuntimeError as exc:
        print(f"dynamic policy guard failed: {exc}")
        return 1

    errors = []
    errors.extend(_check_docs())
    errors.extend(_check_scope(scope_paths))
    errors.extend(_check_runpy_policy_lanes(scope_paths))
    errors.extend(_check_runtime_policy_evidence())
    if errors:
        print("dynamic policy guard violated:")
        for err in errors:
            print(f"- {err}")
        return 1
    print("dynamic policy guard: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
