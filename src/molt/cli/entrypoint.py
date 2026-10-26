from __future__ import annotations

import os
import sys
import time
from collections.abc import Generator
from contextlib import contextmanager
from pathlib import Path
from typing import TYPE_CHECKING, Callable

from molt.cli.entrypoint_parser import _build_entrypoint_parser
from molt.cli.output import fail
from molt.environment_registry import (
    EnvironmentRegistryError,
    check_process_environment,
)
from molt.target_python import require_frontend_python

if TYPE_CHECKING:
    from molt.cli.build_request import BuildRequest


@contextmanager
def _cli_profile() -> Generator[None]:
    """Profile this command when MOLT_CLI_PROFILE names a stats file."""
    target = os.environ.get("MOLT_CLI_PROFILE", "").strip()
    if not target:
        yield
        return
    import cProfile

    profiler = cProfile.Profile()
    profiler.enable()
    try:
        yield
    finally:
        profiler.disable()
        path = Path(target.replace("{pid}", str(os.getpid())))
        path.parent.mkdir(parents=True, exist_ok=True)
        profiler.dump_stats(path)


def _build(request: BuildRequest) -> int:
    """Run `build_command.build`, importing the build pipeline only now.

    Commands that never build, such as ``molt --version``, then skip the
    frontend and backend imports. The lookup happens per call, so a test that
    patches ``build_command.build`` reaches this path.
    """
    from molt.cli import build_command

    returncode = build_command.build(request)
    if returncode == 0 and request.cache:
        _apply_cache_budget_after_build(
            Path(request.cache_dir).expanduser() if request.cache_dir else None,
            verbose=request.verbose,
        )
    return returncode


def _apply_cache_budget_after_build(cache_root: Path | None, *, verbose: bool) -> None:
    """Apply the cache budget once a day, after a build published its output.

    It prunes the cache the build used. The common case reads one lock stamp
    and imports nothing more; a due sweep runs the canonical automatic
    retention. Retention never changes the build's result.
    """
    from molt.default_paths import (
        CLI_CACHE_RETENTION_INTERVAL_SECONDS,
        _default_molt_cache,
        shared_cache_lock_path,
    )

    if os.environ.get("MOLT_CACHE_PRUNE", "1") == "0":
        return
    root = cache_root or _default_molt_cache(environ=os.environ)
    stamp = shared_cache_lock_path(root, "prune")
    try:
        age = time.time() - stamp.stat().st_mtime
    except FileNotFoundError:
        age = None
    except OSError:
        return
    if age is not None and 0 <= age < CLI_CACHE_RETENTION_INTERVAL_SECONDS:
        return
    from molt.cache_eviction import automatic_retention

    report = automatic_retention(
        os.environ,
        cache_root=root,
        min_interval_seconds=CLI_CACHE_RETENTION_INTERVAL_SECONDS,
    )
    if verbose and report is not None and report.get("status") != "recent":
        print(f"molt: cache retention: {report}", file=sys.stderr)


def main(build_fn: Callable[[BuildRequest], int] | None = None) -> int:
    try:
        require_frontend_python()
    except RuntimeError as exc:
        print(f"molt: error: {exc}", file=sys.stderr)
        return 2
    try:
        check_process_environment()
    except EnvironmentRegistryError as exc:
        print(f"molt: error: {exc}", file=sys.stderr)
        return 2
    if build_fn is None:
        build_fn = _build

    parser = _build_entrypoint_parser()
    args = parser.parse_args()
    if args.command is None:
        parser.print_help()
        sys.exit(0)

    # A parsed command needs the project configuration and the dispatcher;
    # `--version`, `--help` and usage errors exit inside parse_args without
    # importing them (HF-228).
    from molt.cli import entrypoint_dispatch
    from molt.cli.config_resolution import (
        _load_molt_config,
        _resolve_build_config,
        _resolve_capabilities_config,
        _resolve_command_config,
    )
    from molt.cli.project_roots import _find_project_root
    from molt.process_guard import (
        GuardInfrastructureError,
        guard_infrastructure_exit_code,
    )

    config_root = _find_project_root(Path.cwd())
    if getattr(args, "file", None):
        try:
            config_root = _find_project_root(Path(args.file).resolve())
        except OSError:
            config_root = _find_project_root(Path.cwd())
    config = _load_molt_config(config_root)
    try:
        build_cfg = _resolve_build_config(config)
    except ValueError as exc:
        return fail(
            str(exc),
            bool(getattr(args, "json", False)),
            code=2,
            command=str(args.command),
        )
    run_cfg = _resolve_command_config(config, "run")
    compare_cfg = _resolve_command_config(config, "compare")
    test_cfg = _resolve_command_config(config, "test")
    diff_cfg = _resolve_command_config(config, "diff")
    extension_cfg = _resolve_command_config(config, "extension")
    publish_cfg = _resolve_command_config(config, "publish")
    cfg_capabilities = _resolve_capabilities_config(config)

    maintenance_cwd = Path.cwd()
    maintenance_environment = None
    if args.command in {
        "run",
        "compare",
        "test",
        "diff",
        "bench",
        "profile",
        "lint",
        "harness",
        "deploy",
    }:
        from molt.dx import checkout_state_maintenance_environment
        from molt.source_root import compiler_source_root

        maintenance_environment = checkout_state_maintenance_environment(
            compiler_source_root(), os.environ
        )
    try:
        from molt.checkout_state import checkout_state_use

        with _cli_profile(), checkout_state_use(((Path(sys.prefix), None),)):
            result = entrypoint_dispatch._dispatch_entrypoint_command(
                args,
                parser=parser,
                build_fn=build_fn,
                config_root=config_root,
                config=config,
                build_cfg=build_cfg,
                run_cfg=run_cfg,
                compare_cfg=compare_cfg,
                test_cfg=test_cfg,
                diff_cfg=diff_cfg,
                extension_cfg=extension_cfg,
                publish_cfg=publish_cfg,
                cfg_capabilities=cfg_capabilities,
            )
    except GuardInfrastructureError as exc:
        # The guard failed, not the command it ran: report it as such, with
        # the guard's own exit code, wherever the failure surfaced.
        result = fail(
            str(exc),
            bool(getattr(args, "json", False)),
            code=guard_infrastructure_exit_code(),
            command=str(args.command),
        )
    if maintenance_environment is not None and not maintenance_environment.get(
        "MOLT_PROOF_QUEUE"
    ):
        from molt.dx import collect_checkout_state_after_result

        # Direct reporters also need a terminal publication barrier.
        sys.stdout.flush()
        sys.stderr.flush()
        collect_checkout_state_after_result(
            maintenance_environment, cwd=maintenance_cwd
        )
    return result


if __name__ == "__main__":
    raise SystemExit(main())
