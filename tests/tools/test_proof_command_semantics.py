"""Execution obligations use typed command boundaries, never incidental tokens."""

from __future__ import annotations

import sys

import pytest

from tools.proof_queue_pkg import command_admission, command_identity, policy


@pytest.mark.parametrize(
    "arguments,selector,subcommand,positionals,forwarded",
    [
        (["+nightly", "test"], "nightly", "test", (), ()),
        (
            [
                "+1.96.1-x86_64-pc-windows-msvc",
                "--config",
                "build.incremental=false",
                "t",
                "--no-run",
            ],
            "1.96.1-x86_64-pc-windows-msvc",
            "test",
            (),
            (),
        ),
        (["+nightly", "--help"], "nightly", None, (), ()),
        (["test"], None, "test", (), ()),
        ([], None, None, (), ()),
        (["test", "+nightly"], None, "test", ("+nightly",), ()),
        (["test", "+"], None, "test", ("+",), ()),
        (["test", "--", "+nightly"], None, "test", (), ("+nightly",)),
        (["test", "--", "+"], None, "test", (), ("+",)),
        (["--config", "+nightly", "test"], None, "test", (), ()),
        (["test", "--package", "+nightly"], None, "test", (), ()),
    ],
)
def test_cargo_retains_only_leading_rustup_toolchain_selector(
    arguments, selector, subcommand, positionals, forwarded
):
    invocation = command_admission.parse_cargo_invocation(["cargo", *arguments])
    assert invocation.toolchain_selector == selector
    assert invocation.subcommand == subcommand
    assert invocation.positionals == positionals
    assert invocation.forwarded == forwarded


@pytest.mark.parametrize("arguments", [["+"], ["+", "test"], ["+", "--help"]])
def test_cargo_rejects_empty_leading_rustup_selector(arguments):
    with pytest.raises(ValueError, match="requires a non-empty toolchain"):
        command_admission.parse_cargo_invocation(["cargo", *arguments])


@pytest.mark.parametrize(
    "arguments,kind",
    [
        (
            [
                "test",
                "--no-run",
                "--lib",
                "-p",
                "molt-runtime",
                "--target",
                "wasm32-wasip1",
            ],
            "build",
        ),
        (
            ["+nightly", "--config", "build.incremental=false", "test", "--no-run"],
            "build",
        ),
        (["t", "--no-run"], "build"),
        (["bench", "--no-run"], "build"),
        (["test", "--lib", "filter"], "test-execution"),
        (["test", "--", "--no-run"], "test-execution"),
        (["test", "--features=--no-run"], "test-execution"),
        (["test", "--package", "--no-run"], "test-execution"),
        (["test", "--config", "--no-run"], "test-execution"),
        (["build", "--package", "test", "--features", "pytest"], "build"),
        (["build", "--test", "test"], "build"),
        (["metadata", "--format-version", "1"], "query"),
        (["test", "--help"], "query"),
        (["--help", "test"], "query"),
        (["test", "-h"], "query"),
        (["--version"], "query"),
        (["help", "test"], "query"),
        (["test", "--no-run", "--help"], "query"),
        (["test", "--", "--list"], "query"),
        (["test", "--", "--help"], "query"),
        (["test", "--", "-qh"], "query"),
        (["test", "--", "filter", "--list"], "query"),
        (["test", "--", "--report-time", "--list"], "query"),
        (["test", "--no-run", "--", "--list"], "build"),
        (["bench", "--", "--list"], "query"),
        (["test", "--package", "--help"], "test-execution"),
        (["test", "--features=--help"], "test-execution"),
        (["--config", "--help", "test"], "test-execution"),
        (["test", "--", "--skip", "--help"], "test-execution"),
        (["test", "--", "--skip=--list"], "test-execution"),
        (["test", "--", "--format", "--help"], "test-execution"),
        (["test", "--", "--", "--help"], "test-execution"),
        (["test", "--", "-Z--help"], "test-execution"),
        (["deny", "--help"], "command"),
    ],
)
@pytest.mark.parametrize("delegated", [False, True])
def test_cargo_proof_operation_controls_counts(arguments, kind, delegated):
    command = (
        policy._canonical_cargo_proof_command(arguments)
        if delegated
        else ["cargo", *arguments]
    )
    envelope = command_admission.envelope_for_command(command)
    assert command_admission.command_proof_kind(envelope) == kind
    assert command_identity._requires_structured_test_counts(envelope) is (
        kind == "test-execution"
    )


