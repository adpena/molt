"""Proof queue command handlers and audit/status orchestration."""

from __future__ import annotations

import argparse
import json
import sqlite3
import time
import tomllib
import uuid
from pathlib import Path

from tools.proof_queue_pkg import (
    custody,
    evidence,
    policy,
    runner,
    scheduling,
    state,
)
from tools.proof_queue_pkg import diagnostics as diagnostic_engine


def _cmd_exec(args: argparse.Namespace) -> int:
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    env_overrides = policy._env_overrides_from_pairs(args.env)
    initial_notes = getattr(args, "note", []) or []
    contention_key = args.contention_key or f"{args.resource_family}:default"
    if args.detach:
        rc, run_id = runner._queue_one(
            args,
            logical_id=args.id,
            reason=args.reason,
            command=command,
            resource_family=args.resource_family,
            contention_key=contention_key,
            scopes=args.scope,
            env_overrides=env_overrides,
            initial_notes=initial_notes,
            depends_on=args.depends_on,
            edge_kind=args.edge_kind,
            edge_note=args.edge_note,
        )
        if rc != 0 or run_id is None:
            return rc
        conn = state._connect(state._db_path(args))
        dispatch = runner._dispatch_detached_runner(
            args,
            conn,
            run_id=run_id,
            timeout=args.timeout,
        )
        if dispatch is None:
            return 0
        pid, runner_log = dispatch
        print(f"detached {run_id} runner_pid={pid}")
        print(f"runner_log: {runner_log}")
        return 0
    return runner._run_one(
        args,
        logical_id=args.id,
        reason=args.reason,
        command=command,
        resource_family=args.resource_family,
        contention_key=contention_key,
        scopes=args.scope,
        env_overrides=env_overrides,
        timeout=args.timeout,
        initial_notes=initial_notes,
        depends_on=args.depends_on,
        edge_kind=args.edge_kind,
        edge_note=args.edge_note,
    )



def _cmd_cargo(args: argparse.Namespace) -> int:
    cargo_args = (
        args.cargo_args[1:] if args.cargo_args[:1] == ["--"] else args.cargo_args
    )
    contention_key = args.contention_key or (
        f"cargo:{policy._cargo_package_for_contention(cargo_args)}"
    )
    command = policy._canonical_cargo_proof_command(cargo_args)
    env_overrides = policy._env_overrides_from_pairs(args.env)
    initial_notes = getattr(args, "note", []) or []
    single_lib_test_error = policy._cold_single_lib_test_policy_error(cargo_args)
    policy_error = None if args.allow_warm_single_test else single_lib_test_error
    if args.allow_warm_single_test and single_lib_test_error is not None:
        initial_notes = [
            (
                "policy: --allow-warm-single-test used; submitter asserts the "
                "Cargo target dir is already warm and this exact lib test will "
                "not pay a cold compile."
            ),
            *initial_notes,
        ]
    if args.detach:
        rc, run_id = runner._queue_one(
            args,
            logical_id=args.id,
            reason=args.reason,
            command=command,
            resource_family="rust",
            contention_key=contention_key,
            scopes=args.scope,
            env_overrides=env_overrides,
            initial_notes=initial_notes,
            depends_on=args.depends_on,
            edge_kind=args.edge_kind,
            edge_note=args.edge_note,
            policy_error=policy_error,
        )
        if rc != 0 or run_id is None:
            return rc
        conn = state._connect(state._db_path(args))
        dispatch = runner._dispatch_detached_runner(
            args,
            conn,
            run_id=run_id,
            timeout=args.timeout,
        )
        if dispatch is None:
            return 0
        pid, runner_log = dispatch
        print(f"detached {run_id} runner_pid={pid}")
        print(f"runner_log: {runner_log}")
        return 0
    return runner._run_one(
        args,
        logical_id=args.id,
        reason=args.reason,
        command=command,
        resource_family="rust",
        contention_key=contention_key,
        scopes=args.scope,
        env_overrides=env_overrides,
        timeout=args.timeout,
        initial_notes=initial_notes,
        depends_on=args.depends_on,
        edge_kind=args.edge_kind,
        edge_note=args.edge_note,
        policy_error=policy_error,
    )



def _load_specs(path: Path) -> list[dict[str, object]]:
    with path.open("rb") as handle:
        payload = tomllib.load(handle)
    raw = payload.get("proof", [])
    if isinstance(raw, dict):
        raw = [raw]
    if not isinstance(raw, list):
        raise SystemExit("proof DSL must contain [[proof]] tables")
    specs: list[dict[str, object]] = []
    for entry in raw:
        if not isinstance(entry, dict):
            raise SystemExit("each proof entry must be a table")
        specs.append(entry)
    return specs



