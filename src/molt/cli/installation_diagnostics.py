"""Read-only installation visibility; never infer package ownership from a path."""

from __future__ import annotations

import os
from pathlib import Path
import sys
from typing import Any

from molt.compiler_distribution import installed_compiler
from molt.toolchain_identity import executable_candidates


def installation_checks(root: Path) -> list[dict[str, Any]]:
    try:
        installed = installed_compiler(root)
        damage: str | None = None
    except (OSError, ValueError) as exc:
        # Damaged installed metadata is installed damage, not a source checkout.
        installed, damage = None, str(exc)
    launcher = None if installed is None else root.parent / installed.launcher["path"]
    mode = (
        f"damaged installation ({damage})"
        if damage is not None
        else "installed distribution"
        if installed is not None
        else "source checkout"
    )
    checks: list[dict[str, Any]] = [
        {
            "name": "molt-installation",
            "ok": damage is None,
            "detail": f"{mode}: {root}; Python: {sys.executable}",
            "source_root": str(root),
            "launcher": None if launcher is None else str(launcher),
            "python": sys.executable,
        }
    ]
    if damage is not None:
        checks[0].update(
            level="error",
            advice=[
                "Reinstall Molt with the installer, package manager or wheel that provided it"
            ],
        )
    # Rust tools are source-development tools; an installed distribution
    # neither uses nor inspects them.
    source_tools = ("cargo", "rustc") if installed is None and damage is None else ()
    for command in ("molt", "uv", *source_tools, "clang"):
        candidates: list[Path] = []
        for candidate in executable_candidates(command, environment=os.environ):
            # PATH aliases, junctions, symlinks and hardlinks to the same file
            # are not multiple installations. No executable is probed here.
            if not any(candidate.samefile(previous) for previous in candidates):
                candidates.append(candidate)
        selected = candidates[0] if candidates else None
        active_mismatch = bool(
            command == "molt"
            and launcher is not None
            and selected is not None
            and not launcher.samefile(selected)
        )
        ambiguous = len(candidates) > 1 or active_mismatch
        if not candidates and not active_mismatch:
            continue
        check: dict[str, Any] = {
            "name": f"installation-{command}",
            "ok": not ambiguous,
            "detail": f"PATH selects {selected}"
            + (
                "; other installations may shadow the intended tool"
                if ambiguous
                else ""
            ),
            "selected": str(selected),
            "candidates": [str(path) for path in candidates],
        }
        if ambiguous:
            check.update(
                level="warning",
                advice=[
                    "Inspect candidates: "
                    + ", ".join(str(path) for path in candidates),
                    "Choose an explicit executable or adjust PATH yourself. Use the original "
                    "package manager to uninstall only a confirmed unwanted copy; Molt changes nothing.",
                ],
            )
        checks.append(check)
    return checks
