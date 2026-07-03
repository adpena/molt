"""Proof-queue argument parser (moved from tools/proof_queue.py).

``_build_parser`` wires the argparse subcommands onto the ``_cmd_*``
handlers that remain in the facade module ``pq``, reached through ``pq.``.
"""

from __future__ import annotations

import argparse

from tools.proof_queue_pkg import pq


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
    pq._add_dependency_args(exec_p)
    exec_p.add_argument("--timeout", type=float, default=1200.0)
    exec_p.add_argument("--detach", action="store_true")
    exec_p.add_argument("command", nargs=argparse.REMAINDER)
    exec_p.set_defaults(func=pq._cmd_exec)

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
    pq._add_dependency_args(cargo_p)
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
    cargo_p.set_defaults(func=pq._cmd_cargo)

    submit_p = sub.add_parser("submit", help="submit proof specs from a TOML DSL")
    submit_p.add_argument("dsl")
    submit_p.set_defaults(func=pq._cmd_submit)

    run_p = sub.add_parser("run", help="run queued proof specs")
    run_p.add_argument("--limit", type=int, default=1)
    run_p.add_argument("--run-id")
    run_p.add_argument("--timeout", type=float, default=1200.0)
    run_p.add_argument("--detach", action="store_true")
    run_p.set_defaults(func=pq._cmd_run)

    status_p = sub.add_parser("status", help="show active and recent proof runs")
    status_p.add_argument("--recent", type=int, default=20)
    status_p.set_defaults(func=pq._cmd_status)

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
    evidence_p.set_defaults(func=pq._cmd_evidence)

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
    audit_p.set_defaults(func=pq._cmd_audit)

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
        choices=sorted(pq.NOTE_KINDS),
        help="note kind used with --append-note",
    )
    diagnose_p.add_argument("--author")
    diagnose_p.add_argument("--no-notebook", action="store_true")
    diagnose_p.set_defaults(func=pq._cmd_diagnose)

    note_p = sub.add_parser("note", help="append an immutable note to a proof run")
    note_p.add_argument("run_id")
    note_p.add_argument("--note", action="append", required=True)
    note_p.add_argument(
        "--kind",
        default=pq.DEFAULT_NOTE_KIND,
        choices=sorted(pq.NOTE_KINDS),
        help="canonical note lane for append-only collaboration",
    )
    note_p.add_argument("--author")
    note_p.add_argument("--output")
    note_p.add_argument("--no-notebook", action="store_true")
    note_p.set_defaults(func=pq._cmd_note)

    link_p = sub.add_parser(
        "link", help="append an immutable proof DAG edge between existing runs"
    )
    link_p.add_argument("child_run_id")
    link_p.add_argument("--parent", required=True)
    link_p.add_argument(
        "--kind",
        default=pq.DEFAULT_EDGE_KIND,
        choices=sorted(pq.EDGE_KINDS),
        help="canonical proof DAG edge kind",
    )
    link_p.add_argument("--note")
    link_p.add_argument("--author")
    link_p.add_argument("--no-notebook", action="store_true")
    link_p.set_defaults(func=pq._cmd_link)

    notebook_p = sub.add_parser(
        "notebook", help="write the deterministic marimo notebook for a proof run"
    )
    notebook_p.add_argument("run_id")
    notebook_p.add_argument("--output")
    notebook_p.set_defaults(func=pq._cmd_notebook)

    prune_p = sub.add_parser("prune-stale", help="mark dead running records stale")
    prune_p.add_argument(
        "--run-id",
        action="append",
        help="Limit pruning to one queued/running proof row; repeat for several rows.",
    )
    prune_p.set_defaults(func=pq._cmd_prune_stale)

    quickstart_p = sub.add_parser(
        "quickstart", help="print canonical queue muscle memory"
    )
    quickstart_p.set_defaults(func=pq._cmd_quickstart)

    template_p = sub.add_parser("template", help="print a proof DSL template")
    template_p.set_defaults(func=pq._cmd_template)

    cargo_template_p = sub.add_parser(
        "cargo-template", help="print the canonical Cargo proof command shape"
    )
    cargo_template_p.set_defaults(func=pq._cmd_cargo_template)

    pact_accept_p = sub.add_parser(
        "pact-witness-acceptance",
        help="run the queue-owned Pact Kernel A browser/WASM acceptance aperture",
    )
    pq._add_named_lane_args(
        pact_accept_p,
        note_help="append submission context to the acceptance run",
    )
    pact_accept_p.set_defaults(func=pq._cmd_pact_witness_acceptance)

    pact_oracle_p = sub.add_parser(
        "pact-witness-oracle",
        help="run the queued Pact Kernel A fixture/reference parity oracle",
    )
    pq._add_named_lane_args(
        pact_oracle_p,
        note_help="append submission context to the oracle run",
    )
    pact_oracle_p.set_defaults(func=pq._cmd_pact_witness_oracle)

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
    pq._add_named_lane_args(
        r6_parity_p,
        note_help="append submission context to the R6 parity run",
    )
    r6_parity_p.set_defaults(func=pq._cmd_r6_target_version_parity)
    return parser