def _cmd_submit(args: argparse.Namespace) -> int:
    specs = _load_specs(Path(args.dsl))
    conn = state._connect(state._db_path(args))
    prepared: list[dict[str, object]] = []
    logical_to_run: dict[str, str] = {}
    for spec in specs:
        logical_id = str(spec.get("id") or spec.get("logical_id") or "proof")
        if logical_id in logical_to_run:
            raise SystemExit(f"duplicate proof logical id {logical_id!r}")
        command = spec.get("command")
        if not isinstance(command, list) or not all(
            isinstance(x, str) for x in command
        ):
            raise SystemExit(f"proof {logical_id!r} needs command = [..]")
        policy_error = policy._proof_command_policy_error(list(command))
        if policy_error is not None:
            raise SystemExit(f"proof {logical_id!r}: {policy_error}")
        edge_kind = str(spec.get("edge_kind") or state.DEFAULT_EDGE_KIND)
        if edge_kind not in state.EDGE_KINDS:
            allowed = ", ".join(sorted(state.EDGE_KINDS))
            raise SystemExit(
                f"proof {logical_id!r}: unknown proof edge kind "
                f"{edge_kind!r}; allowed: {allowed}"
            )
        edge_note_raw = spec.get("edge_note")
        if edge_note_raw is not None and not isinstance(edge_note_raw, str):
            raise SystemExit(f"proof {logical_id!r}: edge_note must be a string")
        env_overrides = policy._env_overrides_from_spec(spec.get("env"))
        env_policy_error = policy._proof_env_policy_error(env_overrides)
        if env_policy_error is not None:
            raise SystemExit(f"proof {logical_id!r}: {env_policy_error}")
        initial_notes = state._notes_from_raw(spec.get("note"))
        initial_notes.extend(state._notes_from_raw(spec.get("notes")))
        depends_on = state._dependencies_from_raw(spec.get("depends_on"))
        depends_on.extend(state._dependencies_from_raw(spec.get("after")))
        run_id = f"{state._compact_utc()}-{state._slug(logical_id)}-{uuid.uuid4().hex[:16]}"
        logical_to_run[logical_id] = run_id
        prepared.append(
            {
                "logical_id": logical_id,
                "command": list(command),
                "reason": str(spec.get("reason") or logical_id),
                "resource_family": str(spec.get("resource_family") or "generic"),
                "contention_key": str(spec.get("contention_key") or "generic:default"),
                "scope": [str(x) for x in spec.get("scope", [])],
                "env_overrides": env_overrides,
                "initial_notes": initial_notes,
                "depends_on": depends_on,
                "edge_kind": edge_kind,
                "edge_note": edge_note_raw or "",
                "run_id": run_id,
            }
        )
    planned_edges: set[tuple[str, str, str]] = set()
    planned_children: dict[str, list[str]] = {}
    for item in prepared:
        child = str(item["run_id"])
        for dependency in item["depends_on"]:
            parent = logical_to_run.get(str(dependency), str(dependency))
            if parent == child:
                raise SystemExit(f"proof {item['logical_id']!r}: depends_on itself")
            if parent not in logical_to_run.values() and not state._run_exists(conn, parent):
                raise SystemExit(
                    f"proof {item['logical_id']!r}: unknown dependency {dependency!r}"
                )
            edge = (parent, child, str(item["edge_kind"]))
            if edge in planned_edges:
                raise SystemExit(
                    f"proof {item['logical_id']!r}: duplicate dependency {dependency!r}"
                )
            planned_edges.add(edge)
            planned_children.setdefault(parent, []).append(child)
    for parent, child, _kind in planned_edges:
        if state._planned_edge_would_create_cycle(planned_children, parent, child):
            raise SystemExit(
                "proof DSL dependency graph would create a cycle: "
                f"{parent!r} -> {child!r}"
            )
    for item in prepared:
        run_id = str(item["run_id"])
        log_path = state._logs_root(args) / f"{run_id}.log"
        summary_json = state._logs_root(args) / f"{run_id}.memory_guard.json"
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=str(item["logical_id"]),
            reason=str(item["reason"]),
            command=list(item["command"]),
            cwd=state._repo_root(args),
            resource_family=str(item["resource_family"]),
            contention_key=str(item["contention_key"]),
            scopes=list(item["scope"]),
            env_overrides=dict(item["env_overrides"]),
            log_path=log_path,
            summary_json=summary_json,
        )
    for item in prepared:
        run_id = str(item["run_id"])
        log_path = state._logs_root(args) / f"{run_id}.log"
        try:
            for dependency in item["depends_on"]:
                state._insert_edge(
                    conn,
                    parent_run_id=logical_to_run.get(str(dependency), str(dependency)),
                    child_run_id=run_id,
                    kind=str(item["edge_kind"]),
                    note=str(item["edge_note"]),
                )
            for note in item["initial_notes"]:
                state._insert_note(conn, run_id=run_id, body=note, kind=state.SUBMISSION_NOTE_KIND)
        except Exception as exc:
            return evidence._fail_preexecution_run(
                args,
                conn,
                run_id=run_id,
                logical_id=str(item["logical_id"]),
                reason=str(item["reason"]),
                repo_root=state._repo_root(args),
                command=list(item["command"]),
                log_path=log_path,
                exc=exc,
                phase="submission metadata",
            )
        evidence._write_queued_submission_log(
            log_path,
            run_id=run_id,
            logical_id=str(item["logical_id"]),
            reason=str(item["reason"]),
            repo_root=state._repo_root(args),
            command=list(item["command"]),
            resource_family=str(item["resource_family"]),
            contention_key=str(item["contention_key"]),
            scopes=list(item["scope"]),
            env_overrides=dict(item["env_overrides"]),
            depends_on=[str(dependency) for dependency in item["depends_on"]],
        )
        if item["initial_notes"] or item["depends_on"]:
            evidence._try_write_marimo_notebook(
                args,
                conn,
                run_id,
                log_path=log_path,
                phase="submission projection",
            )
        print(f"queued {run_id}")
    return 0



