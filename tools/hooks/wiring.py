"""The Claude Code hook wiring: one declaration, projected into its consumers.

``HOOKS`` names each hook Claude Code starts: its event, its matcher, the
``tools/hooks/<hook>.py`` script and its timeout. ``tools/gen_claude_hooks.py``
projects it into ``.claude/settings.json``, which Claude Code reads; the tests
read this module, never the settings text. The hooks stay this small on
purpose (owner, 2026-10-21): a hook must never stop work, and each one costs
time on every event it runs for.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Literal

HookEvent = Literal["SessionStart"]

SETTINGS_SCHEMA = "https://json.schemastore.org/claude-code-settings.json"
LAUNCHER = '"${CLAUDE_PROJECT_DIR}/tools/hooks/run.sh"'


@dataclass(frozen=True, slots=True)
class HookCommand:
    event: HookEvent
    matcher: str | None
    hook: str
    timeout_s: int

    @property
    def command(self) -> str:
        # `sh` on a missing file exits 127; the guard makes it a visible,
        # non-blocking exit 1 that names the missing launcher.
        missing = (
            f"molt hook {self.hook} did not run: "
            "no tools/hooks/run.sh in ${CLAUDE_PROJECT_DIR}"
        )
        return (
            f'[ -f {LAUNCHER} ] || {{ echo "{missing}" >&2; exit 1; }}; '
            f"sh {LAUNCHER} {self.hook}"
        )


HOOKS: tuple[HookCommand, ...] = (
    HookCommand("SessionStart", None, "session_digest", 15),
)


def settings_document() -> str:
    """The exact text of ``.claude/settings.json``."""
    events: dict[str, list[dict[str, object]]] = {}
    for hook in HOOKS:
        group: dict[str, object] = {}
        if hook.matcher is not None:
            group["matcher"] = hook.matcher
        group["hooks"] = [
            {"type": "command", "command": hook.command, "timeout": hook.timeout_s}
        ]
        events.setdefault(hook.event, []).append(group)
    document = {"$schema": SETTINGS_SCHEMA, "hooks": events}
    return json.dumps(document, indent=2) + "\n"
