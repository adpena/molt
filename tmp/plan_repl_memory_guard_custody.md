# REPL Memory-Guard Custody Plan

## Design

`src/molt/repl.py` advertises `molt repl`, but the CLI does not currently expose
the command and the REPL snippet execution path calls `subprocess.run` directly.
That bypasses the adaptive memory guard, orphan cleanup, canonical artifact roots,
and the subprocess guard audit.

The structural fix is to promote guarded subprocess execution into a reusable
`molt.process_guard` module, keep the existing CLI helper APIs as compatibility
wrappers, and wire REPL snippet execution through the same guard contract with a
dedicated `MOLT_REPL` prefix. Snippet temp files should live under
`<MOLT_EXT_ROOT or cwd>/tmp/repl` rather than the host temp root.

## Files

- `src/molt/process_guard.py`: shared CLI/dev subprocess guard helper.
- `src/molt/cli.py`: delegate existing memory-guard helper internals and expose
  `molt repl`.
- `src/molt/repl.py`: remove raw subprocess execution, add canonical temp root,
  fix readline absence, and use `MOLT_REPL` guarded execution.
- `tools/check_subprocess_guard_coverage.py`: include `src/molt/repl.py` in the
  audited default surface.
- `tests/test_repl_process_guard.py`: focused REPL custody and timeout tests.
- `tests/cli/test_cli_setup_validate.py`: CLI parser/handler regression.

## Tests

- `python3 -m pytest tests/test_repl_process_guard.py tests/cli/test_cli_setup_validate.py -k "repl or subprocess_guard" -q`
- `python3 tools/check_subprocess_guard_coverage.py`
- `python3 tools/check_memory_guard_wiring.py`
- Minimal CLI smoke: `python3 -m molt.cli repl --help`

## Exit Criteria

- No raw REPL subprocess call remains.
- `molt repl` is a first-class CLI command.
- REPL subprocesses use `MOLT_REPL` memory guard defaults and respect
  `MOLT_REPL_TIMEOUT_SEC`.
- Subprocess coverage audit scans the REPL surface by default.
