from __future__ import annotations

import argparse
from collections.abc import Mapping, Sequence
import json
from pathlib import Path


def parser(
    *,
    default_poll_interval_sec: float,
    default_samples_max_mb: float,
    hard_max_rss_gb: float,
    hard_max_global_rss_gb: float,
) -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        description="Run a command with a process-tree/process-group RSS ceiling."
    )
    result.add_argument(
        "--max-rss-gb",
        "--max-process-rss-gb",
        dest="max_rss_gb",
        type=float,
        default=None,
        help=(
            "Abort if any child process exceeds this RSS; must be "
            f"<{hard_max_rss_gb:g}GB "
            "(default: adaptive from live available memory)."
        ),
    )
    result.add_argument(
        "--max-total-rss-gb",
        "--max-tree-rss-gb",
        "--max-group-rss-gb",
        dest="max_total_rss_gb",
        type=float,
        default=None,
        help=(
            "Abort if the watched process tree exceeds this aggregate RSS; "
            f"must be <{hard_max_rss_gb:g}GB "
            "(default: adaptive from live available memory)."
        ),
    )
    result.add_argument(
        "--max-global-rss-gb",
        type=float,
        default=None,
        help=(
            "Record and constrain the resolved host-wide RSS custody budget; "
            f"must be <{hard_max_global_rss_gb:g}GB "
            "(default: adaptive from live available memory)."
        ),
    )
    result.add_argument(
        "--poll-interval",
        type=float,
        default=default_poll_interval_sec,
        help=(
            "Process sampling interval in seconds "
            f"(default: {default_poll_interval_sec})."
        ),
    )
    result.add_argument(
        "--summary-json",
        help="Write command result, violation, and peak RSS details as JSON.",
    )
    result.add_argument(
        "--samples-jsonl",
        help="Append per-poll peak and process-tree RSS samples as JSONL.",
    )
    result.add_argument(
        "--samples-max-mb",
        type=float,
        default=default_samples_max_mb,
        help=(
            "Rotate --samples-jsonl after this many MB; set <=0 to disable "
            f"rotation (default: {default_samples_max_mb})."
        ),
    )
    result.add_argument(
        "--stream",
        choices=("stderr", "stdout", "json-stderr", "json-stdout"),
        default="",
        help="Emit per-poll guard samples to this stream without writing artifacts.",
    )
    result.add_argument(
        "--child-rlimit-gb",
        type=float,
        default=None,
        help=(
            "Apply an RLIMIT_RSS backstop to the direct guarded child before "
            "exec; defaults to the adaptive per-process RSS budget and never "
            "constrains sparse virtual-address reservations. Set <=0 to disable "
            "this layer."
        ),
    )
    result.add_argument(
        "--timeout",
        type=float,
        help="Abort the command if wall-clock runtime exceeds this many seconds.",
    )
    result.add_argument("command", nargs=argparse.REMAINDER)
    return result


def load_internal_command(
    environ: Mapping[str, str],
    *,
    worker_env_name: str,
    command_env_name: str,
) -> list[str] | None:
    if environ.get(worker_env_name) != "1":
        return None
    raw = environ.get(command_env_name)
    if not raw:
        raise ValueError(f"{command_env_name} is required")
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise ValueError(f"{command_env_name} is invalid JSON") from exc
    if not isinstance(payload, list) or not all(
        isinstance(item, str) for item in payload
    ):
        raise ValueError(f"{command_env_name} must be a JSON string list")
    if not payload:
        raise ValueError(f"{command_env_name} command must not be empty")
    return payload


def child_env_without_internal_keys(
    environ: Mapping[str, str],
    *,
    internal_env_keys: Sequence[str],
) -> dict[str, str]:
    child_env = dict(environ)
    for key in internal_env_keys:
        child_env.pop(key, None)
    return child_env


def worker_env(
    environ: Mapping[str, str],
    command: Sequence[str],
    *,
    worker_env_name: str,
    command_env_name: str,
) -> dict[str, str]:
    result = dict(environ)
    result[command_env_name] = json.dumps(list(command))
    result[worker_env_name] = "1"
    return result


def worker_argv(
    args: argparse.Namespace,
    *,
    python_executable: str,
    script_path: Path,
) -> list[str]:
    result = [
        python_executable,
        str(script_path),
        "--poll-interval",
        str(args.poll_interval),
    ]
    if args.max_rss_gb is not None:
        result.extend(["--max-rss-gb", str(args.max_rss_gb)])
    if args.max_total_rss_gb is not None:
        result.extend(["--max-total-rss-gb", str(args.max_total_rss_gb)])
    if args.max_global_rss_gb is not None:
        result.extend(["--max-global-rss-gb", str(args.max_global_rss_gb)])
    if args.summary_json:
        result.extend(["--summary-json", args.summary_json])
    if args.samples_jsonl:
        result.extend(["--samples-jsonl", args.samples_jsonl])
        result.extend(["--samples-max-mb", str(args.samples_max_mb)])
    if args.stream:
        result.extend(["--stream", args.stream])
    if args.child_rlimit_gb is not None:
        result.extend(["--child-rlimit-gb", str(args.child_rlimit_gb)])
    if args.timeout is not None:
        result.extend(["--timeout", str(args.timeout)])
    return result
