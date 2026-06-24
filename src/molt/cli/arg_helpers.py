from __future__ import annotations

import argparse
import importlib
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any, Mapping

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
    return _cli_module()._BUILD_PROFILE_CHOICES


def _hash_seed_override_env() -> str:
    return _cli_module()._HASH_SEED_OVERRIDE_ENV


def _hash_seed_sentinel_env() -> str:
    return _cli_module()._HASH_SEED_SENTINEL_ENV


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


def _is_windows_process_model() -> bool:
    return os.name == "nt"


def _flush_standard_streams() -> None:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.flush()
        except (OSError, ValueError):
            pass


def _process_exit_code(returncode: int | None) -> int:
    if returncode is None:
        return 1
    if returncode < 0:
        return 128 + abs(returncode)
    return returncode


def _cli_hash_seed_reexec_argv() -> list[str] | None:
    orig_argv = list(getattr(sys, "orig_argv", ()) or ())
    if len(orig_argv) >= 2 and orig_argv[1] not in {"-c", "-"}:
        return [sys.executable, *orig_argv[1:]]
    if not sys.argv or sys.argv[0] in {"", "-c", "-"}:
        return None
    argv0 = sys.argv[0]
    has_sep = os.sep in argv0 or (os.altsep is not None and os.altsep in argv0)
    if has_sep or Path(argv0).exists() or shutil.which(argv0):
        return [sys.executable, *sys.argv]
    return None


def _reexec_cli_with_hash_seed(env: Mapping[str, str]) -> None:
    argv = _cli_hash_seed_reexec_argv()
    if argv is None:
        return
    if _is_windows_process_model():
        try:
            completed = subprocess.run(argv, env=dict(env), check=False)
        except OSError as exc:
            print(
                f"molt: failed to restart with PYTHONHASHSEED: {exc}", file=sys.stderr
            )
            _flush_standard_streams()
            os._exit(127)
        _flush_standard_streams()
        os._exit(_process_exit_code(completed.returncode))
    os.execvpe(argv[0], argv, env)


def _ensure_cli_hash_seed() -> None:
    desired = os.environ.get(_hash_seed_override_env(), "0").strip()
    if not desired:
        desired = "0"
    if desired.lower() in {"off", "disable", "random"}:
        return
    if os.environ.get("PYTHONHASHSEED") == desired:
        return
    if os.environ.get(_hash_seed_sentinel_env()) == "1":
        return
    env = os.environ.copy()
    env["PYTHONHASHSEED"] = desired
    env[_hash_seed_sentinel_env()] = "1"
    _reexec_cli_with_hash_seed(env)


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
