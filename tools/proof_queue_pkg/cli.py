"""Stable proof queue argument parser and command dispatch."""

from __future__ import annotations

import argparse
import shlex
import sys

from tools.proof_queue_pkg import commands, custody, presentation, state


def _dispatch_pact_command(args: argparse.Namespace) -> int:
    """Load the scientific/Pact stack only for commands that require it."""
    from tools.proof_queue_pkg import pact

    return getattr(pact, args.pact_handler)(args)


def _command_after_dash(argv: list[str]) -> tuple[list[str], list[str]]:
    if "--" not in argv:
        return argv, []
    index = argv.index("--")
    return argv[:index], argv[index + 1 :]



_PROOF_COMMAND_SUBCOMMANDS = frozenset({"exec", "cargo"})

_GLOBAL_OPTIONS_WITH_VALUES = frozenset(
    {"--db", "--logs-root", "--notebooks-root", "--repo-root"}
)

_PROOF_COMMAND_OPTIONS_WITH_VALUES = frozenset(
    {
        "--id",
        "--reason",
        "--resource-family",
        "--contention-key",
        "--scope",
        "--env",
        "--note",
        "--depends-on",
        "--edge-kind",
        "--edge-note",
        "--timeout",
    }
)

_HELP_OPTIONS = frozenset({"-h", "--help"})



def _proof_command_subcommand_index(raw: list[str]) -> int | None:
    index = 0
    while index < len(raw):
        token = raw[index]
        if token == "--":
            return None
        if token in _PROOF_COMMAND_SUBCOMMANDS:
            return index
        if token in _GLOBAL_OPTIONS_WITH_VALUES:
            index += 2
            continue
        if any(
            token.startswith(f"{option}=") for option in _GLOBAL_OPTIONS_WITH_VALUES
        ):
            index += 1
            continue
        index += 1
    return None



def _split_proof_command_argv(
    raw: list[str],
    *,
    subcommand: str,
) -> tuple[list[str], list[str]]:
    if "--" not in raw:
        raise SystemExit(
            f"proof_queue.py {subcommand} requires `--` before the proof command; "
            "quote option values before the delimiter or use the TOML submit lane."
        )
    before, command = _command_after_dash(raw)
    if not command:
        raise SystemExit(
            f"proof_queue.py {subcommand} requires a proof command after `--`."
        )
    return before, command



def _proof_command_help_requested(raw: list[str]) -> bool:
    before_delimiter, _command = _command_after_dash(raw)
    index = 1
    while index < len(before_delimiter):
        token = before_delimiter[index]
        if token in _HELP_OPTIONS:
            return True
        if token in _PROOF_COMMAND_OPTIONS_WITH_VALUES:
            index += 2
            continue
        if any(
            token.startswith(f"{option}=")
            for option in _PROOF_COMMAND_OPTIONS_WITH_VALUES
        ):
            index += 1
            continue
        index += 1
    return False



def _reject_pre_delimiter_remainder(
    args: argparse.Namespace,
    *,
    subcommand: str,
    attr: str,
) -> None:
    residue = list(getattr(args, attr, []) or [])
    if not residue:
        return
    preview = " ".join(shlex.quote(part) for part in residue[:8])
    if len(residue) > 8:
        preview += " ..."
    raise SystemExit(
        f"proof_queue.py {subcommand} saw stray positional argument(s) before "
        f"`--`: {preview}. This usually means a value for --reason, --note, "
        "or another metadata option lost shell quoting; refusing to run with "
        "possibly dropped queue metadata."
    )



def _add_dependency_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--depends-on",
        action="append",
        default=[],
        metavar="RUN_ID",
        help=(
            "append a proof DAG parent; depends_on edges wait until parents "
            "pass, lineage kinds (reruns/supersedes/compares/derives_from) "
            "record provenance without gating"
        ),
    )
    parser.add_argument(
        "--edge-kind",
        default=state.DEFAULT_EDGE_KIND,
        choices=sorted(state.EDGE_KINDS),
        help=(
            "canonical relationship kind for --depends-on edges; only "
            "depends_on gates scheduling"
        ),
    )
    parser.add_argument(
        "--edge-note",
        help="immutable note attached to each --depends-on edge",
    )



