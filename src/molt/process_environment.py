"""Case-preserving child environments with one explicit host name rule."""

from __future__ import annotations

# Cargo policy can be loaded by tools under a checkout-neutral package.
__molt_neutral_authority__ = True

from collections.abc import Iterable, Iterator, Mapping, MutableMapping
from dataclasses import dataclass
from enum import Enum
import os
from types import MappingProxyType


class NameRule(Enum):
    EXACT = "exact"
    WINDOWS = "windows"

    @classmethod
    def host(cls) -> NameRule:
        return cls.WINDOWS if os.name == "nt" else cls.EXACT

    def key(self, name: str) -> str:
        """Use simple uppercase, retaining characters whose uppercase expands."""
        if self is NameRule.EXACT:
            return name
        return "".join(
            upper if len(upper := char.upper()) == 1 else char for char in name
        )


@dataclass(frozen=True, slots=True, init=False, eq=False)
class ProcessEnvironment(Mapping[str, str]):
    """An immutable snapshot; equivalent names retain their first spelling.

    Copies inherit the source rule unless explicitly overridden. The private
    index stores each folded name once, with its original spelling and value.
    Construction is one pass; lookup folds one name and probes one dictionary.
    Only child-block export sorts the names, as Windows' environment block does.
    """

    rule: NameRule
    _values: Mapping[str, tuple[str, str]]

    def __new__(
        cls, values: Mapping[str, str], *, rule: NameRule | None = None
    ) -> ProcessEnvironment:
        if type(values) is cls and (rule is None or rule is values.rule):
            return values
        return object.__new__(cls)

    def __init__(
        self, values: Mapping[str, str], *, rule: NameRule | None = None
    ) -> None:
        if self is values:
            return
        builder = ProcessEnvironmentBuilder(values, rule=rule)
        object.__setattr__(self, "rule", builder.rule)
        object.__setattr__(self, "_values", MappingProxyType(builder._values))

    def __getitem__(self, name: str) -> str:
        return self._values[self.rule.key(name)][1]

    def __iter__(self) -> Iterator[str]:
        return (name for name, _value in self._values.values())

    def __len__(self) -> int:
        return len(self._values)

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, ProcessEnvironment) or self.rule is not other.rule:
            return False
        return len(self) == len(other) and all(
            key in other._values and value == other._values[key][1]
            for key, (_name, value) in self._values.items()
        )

    def updated(self, values: Mapping[str, str]) -> ProcessEnvironment:
        builder = ProcessEnvironmentBuilder(self)
        builder.update(values)
        return ProcessEnvironment(builder)

    def without(self, names: Iterable[str]) -> ProcessEnvironment:
        builder = ProcessEnvironmentBuilder(self)
        for name in names:
            builder.pop(name, None)
        return ProcessEnvironment(builder)

    def child_block(self) -> dict[str, str]:
        """Return an independent spawn dictionary in name-rule order."""
        return dict(self._values[key] for key in sorted(self._values))


class ProcessEnvironmentBuilder(MutableMapping[str, str]):
    """The mutable construction form of a process environment."""

    def __init__(
        self, values: Mapping[str, str], *, rule: NameRule | None = None
    ) -> None:
        self._rule = (
            rule
            if rule is not None
            else values.rule
            if isinstance(values, (ProcessEnvironment, ProcessEnvironmentBuilder))
            else NameRule.host()
        )
        self._values: dict[str, tuple[str, str]] = {}
        self.update(values)

    @property
    def rule(self) -> NameRule:
        return self._rule

    def __getitem__(self, name: str) -> str:
        return self._values[self.rule.key(name)][1]

    def __setitem__(self, name: str, value: str) -> None:
        key = self.rule.key(name)
        spelling = self._values[key][0] if key in self._values else name
        self._values[key] = spelling, value

    def __delitem__(self, name: str) -> None:
        del self._values[self.rule.key(name)]

    def __iter__(self) -> Iterator[str]:
        return (name for name, _value in self._values.values())

    def __len__(self) -> int:
        return len(self._values)