def _cmd_run(args: argparse.Namespace) -> int:
    runner._ensure_disk_headroom_before_build()
    conn = state._connect(state._db_path(args))
    conn.row_factory = sqlite3.Row
    queue_size = state._configured_queue_size(getattr(args, "queue_size", None))
    run_limit = state._configured_run_limit(args, queue_size=queue_size)
    active_running = scheduling._active_running_rows(conn)
    active_keys = {
        str(row["contention_key"])
        for row in active_running
        if not (args.run_id is not None and row["run_id"] == args.run_id)
    }
    active_mutexes = {
        mutex
        for row in active_running
        if not (args.run_id is not None and row["run_id"] == args.run_id)
        for mutex in (
            state._resource_mutex_key(
                resource_family=str(row["resource_family"]),
                contention_key=str(row["contention_key"]),
                command=scheduling._row_command(row),
            ),
        )
        if mutex is not None
    }
    if args.run_id:
        selected = conn.execute(
            "SELECT * FROM proof_runs WHERE run_id = ?",
            (args.run_id,),
        ).fetchone()
        if selected is None:
            raise SystemExit(f"unknown proof run {args.run_id!r}")
        allowed_statuses = {"queued"} if args.detach else state.DETACHED_READY_STATUSES
        if selected["status"] not in allowed_statuses:
            allowed = ", ".join(sorted(allowed_statuses))
            raise SystemExit(
                f"proof run {args.run_id!r} is {selected['status']}, not {allowed}"
            )
        queued = [selected]
        selection_limit = 1
    else:
        queued = list(
            conn.execute(
                "SELECT * FROM proof_runs WHERE status = 'queued' ORDER BY rowid"
            )
        )
        available_slots = max(0, queue_size - len(active_running))
        if available_slots <= 0:
            print(
                f"queue capacity full active={len(active_running)} "
                f"queue_size={queue_size}"
            )
            selection_limit = 0
        elif args.detach:
            selection_limit = min(run_limit, available_slots)
        else:
            selection_limit = run_limit
    rows = []
    selected_detached_keys = set(active_keys)
    selected_detached_mutexes = set(active_mutexes)
    candidate_rows = queued if selection_limit > 0 else []
    for row in candidate_rows:
        contention_key = str(row["contention_key"])
        mutex_key = state._resource_mutex_key(
            resource_family=str(row["resource_family"]),
            contention_key=contention_key,
            command=scheduling._row_command(row),
        )
        if contention_key in active_keys:
            print(f"waiting {row['run_id']} contention_key={contention_key} active")
            continue
        if mutex_key is not None and mutex_key in active_mutexes:
            print(f"waiting {row['run_id']} resource_mutex={mutex_key} active")
            continue
        if args.detach and contention_key in selected_detached_keys:
            print(
                f"waiting {row['run_id']} contention_key={contention_key} "
                "already selected"
            )
            continue
        if (
            args.detach
            and mutex_key is not None
            and mutex_key in selected_detached_mutexes
        ):
            print(
                f"waiting {row['run_id']} resource_mutex={mutex_key} already selected"
            )
            continue
        dependency_state, blockers = scheduling._dependency_state(conn, row["run_id"])
        if dependency_state == "ready":
            rows.append(row)
            if args.detach:
                selected_detached_keys.add(contention_key)
                if mutex_key is not None:
                    selected_detached_mutexes.add(mutex_key)
            if args.run_id or len(rows) >= selection_limit:
                break
            continue
        blocker_summary = scheduling._blocker_summary(blockers)
        if dependency_state == "waiting":
            print(f"waiting {row['run_id']} parents={blocker_summary}")
            continue
        scheduling._mark_queued_dependency_blocked(
            args,
            conn,
            row,
            blockers,
            announce=True,
        )
    rc = 0
    for row in rows:
        payload = evidence._row_to_payload(row)
        if args.detach:
            dispatch = runner._dispatch_detached_runner(
                args,
                conn,
                run_id=str(payload["run_id"]),
                timeout=args.timeout,
            )
            if dispatch is None:
                continue
            pid, runner_log = dispatch
            print(f"detached {payload['run_id']} runner_pid={pid}")
            print(f"runner_log: {runner_log}")
            continue
        rc = runner._run_one(
            args,
            logical_id=str(payload["logical_id"]),
            reason=str(payload["reason"]),
            command=list(payload["command"]),
            resource_family=str(payload["resource_family"]),
            contention_key=str(payload["contention_key"]),
            scopes=list(payload["scopes"]),
            env_overrides=dict(payload["env"]),
            timeout=args.timeout,
            existing_run_id=str(payload["run_id"]),
            existing_log_path=Path(str(payload["log_path"])),
            existing_summary_json=Path(str(payload["summary_json"])),
        )
        if rc != 0:
            break
    if not queued:
        print("no queued proofs")
    elif not rows:
        print("no queued proofs ready")
    return rc



