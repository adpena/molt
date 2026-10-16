from __future__ import annotations

import os
import sys
from collections.abc import Generator
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Callable

from molt.cli.entrypoint_parser import _build_entrypoint_parser
from molt.cli.output import fail
from molt.environment_registry import (
    EnvironmentRegistryError,
    check_process_environment,
)
from molt.target_python import require_frontend_python


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


def _build(*args: Any, **kwargs: Any) -> int:
    """Run `build_command.build`, importing the build pipeline only now.

    Commands that never build, such as ``molt --version``, then skip the
    frontend and backend imports. The lookup happens per call, so a test that
    patches ``build_command.build`` reaches this path.
    """
    from molt.cli import build_command

    return build_command.build(*args, **kwargs)


def main(build_fn: Callable[..., int] | None = None) -> int:
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
    build_cfg = _resolve_build_config(config)
    run_cfg = _resolve_command_config(config, "run")
    compare_cfg = _resolve_command_config(config, "compare")
    test_cfg = _resolve_command_config(config, "test")
    diff_cfg = _resolve_command_config(config, "diff")
    extension_cfg = _resolve_command_config(config, "extension")
    publish_cfg = _resolve_command_config(config, "publish")
    cfg_capabilities = _resolve_capabilities_config(config)

    try:
        with _cli_profile():
            return entrypoint_dispatch._dispatch_entrypoint_command(
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
        return fail(
            str(exc),
            bool(getattr(args, "json", False)),
            code=guard_infrastructure_exit_code(),
            command=str(args.command),
        )


if __name__ == "__main__":
    raise SystemExit(main())
