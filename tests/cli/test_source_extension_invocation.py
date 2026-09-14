from __future__ import annotations

import pytest

from molt.cli.source_extension_invocation import (
    SourceExtensionInvocationError,
    SourceExtensionSetInvocation,
)


_COMMON = [
    "--package",
    "numpy",
    "--package-version",
    "2.5.1",
    "--module-set",
    "pact-witness",
    "--python-version",
    "3.12",
    "--source",
    "C:/upstream/numpy",
    "--build-root",
    "C:/build/numpy",
]


def test_produce_set_invocation_round_trips_canonical_argv() -> None:
    invocation = SourceExtensionSetInvocation.from_arguments(
        [
            "extension",
            "produce-set",
            *_COMMON,
            "--target",
            "wasm",
            "--abi-tier",
            "cpython-abi",
            "--json",
            "--prepared",
            "--expected-identity-sha256",
            "a" * 64,
            "--expected-candidate-identity-sha256",
            "b" * 64,
        ]
    )

    assert invocation.module_argv("C:/locked/python.exe") == (
        "C:/locked/python.exe",
        "-P",
        "-m",
        "molt.cli",
        "extension",
        "produce-set",
        *_COMMON,
        "--target",
        "wasm",
        "--abi-tier",
        "cpython-abi",
        "--json",
        "--prepared",
        "--expected-identity-sha256",
        "a" * 64,
        "--expected-candidate-identity-sha256",
        "b" * 64,
    )


def test_candidate_invocation_requires_output_and_rejects_publication_ids() -> None:
    with pytest.raises(SourceExtensionInvocationError, match="requires an output root"):
        SourceExtensionSetInvocation.from_arguments(
            ["extension", "attest-set-candidate", *_COMMON]
        )
    with pytest.raises(
        SourceExtensionInvocationError, match="does not accept publication"
    ):
        SourceExtensionSetInvocation.from_arguments(
            [
                "extension",
                "attest-set-candidate",
                *_COMMON,
                "--output",
                "C:/candidate",
                "--expected-identity-sha256",
                "a" * 64,
            ]
        )


@pytest.mark.parametrize(
    ("field", "value", "expected"),
    [
        ("json_output", 1, "exact bool"),
        ("prepared", 1, "exact bool"),
        ("source", "source\0root", "NUL-free"),
        ("expected_identity_sha256", "A" * 64, "lowercase SHA-256"),
        ("expected_candidate_identity_sha256", "f" * 63, "lowercase SHA-256"),
        ("abi_tier", "source-compat", "unsupported source-extension ABI tier"),
    ],
)
def test_invocation_rejects_noncanonical_field_values(
    field: str, value: object, expected: str
) -> None:
    values: dict[str, object] = {
        "command": "produce-set",
        "package": "numpy",
        "package_version": "2.5.1",
        "module_set": "pact-witness",
        "python_version": "3.12",
        "source": "C:/upstream/numpy",
        "build_root": "C:/build/numpy",
        "target": "wasm",
        "abi_tier": "cpython-abi",
    }
    values[field] = value

    with pytest.raises(SourceExtensionInvocationError, match=expected):
        SourceExtensionSetInvocation(**values)  # type: ignore[arg-type]


@pytest.mark.parametrize(
    "suffix",
    [
        ["--package", "scipy"],
        ["--unknown", "value"],
        ["--target"],
        ["--prepared", "--prepared"],
    ],
)
def test_invocation_rejects_duplicate_unknown_and_valueless_options(
    suffix: list[str],
) -> None:
    with pytest.raises(SourceExtensionInvocationError):
        SourceExtensionSetInvocation.from_arguments(
            ["extension", "produce-set", *_COMMON, *suffix]
        )


@pytest.mark.parametrize("command", ["produce-set", "attest-set-candidate"])
@pytest.mark.parametrize("prepared", [False, True])
def test_prepared_precondition_round_trips_both_build_modes(command, prepared) -> None:
    from molt.cli.entrypoint_parser import _build_entrypoint_parser

    arguments = ["extension", command, *_COMMON]
    if command == "attest-set-candidate":
        arguments.extend(("--output", "C:/candidate"))
    if prepared:
        arguments.append("--prepared")
    invocation = SourceExtensionSetInvocation.from_arguments(arguments)
    assert invocation.prepared is prepared
    argv = invocation.module_argv("C:/locked/python.exe")
    assert SourceExtensionSetInvocation.from_arguments(argv[4:]) == invocation
    assert _build_entrypoint_parser().parse_args(argv[4:]).prepared is prepared


@pytest.mark.parametrize("command", ["produce-set", "attest-set-candidate"])
def test_prepared_precondition_reaches_shared_execution_body(
    command, monkeypatch, tmp_path
) -> None:
    from molt.cli import (
        entrypoint_dispatch,
        entrypoint_parser,
        source_extension_producer,
    )

    calls = []
    monkeypatch.setattr(
        source_extension_producer,
        "_build_source_extension_set",
        lambda **kwargs: calls.append(kwargs) or 17,
    )
    arguments = ["extension", command, *_COMMON, "--prepared"]
    if command == "attest-set-candidate":
        arguments.extend(("--output", "C:/candidate"))
    parsed = entrypoint_parser._build_entrypoint_parser().parse_args(arguments)
    assert (
        entrypoint_dispatch._dispatch_entrypoint_command(
            parsed,
            build_fn=lambda **_: 0,
            config_root=tmp_path,
            config={},
            build_cfg={},
            run_cfg={},
            compare_cfg={},
            test_cfg={},
            diff_cfg={},
            extension_cfg={},
            publish_cfg={},
            cfg_capabilities=None,
        )
        == 17
    )
    assert len(calls) == 1
    assert calls[0]["prepared"] is True
    assert calls[0].get("candidate_output") == (
        "C:/candidate" if command == "attest-set-candidate" else None
    )