def _cmd_status(args: argparse.Namespace) -> int:
    conn = state._connect(state._db_path(args))
    conn.row_factory = sqlite3.Row
    scheduling._refresh_blocked_queued_runs(args, conn)
    active = list(
        conn.execute(
            f"SELECT * FROM proof_runs WHERE status IN ({state.ACTIVE_SQL_STATUSES}) "
            "ORDER BY started_at"
        )
    )
    recent = list(
        conn.execute(
            f"SELECT * FROM proof_runs WHERE status NOT IN ({state.ACTIVE_SQL_STATUSES}) "
            "ORDER BY finished_at DESC LIMIT ?",
            (args.recent,),
        )
    )
    notes_by_run = state._notes_for_run_ids(
        conn, [row["run_id"] for row in [*active, *recent]]
    )
    edges_by_run = state._edges_for_run_ids(
        conn, [row["run_id"] for row in [*active, *recent]]
    )
    print("proof queue")
    print("active:")
    if not active:
        print("- none")
    for row in active:
        elapsed = f" elapsed={diagnostic_engine._elapsed_since(row['started_at'], row['elapsed_s'])}"
        print(f"- {row['status']}{elapsed} {row['run_id']} {row['reason']}")
        note_summary = state._format_note_summary(notes_by_run.get(row["run_id"], []))
        if note_summary:
            print(note_summary)
        dag_summary = state._format_dag_summary(
            edges_by_run.get(row["run_id"], {"parents": [], "children": []})
        )
        if dag_summary:
            print(dag_summary)
        diagnostic_engine._print_status_diagnostics(row)
        for line in diagnostic_engine._active_log_status(row):
            print(line)
    print("recent:")
    if not recent:
        print("- none")
    for row in recent:
        rc = "?" if row["returncode"] is None else row["returncode"]
        elapsed = "?" if row["elapsed_s"] is None else f"{row['elapsed_s']:.1f}s"
        print(
            f"- {row['status']:9} rc={rc} elapsed={elapsed} {row['run_id']} {row['reason']}"
        )
        note_summary = state._format_note_summary(notes_by_run.get(row["run_id"], []))
        if note_summary:
            print(note_summary)
        dag_summary = state._format_dag_summary(
            edges_by_run.get(row["run_id"], {"parents": [], "children": []})
        )
        if dag_summary:
            print(dag_summary)
        diagnostic_engine._print_status_diagnostics(row)
    return 0



