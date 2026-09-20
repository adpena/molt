"""Canonical source-extension set invocation parsing and re-exec argv shaping."""

from __future__ import annotations

import argparse
import re
from dataclasses import dataclass
from typing import Literal, Sequence


SourceExtensionSetCommand = Literal["produce-set", "attest-set-candidate"]
SOURCE_EXTENSION_SET_ABI_TIERS = ("cpython-abi",)
_COMMON_OPTIONS = (
    ("--package", "package", "Registered package name (for example: scipy)."),
    (
        "--package-version",
        "package_version",
        "Registered upstream package version (for example: 1.18.0).",
    ),
    (
        "--module-set",
        "module_set",
        "Configured extension-set name (for example: pact-witness).",
    ),
    (
        "--python-version",
        "python_version",
        "Registered target CPython feature version (for example: 3.12).",
    ),
    ("--source", "source", "Pinned upstream source checkout."),
    (
        "--build-root",
        "build_root",
        "Absent or empty build root for the single upstream Meson setup.",
    ),
    (
        "--target",
        "target",
        "Extension-set target: native, wasm, wasm-freestanding, or an explicit "
        "Rust target triple (default: wasm).",
    ),
    ("--abi-tier", "abi_tier", "Extension-set ABI tier (default: cpython-abi)."),
)
_OPTION_FIELD = {option: field for option, field, _help in _COMMON_OPTIONS}
_JSON_OPTION = "--json"
_PREPARED_OPTION = "--prepared"
_CANDIDATE_OUTPUT_OPTION = "--output"
_EXPECTED_IDENTITY_OPTION = "--expected-identity-sha256"
_EXPECTED_CANDIDATE_IDENTITY_OPTION = "--expected-candidate-identity-sha256"
_OPTIONAL_VALUE_FIELDS = {
    _CANDIDATE_OUTPUT_OPTION: "candidate_output",
    _EXPECTED_IDENTITY_OPTION: "expected_identity_sha256",
    _EXPECTED_CANDIDATE_IDENTITY_OPTION: "expected_candidate_identity_sha256",
}
_VALUE_OPTIONS = frozenset(_OPTION_FIELD | _OPTIONAL_VALUE_FIELDS)
_SHA256 = re.compile(r"[0-9a-f]{64}\Z")


class SourceExtensionInvocationError(ValueError):
    """A source-extension producer invocation is malformed."""


def _source_extension_set_command(value: object) -> SourceExtensionSetCommand:
    if type(value) is str:
        if value == "produce-set":
            return "produce-set"
        if value == "attest-set-candidate":
            return "attest-set-candidate"
    raise SourceExtensionInvocationError(
        f"unknown source-extension set command {value!r}"
    )


def _required_string(value: object, *, field: str) -> str:
    if type(value) is not str or not value or "\0" in value:
        raise SourceExtensionInvocationError(
            f"source-extension producer {field!r} must be a non-empty NUL-free string"
        )
    return value


def _optional_string(value: object, *, field: str) -> str | None:
    if value is None:
        return None
    return _required_string(value, field=field)


