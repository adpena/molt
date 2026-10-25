"""Execution obligations use typed command boundaries, never incidental tokens."""

from __future__ import annotations

import copy
import sys

import pytest

from tools.proof_queue_pkg import command_admission, command_identity, policy


def _admit_exact_fixture_rows(monkeypatch, *commands):
    """Register each argv as an exact proof-plan row.

    An ad hoc pytest argv is admitted only for owned repository tests (a
    focused proof, HF-179), so fixture commands register exact rows. Count
    obligations follow the payload of an admitted row.
    """
    registry = copy.deepcopy(command_admission._proof_command_registry())
    for index, command in enumerate(commands):
        registry["exact"][tuple(str(value) for value in command)] = {
            "ids": [f"fixture.pytest-{index}"],
            "toolchains": ("python",),
            "cargo_native_units": {},
        }
    monkeypatch.setattr(command_admission, "_proof_command_registry", lambda: registry)


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
    monkeypatch, command, requires_counts
):
    _admit_exact_fixture_rows(monkeypatch, command)
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


def test_compile_only_transcript_is_valid_build_but_not_running_test_evidence(
    monkeypatch, tmp_path
):
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
    _admit_exact_fixture_rows(monkeypatch, [sys.executable, "-m", "pytest"])
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


@pytest.mark.parametrize(
    "arguments",
    [
        ["-I", "-E", "-m", "pytest", "-X", "pycache_prefix=payload"],
        ["-IE", "-c", "print('untouched')", "--", "-Xpycache_prefix=payload"],
        ["--", "script with spaces.py", "-I", "-Xpycache_prefix=payload"],
    ],
)
def test_selected_image_python_prefix_precedes_startup_and_preserves_payload(
    tmp_path, arguments
):
    from tools.proof_command_argv import python_environment_command

    root = tmp_path / "image"
    command = ["python", *arguments]
    assert python_environment_command(command, environment_root=root) == [
        "python",
        "-B",
        "-X",
        f"pycache_prefix={root / 'bytecode'}",
        *arguments,
    ]
    assert not root.exists(), "launch projection must create no state"


@pytest.mark.parametrize("attached", [False, True])
def test_selected_image_python_prefix_admits_one_exact_prefix(tmp_path, attached):
    from tools.proof_command_argv import python_environment_command

    root = tmp_path / "image"
    value = f"pycache_prefix={root / 'bytecode'}"
    prefix = [f"-IX{value}"] if attached else ["-I", "-X", value]
    command = ["python", *prefix, "-m", "pytest", "-X", "payload"]
    assert python_environment_command(command, environment_root=root) == [
        "python",
        "-B",
        *command[1:],
    ]
    with pytest.raises(ValueError, match="selected exactly once"):
        python_environment_command(
            ["python", *prefix, "-X", value, "-m", "pytest"], environment_root=root
        )
    with pytest.raises(ValueError, match="conflicts"):
        python_environment_command(
            ["python", "-IXpycache_prefix=foreign", "-m", "pytest"],
            environment_root=root,
        )


@pytest.mark.parametrize("arguments", [["--help", "-X"], ["--version"], ["-V"]])
def test_selected_image_python_terminal_actions_remain_terminal(tmp_path, arguments):
    from tools.proof_command_argv import python_environment_command

    command = ["python", *arguments]
    assert python_environment_command(command, environment_root=tmp_path) == command


@pytest.mark.parametrize(
    ("tail", "arguments"),
    [
        ('"a b c" d e', ("a b c", "d", "e")),
        (r'"ab\"c" "\\" d', ('ab"c', "\\", "d")),
        (r'a\\\b d"e f"g h', (r"a\\\b", "de fg", "h")),
        (r"a\\\"b c d", ('a\\"b', "c", "d")),
        (r'a\\\\"b c" d e', (r"a\\b c", "d", "e")),
        ('a"b"" c d', ('ab" c d',)),
        ('"" "unterminated', ("", "unterminated")),
    ],
)
def test_windows_python_argv_follows_documented_crt_grammar(tail, arguments):
    from tools.proof_command_argv import split_windows_python_command_line

    # Microsoft's published CRT examples provide the independent expected argv.
    # CommandLineToArgvW differs on the doubled-quote example.
    assert split_windows_python_command_line(
        f'"C:\\Python path\\python.exe" {tail}'
    ) == (
        r"C:\Python path\python.exe",
        *arguments,
    )


def test_windows_python_argv_doubled_quotes_do_not_invent_startup_flags(tmp_path):
    from molt.python_interpreter import parse_python_invocation
    from tools.proof_command_argv import (
        split_windows_python_command_line,
        validate_python_environment_command,
    )

    root = tmp_path / "image"
    warning = f'-W"x"" -B -Xpycache_prefix={root / "bytecode"} "'
    argv = split_windows_python_command_line(f'python.exe {warning} -c "print(1)"')
    assert argv[1] == f'-Wx" -B -Xpycache_prefix={root / "bytecode"} '
    invocation = parse_python_invocation(argv)
    assert invocation.mode == "command"
    assert invocation.target == "print(1)"
    with pytest.raises(ValueError, match="cache prefix"):
        validate_python_environment_command(argv, environment_root=root, environment={})