def _cmd_prune_stale(args: argparse.Namespace) -> int:
    conn = state._connect(state._db_path(args))
    conn.row_factory = sqlite3.Row
    run_ids = tuple(dict.fromkeys(args.run_id or ()))
    scheduling._refresh_blocked_queued_runs(
        args,
        conn,
        run_ids=run_ids or None,
        announce=True,
    )
    if run_ids:
        placeholders = ",".join("?" for _ in run_ids)
        known = {
            str(row["run_id"])
            for row in conn.execute(
                f"SELECT run_id FROM proof_runs WHERE run_id IN ({placeholders})",
                run_ids,
            )
        }
        missing = [run_id for run_id in run_ids if run_id not in known]
        if missing:
            raise SystemExit("unknown proof run(s): " + ", ".join(missing))
        rows = list(
            conn.execute(
                "SELECT * FROM proof_runs "
                f"WHERE status IN ({state.ACTIVE_OR_STALE_SQL_STATUSES}) "
                f"AND run_id IN ({placeholders}) "
                "ORDER BY started_at",
                run_ids,
            )
        )
    else:
        rows = list(
            conn.execute(
                "SELECT * FROM proof_runs "
                f"WHERE status IN ({state.ACTIVE_SQL_STATUSES}) ORDER BY started_at"
            )
        )
    pruned = 0
    for row in rows:
        if row["status"] == "queued":
            if run_ids:
                state._update_run(
                    conn,
                    row["run_id"],
                    status="stale",
                    returncode=custody.PROOF_QUEUE_STALE_EXIT_CODE,
                    finished_at=state._utc_now(),
                )
                pruned += 1
                print(
                    f"stale {row['run_id']} diagnosis=selected-queued-cancellation "
                    "[infra]: explicitly selected queued row never acquired "
                    "process or resource custody"
                )
            continue
        if row["status"] == "dispatched":
            dispatch_age = diagnostic_engine._running_age_seconds(state._row_value(row, "started_at"))
            if (
                dispatch_age is None
                or dispatch_age < custody.PROOF_QUEUE_DISPATCH_STALE_SECONDS
            ):
                continue
            state._update_run(
                conn,
                row["run_id"],
                status="stale",
                returncode=custody.PROOF_QUEUE_STALE_EXIT_CODE,
                finished_at=state._utc_now(),
            )
            pruned += 1
            runner_log = Path(str(row["log_path"])).with_name(
                f"{row['run_id']}.runner.log"
            )
            print(
                f"stale {row['run_id']} diagnosis=dispatch-handoff-expired "
                "[infra]: detached runner did not claim the dispatched row "
                f"within {custody.PROOF_QUEUE_DISPATCH_STALE_SECONDS:.0f}s "
                f"artifacts={runner_log}, {row['log_path']}"
            )
            continue
        if row["status"] == "stale":
            if row["returncode"] is None:
                state._update_run(
                    conn,
                    row["run_id"],
                    returncode=custody.PROOF_QUEUE_STALE_EXIT_CODE,
                )
                pruned += 1
                diagnostics = diagnostic_engine._run_diagnostics(
                    state._row_by_run_id(conn, str(row["run_id"])) or row
                )
                diagnostic_summary = diagnostic_engine._format_diagnostic_summary(diagnostics)
                if diagnostic_summary is None:
                    diagnostic_summary = (
                        "already-stale [infra]: canonicalized stale returncode"
                    )
                print(
                    f"stale {row['run_id']} diagnosis={diagnostic_summary} "
                    f"returncode={custody.PROOF_QUEUE_STALE_EXIT_CODE}"
                )
            continue
        pid = row["guard_pid"]
        recorded_identity = state._row_value(row, "guard_identity")
        recorded_identity_text = (
            str(recorded_identity) if recorded_identity is not None else None
        )
        guard_alive = pid is not None and custody._guard_process_live(
            int(pid), recorded_identity_text
        )
        running_age = diagnostic_engine._running_age_seconds(state._row_value(row, "started_at"))
        age_exceeded = (
            running_age is not None
            and running_age > custody.PROOF_QUEUE_RUNNING_AGE_CEILING_SECONDS
        )
        diagnostics = diagnostic_engine._run_diagnostics(row)
        if (
            guard_alive
            and not age_exceeded
            and not diagnostic_engine._diagnostics_have_terminal_stale_signal(diagnostics)
        ):
            continue
        state._update_run(
            conn,
            row["run_id"],
            status="stale",
            returncode=custody.PROOF_QUEUE_STALE_EXIT_CODE,
            finished_at=state._utc_now(),
        )
        pruned += 1
        diagnostic_summary = diagnostic_engine._format_diagnostic_summary(diagnostics)
        if diagnostic_summary is None:
            if not guard_alive and pid is not None and custody._pid_alive(int(pid)):
                # PID is alive but no longer the recorded guard: Windows recycled
                # the PID to an unrelated process while our guard was already
                # dead.
                diagnostic_summary = (
                    "reused-guard-pid [infra]: guard PID belongs to a different "
                    "process (PID reuse); original proof guard is gone"
                )
            elif guard_alive and age_exceeded:
                diagnostic_summary = (
                    "running-age-ceiling [infra]: proof exceeded the "
                    f"{custody.PROOF_QUEUE_RUNNING_AGE_CEILING_SECONDS:.0f}s running-age "
                    "ceiling without a terminal write"
                )
            else:
                diagnostic_summary = (
                    "dead-guard [infra]: proof guard process is not live"
                )
        line = f"stale {row['run_id']} diagnosis={diagnostic_summary}"
        if diagnostics:
            evidence = diagnostics[0].get("evidence")
            if isinstance(evidence, str) and evidence.strip():
                line += f" evidence={state._shorten(evidence, 220)}"
        artifacts = diagnostic_engine._diagnostic_artifacts(diagnostics)
        if not artifacts:
            artifacts = [str(row["summary_json"]), str(row["log_path"])]
        print(f"{line} artifacts={', '.join(artifacts)}")
    print(f"pruned={pruned}")
    return 0



def _cmd_evidence(args: argparse.Namespace) -> int:
    run_id = args.run_id or args.run_id_option
    if args.run_id and args.run_id_option and args.run_id != args.run_id_option:
        raise SystemExit("pass one proof run id: positional and --run-id disagree")
    conn = state._connect(state._db_path(args))
    conn.row_factory = sqlite3.Row
    if run_id:
        scheduling._refresh_blocked_queued_runs(args, conn, run_ids=[run_id])
    else:
        scheduling._refresh_blocked_queued_runs(args, conn)
    if run_id:
        rows = list(
            conn.execute("SELECT * FROM proof_runs WHERE run_id = ?", (run_id,))
        )
        if not rows:
            raise SystemExit(f"unknown proof run id {run_id!r}")
    else:
        rows = list(
            conn.execute(
                "SELECT * FROM proof_runs ORDER BY rowid DESC LIMIT ?", (args.limit,)
            )
        )
    payload = evidence._run_payload_with_notes(conn, rows)
    text = json.dumps(payload, indent=2, sort_keys=True)
    if args.output:
        Path(args.output).write_text(text + "\n", encoding="utf-8")
    else:
        print(text)
    return 0



