# Monty Compatibility Test Suite

These files are sourced from pydantic/monty (crates/monty/test_cases/).
They are Python programs that produce deterministic output and are used
to verify Molt's CPython parity on the shared Python subset.

Each file contains a self-contained Python program. The expected output
is determined by running the file through CPython 3.12+.

Source: https://github.com/pydantic/monty
License: MIT (see Monty repository)

To update: molt harness complete-tests