def test_windows_python_program_name_uses_pathname_grammar():
    from tools.proof_command_argv import split_windows_python_command_line

    assert split_windows_python_command_line('C:\\a" b"\\python.exe -B') == (
        r"C:\a b\python.exe",
        "-B",
    )
    assert split_windows_python_command_line(" python.exe -B") == (
        "",
        "python.exe",
        "-B",
    )
    for command in ("", "python.exe\x00 -B"):
        with pytest.raises(ValueError, match="empty or contains NUL"):
            split_windows_python_command_line(command)


@pytest.mark.parametrize(
    "program",
    [
        r'"fake\"',
        '"fake"suffix',
        "python.exe\t-B",
        "python.exe\r",
        "python.exe\v",
        "python.exe\f",
    ],
)
def test_windows_python_forwarders_must_keep_the_same_startup_suffix(program):
    from tools.proof_command_argv import validate_windows_python_launch_prefix

    with pytest.raises(ValueError, match="ambiguous forwarding"):
        validate_windows_python_launch_prefix(
            f'{program} -B -X pycache_prefix=selected -c "dummy -c real"'
        )
    with pytest.raises(ValueError, match="ambiguous forwarding"):
        validate_windows_python_launch_prefix(f"{program} --help")


@pytest.mark.parametrize(
    "program",
    [
        r"C:\Python\python.exe",
        r'"C:\Python path\python.exe"',
        r'"\\server\share\python.exe"',
    ],
)
def test_windows_python_forwarders_preserve_canonical_startup_prefixes(program):
    from tools.proof_command_argv import validate_windows_python_launch_prefix

    validate_windows_python_launch_prefix(
        f'{program} -B -X pycache_prefix=selected -c "payload"'
    )


@pytest.mark.parametrize("isolated", [False, True])
def test_selected_image_child_validation_uses_only_honored_startup_inputs(
    tmp_path, isolated
):
    from tools.proof_command_argv import validate_python_environment_command

    root = tmp_path / "image"
    env = {
        "PYTHONPYCACHEPREFIX": str(root / "bytecode"),
        "PYTHONDONTWRITEBYTECODE": "1",
    }
    command = ["python", *(["-IE"] if isolated else []), "-c", "pass"]
    if isolated:
        with pytest.raises(ValueError, match="cache prefix"):
            validate_python_environment_command(
                command, environment_root=root, environment=env
            )
    else:
        validate_python_environment_command(
            command, environment_root=root, environment=env
        )
        with pytest.raises(ValueError, match="disable bytecode writes"):
            validate_python_environment_command(
                command,
                environment_root=root,
                environment={"PYTHONPYCACHEPREFIX": env["PYTHONPYCACHEPREFIX"]},
            )
    explicit = [
        "python",
        "-B",
        "-X",
        f"pycache_prefix={root / 'bytecode'}",
        *command[1:],
    ]
    validate_python_environment_command(explicit, environment_root=root, environment={})
    with pytest.raises(ValueError, match="selected exactly once"):
        validate_python_environment_command(
            [*explicit[:4], "-X", f"pycache_prefix={root / 'bytecode'}", *explicit[4:]],
            environment_root=root,
            environment=env,
        )
    validate_python_environment_command(
        ["python", "--help", "-X"], environment_root=root, environment={}
    )
    assert not root.exists()


def test_selected_image_uv_transport_preserves_typed_values_and_payload(tmp_path):
    from tools.proof_command_argv import uv_environment_command

    root, image = tmp_path / "repo", tmp_path / "image"
    from molt.python_environment_location import environment_python_path

    payload = ["python", "-I", "-c", "pass", "--python", "payload"]
    command = [
        "uv",
        "run",
        "--project",
        ".",
        "-p",
        "3.15",
        "--extra",
        "--active",
        "--no-sync",
        "--",
        *payload,
    ]
    assert uv_environment_command(
        command, repo_root=root, environment_root=image, python_selector="3.15"
    ) == [
        "uv",
        "run",
        "--active",
        "--project",
        str(root),
        "--python",
        str(environment_python_path(image)),
        "--no-sync",
        "--no-config",
        "--extra",
        "--active",
        "--",
        *payload,
    ]
    for prefix, message in [
        (["--python", "4.0"], "Python selector"),
        (["--project", "foreign"], "project selector"),
        (["--isolated"], "isolated"),
    ]:
        with pytest.raises(ValueError, match=message):
            uv_environment_command(
                ["uv", "run", *prefix, "python"],
                repo_root=root,
                environment_root=image,
                python_selector="3.15",
            )
    assert not image.exists()