def _queue_audit_payload(args: argparse.Namespace) -> dict[str, object]:
    conn = state._connect(state._db_path(args))
    conn.row_factory = sqlite3.Row
    scheduling._refresh_blocked_queued_runs(args, conn)
    rows = diagnostic_engine._audit_rows(conn, args)
    run_ids = [str(row["run_id"]) for row in rows]
    notes_by_run = state._notes_for_run_ids(conn, run_ids)
    edges_by_run = state._edges_for_run_ids(conn, run_ids)
    issues: list[dict[str, object]] = []
    frontier_failures: list[dict[str, object]] = []
    diagnostic_counts: dict[str, int] = {}
    classified_failed_runs = 0
    superseded_archaeology_runs = 0

    active_by_key: dict[str, list[str]] = {}
    for row in rows:
        if row["status"] in state.LAUNCHED:
            active_by_key.setdefault(str(row["contention_key"]), []).append(
                str(row["run_id"])
            )
    for key, keyed_run_ids in sorted(active_by_key.items()):
        if len(keyed_run_ids) <= 1:
            continue
        issues.append(
            diagnostic_engine._audit_issue(
                signal_id="queue-contention-duplicate",
                severity="error",
                summary=f"Multiple active rows share contention key {key!r}.",
                evidence=", ".join(keyed_run_ids),
                next_action=(
                    "Inspect the rows before launching more work; prune stale rows "
                    "or fix queue admission if more than one live row owns the key."
                ),
            )
        )

    for row in rows:
        run_id = str(row["run_id"])
        status = str(row["status"])
        notes = notes_by_run.get(run_id, [])
        dag = edges_by_run.get(run_id, {"parents": [], "children": []})
        superseded_terminal_row = (
            status not in state.RUNNING and not args.all and diagnostic_engine._frontier_superseded(dag)
        )
        if superseded_terminal_row:
            superseded_archaeology_runs += 1
            continue

        diagnostics = diagnostic_engine._run_diagnostics(row)
        for item in diagnostics:
            signal_id = str(item["signal_id"])
            diagnostic_counts[signal_id] = diagnostic_counts.get(signal_id, 0) + 1

        if status == "failed":
            if (
                any(
                    str(item["signal_id"]) == "unclassified-failed-proof"
                    for item in diagnostics
                )
                and not superseded_terminal_row
            ):
                issues.append(
                    diagnostic_engine._audit_issue(
                        signal_id="audit-unclassified-failure",
                        severity="error",
                        run_id=run_id,
                        summary="Failed proof row has no deterministic diagnostic.",
                        evidence=diagnostic_engine._format_diagnostic_summary(diagnostics) or "",
                        next_action=(
                            "Inspect the log once and add a queue diagnostic rule "
                            "before this failure pattern becomes tribal knowledge."
                        ),
                    )
                )
            elif diagnostics:
                classified_failed_runs += 1
                if not superseded_terminal_row:
                    frontier = diagnostic_engine._frontier_failure(row, diagnostics)
                    if frontier is not None:
                        frontier_failures.append(frontier)

        for item in diagnostics:
            signal_id = str(item["signal_id"])
            severity = str(item["severity"])
            audit_severity = diagnostic_engine._audit_severity_for_diagnostic(row, signal_id)
            if audit_severity is not None:
                issues.append(
                    diagnostic_engine._audit_issue(
                        signal_id=f"audit-{signal_id}",
                        severity=audit_severity,
                        run_id=run_id,
                        summary=str(item["summary"]),
                        evidence=str(item["evidence"]),
                        next_action=str(item["next_action"]),
                        artifacts=[str(path) for path in item.get("artifacts", [])]
                        if isinstance(item.get("artifacts"), list)
                        else (),
                    )
                )
            elif severity == "unknown" and signal_id != "unclassified-failed-proof":
                issues.append(
                    diagnostic_engine._audit_issue(
                        signal_id="audit-unknown-diagnostic",
                        severity="error",
                        run_id=run_id,
                        summary=str(item["summary"]),
                        evidence=str(item["evidence"]),
                        next_action=str(item["next_action"]),
                    )
                )

        metadata_defects: list[str] = []
        try:
            scopes = json.loads(row["scopes_json"])
        except (TypeError, json.JSONDecodeError):
            scopes = None
        if not isinstance(scopes, list) or not scopes:
            metadata_defects.append("missing scopes")
        if str(row["resource_family"]) == "generic":
            metadata_defects.append("resource_family=generic")
        if str(row["contention_key"]) == "generic:default":
            metadata_defects.append("contention_key=generic:default")
        reason = str(row["reason"]).strip()
        if not reason or reason.startswith(('"', "'")):
            metadata_defects.append(f"suspicious reason={reason!r}")
        if metadata_defects:
            issues.append(
                diagnostic_engine._audit_issue(
                    signal_id="audit-weak-proof-metadata",
                    severity="warning",
                    run_id=run_id,
                    summary="Proof row metadata is too weak for durable evidence.",
                    evidence="; ".join(metadata_defects),
                    next_action=(
                        "Use a scoped resource family, contention key, reason, "
                        "scope, and note; if shell quoting caused this row, rerun "
                        "through the delimiter-guarded queue shape."
                    ),
                )
            )
        if not notes:
            issues.append(
                diagnostic_engine._audit_issue(
                    signal_id="audit-missing-proof-note",
                    severity="warning",
                    run_id=run_id,
                    summary="Proof row has no append-only note.",
                    evidence=f"reason={row['reason']}",
                    next_action=(
                        "Append a note describing what changed, what was tested "
                        "or explored, and why before citing this row as evidence."
                    ),
                )
            )

        if not args.no_notebook_check and evidence._notebook_projection_expected(
            notes=notes, dag=dag
        ):
            notebook_path = evidence._notebooks_root(args) / f"{run_id}.py"
            if not notebook_path.exists():
                issues.append(
                    diagnostic_engine._audit_issue(
                        signal_id="audit-notebook-missing",
                        severity="warning",
                        run_id=run_id,
                        summary="Run has notes or DAG edges but no notebook projection.",
                        evidence=str(notebook_path),
                        next_action=(
                            "Regenerate the projection with `tools/proof_queue.py "
                            f"notebook {run_id}`; the SQLite row remains the source "
                            "of truth."
                        ),
                    )
                )

        if status != "running":
            continue
        pid = row["guard_pid"]
        if pid is None or not custody._pid_alive(int(pid)):
            issues.append(
                diagnostic_engine._audit_issue(
                    signal_id="audit-dead-running-guard",
                    severity="error",
                    run_id=run_id,
                    summary="Running proof row has no live guard process.",
                    evidence=f"guard_pid={pid}",
                    next_action=(
                        "Inspect the queue log and memory-guard summary, then use "
                        "`prune-stale` if the row is truly dead."
                    ),
                )
            )
        try:
            stat = Path(row["log_path"]).stat()
        except OSError:
            if row["status"] == "queued":
                continue
            issues.append(
                diagnostic_engine._audit_issue(
                    signal_id="audit-active-log-missing",
                    severity="error",
                    run_id=run_id,
                    summary="Running proof row log is missing.",
                    evidence=str(row["log_path"]),
                    next_action=(
                        "Treat the row as incomplete evidence; inspect guard "
                        "state before pruning or rerunning."
                    ),
                )
            )
            continue
        age_s = max(0.0, time.time() - stat.st_mtime)
        if age_s > args.stale_log_seconds:
            issues.append(
                diagnostic_engine._audit_issue(
                    signal_id="audit-active-log-stale",
                    severity="warning",
                    run_id=run_id,
                    summary=(
                        "Running proof row has not updated its log within the "
                        "stale-log window."
                    ),
                    evidence=f"last_log_age={diagnostic_engine._format_duration(age_s)}",
                    next_action=(
                        "Inspect the queue log and memory-guard summary; avoid "
                        "interactive interrupts and prefer bounded timeout or "
                        "proof-queue custody."
                    ),
                )
            )

    severity_counts: dict[str, int] = {}
    for issue in issues:
        severity = str(issue["severity"])
        severity_counts[severity] = severity_counts.get(severity, 0) + 1

    return {
        "scanned_runs": len(rows),
        "active_runs": sum(1 for row in rows if row["status"] in state.LAUNCHED),
        "superseded_archaeology_runs": superseded_archaeology_runs,
        "classified_failed_runs": classified_failed_runs,
        "frontier_failures": frontier_failures,
        "diagnostic_counts": {
            key: diagnostic_counts[key] for key in sorted(diagnostic_counts)
        },
        "issue_counts": {key: severity_counts[key] for key in sorted(severity_counts)},
        "issues": issues,
    }



