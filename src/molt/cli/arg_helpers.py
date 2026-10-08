from __future__ import annotations

import argparse
import importlib
from pathlib import Path
from typing import Any

from molt.cli.build_output_layout import _BUILD_PROFILE_CHOICES
from molt.cli.completion import _completion_script


def _cli_module() -> Any:
    return importlib.import_module("molt.cli")


def _fail(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._fail(*args, **kwargs)


def _json_payload(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._json_payload(*args, **kwargs)


def _emit_json(*args: Any, **kwargs: Any) -> Any:
    return _cli_module()._emit_json(*args, **kwargs)


def _build_profile_choices() -> tuple[str, ...]:
    return _BUILD_PROFILE_CHOICES


def completion(shell: str, json_output: bool = False, verbose: bool = False) -> int:
    try:
        script = _completion_script(shell)
    except ValueError as exc:
        return _fail(str(exc), json_output, command="completion")
    if json_output:
        payload = _json_payload(
            "completion",
            "ok",
            data={"shell": shell, "script": script},
        )
        _emit_json(payload, json_output=True)
    else:
        print(script, end="")
    return 0


def _strip_leading_double_dash(args: list[str]) -> list[str]:
    if args and args[0] == "--":
        return args[1:]
    return args


def _extract_output_arg(args: list[str]) -> Path | None:
    for idx, arg in enumerate(args):
        if arg == "--output" and idx + 1 < len(args):
            return Path(args[idx + 1])
        if arg.startswith("--output="):
            return Path(arg.split("=", 1)[1])
    return None


def _extract_out_dir_arg(args: list[str]) -> Path | None:
    for idx, arg in enumerate(args):
        if arg == "--out-dir" and idx + 1 < len(args):
            return Path(args[idx + 1])
        if arg.startswith("--out-dir="):
            return Path(arg.split("=", 1)[1])
    return None


def _extract_emit_arg(args: list[str]) -> str | None:
    for idx, arg in enumerate(args):
        if arg == "--emit" and idx + 1 < len(args):
            return args[idx + 1]
        if arg.startswith("--emit="):
            return arg.split("=", 1)[1]
    return None


def _build_args_has_cache_flag(args: list[str]) -> bool:
    for arg in args:
        if arg in {"--cache", "--no-cache", "--rebuild"}:
            return True
    return False


def _resolve_binary_output(path_str: str) -> Path | None:
    path = Path(path_str)
    if path.exists():
        return path
    fallback = path.with_suffix(".exe")
    if fallback.exists():
        return fallback
    return None


def _build_args_has_trusted_flag(args: list[str]) -> bool:
    for arg in args:
        if arg in {"--trusted", "--no-trusted"}:
            return True
    return False


def _build_args_has_capabilities_flag(args: list[str]) -> bool:
    for arg in args:
        if arg == "--capabilities" or arg.startswith("--capabilities="):
            return True
    return False


def _build_args_has_profile_flag(args: list[str]) -> bool:
    for index, arg in enumerate(args):
        if arg == "--build-profile" or arg.startswith("--build-profile="):
            return True
        if arg == "--profile":
            if index + 1 >= len(args):
                return True
            if args[index + 1] in _build_profile_choices():
                return True
            continue
        if arg.startswith("--profile="):
            if arg.split("=", 1)[1] in _build_profile_choices():
                return True
            continue
    return False


_BUILD_ESSENTIAL_FLAGS = frozenset(
    {
        "file",
        "module",
        "target",
        "release",
        "output",
        "out_dir",
        "verbose",
        "json",
        "rebuild",
        "profile",
        "platform",
        "help",
        "backend",
    }
)


class _BuildHelpFormatter(argparse.RawDescriptionHelpFormatter):
    """Formatter for `molt build` that hides advanced flags.

    Shows only essential flags by default. Advanced flags still work
    but are hidden from --help to reduce noise for new users.
    """

    def _format_action(self, action):
        if action.option_strings:
            dest = action.dest
            if dest not in _BUILD_ESSENTIAL_FLAGS:
                return ""
        return super()._format_action(action)

    def _format_usage(self, usage, actions, groups, prefix):
        filtered = [
            a
            for a in actions
            if not a.option_strings or a.dest in _BUILD_ESSENTIAL_FLAGS
        ]
        return super()._format_usage(usage, filtered, groups, prefix)


class _MoltHelpFormatter(argparse.RawDescriptionHelpFormatter):
    """Custom formatter that groups subcommands by category in --help."""

    def _format_action(self, action: argparse.Action) -> str:
        if isinstance(action, argparse._SubParsersAction):
            parts: list[str] = []
            _core = ["build", "run", "test", "bench", "check", "deploy"]
            _package = ["package", "publish", "deps", "vendor", "install"]
            _toolchain = ["clean", "doctor", "update", "config", "completion"]
            _dev = [
                "compare",
                "diff",
                "parity-run",
                "profile",
                "lint",
                "extension",
                "factgraph",
                "verify",
            ]

            groups = [
                ("Core commands:", _core),
                ("Package commands:", _package),
                ("Toolchain commands:", _toolchain),
                ("Development commands:", _dev),
            ]

            # Build a lookup from dest -> subaction for ordered iteration
            _action_map: dict[str, argparse.Action] = {}
            for subaction in action._get_subactions():
                _action_map[subaction.dest] = subaction

            for title, names in groups:
                section_actions = [_action_map[n] for n in names if n in _action_map]
                if not section_actions:
                    continue
                parts.append(f"\n  {title}")
                for sa in section_actions:
                    help_text = sa.help or ""
                    parts.append(f"    {sa.dest:<22s}{help_text}")

            listed: set[str] = set()
            for _, names in groups:
                listed.update(names)
            extras = []
            for subaction in action._get_subactions():
                if subaction.dest not in listed and subaction.help != argparse.SUPPRESS:
                    extras.append(subaction)
            if extras:
                parts.append("\n  Other commands:")
                for sa in extras:
                    help_text = sa.help or ""
                    parts.append(f"    {sa.dest:<22s}{help_text}")

            return "\n".join(parts) + "\n"
        return super()._format_action(action)


def _add_debug_shared_selector_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--function", help="Function selector for focused debug runs.")
    parser.add_argument("--module", help="Module selector for focused debug runs.")
    parser.add_argument("--pass", dest="pass_name", help="Compiler pass selector.")
    parser.add_argument("--backend", help="Backend selector for debug runs.")
    parser.add_argument("--profile", help="Build/debug profile selector.")
    parser.add_argument(
        "--format",
        choices=["text", "json"],
        default="text",
        help="Summary format emitted to stdout and retained outputs.",
    )
    parser.add_argument(
        "--out",
        help="Retain the debug summary under logs/debug/ using the requested name.",
    )