def _add_named_lane_args(parser: argparse.ArgumentParser, *, note_help: str) -> None:
    parser.add_argument("--env", action="append", default=[], metavar="NAME=VALUE")
    parser.add_argument(
        "--note",
        action="append",
        default=[],
        help=note_help,
    )
    _add_dependency_args(parser)
    parser.add_argument("--timeout", type=float)
    execution = parser.add_mutually_exclusive_group()
    execution.add_argument(
        "--queue-only",
        action="store_true",
        help="submit the named proof row without running it or launching a runner",
    )
    execution.add_argument("--detach", action="store_true")
    parser.add_argument("--print-spec", action="store_true")



def main(argv: list[str] | None = None) -> int:
    custody._normalize_queue_process_environment()
    raw = list(sys.argv[1:] if argv is None else argv)
    proof_subcommand_index = _proof_command_subcommand_index(raw)
    if proof_subcommand_index is not None:
        subcommand = raw[proof_subcommand_index]
        before_subcommand = raw[:proof_subcommand_index]
        subcommand_argv = raw[proof_subcommand_index:]
        if _proof_command_help_requested(subcommand_argv):
            parser = _build_parser()
            args = parser.parse_args(raw)
            return int(args.func(args))
        before, command = _split_proof_command_argv(
            subcommand_argv,
            subcommand=subcommand,
        )
        before = [*before_subcommand, *before]
        parser = _build_parser()
        args = parser.parse_args(before)
        if subcommand == "exec":
            _reject_pre_delimiter_remainder(
                args,
                subcommand=subcommand,
                attr="command",
            )
            args.command = command
        else:
            _reject_pre_delimiter_remainder(
                args,
                subcommand=subcommand,
                attr="cargo_args",
            )
            args.cargo_args = command
    else:
        parser = _build_parser()
        args = parser.parse_args(raw)
    return int(args.func(args))