@pytest.mark.parametrize(
    "arguments,descendants",
    [
        (["test", "--help"], "forbidden"),
        (["--help", "test"], "forbidden"),
        (["test", "--package", "--help"], "declared-toolchains"),
        (["test", "--", "--list"], "declared-toolchains"),
        (["test", "--", "--help"], "declared-toolchains"),
        (["deny", "--help"], "declared-toolchains"),
    ],
)
def test_cargo_help_is_leaf_but_harness_queries_may_compile(arguments, descendants):
    assert (
        command_admission._registered_toolchain_descendants(["cargo", *arguments])
        == descendants
    )


@pytest.mark.parametrize(
    "command,requires_counts",
    [
        ([sys.executable, "-m", "pytest", "-q"], True),
        ([sys.executable, "-I", "-mpytest", "-q"], True),
        (["uv", "run", "pytest", "-q"], True),
        ([sys.executable, "-c", "pass", "pytest"], False),
        ([sys.executable, "-c", "pass", "-m", "pytest"], False),
        ([sys.executable, "script.py", "pytest"], False),
    ],
)
def test_python_test_counts_follow_payload_not_argument_spelling(
    command, requires_counts
):
    envelope = command_admission.envelope_for_command(command)
    assert (
        command_identity._requires_structured_test_counts(envelope) is requires_counts
    )


@pytest.mark.parametrize(
    "cargo_only,refused",
    [
        (["test", "--lib", "one_filter"], True),
        (["test", "--no-run", "--lib", "one_filter"], False),
        (["test", "--lib", "one_filter", "--", "--no-run"], True),
        (["test", "--lib", "one_filter", "--features=--no-run"], True),
        (["test", "--lib", "--package", "test"], False),
        (["test", "one_filter", "--", "--lib"], False),
        (["+nightly", "test", "--lib", "one_filter"], True),
        (["test", "--lib", "one_filter", "--help"], False),
        (["test", "--lib", "one_filter", "--", "--list"], False),
        (["test", "--lib", "one_filter", "--", "--skip", "--list"], True),
    ],
)
def test_compile_only_and_harness_boundaries_share_cold_test_policy(
    cargo_only, refused
):
    assert (
        policy._cold_single_lib_test_policy_error(cargo_only) is not None
    ) is refused


def test_cargo_contention_uses_only_cargo_package_operands():
    assert (
        policy._cargo_package_for_contention(["test", "-pmolt-runtime"])
        == "molt-runtime"
    )
    assert (
        policy._cargo_package_for_contention(
            ["test", "--", "--package", "not-a-cargo-package"]
        )
        == "workspace"
    )


def test_compile_only_transcript_is_valid_build_but_not_running_test_evidence(tmp_path):
    stdout = tmp_path / "stdout.bin"
    stderr = tmp_path / "stderr.bin"
    stdout.write_bytes(b"")
    stderr.write_bytes(
        b"Finished dev-fast profile target(s)\nExecutable unittests (molt_runtime.wasm)\n"
    )
    transcript = {
        "stdout": command_identity._transcript_identity(stdout),
        "stderr": command_identity._transcript_identity(stderr),
    }
    assert all(stream["test_counts"] == {} for stream in transcript.values())
    build = command_admission.envelope_for_command(
        policy._canonical_cargo_proof_command(["test", "--no-run", "--lib"])
    )
    command_identity.validate_structured_test_counts(build, transcript, returncode=0)
    for command in (
        ["cargo", "test", "--lib"],
        ["cargo", "test", "--", "--no-run"],
        [sys.executable, "-m", "pytest"],
    ):
        running = command_admission.envelope_for_command(command)
        with pytest.raises(ValueError, match="no structured test-count authority"):
            command_identity.validate_structured_test_counts(
                running, transcript, returncode=0
            )
        command_identity.validate_structured_test_counts(
            running, transcript, returncode=1
        )
    stdout.write_bytes(b"test result: ok. 3 passed; 0 failed; 0 ignored\n")
    transcript["stdout"] = command_identity._transcript_identity(stdout)
    command_identity.validate_structured_test_counts(
        command_admission.envelope_for_command(["cargo", "test"]),
        transcript,
        returncode=0,
    )


@pytest.mark.parametrize(
    "arguments",
    [
        ["test", "--help"],
        ["--help", "test"],
        ["test", "--", "--list"],
        ["test", "--", "--help"],
    ],
)
def test_query_without_test_counts_is_not_build_or_test_execution(arguments, tmp_path):
    output = tmp_path / "query.stdout.bin"
    output.write_bytes(b"query output without executed-test totals\n")
    transcript = {"stdout": command_identity._transcript_identity(output)}
    envelope = command_admission.envelope_for_command(["cargo", *arguments])
    assert command_admission.command_proof_kind(envelope) == "query"
    assert transcript["stdout"]["test_counts"] == {}
    command_identity.validate_structured_test_counts(envelope, transcript, returncode=0)
