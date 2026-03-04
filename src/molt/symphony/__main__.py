from __future__ import annotations

import argparse
import sys

from .config import build_runtime_config, validate_dispatch_config
from .errors import (
    ConfigValidationError,
    MissingWorkflowFileError,
    SymphonyError,
    WorkflowFrontMatterNotMapError,
    WorkflowParseError,
)
from .logging_utils import log
from .orchestrator import create_orchestrator
from .runtime_features import detect_runtime_features
from .workflow import discover_workflow_path, load_workflow


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python -m molt.symphony",
        description="Run the Symphony orchestrator for this repository.",
    )
    parser.add_argument(
        "workflow_path",
        nargs="?",
        default=None,
        help="Path to WORKFLOW.md (defaults to ./WORKFLOW.md).",
    )
    parser.add_argument(
        "--port",
        type=int,
        default=None,
        help="Optional HTTP dashboard/API port (overrides server.port in workflow).",
    )
    parser.add_argument(
        "--once",
        action="store_true",
        help="Run exactly one poll/reconcile tick then exit.",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    workflow_path = discover_workflow_path(args.workflow_path)
    try:
        workflow = load_workflow(workflow_path)
        config = build_runtime_config(workflow)
        validate_dispatch_config(config)
        runtime_features = detect_runtime_features()
        log("INFO", "runtime_features_detected", **runtime_features.to_log_fields())
    except (
        MissingWorkflowFileError,
        WorkflowParseError,
        WorkflowFrontMatterNotMapError,
        ConfigValidationError,
    ) as exc:
        log(
            "ERROR",
            "startup_validation_failed",
            error=str(exc),
            workflow_path=workflow_path,
        )
        return 1

    try:
        orchestrator = create_orchestrator(
            str(workflow_path),
            port_override=args.port,
            run_once=args.once,
        )
        return orchestrator.run()
    except SymphonyError as exc:
        log("ERROR", "symphony_failed", error=str(exc))
        return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
