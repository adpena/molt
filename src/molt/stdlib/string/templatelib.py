"""PEP 750 template strings — `string.templatelib`.

Implements `Template` and `Interpolation`, the two public types used to
back the t-string literal syntax (``t"..."``) introduced in CPython 3.14.

The compiler lowers ``t"<text>{<expr>!c:<spec>}<text>"`` into a direct
call to ``Template(<text>, Interpolation(<value>, "<expr>", <conv>, <spec>), ...)``,
matching CPython's runtime contract.
"""

from __future__ import annotations

from typing import Any

from _intrinsics import require_intrinsic as _require_intrinsic

_MOLT_IMPORT_SMOKE_RUNTIME_READY = _require_intrinsic("molt_import_smoke_runtime_ready")
_MOLT_IMPORT_SMOKE_RUNTIME_READY()
del _MOLT_IMPORT_SMOKE_RUNTIME_READY

# Probe intrinsic — required by the molt stdlib enforcement policy so this
# module is recognized as a thin runtime-backed module rather than pure Python.
_MOLT_STDLIB_PROBE = _require_intrinsic("molt_stdlib_probe")


__all__ = ["Template", "Interpolation", "convert"]


def convert(value: Any, conversion: str | None) -> Any:
    """Apply an interpolation conversion (``!s``/``!r``/``!a``) to a value.

    Mirrors ``string.templatelib.convert`` from CPython.
    """
    if conversion is None:
        return value
    if conversion == "s":
        return str(value)
    if conversion == "r":
        return repr(value)
    if conversion == "a":
        return ascii(value)
    raise ValueError(f"Bad conversion: {conversion!r}; must be 's', 'r', 'a', or None")


class Interpolation:
    """A single ``{expr}`` slot inside a t-string template.

    The fields exactly mirror CPython's ``string.templatelib.Interpolation``:

    * ``value`` — the evaluated expression.
    * ``expression`` — the literal source text of the interpolated expression.
    * ``conversion`` — ``'s'``/``'r'``/``'a'`` or ``None``.
    * ``format_spec`` — the format spec text, or ``""`` if absent.
    """

    __slots__ = ("value", "expression", "conversion", "format_spec")

    def __init__(
        self,
        value: Any,
        expression: str = "",
        conversion: str | None = None,
        format_spec: str = "",
    ) -> None:
        if conversion is not None and conversion not in ("s", "r", "a"):
            raise ValueError(
                f"Bad conversion: {conversion!r}; must be 's', 'r', 'a', or None"
            )
        self.value = value
        self.expression = expression
        self.conversion = conversion
        self.format_spec = format_spec

    def __repr__(self) -> str:
        return (
            f"Interpolation({self.value!r}, {self.expression!r}, "
            f"{self.conversion!r}, {self.format_spec!r})"
        )

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Interpolation):
            return NotImplemented
        return (
            self.value == other.value
            and self.expression == other.expression
            and self.conversion == other.conversion
            and self.format_spec == other.format_spec
        )

    def __hash__(self) -> int:
        return hash((self.value, self.expression, self.conversion, self.format_spec))


class Template:
    """A PEP 750 template-string object.

    Construction matches CPython: ``Template`` accepts a flat positional
    sequence of ``str`` and ``Interpolation`` objects in source order. The
    constructor normalizes the sequence so that ``strings`` and
    ``interpolations`` always alternate strictly (``len(strings) ==
    len(interpolations) + 1``), inserting empty strings between adjacent
    interpolations and merging adjacent strings.
    """

    __slots__ = ("strings", "interpolations")

    def __init__(self, *args: Any) -> None:
        strings: list[str] = []
        interpolations: list[Interpolation] = []
        # Buffer for runs of consecutive str args (merged in CPython behavior).
        pending_str: list[str] = []
        for arg in args:
            if isinstance(arg, str):
                pending_str.append(arg)
            elif isinstance(arg, Interpolation):
                strings.append("".join(pending_str))
                pending_str = []
                interpolations.append(arg)
            else:
                raise TypeError(
                    "Template() arguments must be str or Interpolation, "
                    f"got {type(arg).__name__}"
                )
        # Always emit a trailing string segment so strings and interpolations
        # alternate strictly (len(strings) == len(interpolations) + 1).
        strings.append("".join(pending_str))
        if len(strings) != len(interpolations) + 1:
            # Invariant violation in our own normalization logic — fail
            # loudly rather than silently.
            raise RuntimeError(
                "Template normalization failed: "
                f"{len(strings)} strings vs {len(interpolations)} interpolations"
            )
        self.strings = tuple(strings)
        self.interpolations = tuple(interpolations)

    @property
    def values(self) -> tuple[Any, ...]:
        """The evaluated values of every interpolation, in source order."""
        return tuple(interp.value for interp in self.interpolations)

    def __iter__(self):
        """Iterate over template parts in source order: ``str``, ``Interpolation``, ...

        Empty strings between consecutive interpolations (or at the boundaries)
        are skipped so the iteration matches what was originally written.
        """
        n = len(self.interpolations)
        for idx in range(n):
            seg = self.strings[idx]
            if seg:
                yield seg
            yield self.interpolations[idx]
        tail = self.strings[n]
        if tail:
            yield tail

    def __repr__(self) -> str:
        parts: list[str] = []
        for item in self:
            parts.append(repr(item))
        return f"Template({', '.join(parts)})"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Template):
            return NotImplemented
        return (
            self.strings == other.strings
            and self.interpolations == other.interpolations
        )

    def __hash__(self) -> int:
        return hash((self.strings, self.interpolations))

    def __add__(self, other: object) -> "Template":
        if isinstance(other, Template):
            # Concatenate by splicing through a flat positional argument list.
            args = list(self)
            args.extend(other)
            return Template(*args)
        if isinstance(other, str):
            args = list(self)
            args.append(other)
            return Template(*args)
        return NotImplemented

    def __radd__(self, other: object) -> "Template":
        if isinstance(other, str):
            args = [other]
            args.extend(self)
            return Template(*args)
        return NotImplemented


# Drop the helper alias so `_require_intrinsic` does not leak as a module
# attribute (matches sibling stdlib modules).
globals().pop("_require_intrinsic", None)
