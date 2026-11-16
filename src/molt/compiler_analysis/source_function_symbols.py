"""Code symbols of a module's source functions, from its AST alone.

The defining module reserves a symbol for each module-level function before
it lowers any body (``SymbolNamingMixin._reserve_source_callables``): the
module's function symbol for its name (``molt.compiled_symbols``), with a
counter suffix when that symbol is already taken. Nested functions are named later,
so only module-level definitions compete for these symbols.

A caller in another module needs the definer's symbol, never a spelling of
it. ``unambiguous_symbol_bases`` names the module-level functions whose
symbol needs no suffix in any order of reservation: no other reserved
definition shares the base, and no base is another's ``_poll`` companion.
For every other name a caller has no static symbol and keeps the generic
call.
"""

from __future__ import annotations

import ast
from collections import Counter
from collections.abc import Iterator

_POLL = "_poll"


def module_level_function_definitions(
    tree: ast.Module,
) -> Iterator[ast.FunctionDef | ast.AsyncFunctionDef]:
    """The definitions the defining module reserves, in reservation order:
    every function statement outside class bodies, lambdas and other
    functions, in source order."""
    pending: list[ast.AST] = list(reversed(tree.body))
    while pending:
        node = pending.pop()
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            yield node
            continue
        if isinstance(node, (ast.ClassDef, ast.Lambda)):
            continue
        pending.extend(reversed(list(ast.iter_child_nodes(node))))


def unambiguous_symbol_bases(tree: ast.Module) -> dict[str, str]:
    """Module-level function name -> symbol base, for each name whose symbol
    is exactly the module's function symbol for that base."""
    bases = [node.name for node in module_level_function_definitions(tree)]
    counts = Counter(bases)
    taken = set(bases)
    return {
        base: base
        for base in bases
        if counts[base] == 1
        and f"{base}{_POLL}" not in taken
        and not (base.endswith(_POLL) and base[: -len(_POLL)] in taken)
    }
