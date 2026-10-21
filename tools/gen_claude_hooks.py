#!/usr/bin/env python3
"""Project the Claude Code hook wiring into the files that consume it.

``tools/hooks/wiring.py`` declares the hooks; this writes
``.claude/settings.json`` from it.
"""

from __future__ import annotations

import sys
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.generator_io import generator_main  # noqa: E402
from tools.hooks import wiring  # noqa: E402

SETTINGS = _REPO_ROOT / ".claude" / "settings.json"


def generated_outputs() -> dict[Path, str]:
    return {SETTINGS: wiring.settings_document()}


def main(argv: list[str] | None = None) -> int:
    return generator_main(generated_outputs, argv, description=__doc__)


if __name__ == "__main__":
    raise SystemExit(main())
