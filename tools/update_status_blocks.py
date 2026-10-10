#!/usr/bin/env python3
"""Project the stdlib compatibility summary block of docs/spec/STATUS.md.

The block is a pure function of the stdlib intrinsics audit and platform
availability documents; the rest of STATUS.md is hand-written.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.generator_io import generator_main  # noqa: E402

STATUS_DOC = ROOT / "docs/spec/STATUS.md"
STDLIB_AUDIT_DOC = (
    ROOT / "docs/spec/areas/compat/surfaces/stdlib/stdlib_intrinsics_audit.generated.md"
)
STDLIB_PLATFORM_DOC = (
    ROOT
    / "docs/spec/areas/compat/surfaces/stdlib/stdlib_platform_availability.generated.md"
)

COMPAT_START = "<!-- GENERATED:compat-summary:start -->"
COMPAT_END = "<!-- GENERATED:compat-summary:end -->"


def _extract_count(text: str, label: str) -> int:
    pattern = re.compile(rf"- {re.escape(label)}: `(\d+)`")
    match = pattern.search(text)
    if match is None:
        raise ValueError(f"missing summary line for {label!r}")
    return int(match.group(1))


def _render_compat_summary(audit_text: str, platform_text: str) -> str:
    audited = _extract_count(audit_text, "Total audited modules")
    intrinsic_backed = _extract_count(audit_text, "`intrinsic-backed`")
    intrinsic_partial = _extract_count(audit_text, "`intrinsic-partial`")
    intrinsic_support = _extract_count(audit_text, "`intrinsic-support`")
    python_compiled = _extract_count(audit_text, "`python-compiled`")
    stub = _extract_count(audit_text, "`stub`")
    policy_gate = _extract_count(audit_text, "`policy-gate`")
    availability = _extract_count(
        platform_text, "Modules with explicit Availability metadata"
    )
    wasi_blocked = _extract_count(platform_text, "WASI blocked (any lane)")
    emscripten_blocked = _extract_count(platform_text, "Emscripten blocked (any lane)")
    return "\n".join(
        [
            (
                f"- Stdlib lowering audit: `{audited}` modules audited; "
                f"`{intrinsic_backed}` intrinsic-backed; "
                f"`{intrinsic_partial}` intrinsic-partial; "
                f"`{intrinsic_support}` intrinsic-support; "
                f"`{python_compiled}` python-compiled; "
                f"`{stub}` stub; "
                f"`{policy_gate}` policy-gate."
            ),
            (
                f"- Platform availability metadata: `{availability}` modules with "
                f"explicit availability notes; `{wasi_blocked}` WASI-blocked; "
                f"`{emscripten_blocked}` Emscripten-blocked in CPython docs."
            ),
            (
                "- Deep evidence: see the stdlib intrinsics audit and platform "
                "availability matrices under "
                "`docs/spec/areas/compat/surfaces/stdlib/`."
            ),
        ]
    )


def _replace_block(text: str, start_marker: str, end_marker: str, block: str) -> str:
    if start_marker not in text or end_marker not in text:
        raise ValueError(f"missing status markers {start_marker} / {end_marker}")
    before, rest = text.split(start_marker, maxsplit=1)
    _, after = rest.split(end_marker, maxsplit=1)
    return f"{before}{start_marker}\n{block}\n{end_marker}{after}"


def _build_updated_status() -> str:
    status_text = STATUS_DOC.read_text(encoding="utf-8")
    audit_text = STDLIB_AUDIT_DOC.read_text(encoding="utf-8")
    platform_text = STDLIB_PLATFORM_DOC.read_text(encoding="utf-8")
    compat_block = _render_compat_summary(audit_text, platform_text)
    return _replace_block(status_text, COMPAT_START, COMPAT_END, compat_block)


def generated_outputs() -> dict[Path, str]:
    return {STATUS_DOC: _build_updated_status()}


def main(argv: list[str] | None = None) -> int:
    return generator_main(generated_outputs, argv, description=__doc__)


if __name__ == "__main__":
    raise SystemExit(main())