def _cmd_audit(args: argparse.Namespace) -> int:
    payload = _queue_audit_payload(args)
    text = json.dumps(payload, indent=2, sort_keys=True)
    if args.output:
        Path(args.output).write_text(text + "\n", encoding="utf-8")
    if args.json:
        print(text)
    else:
        print("proof queue audit")
        print(
            f"scanned={payload['scanned_runs']} active={payload['active_runs']} "
            f"classified_failed={payload['classified_failed_runs']} "
            f"issues={len(payload['issues'])}"
        )
        if payload["superseded_archaeology_runs"]:
            print(
                "archaeology: "
                f"superseded_terminal={payload['superseded_archaeology_runs']}"
            )
        diagnostics = payload["diagnostic_counts"]
        if diagnostics:
            print(
                "diagnostics: "
                + ", ".join(f"{key}={diagnostics[key]}" for key in sorted(diagnostics))
            )
        if payload["issue_counts"]:
            print(
                "issue_severity: "
                + ", ".join(
                    f"{key}={payload['issue_counts'][key]}"
                    for key in sorted(payload["issue_counts"])
                )
            )
        frontier_failures = payload["frontier_failures"]
        if frontier_failures:
            print("frontier:")
            for item in frontier_failures[:5]:
                print(f"- {item['diagnostic']} run={item['run_id']}: {item['summary']}")
                print(f"  log: {item['log_path']}")
                print(f"  next: {item['next_action']}")
            hidden_frontier = len(frontier_failures) - min(5, len(frontier_failures))
            if hidden_frontier > 0:
                print(
                    f"- showing 5 of {len(frontier_failures)} frontier failures; "
                    "use --json or --output for the complete payload"
                )
        raw_issues = list(payload["issues"])
        if args.errors_only:
            issues_source = [
                issue for issue in raw_issues if str(issue["severity"]) == "error"
            ]
        else:
            issues_source = raw_issues
        if not issues_source:
            if args.errors_only:
                print("- no error queue health issues")
            else:
                print("- no queue health issues")
        max_issues = max(0, int(args.max_issues))
        issues = issues_source if max_issues == 0 else issues_source[:max_issues]
        for issue in issues:
            run = f" run={issue['run_id']}" if issue.get("run_id") else ""
            print(
                f"- {issue['severity']} {issue['signal_id']}{run}: {issue['summary']}"
            )
            if issue["evidence"]:
                print(f"  evidence: {issue['evidence']}")
            artifacts = issue.get("artifacts", [])
            if artifacts:
                print(f"  artifacts: {', '.join(str(path) for path in artifacts)}")
            print(f"  next: {issue['next_action']}")
        if args.errors_only:
            hidden_warnings = len(raw_issues) - len(issues_source)
            if hidden_warnings > 0:
                print(
                    f"- hidden {hidden_warnings} warning issue(s) due to "
                    "--errors-only; use full audit, --json, or --output for "
                    "the complete payload"
                )
        hidden = len(issues_source) - len(issues)
        if hidden > 0:
            print(
                f"- showing {len(issues)} of {len(issues_source)} issues; "
                "use --max-issues 0, --json, or --output for the complete payload"
            )

    error_count = int(payload["issue_counts"].get("error", 0))
    warning_count = int(payload["issue_counts"].get("warning", 0))
    if error_count or (args.strict and warning_count):
        return 1
    return 0