def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Submit, run, and inspect Molt proof lanes with contention limits."
    )
    parser.add_argument("--db")
    parser.add_argument("--logs-root")
    parser.add_argument("--notebooks-root")
    parser.add_argument("--repo-root")
    sub = parser.add_subparsers(dest="cmd", required=True)

    exec_p = sub.add_parser(
        "exec",
        help="submit and run one inline proof",
        description="submit and run one inline proof",
    )
    exec_p.add_argument("--id", required=True)
    exec_p.add_argument("--reason", required=True)
    exec_p.add_argument("--resource-family", default="generic")
    exec_p.add_argument("--contention-key")
    exec_p.add_argument("--scope", action="append", default=[])
    exec_p.add_argument("--env", action="append", default=[], metavar="NAME=VALUE")
    exec_p.add_argument(
        "--note",
        action="append",
        default=[],
        help=(
            "append a submission note describing what changed, what is being "
            "tested or explored, and why"
        ),
    )
    _add_dependency_args(exec_p)
    exec_p.add_argument("--timeout", type=float, default=1200.0)
    exec_p.add_argument("--detach", action="store_true")
    exec_p.add_argument("command", nargs=argparse.REMAINDER)
    exec_p.set_defaults(func=commands._cmd_exec)

    cargo_p = sub.add_parser(
        "cargo",
        help="submit a queue-owned Cargo proof with canonical uv and guard wrapping",
        description=(
            "submit a queue-owned Cargo proof with canonical uv and guard wrapping"
        ),
    )
    cargo_p.add_argument("--id", required=True)
    cargo_p.add_argument("--reason", required=True)
    cargo_p.add_argument("--contention-key")
    cargo_p.add_argument("--scope", action="append", default=[])
    cargo_p.add_argument("--env", action="append", default=[], metavar="NAME=VALUE")
    cargo_p.add_argument(
        "--note",
        action="append",
        default=[],
        help=(
            "append a submission note describing what changed, what is being "
            "tested or explored, and why"
        ),
    )
    _add_dependency_args(cargo_p)
    cargo_p.add_argument("--timeout", type=float, default=1200.0)
    cargo_p.add_argument("--detach", action="store_true")
    cargo_p.add_argument(
        "--allow-warm-single-test",
        action="store_true",
        help=(
            "allow `cargo test --lib <one filter>` only after a target-dir warmup "
            "has already been verified and recorded in --note"
        ),
    )
    cargo_p.add_argument("cargo_args", nargs=argparse.REMAINDER)
    cargo_p.set_defaults(func=commands._cmd_cargo)

    submit_p = sub.add_parser("submit", help="submit proof specs from a TOML DSL")
    submit_p.add_argument("dsl")
    submit_p.set_defaults(func=commands._cmd_submit)

    run_p = sub.add_parser("run", help="run queued proof specs")
    run_p.add_argument(
        "--limit",
        "--jobs",
        dest="limit",
        type=int,
        default=None,
        help=(
            "maximum queued rows to run; defaults to 1, or to --queue-size "
            "when --detach is used. --jobs is the operator-facing alias."
        ),
    )
    run_p.add_argument(
        "--queue-size",
        type=int,
        default=None,
        help=(
            "maximum concurrently dispatched/running rows; defaults to "
            f"{state.DEFAULT_PROOF_QUEUE_SIZE} or {state.PROOF_QUEUE_SIZE_ENV}"
        ),
    )
    run_p.add_argument("--run-id")
    run_p.add_argument("--timeout", type=float, default=1200.0)
    run_p.add_argument("--detach", action="store_true")
    run_p.set_defaults(func=commands._cmd_run)

    status_p = sub.add_parser("status", help="show active and recent proof runs")
    status_p.add_argument("--recent", type=int, default=20)
    status_p.set_defaults(func=commands._cmd_status)

    evidence_p = sub.add_parser(
        "evidence", help="export machine-readable proof evidence"
    )
    evidence_p.add_argument(
        "run_id",
        nargs="?",
        help="proof run id to export (positional, mirrors diagnose)",
    )
    evidence_p.add_argument(
        "--run-id",
        dest="run_id_option",
        help="proof run id to export",
    )
    evidence_p.add_argument("--limit", type=int, default=20)
    evidence_p.add_argument("--output")
    evidence_p.set_defaults(func=commands._cmd_evidence)

    audit_p = sub.add_parser(
        "audit",
        help="adversarially inspect queue health across rows, notes, DAG, logs, and projections",
    )
    audit_p.add_argument("--limit", type=int, default=50)
    audit_p.add_argument("--all", action="store_true")
    audit_p.add_argument("--strict", action="store_true")
    audit_p.add_argument("--json", action="store_true")
    audit_p.add_argument("--output")
    audit_p.add_argument(
        "--max-issues",
        type=int,
        default=20,
        help="maximum human issue rows to print; use 0 for all",
    )
    audit_p.add_argument(
        "--errors-only",
        action="store_true",
        help="hide warning issue rows from human output without changing JSON/output payloads or exit status",
    )
    audit_p.add_argument("--stale-log-seconds", type=float, default=900.0)
    audit_p.add_argument("--no-notebook-check", action="store_true")
    audit_p.set_defaults(func=commands._cmd_audit)

    diagnose_p = sub.add_parser(
        "diagnose",
        help="classify a proof run failure from recorded queue facts and log tail",
    )
    diagnose_p.add_argument("run_id", nargs="?")
    diagnose_p.add_argument(
        "--logical-id",
        help="diagnose the latest run with this logical id when run_id is omitted",
    )
    diagnose_p.add_argument("--json", action="store_true")
    diagnose_p.add_argument("--output")
    diagnose_p.add_argument(
        "--append-note",
        action="store_true",
        help="append the deterministic diagnosis as an immutable proof note",
    )
    diagnose_p.add_argument(
        "--kind",
        default="finding",
        choices=sorted(state.NOTE_KINDS),
        help="note kind used with --append-note",
    )
    diagnose_p.add_argument("--author")
    diagnose_p.add_argument("--no-notebook", action="store_true")
    diagnose_p.set_defaults(func=commands._cmd_diagnose)

    note_p = sub.add_parser("note", help="append an immutable note to a proof run")
    note_p.add_argument("run_id")
    note_p.add_argument("--note", action="append", required=True)
    note_p.add_argument(
        "--kind",
        default=state.DEFAULT_NOTE_KIND,
        choices=sorted(state.NOTE_KINDS),
        help="canonical note lane for append-only collaboration",
    )
    note_p.add_argument("--author")
    note_p.add_argument("--output")
    note_p.add_argument("--no-notebook", action="store_true")
    note_p.set_defaults(func=commands._cmd_note)

    link_p = sub.add_parser(
        "link", help="append an immutable proof DAG edge between existing runs"
    )
    link_p.add_argument("child_run_id")
    link_p.add_argument("--parent", required=True)
    link_p.add_argument(
        "--kind",
        default=state.DEFAULT_EDGE_KIND,
        choices=sorted(state.EDGE_KINDS),
        help="canonical proof DAG edge kind",
    )
    link_p.add_argument("--note")
    link_p.add_argument("--author")
    link_p.add_argument("--no-notebook", action="store_true")
    link_p.set_defaults(func=commands._cmd_link)

    notebook_p = sub.add_parser(
        "notebook", help="write the deterministic marimo notebook for a proof run"
    )
    notebook_p.add_argument("run_id")
    notebook_p.add_argument("--output")
    notebook_p.set_defaults(func=commands._cmd_notebook)

    prune_p = sub.add_parser("prune-stale", help="mark dead running records stale")
    prune_p.add_argument(
        "--run-id",
        action="append",
        help="Limit pruning to one queued/running proof row; repeat for several rows.",
    )
    prune_p.set_defaults(func=commands._cmd_prune_stale)

    quickstart_p = sub.add_parser(
        "quickstart", help="print canonical queue muscle memory"
    )
    quickstart_p.set_defaults(func=presentation._cmd_quickstart)

    template_p = sub.add_parser("template", help="print a proof DSL template")
    template_p.set_defaults(func=presentation._cmd_template)

    cargo_template_p = sub.add_parser(
        "cargo-template", help="print the canonical Cargo proof command shape"
    )
    cargo_template_p.set_defaults(func=presentation._cmd_cargo_template)

    pact_accept_p = sub.add_parser(
        "pact-witness-acceptance",
        help="run the queue-owned Pact Kernel A browser/WASM acceptance aperture",
    )
    _add_named_lane_args(
        pact_accept_p,
        note_help="append submission context to the acceptance run",
    )
    pact_accept_p.set_defaults(
        func=_dispatch_pact_command,
        pact_handler="_cmd_pact_witness_acceptance",
    )

    pact_oracle_p = sub.add_parser(
        "pact-witness-oracle",
        help="run the queued Pact Kernel A fixture/reference parity oracle",
    )
    _add_named_lane_args(
        pact_oracle_p,
        note_help="append submission context to the oracle run",
    )
    pact_oracle_p.set_defaults(
        func=_dispatch_pact_command,
        pact_handler="_cmd_pact_witness_oracle",
    )

    r6_parity_p = sub.add_parser(
        "r6-target-version-parity",
        help="run the queued R6 target-version stdlib parity shard",
    )
    r6_parity_p.add_argument(
        "--python-version",
        default="3.12",
        help="target CPython minor to validate (default: 3.12)",
    )
    r6_parity_p.add_argument(
        "--fixture",
        action="append",
        help=(
            "limit the named R6 lane to one checked-in fixture; accepts full path, "
            "basename, or stem; repeat for a shard"
        ),
    )
    _add_named_lane_args(
        r6_parity_p,
        note_help="append submission context to the R6 parity run",
    )
    r6_parity_p.set_defaults(
        func=_dispatch_pact_command,
        pact_handler="_cmd_r6_target_version_parity",
    )

    native_run_p = sub.add_parser(
        "native-molt-run",
        help="queue a native `molt run` entrypoint probe",
        description=(
            "queue a native `molt run` entrypoint probe with canonical uv, "
            "memory-guard, log, DAG, and contention custody"
        ),
    )
    _add_named_lane_args(
        native_run_p,
        note_help="append submission context to the native Molt run",
    )
    native_run_p.add_argument("entry", help="repo-local Python entrypoint to run")
    native_run_p.add_argument(
        "script_args",
        nargs=argparse.REMAINDER,
        help="arguments passed to the entrypoint (use -- to separate)",
    )
    native_run_p.set_defaults(
        func=_dispatch_pact_command,
        pact_handler="_cmd_native_molt_run",
    )
    return parser
