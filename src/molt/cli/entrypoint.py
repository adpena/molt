from __future__ import annotations

import sys
from pathlib import Path
from typing import Callable

from molt.cli import build_inputs as _build_inputs
from molt.cli.config_resolution import (
    _resolve_build_config,
    _resolve_capabilities_config,
    _resolve_command_config,
)
from molt.cli.entrypoint_dispatch import _dispatch_entrypoint_command
from molt.cli.entrypoint_parser import _build_entrypoint_parser
from molt.cli.project_roots import _find_project_root
from molt.cli.output import fail
from molt.environment_registry import (
    EnvironmentRegistryError,
    check_process_environment,
)
from molt.process_guard import GuardInfrastructureError, guard_infrastructure_exit_code
from molt.target_python import require_frontend_python


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
        from molt import cli as _cli

        build_fn = _cli.build

    parser = _build_entrypoint_parser()
    args = parser.parse_args()
    if args.command is None:
        parser.print_help()
        sys.exit(0)

    config_root = _find_project_root(Path.cwd())
    if getattr(args, "file", None):
        try:
            config_root = _find_project_root(Path(args.file).resolve())
        except OSError:
            config_root = _find_project_root(Path.cwd())
    config = _build_inputs._load_molt_config(config_root)
    build_cfg = _resolve_build_config(config)
    run_cfg = _resolve_command_config(config, "run")
    compare_cfg = _resolve_command_config(config, "compare")
    test_cfg = _resolve_command_config(config, "test")
    diff_cfg = _resolve_command_config(config, "diff")
    extension_cfg = _resolve_command_config(config, "extension")
    publish_cfg = _resolve_command_config(config, "publish")
    cfg_capabilities = _resolve_capabilities_config(config)

    try:
        return _dispatch_entrypoint_command(
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