def _cmd_diagnose(args: argparse.Namespace) -> int:
    conn = state._connect(state._db_path(args))
    scheduling._refresh_blocked_queued_runs(
        args,
        conn,
        run_ids=[args.run_id] if args.run_id else None,
    )
    row = diagnostic_engine._diagnose_row(conn, args)
    diagnostics = diagnostic_engine._run_diagnostics(row)
    payload = evidence._row_to_payload(row)
    payload["diagnostics"] = diagnostics
    if args.output:
        Path(args.output).write_text(
            json.dumps(payload, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        rc = "?" if row["returncode"] is None else row["returncode"]
        print(
            f"diagnosis {row['run_id']} status={row['status']} rc={rc} "
            f"log={row['log_path']}"
        )
        if not diagnostics:
            print("- no diagnostic signals")
        for item in diagnostics:
            print(f"- {item['signal_id']} [{item['severity']}] {item['summary']}")
            if item["evidence"]:
                print(f"  evidence: {item['evidence']}")
            artifacts = item.get("artifacts", [])
            if isinstance(artifacts, list) and artifacts:
                print(f"  artifacts: {', '.join(str(path) for path in artifacts)}")
            print(f"  next: {item['next_action']}")
    if args.append_note:
        note_id = state._insert_note(
            conn,
            run_id=row["run_id"],
            body=diagnostic_engine._diagnosis_note_body(row, diagnostics),
            kind=args.kind,
            author=args.author,
        )
        print(f"noted {row['run_id']} note_id={note_id}")
        if not args.no_notebook:
            path = evidence._try_write_marimo_notebook(
                args,
                conn,
                row["run_id"],
                log_path=Path(row["log_path"]),
                phase="diagnosis projection",
            )
            if path is not None:
                print(f"notebook: {path}")
    return 0



def _cmd_note(args: argparse.Namespace) -> int:
    conn = state._connect(state._db_path(args))
    note_ids = []
    for body in args.note:
        note_ids.append(
            state._insert_note(
                conn,
                run_id=args.run_id,
                body=body,
                kind=args.kind,
                author=args.author,
            )
        )
    notebook_path = None
    if not args.no_notebook:
        notebook_path = evidence._try_write_marimo_notebook(
            args,
            conn,
            args.run_id,
            log_path=state._log_path_for_run(conn, args.run_id),
            phase="note projection",
            output=args.output,
        )
    print(
        f"noted {args.run_id} note_ids={','.join(str(note_id) for note_id in note_ids)}"
    )
    if notebook_path is not None:
        print(f"notebook: {notebook_path}")
    return 0



def _cmd_link(args: argparse.Namespace) -> int:
    conn = state._connect(state._db_path(args))
    edge_id = state._insert_edge(
        conn,
        parent_run_id=args.parent,
        child_run_id=args.child_run_id,
        kind=args.kind,
        note=args.note,
        author=args.author,
    )
    notebook_paths = []
    if not args.no_notebook:
        for run_id in (args.parent, args.child_run_id):
            path = evidence._try_write_marimo_notebook(
                args,
                conn,
                run_id,
                log_path=state._log_path_for_run(conn, run_id),
                phase="link projection",
            )
            if path is not None:
                notebook_paths.append(path)
    print(
        f"linked {args.parent} -> {args.child_run_id} "
        f"kind={args.kind} edge_id={edge_id}"
    )
    for path in notebook_paths:
        print(f"notebook: {path}")
    return 0



def _cmd_notebook(args: argparse.Namespace) -> int:
    conn = state._connect(state._db_path(args))
    scheduling._refresh_blocked_queued_runs(args, conn, run_ids=[args.run_id])
    path = evidence._write_marimo_notebook(args, conn, args.run_id, args.output)
    print(f"notebook: {path}")
    return 0
