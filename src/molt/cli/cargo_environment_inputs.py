"""Immutable observations of Cargo's ambient environment dependencies."""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from dataclasses import dataclass
import hashlib
from typing import cast

from molt.exact_json import canonical_json_sha256
from molt.process_environment import NameRule, ProcessEnvironment

_SCHEMA = "molt.cargo-environment-inputs.v1"


def _value_digest(value: str | None) -> str | None:
    # POSIX var_os dependencies can contain surrogate-escaped bytes. Hash the
    # exact frozen Python string without losing or replacing those code points.
    return (
        None
        if value is None
        else hashlib.sha256(value.encode("utf-8", "surrogatepass")).hexdigest()
    )


def _valid_name(name: object) -> bool:
    return type(name) is str and bool(name) and "\0" not in name and "=" not in name


@dataclass(frozen=True, slots=True)
class CargoEnvironmentInputs:
    """A build's observed ambient names, without retaining plaintext values.

    Unit-owned Cargo and build-script values are resolved before constructing
    this record. An absent variable has no digest; an empty value has SHA-256
    of the empty byte string. Names use the captured environment's rule.
    """

    name_rule: NameRule
    variables: tuple[tuple[str, str | None], ...]
    local_unit_count: int

    def __post_init__(self) -> None:
        if not isinstance(self.name_rule, NameRule):
            raise ValueError("Cargo environment inputs have an invalid name rule")
        if type(self.local_unit_count) is not int or self.local_unit_count <= 0:
            raise ValueError(
                "Cargo environment inputs require positive local unit coverage"
            )
        if type(self.variables) is not tuple:
            raise ValueError("Cargo environment inputs must own immutable variables")
        previous: str | None = None
        for row in self.variables:
            if type(row) is not tuple or len(row) != 2:
                raise ValueError("Cargo environment input row is invalid")
            name, digest = row
            if (
                not _valid_name(name)
                or self.name_rule.key(name) != name
                or (previous is not None and name <= previous)
            ):
                raise ValueError(
                    "Cargo environment input names must be canonical and sorted"
                )
            if digest is not None and (
                type(digest) is not str
                or len(digest) != 64
                or any(character not in "0123456789abcdef" for character in digest)
            ):
                raise ValueError("Cargo environment input digest is invalid")
            previous = name

    @classmethod
    def capture(
        cls,
        environment: ProcessEnvironment,
        names: Iterable[str],
        *,
        local_unit_count: int,
    ) -> CargoEnvironmentInputs:
        if not isinstance(environment, ProcessEnvironment):
            raise TypeError("Cargo environment inputs require a ProcessEnvironment")
        selected: set[str] = set()
        for name in names:
            if not _valid_name(name):
                raise ValueError("Cargo environment input name is invalid")
            selected.add(environment.rule.key(name))
        return cls(
            environment.rule,
            tuple(
                (name, _value_digest(environment.get(name)))
                for name in sorted(selected)
            ),
            local_unit_count,
        )

    def to_dict(self) -> dict[str, object]:
        return {
            "schema": _SCHEMA,
            "name_rule": self.name_rule.value,
            "local_unit_count": self.local_unit_count,
            "variables": [
                {"name": name, "sha256": digest} for name, digest in self.variables
            ],
        }

    @classmethod
    def from_dict(cls, payload: object) -> CargoEnvironmentInputs:
        if (
            not isinstance(payload, Mapping)
            or set(payload) != {"schema", "name_rule", "local_unit_count", "variables"}
            or payload.get("schema") != _SCHEMA
        ):
            raise ValueError("Cargo environment inputs schema is invalid")
        if type(payload["name_rule"]) is not str:
            raise ValueError("Cargo environment inputs name rule is invalid")
        try:
            rule = NameRule(payload["name_rule"])
        except (TypeError, ValueError) as exc:
            raise ValueError("Cargo environment inputs name rule is invalid") from exc
        rows = payload["variables"]
        if not isinstance(rows, (list, tuple)):
            raise ValueError("Cargo environment input variables are invalid")
        variables: list[tuple[str, str | None]] = []
        for row in rows:
            if not isinstance(row, Mapping) or set(row) != {"name", "sha256"}:
                raise ValueError("Cargo environment input row is invalid")
            variables.append((cast(str, row["name"]), cast(str | None, row["sha256"])))
        return cls(rule, tuple(variables), cast(int, payload["local_unit_count"]))

    def verify(self, environment: ProcessEnvironment) -> None:
        if not isinstance(environment, ProcessEnvironment):
            raise TypeError("Cargo environment inputs require a ProcessEnvironment")
        if environment.rule is not self.name_rule:
            raise ValueError(
                "Cargo environment name rule differs for observed names: "
                + ", ".join(name for name, _ in self.variables)
            )
        changed = [
            name
            for name, digest in self.variables
            if _value_digest(environment.get(name)) != digest
        ]
        if changed:
            raise ValueError("Cargo environment inputs changed: " + ", ".join(changed))

    @property
    def digest(self) -> str:
        return canonical_json_sha256(self.to_dict())
