"""The runtime callable catalog: one typed identity per runtime callable (HF-881).

A runtime callable is a runtime function published as a Python function
object. Its identity is a key derived from its catalog name, never its code
address: rustc's merge-functions pass gives byte-identical functions one
address, so an address cannot tell two callables apart.

The key is stable. It changes only when the callable is renamed, and a rename
already changes the link symbol, so a compiled object that embeds a key cannot
silently go stale when the catalog grows.

    key(name) = KEY_BASE | (fmix64(fnv1a64(utf8(name))) >> 24)

The finalizer matters: plain FNV-1a folded to 40 bits collides on adjacent
names (``molt_probe_248946`` and ``molt_probe_248947``). With ``fmix64`` the
first collision in a sequential search falls at 1,518,733 names, the 40-bit
birthday bound. The catalog refuses any collision; rustc refuses a duplicate
enum discriminant as a second check.
"""

from __future__ import annotations

from collections.abc import Iterable
from dataclasses import dataclass

# Keys sit above every native code address and every wasm32 table index, so a
# range test separates a runtime callable's key from a compiled entry address.
KEY_BASE = 0xFFFF_FF00_0000_0000
_MASK64 = 0xFFFF_FFFF_FFFF_FFFF


class RuntimeCallableCatalogError(ValueError):
    """The runtime callable declarations do not form one closed identity set."""


def fnv1a64(data: bytes) -> int:
    value = 0xCBF2_9CE4_8422_2325
    for byte in data:
        value ^= byte
        value = (value * 0x0000_0100_0000_01B3) & _MASK64
    return value


def fmix64(value: int) -> int:
    value ^= value >> 33
    value = (value * 0xFF51_AFD7_ED55_8CCD) & _MASK64
    value ^= value >> 33
    value = (value * 0xC4CE_B9FE_1A85_EC53) & _MASK64
    value ^= value >> 33
    return value


def callable_key(name: str) -> int:
    """The stable key of the runtime callable named ``name``."""
    return KEY_BASE | (fmix64(fnv1a64(name.encode("utf-8"))) >> 24)


_RUST_KEYWORDS = frozenset(
    "as async await box break const continue crate dyn else enum extern false fn "
    "for gen if impl in let loop match mod move mut pub ref return self static "
    "struct super trait true try type unsafe use where while yield".split()
)


def variant_name(name: str) -> str:
    """The ``RuntimeCallable`` variant of a catalog name: the name itself.

    The variant spells the callable exactly as its Rust item and its link
    symbol do, so a macro that defines a callable can name its identity with
    the same identifier (`fn_ref!(name)`); macro_rules cannot re-case one.
    """
    if (
        not name
        or not (name[0].isascii() and (name[0].isalpha() or name[0] == "_"))
        or not all(ch.isascii() and (ch.isalnum() or ch == "_") for ch in name)
        or name in _RUST_KEYWORDS
    ):
        raise RuntimeCallableCatalogError(f"invalid runtime callable name {name!r}")
    return name


@dataclass(frozen=True)
class RuntimeCallable:
    """One catalog row.

    ``path`` is the crate-relative Rust path of the row's molt-runtime
    re-export, or None when no runtime code names the callable directly (an
    intrinsic reached only through its resolver). ``cfg`` gates only that
    re-export: the variant and its key exist on every target.
    """

    name: str
    variant: str
    key: int
    path: str | None
    cfg: str | None
    poll: bool


def build_catalog(
    *,
    declared_names: Iterable[str],
    references: Iterable[tuple[str, str, str | None]],
    polls: Iterable[str],
) -> tuple[RuntimeCallable, ...]:
    """Union the membership declarations into one checked catalog.

    ``declared_names`` are callables declared elsewhere (manifest intrinsics,
    callable runtime imports). ``references`` are ``(name, path, cfg)`` rows
    for callables molt-runtime names directly. ``polls`` are the runtime polls.
    A name means one symbol: two declarations of one name share one key, but a
    reference that names an already declared callable must point at that
    symbol (its path must end in the name). The catalog refuses two rows with
    one path, two names with one variant and two names with one key.
    """

    names: set[str] = set(declared_names)
    poll_names = set(polls)
    names |= poll_names
    paths: dict[str, tuple[str, str | None]] = {}
    path_owner: dict[str, str] = {}
    for name, path, cfg in references:
        if name in paths:
            raise RuntimeCallableCatalogError(
                f"runtime callable {name!r} is referenced twice"
            )
        if name in names and path.rsplit("::", 1)[-1] != name:
            raise RuntimeCallableCatalogError(
                f"runtime callable {name!r} is declared elsewhere, so its "
                f"reference path {path!r} must name that symbol"
            )
        owner = path_owner.get(path)
        if owner is not None:
            raise RuntimeCallableCatalogError(
                f"runtime callables {owner!r} and {name!r} name one symbol {path!r}"
            )
        path_owner[path] = name
        paths[name] = (path, cfg)
        names.add(name)

    rows: list[RuntimeCallable] = []
    by_variant: dict[str, str] = {}
    by_key: dict[int, str] = {}
    for name in sorted(names):
        variant = variant_name(name)
        if variant in by_variant:
            raise RuntimeCallableCatalogError(
                f"runtime callables {by_variant[variant]!r} and {name!r} share "
                f"the variant {variant}"
            )
        by_variant[variant] = name
        key = callable_key(name)
        if key in by_key:
            raise RuntimeCallableCatalogError(
                f"runtime callables {by_key[key]!r} and {name!r} share the key "
                f"0x{key:016X}; rename one"
            )
        by_key[key] = name
        path, cfg = paths.get(name, (None, None))
        rows.append(
            RuntimeCallable(
                name=name,
                variant=variant,
                key=key,
                path=path,
                cfg=cfg,
                poll=name in poll_names,
            )
        )
    return tuple(rows)
