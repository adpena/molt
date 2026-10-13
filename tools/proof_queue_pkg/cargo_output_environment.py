"""One derived-output environment policy for Cargo identity and execution."""

from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path
from typing import Mapping

from molt.dx import (
    PROOF_SCRATCH_ROOT_ENV,
    SCRATCH_ENV_KEYS,
    generated_session_id,
    proof_scratch_root,
)
from tools.proof_queue_pkg import command_admission

TEMPORARY_VARIABLE_NAMES = ("TEMP", "TMP", "TMPDIR")


def require_independent_cargo_target(env: Mapping[str, str]) -> None:
    """A queued driver cannot replace the Cargo generation it was admitted into."""
    if env.get(PROOF_SCRATCH_ROOT_ENV, "").strip():
        raise ValueError("queued Cargo ownership forbids secondary target relocation")


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
    python_driver: bool = False
    cache_domain: str = "none"

    @classmethod
    def for_envelope(cls, envelope: Mapping[str, object]) -> CargoOutputEnvironment:
        """Derive policy from admitted semantics, never execution transport.

        Python custody may wrap the exact execution command after admission.
        The delegated envelope retains the original Cargo operation; reparsing
        that transformed transport would invent a second admission boundary.
        """
        cache_domain = command_admission.cargo_cache_domain(envelope)
        external_placement = envelope.get("cargo_output_root") is not None
        delegated = envelope.get("delegated")
        if delegated is not None:
            if not isinstance(delegated, Mapping):
                raise ValueError("Cargo output policy has no admitted delegation")
            envelope = delegated
        argv = envelope.get("argv")
        if not isinstance(argv, list) or not all(isinstance(arg, str) for arg in argv):
            raise ValueError("Cargo output policy has no admitted argv")
        invocation = command_admission.cargo_invocation_for_envelope(envelope)
        # A declared Python driver can build documentation as a descendant.
        # Bind the complete output-variable family instead of guessing its argv.
        if invocation is None:
            return cls(
                documentation=True,
                cache_domain=cache_domain,
                external_placement=external_placement,
                python_driver="cargo" in envelope.get("toolchains", []),
            )
        policy = cls.for_invocation(invocation)
        if external_placement and any(
            name == "--artifact-dir" for name, _value in invocation.option_values
        ):
            raise ValueError("Cargo --artifact-dir bypasses declared output placement")
        return cls(
            policy.documentation,
            external_placement=external_placement,
            cache_domain=cache_domain,
        )

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
    def build_names(self) -> tuple[str, ...]:
        names = ("CARGO_TARGET_DIR",)
        if self.documentation or self.external_placement:
            names += TEMPORARY_VARIABLE_NAMES
        if self.external_placement:
            names += ("PYTHONPYCACHEPREFIX",)
        if self.python_driver:
            names += ("MOLT_DIFF_CARGO_TARGET_DIR", "MOLT_WASM_TEST_CARGO_TARGET_DIR")
        return names

    @property
    def scratch_names(self) -> tuple[str, ...]:
        return (
            ("MOLT_DIFF_TMPDIR", "MOLT_COMPAT_SCRATCH_ROOT")
            if self.python_driver
            else ()
        )

    @property
    def names(self) -> tuple[str, ...]:
        return self.build_names + self.scratch_names

    def identity(self) -> dict[str, object]:
        return {
            "schema": "molt.proof-cargo-output-environment.v3",
            "cache_domain": self.cache_domain,
            "bindings": {
                **{name: {"role": "build-output"} for name in self.build_names},
                **{name: {"role": "scratch-output"} for name in self.scratch_names},
            },
        }

    def validate_request_environment(self, env: Mapping[str, str]) -> None:
        if not self.python_driver:
            return
        from tools.compat.diff_output_layout import IDENTITY_ENV, ROOT_ENV

        if any(env.get(name, "").strip() for name in (ROOT_ENV, IDENTITY_ENV)):
            raise ValueError(
                "independent differential guest output conflicts with queued Cargo "
                "ownership; select the queue --cargo-output-root instead"
            )

    def _bindings(self, env: Mapping[str, str], *, target: Path) -> dict[str, str]:
        bindings = {name: str(target) for name in self.build_names}
        if self.scratch_names:
            raw = env.get(PROOF_SCRATCH_ROOT_ENV, "")
            if not raw or not Path(raw).is_absolute():
                raise ValueError("Python Cargo outputs require admitted proof scratch")
            scratch = proof_scratch_root(target, env)
            if os.path.normcase(raw) != os.path.normcase(str(scratch)):
                raise ValueError("Python Cargo proof scratch is not canonical")
            bindings.update({name: str(scratch) for name in self.scratch_names})
        return bindings

    def owns(self, name: str) -> bool:
        return _name_key(name) in self.names

    def workspace_environment(self, env: Mapping[str, str]) -> dict[str, str]:
        """Project existing run scratch/session roles out of Cargo workspace storage."""
        ignored = set(SCRATCH_ENV_KEYS)
        if generated_session_id(env):
            ignored.update(("MOLT_SESSION_ID", "MOLT_SESSION_ID_GENERATED"))
        return {
            name: value
            for name, value in self.caller_environment(env).items()
            if name not in ignored
        }

    def caller_environment(self, env: Mapping[str, str]) -> dict[str, str]:
        return {name: value for name, value in env.items() if not self.owns(name)}

    def bind(self, env: Mapping[str, str], *, target: Path) -> dict[str, str]:
        self.validate_request_environment(env)
        if (
            not target.is_absolute()
            or not target.is_dir()
            or os.path.normcase(str(target))
            != os.path.normcase(str(target.resolve(strict=True)))
        ):
            raise ValueError(
                "Cargo output environment requires the canonical leased target directory"
            )
        bindings = self._bindings(env, target=target)
        if self.scratch_names and not Path(bindings[self.scratch_names[0]]).is_dir():
            raise ValueError("Python Cargo proof scratch is not an existing directory")
        selected = self.caller_environment(env)
        selected.update(bindings)
        return selected

    def validate(self, env: Mapping[str, str], *, target: Path) -> None:
        # Do not reopen outputs: the parent also verifies historical terminal
        # receipts after an owner has retired a generation. Values, not current
        # artifact existence, establish this binding.
        actual = {name: value for name, value in env.items() if self.owns(name)}
        expected = self._bindings(env, target=target)
        if actual != expected:
            raise ValueError(
                "Cargo output environment differs from its leased generation"
            )
