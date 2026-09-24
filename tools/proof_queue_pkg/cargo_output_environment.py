"""One derived-output environment policy for Cargo identity and execution."""

from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path
from typing import Mapping

from tools.proof_queue_pkg import command_admission

TEMPORARY_VARIABLE_NAMES = ("TEMP", "TMP", "TMPDIR")


def _name_key(name: str) -> str:
    return name.upper() if os.name == "nt" else name


@dataclass(frozen=True)
class CargoOutputEnvironment:
    """Bind symbolic build-output roles to an exclusively allocated generation.

    The identity key cannot contain the path derived from that same key. Only
    explicitly owned variables are symbolic; every other selected value remains
    a compilation input. Final validation checks actual paths independently.
    """

    documentation: bool
    external_placement: bool = False

    @classmethod
    def for_envelope(cls, envelope: Mapping[str, object]) -> CargoOutputEnvironment:
        """Derive policy from admitted semantics, never execution transport.

        Python custody may wrap the exact execution command after admission.
        The delegated envelope retains the original Cargo operation; reparsing
        that transformed transport would invent a second admission boundary.
        """
        external_placement = envelope.get("cargo_output_root") is not None
        delegated = envelope.get("delegated")
        if delegated is not None:
            if not isinstance(delegated, Mapping):
                raise ValueError("Cargo output policy has no admitted delegation")
            envelope = delegated
        argv = envelope.get("argv")
        if not isinstance(argv, list) or not all(isinstance(arg, str) for arg in argv):
            raise ValueError("Cargo output policy has no admitted argv")
        invocation = command_admission.parse_cargo_invocation(argv)
        policy = cls.for_invocation(invocation)
        if external_placement and any(
            name == "--artifact-dir" for name, _value in invocation.option_values
        ):
            raise ValueError("Cargo --artifact-dir bypasses declared output placement")
        return cls(policy.documentation, external_placement=external_placement)

    @classmethod
    def for_invocation(
        cls, invocation: command_admission.CargoInvocation
    ) -> CargoOutputEnvironment:
        if any(name == "--target-dir" for name, _value in invocation.option_values):
            raise ValueError(
                "Cargo --target-dir bypasses proof target custody; supply "
                "CARGO_TARGET_DIR as the requested target instead"
            )
        return cls(documentation=invocation.requires_documenter)

    @property
    def names(self) -> tuple[str, ...]:
        if self.external_placement:
            return (
                "CARGO_TARGET_DIR",
                *TEMPORARY_VARIABLE_NAMES,
                "PYTHONPYCACHEPREFIX",
            )
        return (
            ("CARGO_TARGET_DIR", *TEMPORARY_VARIABLE_NAMES)
            if self.documentation
            else ("CARGO_TARGET_DIR",)
        )

    def identity(self) -> dict[str, object]:
        return {
            "schema": "molt.proof-cargo-output-environment.v1",
            "bindings": {name: {"role": "build-output"} for name in self.names},
        }

    def owns(self, name: str) -> bool:
        return _name_key(name) in self.names

    def caller_environment(self, env: Mapping[str, str]) -> dict[str, str]:
        return {name: value for name, value in env.items() if not self.owns(name)}

    def bind(self, env: Mapping[str, str], *, target: Path) -> dict[str, str]:
        if (
            not target.is_absolute()
            or not target.is_dir()
            or os.path.normcase(str(target))
            != os.path.normcase(str(target.resolve(strict=True)))
        ):
            raise ValueError(
                "Cargo output environment requires the canonical leased target directory"
            )
        selected = self.caller_environment(env)
        selected.update({name: str(target) for name in self.names})
        return selected

    def validate(self, env: Mapping[str, str], *, target: Path) -> None:
        # Do not reopen outputs: the parent also verifies historical terminal
        # receipts after an owner has retired a generation. Values, not current
        # artifact existence, establish this binding.
        actual = {name: value for name, value in env.items() if self.owns(name)}
        expected = {name: str(target) for name in self.names}
        if actual != expected:
            raise ValueError(
                "Cargo output environment differs from its leased generation"
            )
