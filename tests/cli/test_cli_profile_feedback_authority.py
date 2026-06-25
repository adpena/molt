from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import profile_feedback

_PROFILE_FEEDBACK_NAMES = (
    "_extract_hot_functions",
    "_extract_runtime_feedback_hot_functions",
    "_load_pgo_profile",
    "_load_runtime_feedback",
    "_pgo_hotspot_entries",
)

_PROFILE_FEEDBACK_DEFINITIONS = (
    "def _extract_hot_functions(",
    "def _extract_runtime_feedback_hot_functions(",
    "def _load_pgo_profile(",
    "def _load_runtime_feedback(",
    "def _pgo_hotspot_entries(",
)


def test_cli_profile_feedback_authority_is_single_home() -> None:
    for name in _PROFILE_FEEDBACK_NAMES:
        assert getattr(cli, name) is getattr(profile_feedback, name)

    cli_source = inspect.getsource(cli)
    for marker in _PROFILE_FEEDBACK_DEFINITIONS:
        assert marker not in cli_source
