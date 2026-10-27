#!/usr/bin/env python3
"""Schedule the existing collector from published, retained controller source."""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path
from collections.abc import Mapping
from typing import Literal

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from molt.checkout_state import checkout_state_gc_epoch  # noqa: E402
from molt.custody_layout import git_directories  # noqa: E402
from molt.process_environment import ProcessEnvironment  # noqa: E402


def schedule(root: Path, environment: Mapping[str, str], *, cwd: Path) -> Path | None:
    """Dispatch one due epoch; the worker owns daily admission and its result.

    Locally known published main supplies collector policy; captured environment
    supplies scope. No fetch, package installation or retention sweep occurs.
    """
    root = root.resolve(strict=True)
    epoch = checkout_state_gc_epoch(root)
    if epoch is None or not epoch.due:
        return None
    return _schedule(
        root, environment, cwd=cwd, epoch=epoch.identity, operation="checkout"
    )


def schedule_cache(
    root: Path, environment: Mapping[str, str], *, cwd: Path, epoch: str
) -> Path:
    """Dispatch a due CLI cache sweep through the same retained supervisor.

    The caller observed the cache's prune stamp; only the canonical pruner
    admits the attempt and applies policy under its existing prune lock. This
    retained worker waits for contention instead of consuming a busy epoch.
    """
    if epoch != "initial" and (not epoch.isascii() or not epoch.isdecimal()):
        raise ValueError("invalid cache retention epoch")
    return _schedule(
        root.resolve(strict=True), environment, cwd=cwd, epoch=epoch, operation="cache"
    )


def _schedule(
    root: Path,
    environment: Mapping[str, str],
    *,
    cwd: Path,
    epoch: str,
    operation: Literal["checkout", "cache"],
) -> Path:
    from tools.driver_errors import DriverError
    from tools.controller_source import (
        _git_bytes,
        controller_source,
        controller_for_source,
        controllers_root,
    )
    from tools.detached_execution import launch_detached
    from tools.git_identity import require_git_object_id

    directory = "disk_guard" if operation == "checkout" else "cache_retention"
    prefix = "gc" if operation == "checkout" else "prune"
    state = root / "logs" / directory / "automatic" / f"{prefix}-{epoch}"
    if state.exists():
        raise RuntimeError(
            f"maintenance epoch already has custody at {state}; "
            "inspect run.log/rc with detached-verify before recovery"
        )
    captured = dict(environment)
    if operation == "checkout":
        captured["MOLT_EXT_ROOT"] = str(root)
    policy_cwd = cwd.resolve()
    if operation == "cache":
        from molt.default_paths import _default_molt_bin, _default_molt_home

        # Resolve protected installation state against the original command,
        # before the worker moves to source that survives checkout retirement.
        captured["MOLT_CACHE"] = str(root)
        captured["MOLT_HOME"] = str(
            _default_molt_home(environ=captured, cwd=policy_cwd)
        )
        captured["MOLT_BIN"] = str(_default_molt_bin(environ=captured, cwd=policy_cwd))
    # These two protected-target inputs belong to the completed command's cwd.
    for key in ("CARGO_TARGET_DIR", "MOLT_DIFF_CARGO_TARGET_DIR"):
        if raw := captured.get(key, "").strip():
            selected = Path(raw).expanduser()
            captured[key] = str(
                (
                    selected if selected.is_absolute() else policy_cwd / selected
                ).resolve()
            )
    try:
        repository = _REPO_ROOT
        directories = git_directories(repository)
        if directories is None:
            source = controller_for_source(repository)
            if source is None or len(source.root.parents) < 3:
                raise RuntimeError("automatic maintenance needs published Git source")
            common_dir = source.root.parents[2]
            if controllers_root(common_dir) / source.key != source.root:
                raise RuntimeError("maintenance controller has no common Git binding")
            repository = common_dir
        else:
            _, common_dir = directories
        published = require_git_object_id(
            os.fsdecode(
                _git_bytes(
                    repository,
                    "rev-parse",
                    "--verify",
                    "refs/remotes/origin/main^{commit}",
                )
            ).strip(),
            label="published maintenance source",
        )
        with controller_source(repository, common_dir, published) as controller:
            entrypoint = (
                controller.root
                / "tools"
                / (
                    "checkout_maintenance.py"
                    if operation == "checkout"
                    else "cache_maintenance.py"
                )
            )
            if not entrypoint.is_file():
                raise RuntimeError(
                    f"published controller {controller.key} has no automatic "
                    "maintenance entrypoint; daily admission remains pending"
                )
            command = [
                sys.executable,
                "-I",
                "-S",
                "-B",
                "-X",
                f"pycache_prefix={sys.pycache_prefix or ''}",
                "-X",
                "utf8=1",
                str(entrypoint),
                "--root",
                str(root),
                "--policy-cwd",
                str(policy_cwd),
            ]
            launch_detached(
                state,
                command,
                cwd=controller.root,
                environment=ProcessEnvironment(captured),
                controller=controller,
            )
    except DriverError as exc:
        raise RuntimeError(str(exc)) from exc
    return state


def maintenance_arguments(argv: list[str] | None = None) -> argparse.Namespace:
    """Parse the common captured scope without loading collection policy."""
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--policy-cwd", required=True, type=Path)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = maintenance_arguments(argv)
    from tools.disk_guard import collect_due

    results = collect_due(root=args.root, env=ProcessEnvironment(os.environ))
    print(
        json.dumps(
            {
                "schema": "molt.checkout-maintenance.v1",
                "root": str(args.root),
                "policy_cwd": str(args.policy_cwd),
                "results": [result.to_dict() for result in results],
            }
        ),
        flush=True,
    )
    return int(any(result.errors for result in results))


if __name__ == "__main__":
    raise SystemExit(main())
