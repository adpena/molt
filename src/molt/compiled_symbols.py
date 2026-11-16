"""Linker symbols of the code Molt compiles: the one place that spells them.

A compiled program links its own code with the Rust runtime and the host's C
libraries. Every runtime export is a C identifier (``molt_*``, ``__molt_*``,
WASI and host imports), and so is every C or Rust symbol. Each symbol this
module spells starts with a reserved prefix that holds a ``.``, a character no
C or Rust identifier can hold, so no runtime or library symbol can occupy
compiled code's namespace. ``tools/wasm_abi_gen/execution_context.py`` checks,
when it generates the runtime ABI, that every runtime symbol is an identifier
outside these prefixes.

The spelling is injective. A module name is a dotted path; a local name holds
no ``.``. So ``py.a.b.c`` is module ``a.b``'s local ``c`` and nothing else, and
the three prefixes differ before their first ``.``.

Consumers never parse a symbol back into its module. A function record carries
its ``module`` from the frontend that produced it; a consumer that needs the
init symbol of a module calls :func:`module_init_symbol`.

The fixed program entries (``molt_main``, ``molt_host_init`` and the isolate
entries) are the runtime ABI's ``execution_context_abi.compiled_entries``: hosts
call them by name, so they keep their runtime-ABI spelling.

An app export is external ABI too. A host calls the entry module's exported
function ``f`` as :func:`app_export_name` (``<module>__f``), a name that lives
only in the linked module's export table and maps to the function's compiled
symbol there. It never changes when the compiled spelling changes.
"""

from __future__ import annotations

FUNCTION_PREFIX = "py."
MODULE_INIT_PREFIX = "pyinit."
NATIVE_WRAPPER_PREFIX = "pywrap."
COMPILED_SYMBOL_PREFIXES: tuple[str, ...] = (
    FUNCTION_PREFIX,
    MODULE_INIT_PREFIX,
    NATIVE_WRAPPER_PREFIX,
)


def _spell_module(module: str) -> str:
    if not module:
        raise ValueError("a compiled symbol needs its module name")
    return "".join(ch if ch.isalnum() or ch in "_." else "_" for ch in module)


def module_init_symbol(module: str) -> str:
    """The symbol of the function that runs ``module``'s body."""
    return f"{MODULE_INIT_PREFIX}{_spell_module(module)}"


def module_function_prefix(module: str) -> str:
    """The prefix of every function symbol ``module`` allocates."""
    return f"{FUNCTION_PREFIX}{_spell_module(module)}."


def module_function_symbol(module: str, local: str) -> str:
    """The symbol of ``module``'s function with the local name ``local``."""
    if not local or "." in local:
        raise ValueError(f"invalid compiled local name {local!r}")
    return f"{module_function_prefix(module)}{local}"


def app_export_name(module: str, name: str) -> str:
    """The public export name a host calls for ``module``'s exported ``name``.

    ``module`` keeps its dots as ``_`` (``a.b`` exports ``a_b__f``): the
    spelling hosts have called since the first app export.
    """
    if not name or "." in name:
        raise ValueError(f"invalid app export name {name!r}")
    return f"{_spell_module(module).replace('.', '_')}__{name}"


def native_wrapper_symbol(qualified_name: str, digest: str) -> str:
    """The symbol of the compiled wrapper around a native callable export."""
    if not digest or "." in digest:
        raise ValueError(f"invalid native wrapper digest {digest!r}")
    return f"{NATIVE_WRAPPER_PREFIX}{_spell_module(qualified_name)}.{digest}"


def is_compiled_symbol(symbol: str) -> bool:
    """Whether ``symbol`` (logical or physical) names code Molt compiled.

    Only artifacts that hold nothing but symbol text (object files, profiles)
    need this test; IR consumers read the function record instead.
    """
    return symbol.startswith(COMPILED_SYMBOL_PREFIXES)