@dataclass(frozen=True, slots=True)
class SourceExtensionSetInvocation:
    command: SourceExtensionSetCommand
    package: str
    package_version: str
    module_set: str
    python_version: str
    source: str
    build_root: str
    target: str
    abi_tier: str
    json_output: bool = False
    prepared: bool = False
    candidate_output: str | None = None
    expected_identity_sha256: str | None = None
    expected_candidate_identity_sha256: str | None = None

    def __post_init__(self) -> None:
        _source_extension_set_command(self.command)
        for _option, field, _help in _COMMON_OPTIONS:
            _required_string(getattr(self, field), field=field)
        if self.abi_tier not in SOURCE_EXTENSION_SET_ABI_TIERS:
            raise SourceExtensionInvocationError(
                f"unsupported source-extension ABI tier {self.abi_tier!r}"
            )
        for field in ("json_output", "prepared"):
            if type(getattr(self, field)) is not bool:
                raise SourceExtensionInvocationError(
                    f"source-extension producer {field} must be an exact bool"
                )
        candidate_output = _optional_string(
            self.candidate_output, field="candidate_output"
        )
        expected_identity = _optional_string(
            self.expected_identity_sha256, field="expected_identity_sha256"
        )
        expected_candidate_identity = _optional_string(
            self.expected_candidate_identity_sha256,
            field="expected_candidate_identity_sha256",
        )
        for field, digest in (
            ("expected_identity_sha256", expected_identity),
            ("expected_candidate_identity_sha256", expected_candidate_identity),
        ):
            if digest is not None and _SHA256.fullmatch(digest) is None:
                raise SourceExtensionInvocationError(
                    f"source-extension producer {field!r} must be lowercase SHA-256"
                )
        if self.command == "attest-set-candidate":
            if candidate_output is None:
                raise SourceExtensionInvocationError(
                    "candidate attestation requires an output root"
                )
            if expected_identity is not None or expected_candidate_identity is not None:
                raise SourceExtensionInvocationError(
                    "candidate attestation does not accept publication identities"
                )
        elif candidate_output is not None:
            raise SourceExtensionInvocationError(
                "registered publication cannot receive candidate-only output custody"
            )

    @classmethod
    def from_arguments(cls, arguments: Sequence[str]) -> "SourceExtensionSetInvocation":
        """Parse ``extension <command>`` arguments without argparse/SystemExit."""

        values = tuple(arguments)
        if any(type(value) is not str for value in values):
            raise SourceExtensionInvocationError(
                "source-extension invocation arguments must be exact strings"
            )
        if len(values) < 2 or values[0] != "extension":
            raise SourceExtensionInvocationError(
                "source-extension invocation must begin with 'extension <command>'"
            )
        raw_command = values[1]
        parsed: dict[str, str] = {}
        json_output = False
        prepared = False
        index = 2
        while index < len(values):
            option = values[index]
            if option == _PREPARED_OPTION:
                if prepared:
                    raise SourceExtensionInvocationError(
                        "source-extension producer repeats --prepared"
                    )
                prepared = True
                index += 1
                continue
            if option == _JSON_OPTION:
                if json_output:
                    raise SourceExtensionInvocationError(
                        "source-extension producer repeats --json"
                    )
                json_output = True
                index += 1
                continue
            if option not in _VALUE_OPTIONS or index + 1 >= len(values):
                raise SourceExtensionInvocationError(
                    "source-extension producer has an unknown or valueless option: "
                    f"{option!r}"
                )
            if option in parsed:
                raise SourceExtensionInvocationError(
                    f"source-extension producer repeats option {option!r}"
                )
            parsed[option] = _required_string(
                values[index + 1], field=f"option {option}"
            )
            index += 2
        required = {
            option
            for option, field, _help in _COMMON_OPTIONS
            if field not in {"target", "abi_tier"}
        }
        missing = sorted(required - parsed.keys())
        if missing:
            raise SourceExtensionInvocationError(
                "source-extension producer is missing required options: "
                + ", ".join(missing)
            )
        command = _source_extension_set_command(raw_command)
        return cls(
            command=command,
            package=parsed["--package"],
            package_version=parsed["--package-version"],
            module_set=parsed["--module-set"],
            python_version=parsed["--python-version"],
            source=parsed["--source"],
            build_root=parsed["--build-root"],
            target=parsed.get("--target", "wasm"),
            abi_tier=parsed.get("--abi-tier", "cpython-abi"),
            json_output=json_output,
            prepared=prepared,
            candidate_output=parsed.get(_CANDIDATE_OUTPUT_OPTION),
            expected_identity_sha256=parsed.get(_EXPECTED_IDENTITY_OPTION),
            expected_candidate_identity_sha256=parsed.get(
                _EXPECTED_CANDIDATE_IDENTITY_OPTION
            ),
        )

    def module_argv(self, python_executable: str) -> tuple[str, ...]:
        """Return the canonical locked-interpreter module invocation."""

        executable = _required_string(python_executable, field="python_executable")
        argv = [executable, "-P", "-m", "molt.cli", "extension", self.command]
        for option, field, _help in _COMMON_OPTIONS:
            argv.extend((option, getattr(self, field)))
        if self.candidate_output is not None:
            argv.extend((_CANDIDATE_OUTPUT_OPTION, self.candidate_output))
        if self.json_output:
            argv.append(_JSON_OPTION)
        if self.prepared:
            argv.append(_PREPARED_OPTION)
        for option, field in _OPTIONAL_VALUE_FIELDS.items():
            value = getattr(self, field)
            if value is not None and field != "candidate_output":
                argv.extend((option, value))
        return tuple(argv)


def add_source_extension_set_build_arguments(
    parser: argparse.ArgumentParser,
    *,
    command: SourceExtensionSetCommand,
    include_prepared: bool = True,
) -> None:
    """Project the invocation authority into the human-facing argparse surface."""

    for option, field, help_text in _COMMON_OPTIONS:
        if field == "target":
            parser.add_argument(option, default="wasm", help=help_text)
        elif field == "abi_tier":
            parser.add_argument(
                option,
                choices=SOURCE_EXTENSION_SET_ABI_TIERS,
                default="cpython-abi",
                help=help_text,
            )
        else:
            parser.add_argument(option, required=True, help=help_text)
    if command == "produce-set":
        parser.add_argument(
            _EXPECTED_IDENTITY_OPTION,
            help=(
                "Reproduce an existing canonical seal transactionally and publish "
                "nothing unless both incumbent and candidate match this canonical "
                "target/content identity."
            ),
        )
        parser.add_argument(
            _EXPECTED_CANDIDATE_IDENTITY_OPTION,
            help=(
                "Require the transactionally built candidate to match this declared "
                "canonical identity; equal incumbent/candidate identities are a no-op, "
                "different identities use crash-recoverable compare-and-swap publication."
            ),
        )
    elif command == "attest-set-candidate":
        parser.add_argument(
            _CANDIDATE_OUTPUT_OPTION,
            required=True,
            help="New detached bundle root below canonical package-candidates custody.",
        )
    else:
        raise ValueError(f"unknown source-extension set command {command!r}")
    parser.add_argument(
        _JSON_OPTION, action="store_true", help="Emit JSON output for tooling."
    )
    if include_prepared:
        parser.add_argument(
            _PREPARED_OPTION,
            action="store_true",
            help=(
                "Require already-prepared source and the active locked interpreter; "
                "verify all prerequisites without provisioning or re-execution."
            ),
        )
